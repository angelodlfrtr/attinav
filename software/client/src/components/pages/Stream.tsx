import { useMemo } from "react";
import { ack, toError } from "../../api";
import { STREAM, sampleSourceName } from "../../protocol";
import { streamBuffers } from "../../streamBuffers";
import { useAppDispatch, useAppSelector } from "../../store";
import { streamOptions, streamStarted, streamStopped } from "../../store/streamSlice";
import { toast } from "../../store/uiSlice";
import { num } from "../../util/format";
import { TimeSeries, type Series } from "../charts/TimeSeries";
import { Card, Field, NumberInput, Stat, Toggle } from "../ui";

const SENSOR_BITS: { bit: number; label: string; tag: number }[] = [
  { bit: STREAM.MAG1, label: "Mag 1", tag: 0 },
  { bit: STREAM.MAG2, label: "Mag 2", tag: 1 },
  { bit: STREAM.MAG3, label: "Mag 3", tag: 2 },
  { bit: STREAM.ACC1, label: "Acc 1", tag: 3 },
  { bit: STREAM.ACC2, label: "Acc 2", tag: 4 },
  { bit: STREAM.IMU_GYRO, label: "IMU gyro", tag: 5 },
  { bit: STREAM.IMU_ACC, label: "IMU acc", tag: 6 },
];

const AXIS_COLORS = ["#f87171", "#34d399", "#38bdf8"];

export function Stream() {
  const dispatch = useAppDispatch();
  const connected = useAppSelector((s) => s.link.connected);
  const stream = useAppSelector((s) => s.stream);

  const start = async () => {
    try {
      streamBuffers.clear();
      const rate = await ack({ kind: "Stream", data: { enable: true, rate_hz: stream.rateHz, sensors: stream.sensors } });
      dispatch(streamStarted({ effectiveRateHz: rate }));
    } catch (e) {
      dispatch(toast({ level: "error", text: `stream: ${toError(e).message}` }));
    }
  };
  const stop = async () => {
    try {
      await ack({ kind: "Stream", data: { enable: false, rate_hz: 0, sensors: 0 } });
      dispatch(streamStopped());
    } catch (e) {
      dispatch(toast({ level: "error", text: `stream: ${toError(e).message}` }));
    }
  };

  const charts = useMemo(() => {
    const out: { tag: number; series: Series[] }[] = [];
    for (const s of SENSOR_BITS) {
      if ((stream.sensors & s.bit) === 0) continue;
      const rings = streamBuffers.sensor(s.tag);
      out.push({
        tag: s.tag,
        series: (["x", "y", "z"] as const).map((axis, i) => ({
          label: axis.toUpperCase(),
          color: AXIS_COLORS[i] ?? "#fff",
          ring: () => rings[axis],
        })),
      });
    }
    return out;
  }, [stream.sensors]);

  const temps = stream.temps;

  return (
    <div className="flex flex-col gap-4">
      <Card title="Raw streaming">
        <div className="flex flex-wrap items-end gap-4">
          <div className="flex flex-wrap gap-3">
            {SENSOR_BITS.map((s) => (
              <Toggle
                key={s.bit}
                label={s.label}
                checked={(stream.sensors & s.bit) !== 0}
                disabled={stream.running}
                onChange={(v) => dispatch(streamOptions({ sensors: v ? stream.sensors | s.bit : stream.sensors & ~s.bit }))}
              />
            ))}
            <Toggle label="Temperatures" checked={(stream.sensors & STREAM.TEMPS) !== 0} disabled={stream.running} onChange={(v) => dispatch(streamOptions({ sensors: v ? stream.sensors | STREAM.TEMPS : stream.sensors & ~STREAM.TEMPS }))} />
          </div>
          <div className="w-28">
            <Field label="Rate (Hz)" hint="1–500; mags cap it">
              <NumberInput value={stream.rateHz} min={1} max={500} disabled={stream.running} onChange={(v) => dispatch(streamOptions({ rateHz: Number.isFinite(v) ? Math.round(v) : 1 }))} />
            </Field>
          </div>
          <div className="w-28">
            <Field label="Window (s)">
              <NumberInput value={stream.windowSeconds} min={1} max={120} onChange={(v) => dispatch(streamOptions({ windowSeconds: Number.isFinite(v) && v > 0 ? v : 10 }))} />
            </Field>
          </div>
          <div className="flex gap-2">
            {stream.running ? (
              <button className="btn btn-danger" onClick={() => void stop()}>
                Stop
              </button>
            ) : (
              <button className="btn btn-primary" disabled={!connected || (stream.sensors & 0x7f) === 0} onClick={() => void start()}>
                Start
              </button>
            )}
          </div>
          <div className="ml-auto flex gap-6">
            <Stat label="Device rate" value={stream.effectiveRateHz ?? "—"} unit="Hz" />
            <Stat label="Measured" value={stream.measuredRateHz.toFixed(0)} unit="samples/s" />
            <Stat label="Total" value={stream.total} />
          </div>
        </div>
      </Card>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        {charts.map((c) => (
          <TimeSeries key={c.tag} title={`${sampleSourceName(c.tag)} — raw counts`} series={c.series} windowSeconds={stream.windowSeconds} />
        ))}
      </div>

      {(stream.sensors & STREAM.TEMPS) !== 0 && (
        <Card title="Temperatures (1 Hz)">
          {temps ? (
            <div className="grid grid-cols-3 gap-4 md:grid-cols-7">
              {temps.mag_temp_c.map((t, i) => (
                <Stat key={`m${i}`} label={`Coil ${i + 1}`} value={num(t, 2)} unit="°C" tone={t === null ? "bad" : undefined} />
              ))}
              {temps.acc_temp_c.map((t, i) => (
                <Stat key={`a${i}`} label={`Acc ${i + 1} die`} value={num(t, 2)} unit="°C" tone={t === null ? "bad" : undefined} />
              ))}
              <Stat label="IMU die" value={num(temps.imu_temp_c, 2)} unit="°C" />
              <Stat label="VDDA" value={temps.vdda_mv} unit="mV" tone={temps.vdda_mv < 3150 || temps.vdda_mv > 3450 ? "warn" : undefined} />
            </div>
          ) : (
            <div className="text-sm text-ink-3">waiting for the first Temps frame…</div>
          )}
        </Card>
      )}
    </div>
  );
}
