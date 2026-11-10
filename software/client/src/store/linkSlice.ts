import { createSlice, type PayloadAction } from "@reduxjs/toolkit";
import type { AppInfo, PortInfo } from "../api";

export interface LinkState {
  connected: boolean;
  description: string | null;
  simulator: boolean;
  lastCloseReason: string | null;
  ports: PortInfo[];
  selectedPort: string | null;
  baud: number;
  connecting: boolean;
  appInfo: AppInfo | null;
}

const initialState: LinkState = {
  connected: false,
  description: null,
  simulator: false,
  lastCloseReason: null,
  ports: [],
  selectedPort: null,
  baud: 115200,
  connecting: false,
  appInfo: null,
};

export const linkSlice = createSlice({
  name: "link",
  initialState,
  reducers: {
    linkChanged(state, a: PayloadAction<{ connected: boolean; description: string | null; simulator: boolean; reason?: string | null }>) {
      state.connected = a.payload.connected;
      state.description = a.payload.connected ? a.payload.description : null;
      state.simulator = a.payload.connected && a.payload.simulator;
      state.lastCloseReason = a.payload.connected ? null : (a.payload.reason ?? null);
      state.connecting = false;
    },
    connecting(state, a: PayloadAction<boolean>) {
      state.connecting = a.payload;
    },
    portsLoaded(state, a: PayloadAction<PortInfo[]>) {
      state.ports = a.payload;
      if (state.selectedPort === null || !a.payload.some((p) => p.path === state.selectedPort)) {
        state.selectedPort = a.payload.find((p) => p.is_attinav)?.path ?? a.payload[0]?.path ?? null;
      }
    },
    portSelected(state, a: PayloadAction<string>) {
      state.selectedPort = a.payload;
    },
    baudChanged(state, a: PayloadAction<number>) {
      state.baud = a.payload;
    },
    appInfoLoaded(state, a: PayloadAction<AppInfo>) {
      state.appInfo = a.payload;
    },
  },
});

export const { linkChanged, connecting, portsLoaded, portSelected, baudChanged, appInfoLoaded } = linkSlice.actions;
