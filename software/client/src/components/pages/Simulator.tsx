import { useEffect, useState } from "react";
import { api, toError, type SimControl } from "../../api";
import { useAppDispatch, useAppSelector } from "../../store";
import { toast } from "../../store/uiSlice";
import { Card, Field, NumberInput, Toggle } from "../ui";

export function Simulator() {
  const dispatch = useAppDispatch();
  const simulator = useAppSelector((s) => s.link.simulator);
  const [ctl, setCtl] = useState<SimControl | null>(null);

  useEffect(() => {
    api
      .simGet()
      .then(setCtl)
      .catch((e: unknown) => dispatch(toast({ level: "error", text: toError(e).message })));
  }, [dispatch]);

  const push = async (next: SimControl) => {
    setCtl(next);
    try {
      await api.simSet(next);
    } catch (e) {
      dispatch(toast({ level: "error", text: toError(e).message }));
    }
  };
  if (!simulator) return <div className="text-sm text-ink-3">The simulator page is only available while connected to the simulator.</div>;
  if (!ctl) return null;
  const set = (patch: Partial<SimControl>) => void push({ ...ctl, ...patch });
  const n = (v: number, fallback: number) => (Number.isFinite(v) ? v : fallback);

  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
      <Card title="Pose (truth)">
        <p className="mb-3 text-xs text-ink-3">
          The attitude the simulated instrument actually has. Shots should reproduce it once a calibration is stored; before that, the nominal-gain result will be off by the simulated misalignments (a degree or two).
        </p>
        <div className="grid grid-cols-3 gap-4">
          <Field label="Azimuth (°)">
            <NumberInput value={ctl.azimuth_deg} step={0.5} onChange={(v) => set({ azimuth_deg: n(v, 0) })} />
          </Field>
          <Field label="Inclination (°)">
            <NumberInput value={ctl.inclination_deg} step={0.5} min={-90} max={90} onChange={(v) => set({ inclination_deg: n(v, 0) })} />
          </Field>
          <Field label="Roll (°)">
            <NumberInput value={ctl.roll_deg} step={1} onChange={(v) => set({ roll_deg: n(v, 0) })} />
          </Field>
        </div>
        <input type="range" className="mt-3 w-full accent-accent" min={0} max={359.5} step={0.5} value={ctl.azimuth_deg} onChange={(e) => set({ azimuth_deg: Number(e.target.value) })} />
        <Toggle
          checked={ctl.auto_pose_for_calibration}
          onChange={(v) => set({ auto_pose_for_calibration: v })}
          label="Calibration shots take a well-spread pose derived from (group, roll) instead of this one"
        />
      </Card>

      <Card title="Site">
        <div className="grid grid-cols-3 gap-4">
          <Field label="|B| (µT)">
            <NumberInput value={ctl.field_ut} step={0.1} min={1} onChange={(v) => set({ field_ut: n(v, 48) })} />
          </Field>
          <Field label="Dip (°)">
            <NumberInput value={ctl.dip_deg} step={0.1} min={-90} max={90} onChange={(v) => set({ dip_deg: n(v, 63) })} />
          </Field>
          <Field label="Temperature (°C)">
            <NumberInput value={ctl.temp_c} step={0.5} onChange={(v) => set({ temp_c: n(v, 11) })} />
          </Field>
        </div>
      </Card>

      <Card title="Disturbances">
        <div className="grid grid-cols-3 gap-4">
          <Field label="Mag noise (counts/sample)" hint="12 ≈ datasheet at CC 400">
            <NumberInput value={ctl.mag_noise_counts} step={1} min={0} onChange={(v) => set({ mag_noise_counts: n(v, 12) })} />
          </Field>
          <Field label="Acc noise (counts/sample)">
            <NumberInput value={ctl.acc_noise_counts} step={0.5} min={0} onChange={(v) => set({ acc_noise_counts: n(v, 4) })} />
          </Field>
          <Field label="Motion (°/s)" hint="> motion threshold flags MOTION">
            <NumberInput value={ctl.motion_dps} step={0.05} min={0} onChange={(v) => set({ motion_dps: n(v, 0.05) })} />
          </Field>
          <Field label="Nearby iron on triad 1 (µT)" hint="gradient: triads disagree">
            <NumberInput value={ctl.nearby_iron_ut} step={0.1} onChange={(v) => set({ nearby_iron_ut: n(v, 0) })} />
          </Field>
        </div>
      </Card>

      <Card title="Sensor faults">
        <p className="mb-2 text-xs text-ink-3">Uncheck a sensor to make it absent (no data; the device reports it missing in Info.sensors_ok).</p>
        <div className="flex flex-wrap gap-4">
          {["Mag 1", "Mag 2", "Mag 3", "Acc 1", "Acc 2", "IMU"].map((name, i) => (
            <Toggle key={name} label={name} checked={(ctl.sensors_ok & (1 << i)) !== 0} onChange={(v) => set({ sensors_ok: v ? ctl.sensors_ok | (1 << i) : ctl.sensors_ok & ~(1 << i) })} />
          ))}
        </div>
      </Card>
    </div>
  );
}
