import { useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { ack, api, shotTimeout, toError } from "../../api";
import { SHOT_NO_QUIET, SHOT_WANT_RAW, isSevere, sensorName, shotDurationMs, type ShotRaw, type ShotResult } from "../../protocol";
import { useAppDispatch, useAppSelector } from "../../store";
import { historyCleared, shotFinished, shotOptions, shotRecorded, shotSelected, shotStarted } from "../../store/shotsSlice";
import { toast } from "../../store/uiSlice";
import { clockTime, csv, deg, num } from "../../util/format";
import { Attitude } from "../charts/Attitude";
import { Badge, Card, Empty, Field, FlagChips, NumberInput, Spinner, Stat, Toggle } from "../ui";

export function Shots() {
  const dispatch = useAppDispatch();
  const connected = useAppSelector((s) => s.link.connected);
  const config = useAppSelector((s) => s.device.config);
  const shots = useAppSelector((s) => s.shots);
  const [progress, setProgress] = useState(0);

  useEffect(() => {
    if (!shots.inProgress || shots.startedAt === null) {
      setProgress(0);
      return;
    }
    const started = shots.startedAt;
    const t = setInterval(() => setProgress(Math.min(1, (Date.now() - started) / Math.max(1, shots.expectedMs))), 100);
    return () => {
      clearInterval(t);
    };
  }, [shots.inProgress, shots.startedAt, shots.expectedMs]);

  const take = async () => {
    const flags = (shots.wantRaw ? SHOT_WANT_RAW : 0) | (shots.noQuiet ? SHOT_NO_QUIET : 0);
    const samples = shots.magSamples === 0 ? config.mag_samples : shots.magSamples;
    const cc = shots.cycleCount === 0 ? config.mag_cycle_count : shots.cycleCount;
    dispatch(shotStarted({ expectedMs: shotDurationMs(samples, cc, shots.noQuiet ? 0 : config.quiet_lead_ms) }));
    try {
      const replies = await api.request(
        { kind: "Shot", data: { flags, mag_samples: shots.magSamples, cycle_count: shots.cycleCount } },
        shotTimeout(shots.magSamples, shots.cycleCount, config),
      );
      let result: ShotResult | null = null;
      const raw: ShotRaw[] = [];
      for (const r of replies) {
        if (r.kind === "ShotResult") result = r.data;
        else if (r.kind === "ShotRaw") raw.push(r.data);
        else if (r.kind === "Ack" && r.data.status !== "Ok") throw new Error(`device answered ${r.data.status} (detail ${r.data.detail})`);
      }
      if (!result) throw new Error("no ShotResult in reply");
      dispatch(shotRecorded({ result, raw }));
    } catch (e) {
      dispatch(shotFinished());
      dispatch(toast({ level: "error", text: `shot: ${toError(e).message}` }));
    }
  };

  const abort = async () => {
    try {
      await ack({ kind: "Abort" }, 2000);
    } catch (e) {
      dispatch(toast({ level: "error", text: `abort: ${toError(e).message}` }));
    }
  };

  const exportCsv = async () => {
    const path = await save({ defaultPath: "attinav-shots.csv", filters: [{ name: "CSV", extensions: ["csv"] }] });
    if (!path) return;
    const header = [
      "shot_id",
      "received_at",
      "unix_time",
      "azimuth_deg",
      "inclination_deg",
      "roll_deg",
      "dip_deg",
      "b_total_ut",
      "g_norm",
      "azimuth_sigma_deg",
      "inclination_sigma_deg",
      "mag_spread_deg",
      "acc_spread_deg",
      "gyro_peak_dps",
      "flags",
      "calibration_id",
      "mag_samples",
      "acc_samples",
      "duration_ms",
      "note",
    ];
    const rows = [...shots.history].reverse().map((h) => {
      const r = h.result;
      return [
        r.shot_id,
        new Date(h.receivedAt).toISOString(),
        r.unix_time,
        r.azimuth_deg,
        r.inclination_deg,
        r.roll_deg,
        r.dip_deg,
        r.b_total_ut,
        r.g_norm,
        r.azimuth_sigma_deg,
        r.inclination_sigma_deg,
        r.mag_spread_deg,
        r.acc_spread_deg,
        r.gyro_peak_dps,
        r.flags,
        r.calibration_id,
        r.mag_samples,
        r.acc_samples,
        r.duration_ms,
        h.note,
      ]
        .map(csv)
        .join(",");
    });
    try {
      await api.writeText(path, [header.join(","), ...rows].join("\n") + "\n");
      dispatch(toast({ level: "success", text: `exported ${rows.length} shots` }));
    } catch (e) {
      dispatch(toast({ level: "error", text: toError(e).message }));
    }
  };

  const selected = shots.history.find((h) => h.result.shot_id === shots.selected) ?? shots.history[0];

  return (
    <div className="grid grid-cols-1 gap-4 2xl:grid-cols-[1fr_420px]">
      <div className="flex flex-col gap-4">
        <Card title="Take a shot">
          <div className="grid grid-cols-2 gap-3 md:grid-cols-5">
            <Field label="Mag samples" hint={`0 = device default (${config.mag_samples})`}>
              <NumberInput value={shots.magSamples} min={0} max={4000} onChange={(v) => dispatch(shotOptions({ magSamples: Number.isFinite(v) ? v : 0 }))} />
            </Field>
            <Field label="Cycle count" hint={`0 = device default (${config.mag_cycle_count})`}>
              <NumberInput value={shots.cycleCount} min={0} max={4000} onChange={(v) => dispatch(shotOptions({ cycleCount: Number.isFinite(v) ? v : 0 }))} />
            </Field>
            <div className="flex flex-col justify-center gap-2">
              <Toggle checked={shots.wantRaw} onChange={(v) => dispatch(shotOptions({ wantRaw: v }))} label="Raw per-sensor data" />
              <Toggle checked={shots.noQuiet} onChange={(v) => dispatch(shotOptions({ noQuiet: v }))} label="Skip QUIET (test)" />
            </div>
            <div className="col-span-2 flex items-end gap-2">
              <button className="btn btn-primary" disabled={!connected || shots.inProgress} onClick={() => void take()}>
                {shots.inProgress ? <Spinner /> : null} Shoot
              </button>
              <button className="btn" disabled={!connected || !shots.inProgress} onClick={() => void abort()}>
                Abort
              </button>
            </div>
          </div>
          {shots.inProgress && (
            <div className="mt-3 h-1.5 w-full overflow-hidden rounded bg-line">
              <div className="h-full bg-accent transition-[width] duration-100" style={{ width: `${(progress * 100).toFixed(0)}%` }} />
            </div>
          )}
        </Card>

        {selected ? (
          <Card
            title={`Shot #${selected.result.shot_id}`}
            actions={
              <span className="text-xs text-ink-3">
                {clockTime(selected.receivedAt)} · {selected.result.duration_ms} ms · {selected.result.mag_samples} mag / {selected.result.acc_samples} acc samples · cal #{selected.result.calibration_id}
              </span>
            }
          >
            <div className="flex flex-wrap items-start gap-8">
              <Attitude azimuth={selected.result.azimuth_deg} inclination={selected.result.inclination_deg} roll={selected.result.roll_deg} sigmaAz={selected.result.azimuth_sigma_deg} />
              <div className="grid flex-1 grid-cols-2 gap-x-6 gap-y-3 md:grid-cols-3">
                <Stat label="Azimuth" value={deg(selected.result.azimuth_deg, 2)} sub={`σ ${deg(selected.result.azimuth_sigma_deg, 3)}`} />
                <Stat label="Inclination" value={deg(selected.result.inclination_deg, 2)} sub={`σ ${deg(selected.result.inclination_sigma_deg, 3)}`} />
                <Stat label="Roll" value={deg(selected.result.roll_deg, 1)} />
                <Stat label="|B|" value={num(selected.result.b_total_ut, 2)} unit="µT" />
                <Stat label="Dip" value={deg(selected.result.dip_deg, 2)} />
                <Stat label="|g|" value={num(selected.result.g_norm, 4)} tone={Math.abs(selected.result.g_norm - 1) > 0.02 ? "warn" : undefined} />
                <Stat label="Mag spread" value={deg(selected.result.mag_spread_deg, 3)} />
                <Stat label="Acc spread" value={deg(selected.result.acc_spread_deg, 3)} />
                <Stat label="Gyro peak" value={num(selected.result.gyro_peak_dps, 3)} unit="°/s" />
                <Stat label="Coil temps" value={<span className="text-sm">{selected.result.mag_temp_c.map((t) => num(t, 1)).join(" / ")}</span>} unit="°C" />
                <Stat label="Die temps" value={<span className="text-sm">{selected.result.acc_temp_c.map((t) => num(t, 1)).join(" / ")}</span>} unit="°C" />
                <div className="col-span-2 md:col-span-3">
                  <div className="mb-1 text-[11px] uppercase tracking-wider text-ink-3">Quality</div>
                  <FlagChips bits={selected.result.flags} />
                </div>
              </div>
            </div>
            {selected.raw.length > 0 && (
              <table className="table mt-4">
                <thead>
                  <tr>
                    <th>Sensor</th>
                    <th>n</th>
                    <th>mean (raw)</th>
                    <th>σ (raw)</th>
                    <th>calibrated</th>
                    <th>|cal|</th>
                  </tr>
                </thead>
                <tbody>
                  {selected.raw.map((r) => (
                    <tr key={r.sensor}>
                      <td>{sensorName(r.sensor)}</td>
                      <td className="num">{r.count}</td>
                      <td className="num">{r.mean.map((v) => v.toFixed(1)).join(", ")}</td>
                      <td className="num">{r.std_dev.map((v) => v.toFixed(2)).join(", ")}</td>
                      <td className="num">{r.calibrated.map((v) => v.toFixed(5)).join(", ")}</td>
                      <td className="num">{Math.hypot(...r.calibrated).toFixed(5)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </Card>
        ) : (
          <Card>
            <Empty>no shot yet</Empty>
          </Card>
        )}
      </div>

      <Card
        title={`History (${shots.history.length})`}
        actions={
          <>
            <button className="btn btn-sm" disabled={shots.history.length === 0} onClick={() => void exportCsv()}>
              Export CSV
            </button>
            <button className="btn btn-sm" disabled={shots.history.length === 0} onClick={() => dispatch(historyCleared())}>
              Clear
            </button>
          </>
        }
      >
        {shots.history.length === 0 ? (
          <Empty>empty</Empty>
        ) : (
          <div className="max-h-[70vh] overflow-auto">
            <table className="table">
              <thead>
                <tr>
                  <th>#</th>
                  <th>time</th>
                  <th>az</th>
                  <th>inc</th>
                  <th>roll</th>
                  <th>flags</th>
                </tr>
              </thead>
              <tbody>
                {shots.history.map((h) => (
                  <tr key={h.result.shot_id} className={`cursor-pointer ${selected?.result.shot_id === h.result.shot_id ? "bg-panel-2" : ""}`} onClick={() => dispatch(shotSelected(h.result.shot_id))}>
                    <td className="num">{h.result.shot_id}</td>
                    <td className="num text-ink-2">{clockTime(h.receivedAt)}</td>
                    <td className="num">{h.result.azimuth_deg.toFixed(2)}</td>
                    <td className="num">{h.result.inclination_deg.toFixed(2)}</td>
                    <td className="num">{h.result.roll_deg.toFixed(1)}</td>
                    <td>{h.result.flags === 0 ? <Badge tone="ok">clean</Badge> : <Badge tone={isSevere(h.result.flags) ? "bad" : "warn"}>{h.result.flags.toString(2).split("1").length - 1} flag(s)</Badge>}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}
