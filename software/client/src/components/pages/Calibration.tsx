import { useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { ack, api, expect, shotTimeout, toError } from "../../api";
import { CAL_GROUPS, CAL_ROLLS, isSevere, type Calibration as CalibrationData, type CalShotReport, type SolveReport } from "../../protocol";
import { useAppDispatch, useAppSelector } from "../../store";
import {
  busyMessage,
  calibrationReceived,
  pendingChanged,
  sessionChanged,
  slotDone,
  slotFailed,
  slotTaking,
  slotsReplaced,
  solvedReceived,
  solvingChanged,
  storeIdChanged,
} from "../../store/calibrationSlice";
import { toast } from "../../store/uiSlice";
import { deg, num, unixTime } from "../../util/format";
import { Badge, Card, Empty, Field, FlagChips, NumberInput, Spinner, Stat } from "../ui";

export function Calibration() {
  const dispatch = useAppDispatch();
  const connected = useAppSelector((s) => s.link.connected);
  const config = useAppSelector((s) => s.device.config);
  const calInfo = useAppSelector((s) => s.device.calInfo);
  const cal = useAppSelector((s) => s.calibration);
  const [hover, setHover] = useState<CalShotReport | null>(null);

  const refreshInfo = () => api.request({ kind: "CalInfo" }).catch(() => undefined);

  const run = async (label: string, f: () => Promise<void>) => {
    dispatch(busyMessage(label));
    try {
      await f();
    } catch (e) {
      dispatch(toast({ level: "error", text: `${label}: ${toError(e).message}` }));
    } finally {
      dispatch(busyMessage(null));
      void refreshInfo();
    }
  };

  const begin = () =>
    run("begin session", async () => {
      await ack({ kind: "CalBegin" });
      dispatch(sessionChanged(true));
    });
  const end = () =>
    run("end session", async () => {
      await ack({ kind: "CalEnd" });
      dispatch(sessionChanged(false));
    });
  const resume = () =>
    run("fetch buffered shots", async () => {
      const replies = await api.request({ kind: "CalGetShots" }, 10000);
      const reports = replies.flatMap((r) => (r.kind === "CalShotData" ? [r.data] : []));
      dispatch(slotsReplaced(reports));
      dispatch(toast({ level: "info", text: `${reports.length} buffered shot(s) fetched` }));
    });

  const takeSlot = (group: number, roll: number) =>
    run(`shot g${group + 1} r${roll + 1}`, async () => {
      dispatch(slotTaking({ group, roll }));
      try {
        const report = await expect({ kind: "CalShot", data: { group, roll } }, "CalShotDone", shotTimeout(0, 0, config));
        dispatch(slotDone(report));
        if (isSevere(report.flags)) dispatch(toast({ level: "info", text: `g${group + 1} r${roll + 1} has severe flags: consider retaking` }));
      } catch (e) {
        dispatch(slotFailed({ group, roll }));
        throw e;
      }
    });
  const removeSlot = (group: number, roll: number) =>
    run("remove shot", async () => {
      await ack({ kind: "CalRemove", data: { group, roll } });
      dispatch(slotFailed({ group, roll }));
    });

  const solve = () =>
    run("solve", async () => {
      dispatch(solvingChanged(true));
      try {
        const solved = await expect({ kind: "CalSolve" }, "CalSolved", 300_000);
        dispatch(solvedReceived(solved));
        if (solved.error) dispatch(toast({ level: "error", text: `solve failed: ${solved.error}` }));
        else dispatch(toast({ level: "success", text: `solved: residual ${solved.report.residual_deg.toFixed(3)}°` }));
      } finally {
        dispatch(solvingChanged(false));
      }
    });
  const storeSet = () =>
    run("store", async () => {
      const idx = await ack({ kind: "CalStore", data: { id: cal.storeId } }, 10000);
      dispatch(pendingChanged(false));
      dispatch(toast({ level: "success", text: `stored as set ${idx + 1} under id ${cal.storeId}` }));
      await api.request({ kind: "GetInfo" });
    });
  const fetchCal = () =>
    run("read calibration", async () => {
      const data = await expect({ kind: "CalGet" }, "CalData", 10000);
      dispatch(calibrationReceived(data));
    });
  const exportCal = () =>
    run("export", async () => {
      const data = await expect({ kind: "CalGet" }, "CalData", 10000);
      dispatch(calibrationReceived(data));
      const path = await save({ defaultPath: `attinav-calibration-${data.id}.json`, filters: [{ name: "JSON", extensions: ["json"] }] });
      if (!path) return;
      await api.writeJson(path, { format: "attinav-calibration", version: 1, exported_at: new Date().toISOString(), calibration: data });
      dispatch(toast({ level: "success", text: "calibration exported" }));
    });
  const importCal = () =>
    run("import", async () => {
      const path = await open({ multiple: false, filters: [{ name: "JSON", extensions: ["json"] }] });
      if (typeof path !== "string") return;
      const raw: unknown = await api.readJson(path);
      const data = unwrapCalibration(raw);
      if (!data) throw new Error("not a calibration file");
      await ack({ kind: "CalSet", data }, 10000);
      dispatch(calibrationReceived(data));
      await api.request({ kind: "GetInfo" });
      dispatch(toast({ level: "success", text: `calibration #${data.id} written to the device` }));
    });
  const clearCal = () =>
    run("clear", async () => {
      await ack({ kind: "CalClear" }, 10000);
      dispatch(calibrationReceived(null));
      await api.request({ kind: "GetInfo" });
    });
  const reload = () =>
    run("reload from flash", async () => {
      const n = await ack({ kind: "CalLoad" }, 10000);
      dispatch(toast({ level: "info", text: `reloaded: ${n} set(s)` }));
      await api.request({ kind: "GetInfo" });
    });

  const done = cal.slots.flat().filter((s) => s.status === "done").length;
  const busy = cal.busyMessage !== null;

  return (
    <div className="grid grid-cols-1 gap-4 2xl:grid-cols-[1fr_440px]">
      <div className="flex flex-col gap-4">
        <Card
          title="Procedure"
          actions={
            <>
              {busy && (
                <span className="flex items-center gap-2 text-xs text-ink-2">
                  <Spinner /> {cal.busyMessage}
                </span>
              )}
              {cal.sessionOpen ? (
                <button className="btn btn-sm" disabled={busy} onClick={() => void end()}>
                  End session
                </button>
              ) : (
                <button className="btn btn-sm btn-primary" disabled={!connected || busy} onClick={() => void begin()}>
                  Begin session
                </button>
              )}
              <button className="btn btn-sm" disabled={!connected || busy} onClick={() => void resume()} title="Fetch the shots still buffered on the device (after a client restart)">
                Fetch buffered
              </button>
            </>
          }
        >
          <p className="mb-3 text-xs text-ink-3">
            Point the instrument in {CAL_GROUPS} well-spread directions (rows); in each direction take {CAL_ROLLS} shots rolled ~90° apart (columns). Click a cell to take that shot; click again to retake. Cells turn red when the shot carries a severe flag (motion, sensor fault) and should be retaken. At least 24 shots in 6 groups are needed; use all 56 for a real calibration.
          </p>
          <div className="grid grid-cols-[2.5rem_repeat(4,1fr)] gap-1.5">
            <div />
            {Array.from({ length: CAL_ROLLS }, (_, r) => (
              <div key={r} className="text-center text-[11px] uppercase tracking-wider text-ink-3">
                roll {r + 1} · ~{r * 90}°
              </div>
            ))}
            {cal.slots.map((row, g) => (
              <SlotRow
                key={g}
                group={g}
                row={row}
                disabled={!connected || !cal.sessionOpen || busy}
                residuals={cal.solved && !cal.solvedStale && !cal.solved.error ? cal.solved.report : null}
                onTake={takeSlot}
                onRemove={removeSlot}
                onHover={setHover}
              />
            ))}
          </div>
          <div className="mt-3 flex items-center gap-4 text-sm">
            <span>
              <span className="num">{done}</span>
              <span className="text-ink-3"> / {CAL_GROUPS * CAL_ROLLS} shots</span>
            </span>
            {calInfo && (
              <span className="text-ink-3">
                device buffer: <span className="num">{calInfo.buffered_shots}</span> · session {calInfo.session_open ? "open" : "closed"} · {calInfo.pending_set ? "solved set pending" : "nothing pending"}
              </span>
            )}
          </div>
        </Card>

        <Card title="Solve and store">
          <div className="flex flex-wrap items-end gap-3">
            <button className="btn btn-primary" disabled={!connected || busy || done < 24} onClick={() => void solve()}>
              {cal.solving ? <Spinner /> : null} Solve on device
            </button>
            <div className="w-44">
              <Field label="Calibration id" hint="unix time by convention">
                <NumberInput value={cal.storeId} min={1} onChange={(v) => dispatch(storeIdChanged(Number.isFinite(v) ? Math.max(1, Math.round(v)) : 1))} />
              </Field>
            </div>
            <button className="btn" disabled={!connected || busy || !cal.pendingSet} onClick={() => void storeSet()}>
              Store pending set
            </button>
          </div>
          {cal.solved && (
            <div className="mt-4">
              {cal.solved.error ? (
                <Badge tone="bad">{cal.solved.error}</Badge>
              ) : (
                <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
                  <Stat label="RMS residual" value={deg(cal.solved.report.residual_deg, 3)} tone={cal.solved.report.residual_deg > 0.2 ? "warn" : "ok"} />
                  <Stat
                    label="Shots used"
                    value={cal.solved.report.shots}
                    tone={cal.solved.report.excluded.length > 0 ? "warn" : undefined}
                    sub={
                      cal.solved.report.excluded.length > 0
                        ? `${cal.solved.report.excluded.length} rejected as outlier(s): retake the red cells`
                        : `${cal.solved.report.iterations} iterations`
                    }
                  />
                  <Stat label="Fitted dip" value={deg(cal.solved.dip_deg, 2)} />
                  <Stat label="Fitted |B|" value={num(cal.solved.field_ut, 2)} unit="µT" />
                  <div className="col-span-2 md:col-span-4">
                    <div className="text-[11px] uppercase tracking-wider text-ink-3">Per-sensor residual</div>
                    <div className="num mt-1 flex flex-wrap gap-4 text-sm">
                      {cal.solved.report.mag_residual_deg.map((v, i) => (
                        <span key={`m${i}`}>
                          Mag {i + 1}: {deg(v, 3)}
                        </span>
                      ))}
                      {cal.solved.report.acc_residual_deg.map((v, i) => (
                        <span key={`a${i}`}>
                          Acc {i + 1}: {deg(v, 3)}
                        </span>
                      ))}
                    </div>
                  </div>
                </div>
              )}
            </div>
          )}
        </Card>
      </div>

      <div className="flex flex-col gap-4">
        <Card
          title="Stored calibration"
          actions={
            <>
              <button className="btn btn-sm" disabled={!connected || busy} onClick={() => void exportCal()}>
                Export
              </button>
              <button className="btn btn-sm" disabled={!connected || busy} onClick={() => void importCal()}>
                Import
              </button>
              <button className="btn btn-sm" disabled={!connected || busy} onClick={() => void reload()}>
                Reload
              </button>
              <button className="btn btn-sm btn-danger" disabled={!connected || busy} onClick={() => void clearCal()}>
                Clear
              </button>
            </>
          }
        >
          {calInfo ? (
            <>
              <div className="grid grid-cols-2 gap-3">
                <Stat label="Id" value={calInfo.id === 0 ? "none" : `#${calInfo.id}`} />
                <Stat label="Date" value={<span className="text-sm">{unixTime(calInfo.date_unix)}</span>} />
                <Stat label="Cycle count" value={calInfo.mag_cycle_count} tone={calInfo.mag_cycle_count !== config.mag_cycle_count && calInfo.n_sets > 0 ? "warn" : undefined} />
                <Stat label="Sets" value={calInfo.n_sets} />
              </div>
              {calInfo.n_sets > 0 && (
                <table className="table mt-3">
                  <thead>
                    <tr>
                      <th>#</th>
                      <th>coil °C</th>
                      <th>die °C</th>
                      <th>residual</th>
                      <th>shots</th>
                      <th>|B|</th>
                      <th>dip</th>
                    </tr>
                  </thead>
                  <tbody>
                    {calInfo.sets.slice(0, calInfo.n_sets).map((s, i) => (
                      <tr key={i}>
                        <td className="num">{i + 1}</td>
                        <td className="num">{s.mag_temp_c.toFixed(1)}</td>
                        <td className="num">{s.acc_temp_c.toFixed(1)}</td>
                        <td className="num">{s.residual_deg.toFixed(3)}°</td>
                        <td className="num">{s.shots}</td>
                        <td className="num">{s.field_ut.toFixed(2)}</td>
                        <td className="num">{s.dip_deg.toFixed(2)}°</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              <button className="btn btn-sm mt-3" disabled={!connected || busy} onClick={() => void fetchCal()}>
                Show matrices
              </button>
              {cal.calibration && <CalMatrices data={cal.calibration} />}
            </>
          ) : (
            <Empty>connect to see the stored calibration</Empty>
          )}
        </Card>

        <Card title="Shot under cursor">
          {hover ? (
            <div className="text-sm">
              <div className="mb-2 flex items-center gap-2">
                <Badge tone="accent">
                  g{hover.shot.group + 1} r{hover.shot.roll + 1}
                </Badge>
                <FlagChips bits={hover.flags} />
              </div>
              <table className="table">
                <tbody>
                  {hover.shot.mag.map((v, i) => (
                    <tr key={`m${i}`}>
                      <td>Mag {i + 1}</td>
                      <td className="num">
                        {v.x.toFixed(0)}, {v.y.toFixed(0)}, {v.z.toFixed(0)}
                      </td>
                      <td className="num text-ink-3">{num(hover.shot.mag_temp_c[i], 1)} °C</td>
                    </tr>
                  ))}
                  {hover.shot.acc.map((v, i) => (
                    <tr key={`a${i}`}>
                      <td>Acc {i + 1}</td>
                      <td className="num">
                        {v.x.toFixed(0)}, {v.y.toFixed(0)}, {v.z.toFixed(0)}
                      </td>
                      <td className="num text-ink-3">{num(hover.shot.acc_temp_c[i], 1)} °C</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <Empty>hover a filled cell</Empty>
          )}
        </Card>
      </div>
    </div>
  );
}

/** Accepts either the export envelope `{ calibration: … }` or a bare `Calibration`. */
function unwrapCalibration(raw: unknown): CalibrationData | null {
  if (typeof raw !== "object" || raw === null) return null;
  const obj = raw as Record<string, unknown>;
  const candidate = typeof obj.calibration === "object" && obj.calibration !== null ? (obj.calibration as Record<string, unknown>) : obj;
  if (typeof candidate.n_sets !== "number" || !Array.isArray(candidate.sets) || typeof candidate.mag_cycle_count !== "number") return null;
  return candidate as unknown as CalibrationData;
}

function SlotRow({
  group,
  row,
  disabled,
  residuals,
  onTake,
  onRemove,
  onHover,
}: {
  group: number;
  row: { status: string; report?: CalShotReport }[];
  disabled: boolean;
  /** Per-shot residuals of the last solve, when they still describe this buffer. */
  residuals: SolveReport | null;
  onTake: (g: number, r: number) => Promise<void>;
  onRemove: (g: number, r: number) => Promise<void>;
  onHover: (r: CalShotReport | null) => void;
}) {
  return (
    <>
      <div className="flex items-center text-xs text-ink-3">g{group + 1}</div>
      {row.map((slot, r) => {
        const report = slot.status === "done" ? slot.report : undefined;
        const severe = report ? isSevere(report.flags) : false;
        const excluded = report !== undefined && (residuals?.excluded.includes(report.index) ?? false);
        const residual = report && residuals ? residuals.shot_residual_deg[report.index] : undefined;
        const cls =
          slot.status === "taking"
            ? "border-accent bg-accent/10"
            : report
              ? excluded || severe
                ? "border-bad/60 bg-bad/10"
                : report.flags !== 0
                  ? "border-warn/60 bg-warn/10"
                  : "border-ok/60 bg-ok/10"
              : "border-line bg-ground hover:border-ink-3";
        return (
          <button
            key={r}
            className={`flex h-9 flex-col items-center justify-center rounded border text-xs leading-tight transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${cls}`}
            disabled={disabled || slot.status === "taking"}
            onClick={() => void onTake(group, r)}
            onContextMenu={(e) => {
              e.preventDefault();
              if (report) void onRemove(group, r);
            }}
            onMouseEnter={() => onHover(report ?? null)}
            onMouseLeave={() => onHover(null)}
            title={report ? (excluded ? "rejected by the solver as an outlier: retake" : "click: retake · right-click: remove") : "click: take"}
          >
            {slot.status === "taking" ? (
              <Spinner />
            ) : report ? (
              <>
                <span className={`num ${excluded ? "line-through" : ""}`}>{report.index + 1}</span>
                {residual !== undefined && <span className="num text-[10px] text-ink-3">{residual.toFixed(3)}°</span>}
              </>
            ) : (
              "·"
            )}
          </button>
        );
      })}
    </>
  );
}

function CalMatrices({ data }: { data: CalibrationData }) {
  return (
    <div className="mt-3 max-h-80 overflow-auto rounded border border-line bg-ground p-2 text-[11px]">
      {data.sets.slice(0, data.n_sets).map((set, i) => (
        <div key={i} className="mb-3">
          <div className="mb-1 font-semibold text-ink-2">
            set {i + 1} · coils {set.mag_temp_c.map((t) => t.toFixed(1)).join("/")} °C · residual {set.residual_deg.toFixed(3)}°
          </div>
          {[...set.mag.map((a, k) => ({ name: `Mag ${k + 1}`, a })), ...set.acc.map((a, k) => ({ name: `Acc ${k + 1}`, a }))].map(({ name, a }) => (
            <div key={name} className="num mb-1 grid grid-cols-[4rem_1fr] gap-2">
              <span className="text-ink-3">{name}</span>
              <span>
                b = [{a.offset.x.toFixed(2)}, {a.offset.y.toFixed(2)}, {a.offset.z.toFixed(2)}]
                <br />
                {a.matrix.m.map((row, ri) => (
                  <span key={ri} className="block">
                    M{ri} = [{row.map((v) => v.toExponential(4)).join(", ")}]
                  </span>
                ))}
              </span>
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
