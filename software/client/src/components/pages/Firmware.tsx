import { open } from "@tauri-apps/plugin-dialog";
import { ack, api, toError } from "../../api";
import { BOOT_STATES } from "../../protocol";
import { useAppDispatch, useAppSelector } from "../../store";
import { fileChosen, updateFinished, updateStarted, versionChanged } from "../../store/firmwareSlice";
import { toast } from "../../store/uiSlice";
import { bytes, hex } from "../../util/format";
import { Badge, Card, Field, NumberInput, Spinner, Stat } from "../ui";

export function Firmware() {
  const dispatch = useAppDispatch();
  const connected = useAppSelector((s) => s.link.connected);
  const info = useAppSelector((s) => s.device.info);
  const fw = useAppSelector((s) => s.firmware);

  const choose = async () => {
    try {
      const path = await open({ multiple: false, filters: [{ name: "Firmware image", extensions: ["bin"] }] });
      if (typeof path !== "string") return;
      const file = await api.firmwareInspect(path);
      dispatch(fileChosen({ path, file }));
    } catch (e) {
      dispatch(toast({ level: "error", text: toError(e).message }));
    }
  };

  const update = async () => {
    if (!fw.path) return;
    dispatch(updateStarted());
    try {
      await api.firmwareUpdate(fw.path, fw.version);
      dispatch(updateFinished("device accepted the image and is resetting; reconnect in a few seconds and check the version"));
      dispatch(toast({ level: "success", text: "firmware update accepted" }));
    } catch (e) {
      const msg = toError(e).message;
      dispatch(updateFinished(`failed: ${msg}`));
      dispatch(toast({ level: "error", text: `update: ${msg}` }));
    }
  };

  const reset = async () => {
    try {
      await ack({ kind: "Reset" });
    } catch (e) {
      dispatch(toast({ level: "error", text: toError(e).message }));
    }
  };

  const pct = fw.progress && fw.progress.total > 0 ? (fw.progress.sent / fw.progress.total) * 100 : 0;
  const trial = info?.boot_state === 1;

  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
      <Card title="Running firmware">
        {info ? (
          <div className="grid grid-cols-2 gap-4">
            <Stat label="Version" value={info.firmware.join(".")} />
            <Stat label="Boot state" value={BOOT_STATES[info.boot_state] ?? info.boot_state} tone={info.boot_state === 0 ? "ok" : info.boot_state === 1 ? "warn" : "bad"} />
            <div className="col-span-2 text-xs text-ink-3">
              {trial
                ? "The device is running a trial image that has not confirmed itself. Updating now is refused; reset the device and check again."
                : info.boot_state === 2
                  ? "The last update was reverted by the bootloader: the new image never confirmed (crash, watchdog). Fix the image before retrying."
                  : "Updates stage the new image in the second flash bank, verify its CRC from flash, then swap and reboot. The previous image is restored automatically if the new one fails to start."}
            </div>
          </div>
        ) : (
          <div className="text-sm text-ink-3">connect to read the device</div>
        )}
      </Card>

      <Card title="Image">
        <div className="flex flex-wrap items-end gap-3">
          <button className="btn" onClick={() => void choose()} disabled={fw.running}>
            Choose .bin…
          </button>
          {fw.file && fw.path && (
            <div className="min-w-0 flex-1 text-sm">
              <div className="truncate text-ink-2" title={fw.path}>
                {fw.path}
              </div>
              <div className="num text-ink-3">
                {bytes(fw.file.size)} · CRC-32 {hex(fw.file.crc32, 8)}
              </div>
            </div>
          )}
        </div>
        <div className="mt-4 grid grid-cols-3 gap-3 md:w-80">
          {(["major", "minor", "patch"] as const).map((label, i) => (
            <Field key={label} label={label}>
              <NumberInput
                value={fw.version[i] ?? 0}
                min={0}
                max={255}
                disabled={fw.running}
                onChange={(v) => {
                  const next: [number, number, number] = [...fw.version];
                  next[i] = Number.isFinite(v) ? Math.max(0, Math.min(255, Math.round(v))) : 0;
                  dispatch(versionChanged(next));
                }}
              />
            </Field>
          ))}
        </div>
        <p className="mt-2 text-xs text-ink-3">
          Build the image with <code className="num">cargo fw-bin</code> in <code className="num">software/firmware</code>; the version is informational (the image carries its own).
        </p>
      </Card>

      <Card title="Update" className="xl:col-span-2">
        <div className="flex flex-wrap items-center gap-3">
          <button className="btn btn-primary" disabled={!connected || !fw.path || fw.running || trial} onClick={() => void update()}>
            {fw.running ? <Spinner /> : null} Update device
          </button>
          <button className="btn" disabled={!connected || fw.running} onClick={() => void reset()}>
            Reset device
          </button>
          {fw.progress && (
            <Badge tone={fw.progress.stage === "failed" ? "bad" : fw.progress.stage === "done" ? "ok" : "accent"}>
              {fw.progress.stage}
            </Badge>
          )}
        </div>
        <div className="mt-3 h-2 w-full overflow-hidden rounded bg-line">
          <div className={`h-full transition-[width] ${fw.progress?.stage === "failed" ? "bg-bad" : "bg-accent"}`} style={{ width: `${pct.toFixed(1)}%` }} />
        </div>
        {fw.progress && (
          <div className="num mt-1 text-xs text-ink-3">
            {bytes(fw.progress.sent)} / {bytes(fw.progress.total)}
          </div>
        )}
        {fw.log.length > 0 && (
          <pre className="num mt-3 max-h-48 overflow-auto rounded border border-line bg-ground p-2 text-xs text-ink-2">{fw.log.join("\n")}</pre>
        )}
      </Card>
    </div>
  );
}
