/**
 * Bridges Tauri events into the Redux store (and the stream ring buffers).
 *
 * Every device frame lands here once, whatever request produced it, so the store always reflects
 * the device even for frames consumed by a `request()` elsewhere.
 */
import type { Store } from "@reduxjs/toolkit";
import { api, onDeviceMessage, onFwProgress, onLink, toError } from "./api";
import { describeMessage, type DeviceMessage } from "./protocol";
import { streamBuffers } from "./streamBuffers";
import type { RootState } from "./store";
import { appInfoLoaded, linkChanged, portsLoaded } from "./store/linkSlice";
import { calInfoReceived, configReceived, deviceForgotten, eventReceived, infoReceived } from "./store/deviceSlice";
import { streamCounters, streamStopped, tempsReceived } from "./store/streamSlice";
import { slotDone } from "./store/calibrationSlice";
import { progressReceived } from "./store/firmwareSlice";
import { consoleActions } from "./store/consoleSlice";
import { toast } from "./store/uiSlice";

type AppStore = Store<RootState>;

export async function refreshPorts(store: AppStore): Promise<void> {
  try {
    store.dispatch(portsLoaded(await api.listPorts()));
  } catch (e) {
    store.dispatch(toast({ level: "error", text: `port scan: ${toError(e).message}` }));
  }
}

/** Fetches `Info`, `Config` and `CalInfo` after a connect (and on demand). */
export async function refreshDevice(store: AppStore): Promise<void> {
  try {
    await api.request({ kind: "GetInfo" }, 3000);
    await api.request({ kind: "GetConfig" }, 3000);
    await api.request({ kind: "CalInfo" }, 3000);
  } catch (e) {
    store.dispatch(toast({ level: "error", text: `device refresh: ${toError(e).message}` }));
  }
}

function handleMessage(store: AppStore, seq: number, m: DeviceMessage): void {
  store.dispatch(consoleActions.counted(m.kind));
  const state = store.getState();
  const quiet = m.kind === "Sample" || m.kind === "Temps";
  if (!(quiet && state.console.hideSamples)) {
    store.dispatch(
      consoleActions.logged({
        direction: "rx",
        seq,
        kind: m.kind,
        summary: describeMessage(m),
        json: m.kind === "CalData" ? "(calibration data)" : JSON.stringify(m.data),
      }),
    );
  }
  switch (m.kind) {
    case "Info":
      store.dispatch(infoReceived(m.data));
      break;
    case "Config":
      store.dispatch(configReceived(m.data));
      break;
    case "CalInfo":
      store.dispatch(calInfoReceived(m.data));
      break;
    case "Sample":
      streamBuffers.push(m.data);
      break;
    case "Temps":
      streamBuffers.temps = m.data;
      store.dispatch(tempsReceived(m.data));
      break;
    case "CalShotDone":
      store.dispatch(slotDone(m.data));
      break;
    case "Event":
      store.dispatch(eventReceived(m.data));
      if (m.data.kind === "StreamStopped") store.dispatch(streamStopped());
      if (m.data.kind === "Boot") {
        store.dispatch(toast({ level: "info", text: "device booted" }));
        void refreshDevice(store);
      }
      if (m.data.kind === "SensorFault") store.dispatch(toast({ level: "error", text: `sensor fault: ${m.data.detail}` }));
      break;
    case "Log":
      store.dispatch(consoleActions.logged({ direction: "sys", seq, kind: "Log", summary: m.data.text, json: "" }));
      // Device warnings and errors deserve attention outside the console.
      if (m.data.level <= 1) store.dispatch(toast({ level: m.data.level === 0 ? "error" : "info", text: `device: ${m.data.text}` }));
      break;
    default:
      break;
  }
}

let started = false;

export async function startListeners(store: AppStore): Promise<void> {
  if (started) return;
  started = true;

  try {
    store.dispatch(appInfoLoaded(await api.appInfo()));
  } catch (e) {
    store.dispatch(toast({ level: "error", text: `app info: ${toError(e).message}` }));
  }

  await onDeviceMessage((e) => {
    handleMessage(store, e.seq, e.message);
  });
  await onLink((e) => {
    store.dispatch(
      linkChanged({ connected: e.connected, description: e.description, simulator: e.simulator, reason: e.reason }),
    );
    store.dispatch(
      consoleActions.logged({
        direction: "sys",
        seq: 0,
        kind: "link",
        summary: e.connected ? `connected: ${e.description}` : `disconnected: ${e.description}${e.reason ? ` (${e.reason})` : ""}`,
        json: "",
      }),
    );
    if (e.connected) {
      streamBuffers.clear();
      void refreshDevice(store);
    } else {
      store.dispatch(deviceForgotten());
      store.dispatch(streamStopped());
      if (e.reason) store.dispatch(toast({ level: "error", text: `link lost: ${e.reason}` }));
    }
  });
  await onFwProgress((p) => {
    store.dispatch(progressReceived(p));
  });

  // Stream counters at a human rate.
  setInterval(() => {
    const s = store.getState().stream;
    if (s.running || s.total !== streamBuffers.total) {
      store.dispatch(streamCounters({ total: streamBuffers.total, measuredRateHz: streamBuffers.rateHz }));
    }
  }, 500);

  // Recover a link that survived a frontend reload (Vite HMR, `cargo tauri dev`).
  try {
    const status = await api.linkStatus();
    if (status.connected) {
      store.dispatch(linkChanged({ connected: true, description: status.description, simulator: status.simulator }));
      void refreshDevice(store);
    }
  } catch {
    // Not running inside Tauri (plain `vite`): the UI still renders, nothing to recover.
  }
  await refreshPorts(store);
}
