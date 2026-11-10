import { useEffect, useState } from "react";
import { ack, api, toError } from "../../api";
import { CYCLE_COUNT_MAX, CYCLE_COUNT_MIN, DEFAULT_CONFIG, configProblems, type DeviceConfig, type Thresholds } from "../../protocol";
import { useAppDispatch, useAppSelector } from "../../store";
import { toast } from "../../store/uiSlice";
import { Badge, Card, Field, NumberInput, Toggle } from "../ui";

const THRESHOLD_FIELDS: { key: keyof Thresholds; label: string; hint: string; step: number }[] = [
  { key: "mag_disagree_deg", label: "Mag disagree (°)", hint: "max angle between triads", step: 0.05 },
  { key: "acc_disagree_deg", label: "Acc disagree (°)", hint: "max angle between accelerometers", step: 0.05 },
  { key: "mag_norm_tol", label: "|B| tolerance", hint: "relative, vs reference / calibration", step: 0.005 },
  { key: "acc_norm_tol", label: "|g| tolerance", hint: "relative", step: 0.005 },
  { key: "dip_tol_deg", label: "Dip tolerance (°)", hint: "vs reference", step: 0.1 },
  { key: "motion_dps", label: "Motion (°/s)", hint: "gyro peak during the window", step: 0.05 },
  { key: "mag_noise_rel", label: "Mag noise", hint: "per-axis σ relative to |B|", step: 0.001 },
  { key: "acc_noise_rel", label: "Acc noise", hint: "per-axis σ relative to g", step: 0.001 },
];

export function Config() {
  const dispatch = useAppDispatch();
  const connected = useAppSelector((s) => s.link.connected);
  const deviceConfig = useAppSelector((s) => s.device.config);
  const fromDevice = useAppSelector((s) => s.device.configFromDevice);
  const [draft, setDraft] = useState<DeviceConfig>(deviceConfig);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setDraft(deviceConfig);
  }, [deviceConfig]);

  const problems = configProblems(draft);
  const dirty = JSON.stringify(draft) !== JSON.stringify(deviceConfig);
  const set = (patch: Partial<DeviceConfig>) => setDraft((d) => ({ ...d, ...patch }));
  const setThr = (key: keyof Thresholds, v: number) => setDraft((d) => ({ ...d, thresholds: { ...d.thresholds, [key]: v } }));

  const apply = async () => {
    setBusy(true);
    try {
      await ack({ kind: "SetConfig", data: draft }, 10000);
      await api.request({ kind: "GetConfig" });
      dispatch(toast({ level: "success", text: "configuration applied and saved to flash" }));
    } catch (e) {
      dispatch(toast({ level: "error", text: `apply: ${toError(e).message}` }));
    } finally {
      setBusy(false);
    }
  };
  const reload = async () => {
    try {
      await api.request({ kind: "GetConfig" });
    } catch (e) {
      dispatch(toast({ level: "error", text: toError(e).message }));
    }
  };

  const invalid = (k: string) => problems.includes(k);
  const shotSeconds = (draft.mag_samples * 3 * draft.mag_cycle_count) / 88000 + draft.quiet_lead_ms / 1000;

  return (
    <div className="flex flex-col gap-4">
      <Card
        title="Measurement"
        actions={
          <>
            {!fromDevice && <Badge tone="warn">defaults, not read from device</Badge>}
            {dirty && <Badge tone="accent">unsaved changes</Badge>}
            <button className="btn btn-sm" disabled={!connected} onClick={() => void reload()}>
              Reload
            </button>
            <button className="btn btn-sm" onClick={() => setDraft(DEFAULT_CONFIG)}>
              Defaults
            </button>
            <button className="btn btn-sm btn-primary" disabled={!connected || busy || problems.length > 0 || !dirty} onClick={() => void apply()}>
              Apply to device
            </button>
          </>
        }
      >
        <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
          <Field label="RM3100 cycle count" hint={`${CYCLE_COUNT_MIN}–${CYCLE_COUNT_MAX}; 400 ≈ 73 Hz, 148 LSB/µT`} invalid={invalid("mag_cycle_count")}>
            <NumberInput value={draft.mag_cycle_count} min={CYCLE_COUNT_MIN} max={CYCLE_COUNT_MAX} invalid={invalid("mag_cycle_count")} onChange={(v) => set({ mag_cycle_count: v })} />
          </Field>
          <Field label="Mag samples per shot" hint="1–4000" invalid={invalid("mag_samples")}>
            <NumberInput value={draft.mag_samples} min={1} max={4000} invalid={invalid("mag_samples")} onChange={(v) => set({ mag_samples: v })} />
          </Field>
          <Field label="Accelerometer rate (Hz)" hint="10–2000 during a shot" invalid={invalid("acc_rate_hz")}>
            <NumberInput value={draft.acc_rate_hz} min={10} max={2000} invalid={invalid("acc_rate_hz")} onChange={(v) => set({ acc_rate_hz: v })} />
          </Field>
          <Field label="QUIET lead (ms)" hint="settle time after asserting QUIET" invalid={invalid("quiet_lead_ms")}>
            <NumberInput value={draft.quiet_lead_ms} min={0} max={5000} invalid={invalid("quiet_lead_ms")} onChange={(v) => set({ quiet_lead_ms: v })} />
          </Field>
          <div className="flex flex-col justify-center gap-2">
            <Toggle checked={draft.quiet_enabled} onChange={(v) => set({ quiet_enabled: v })} label="Assert QUIET during shots" />
            <Toggle checked={draft.led_enabled} onChange={(v) => set({ led_enabled: v })} label="Status LED" />
          </div>
          <div className="col-span-2 flex items-center text-sm text-ink-2 md:col-span-3">
            A shot with these settings lasts about <span className="num mx-1 text-ink">{shotSeconds.toFixed(2)} s</span>
            {shotSeconds > 10 && <Badge tone="warn">long</Badge>}
          </div>
        </div>
      </Card>

      <Card title="Quality thresholds">
        <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
          {THRESHOLD_FIELDS.map((f) => (
            <Field key={f.key} label={f.label} hint={f.hint} invalid={invalid(`thresholds.${f.key}`)}>
              <NumberInput value={draft.thresholds[f.key]} step={f.step} min={0} invalid={invalid(`thresholds.${f.key}`)} onChange={(v) => setThr(f.key, v)} />
            </Field>
          ))}
        </div>
      </Card>

      <Card title="Site reference and carrier link">
        <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
          <Field label="Reference |B| (µT)" hint="IGRF/WMM value for the cave; 0 = use the calibration site" invalid={invalid("ref_b_total_ut")}>
            <NumberInput value={draft.ref_b_total_ut} step={0.1} min={0} invalid={invalid("ref_b_total_ut")} onChange={(v) => set({ ref_b_total_ut: v })} />
          </Field>
          <Field label="Reference dip (°)" hint="positive = field points down" invalid={invalid("ref_dip_deg")}>
            <NumberInput value={draft.ref_dip_deg} step={0.1} invalid={invalid("ref_dip_deg")} onChange={(v) => set({ ref_dip_deg: v })} />
          </Field>
          <Field label="UART baud" hint="carrier link; applied at the next boot" invalid={invalid("uart_baud")}>
            <select className={`input num ${invalid("uart_baud") ? "input-invalid" : ""}`} value={draft.uart_baud} onChange={(e) => set({ uart_baud: Number(e.target.value) })}>
              {[9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600, 1000000, 2000000].map((b) => (
                <option key={b} value={b}>
                  {b}
                </option>
              ))}
            </select>
          </Field>
        </div>
        {problems.length > 0 && (
          <div className="mt-3 text-xs text-bad">
            Invalid: {problems.join(", ")}
          </div>
        )}
      </Card>
    </div>
  );
}
