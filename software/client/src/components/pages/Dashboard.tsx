import { useEffect, useState } from "react";
import { ack, api, toError } from "../../api";
import { BOOT_STATES } from "../../protocol";
import { useAppDispatch, useAppSelector } from "../../store";
import { refreshDevice } from "../../listeners";
import { store } from "../../store";
import { toast } from "../../store/uiSlice";
import { deg, num, uidHex, unixTime, uptime } from "../../util/format";
import { Badge, Card, Empty, FlagChips, SensorBits, Stat } from "../ui";

export function Dashboard() {
  const dispatch = useAppDispatch();
  const link = useAppSelector((s) => s.link);
  const device = useAppSelector((s) => s.device);
  const lastShot = useAppSelector((s) => s.shots.history[0]);
  const [pingMs, setPingMs] = useState<number | null>(null);

  // Keep uptime fresh while connected.
  useEffect(() => {
    if (!link.connected) return;
    const t = setInterval(() => void api.request({ kind: "GetInfo" }, 2000).catch(() => undefined), 5000);
    return () => {
      clearInterval(t);
    };
  }, [link.connected]);

  const ping = async () => {
    const t0 = performance.now();
    try {
      await ack({ kind: "Ping" }, 2000);
      setPingMs(performance.now() - t0);
    } catch (e) {
      dispatch(toast({ level: "error", text: `ping: ${toError(e).message}` }));
    }
  };
  const setTime = async () => {
    try {
      await ack({ kind: "SetTime", data: Math.floor(Date.now() / 1000) });
      await api.request({ kind: "GetInfo" });
      dispatch(toast({ level: "success", text: "device clock set" }));
    } catch (e) {
      dispatch(toast({ level: "error", text: `set time: ${toError(e).message}` }));
    }
  };
  const reset = async () => {
    try {
      await ack({ kind: "Reset" });
      dispatch(toast({ level: "info", text: "device resetting" }));
    } catch (e) {
      dispatch(toast({ level: "error", text: `reset: ${toError(e).message}` }));
    }
  };

  if (!link.connected) {
    return (
      <Empty>
        Connect to a device with the bar above, or open the <b>Simulator</b> to explore the client without hardware.
      </Empty>
    );
  }
  const info = device.info;
  const bootTone = info?.boot_state === 0 ? "ok" : info?.boot_state === 1 ? "warn" : info ? "bad" : "neutral";

  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
      <Card
        title="Device"
        actions={
          <>
            <button className="btn btn-sm" onClick={() => void refreshDevice(store)}>
              Refresh
            </button>
            <button className="btn btn-sm" onClick={() => void ping()}>
              Ping{pingMs !== null && <span className="num text-ink-3">{pingMs.toFixed(0)} ms</span>}
            </button>
          </>
        }
      >
        {info ? (
          <div className="grid grid-cols-2 gap-x-6 gap-y-4 md:grid-cols-3">
            <Stat label="Firmware" value={info.firmware.join(".")} sub={`protocol v${info.protocol} · hw rev ${info.hardware_rev}`} />
            <Stat label="Uptime" value={uptime(info.uptime_ms)} />
            <Stat label="Boot state" value={BOOT_STATES[info.boot_state] ?? info.boot_state} tone={bootTone} />
            <div className="col-span-2 md:col-span-3">
              <div className="text-[11px] uppercase tracking-wider text-ink-3">Sensors answering at boot</div>
              <div className="mt-1">
                <SensorBits mask={info.sensors_ok} />
              </div>
            </div>
            <Stat label="MCU id" value={<span className="text-sm">{uidHex(info.uid)}</span>} />
            <Stat
              label="Clock"
              value={<span className="text-sm">{info.time_set ? unixTime(info.unix_time) : "not set"}</span>}
              tone={info.time_set ? undefined : "warn"}
              sub={
                <button className="text-accent hover:underline" onClick={() => void setTime()}>
                  set from this computer
                </button>
              }
            />
            <Stat
              label="Calibration"
              value={info.calibration_id === 0 ? "none" : `#${info.calibration_id}`}
              tone={info.calibration_sets === 0 ? "warn" : "ok"}
              sub={info.calibration_sets > 0 ? `${info.calibration_sets} set(s) · ${unixTime(info.calibration_date)}` : "shots will be flagged NO_CALIBRATION"}
            />
          </div>
        ) : (
          <Empty>waiting for Info…</Empty>
        )}
      </Card>

      <Card title="Last shot">
        {lastShot ? (
          <div className="grid grid-cols-3 gap-4">
            <Stat label="Azimuth" value={deg(lastShot.result.azimuth_deg)} sub={`σ ${deg(lastShot.result.azimuth_sigma_deg, 3)}`} />
            <Stat label="Inclination" value={deg(lastShot.result.inclination_deg)} sub={`σ ${deg(lastShot.result.inclination_sigma_deg, 3)}`} />
            <Stat label="Roll" value={deg(lastShot.result.roll_deg, 1)} />
            <Stat label="|B|" value={num(lastShot.result.b_total_ut, 2)} unit="µT" />
            <Stat label="Dip" value={deg(lastShot.result.dip_deg, 2)} />
            <Stat label="|g|" value={num(lastShot.result.g_norm, 4)} />
            <div className="col-span-3">
              <FlagChips bits={lastShot.result.flags} />
            </div>
          </div>
        ) : (
          <Empty>no shot yet — take one from the Shots page</Empty>
        )}
      </Card>

      <Card title="Configuration summary">
        <div className="grid grid-cols-2 gap-x-6 gap-y-3 md:grid-cols-4">
          <Stat label="Cycle count" value={device.config.mag_cycle_count} />
          <Stat label="Mag samples" value={device.config.mag_samples} />
          <Stat label="Acc rate" value={device.config.acc_rate_hz} unit="Hz" />
          <Stat label="UART" value={device.config.uart_baud} unit="baud" />
          <div className="col-span-2 flex flex-wrap gap-1 md:col-span-4">
            <Badge tone={device.config.quiet_enabled ? "ok" : "warn"}>QUIET {device.config.quiet_enabled ? "on" : "off"}</Badge>
            <Badge tone="neutral">LED {device.config.led_enabled ? "on" : "off"}</Badge>
            <Badge tone={device.config.ref_b_total_ut > 0 ? "accent" : "neutral"}>
              {device.config.ref_b_total_ut > 0 ? `reference ${device.config.ref_b_total_ut.toFixed(1)} µT / ${device.config.ref_dip_deg.toFixed(1)}°` : "no external field reference"}
            </Badge>
            {!device.configFromDevice && <Badge tone="warn">defaults shown, not read from device</Badge>}
          </div>
        </div>
      </Card>

      <Card title="Maintenance">
        <div className="flex flex-wrap gap-2">
          <button className="btn" onClick={() => void setTime()}>
            Set device clock
          </button>
          <button className="btn btn-danger" onClick={() => void reset()}>
            Reset device
          </button>
        </div>
        <p className="mt-3 text-xs text-ink-3">
          The clock is volatile: set it after every power-up so shot results carry a timestamp. A reset re-enumerates USB; the client reconnects when you press Connect again.
        </p>
      </Card>
    </div>
  );
}
