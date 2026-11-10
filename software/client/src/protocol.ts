/**
 * TypeScript mirror of `attinav-core::protocol` as serialised by serde (adjacent tagging:
 * `{ kind, data }`). Keep in sync with `software/firmware/PROTOCOL.md`.
 *
 * Floats that the firmware may set to NaN (temperatures, gyro peak) arrive as `null` because
 * serde_json has no NaN literal.
 */

export interface Vec3 {
  x: number;
  y: number;
  z: number;
}
export interface Mat3 {
  m: [number[], number[], number[]];
}

export type Status =
  | "Ok"
  | "Busy"
  | "InvalidArgument"
  | "Unsupported"
  | "SensorError"
  | "Timeout"
  | "NoCalibration"
  | "FlashError"
  | "SolveFailed"
  | "NotCalibrating"
  | "Aborted"
  | "BadPayload"
  | "Internal"
  | "BadImage";

export interface Ack {
  request: number;
  status: Status;
  detail: number;
}

export interface Info {
  protocol: number;
  firmware: [number, number, number];
  hardware_rev: number;
  uid: number[];
  uptime_ms: number;
  calibration_id: number;
  calibration_sets: number;
  calibration_date: number;
  sensors_ok: number;
  time_set: boolean;
  unix_time: number;
  boot_state: number;
}

export interface Thresholds {
  mag_disagree_deg: number;
  acc_disagree_deg: number;
  mag_norm_tol: number;
  acc_norm_tol: number;
  dip_tol_deg: number;
  motion_dps: number;
  mag_noise_rel: number;
  acc_noise_rel: number;
}

export interface DeviceConfig {
  mag_cycle_count: number;
  mag_samples: number;
  acc_rate_hz: number;
  quiet_lead_ms: number;
  quiet_enabled: boolean;
  led_enabled: boolean;
  thresholds: Thresholds;
  ref_b_total_ut: number;
  ref_dip_deg: number;
  uart_baud: number;
}

export const DEFAULT_CONFIG: DeviceConfig = {
  mag_cycle_count: 400,
  mag_samples: 100,
  acc_rate_hz: 200,
  quiet_lead_ms: 20,
  quiet_enabled: true,
  led_enabled: true,
  thresholds: {
    mag_disagree_deg: 0.5,
    acc_disagree_deg: 0.3,
    mag_norm_tol: 0.03,
    acc_norm_tol: 0.02,
    dip_tol_deg: 1.0,
    motion_dps: 0.5,
    mag_noise_rel: 0.004,
    acc_noise_rel: 0.01,
  },
  ref_b_total_ut: 0,
  ref_dip_deg: 0,
  uart_baud: 115200,
};

export const CYCLE_COUNT_MIN = 30;
export const CYCLE_COUNT_MAX = 4000;

/** Mirrors `DeviceConfig::is_valid`. Returns the offending field names. */
export function configProblems(c: DeviceConfig): string[] {
  const p: string[] = [];
  const inRange = (v: number, lo: number, hi: number) => Number.isFinite(v) && v >= lo && v <= hi && Number.isInteger(v);
  if (!inRange(c.mag_cycle_count, CYCLE_COUNT_MIN, CYCLE_COUNT_MAX)) p.push("mag_cycle_count");
  if (!inRange(c.mag_samples, 1, 4000)) p.push("mag_samples");
  if (!inRange(c.acc_rate_hz, 10, 2000)) p.push("acc_rate_hz");
  if (!inRange(c.quiet_lead_ms, 0, 5000)) p.push("quiet_lead_ms");
  if (!inRange(c.uart_baud, 9600, 2_000_000)) p.push("uart_baud");
  if (!Number.isFinite(c.ref_b_total_ut)) p.push("ref_b_total_ut");
  if (!Number.isFinite(c.ref_dip_deg)) p.push("ref_dip_deg");
  for (const [k, v] of Object.entries(c.thresholds)) {
    if (!Number.isFinite(v) || v < 0) p.push(`thresholds.${k}`);
  }
  return p;
}

export interface ShotRequest {
  flags: number;
  mag_samples: number;
  cycle_count: number;
}
export const SHOT_WANT_RAW = 1 << 0;
export const SHOT_NO_QUIET = 1 << 1;

export interface StreamRequest {
  enable: boolean;
  rate_hz: number;
  sensors: number;
}
export const STREAM = {
  MAG1: 1 << 0,
  MAG2: 1 << 1,
  MAG3: 1 << 2,
  ACC1: 1 << 3,
  ACC2: 1 << 4,
  IMU_GYRO: 1 << 5,
  IMU_ACC: 1 << 6,
  TEMPS: 1 << 7,
} as const;

export interface ShotResult {
  shot_id: number;
  flags: number;
  unix_time: number;
  azimuth_deg: number;
  inclination_deg: number;
  roll_deg: number;
  dip_deg: number;
  b_total_ut: number;
  g_norm: number;
  mag_spread_deg: number;
  acc_spread_deg: number;
  azimuth_sigma_deg: number;
  inclination_sigma_deg: number;
  gyro_peak_dps: number | null;
  mag_temp_c: (number | null)[];
  acc_temp_c: (number | null)[];
  calibration_id: number;
  mag_samples: number;
  acc_samples: number;
  duration_ms: number;
}

export interface ShotRaw {
  shot_id: number;
  sensor: number;
  count: number;
  mean: [number, number, number];
  std_dev: [number, number, number];
  calibrated: [number, number, number];
}

export interface Sample {
  t_ms: number;
  sensor: number;
  raw: [number, number, number];
}

export interface Temps {
  t_ms: number;
  mag_temp_c: (number | null)[];
  acc_temp_c: (number | null)[];
  imu_temp_c: number | null;
  vdda_mv: number;
}

export interface CalShot {
  group: number;
  roll: number;
  mag: Vec3[];
  acc: Vec3[];
  mag_temp_c: (number | null)[];
  acc_temp_c: (number | null)[];
}

export interface CalShotReport {
  index: number;
  count: number;
  flags: number;
  shot: CalShot;
}

export type SolveError =
  | "NotEnoughShots"
  | "GroupTooLarge"
  | "BadInput"
  | "Singular"
  | "NotConverged"
  | "ResidualTooLarge";

export interface SolveReport {
  iterations: number;
  cost: number;
  residual_deg: number;
  mag_residual_deg: number[];
  acc_residual_deg: number[];
  shots: number;
}

export interface CalSolved {
  error: SolveError | null;
  report: SolveReport;
  dip_deg: number;
  field_ut: number;
}

export interface CalSetSummary {
  mag_temp_c: number;
  acc_temp_c: number;
  residual_deg: number;
  shots: number;
  field_ut: number;
  dip_deg: number;
}

export interface CalInfo {
  id: number;
  date_unix: number;
  mag_cycle_count: number;
  n_sets: number;
  sets: CalSetSummary[];
  buffered_shots: number;
  session_open: boolean;
  pending_set: boolean;
}

export interface Affine {
  offset: Vec3;
  matrix: Mat3;
}

export interface SensorSet {
  mag_temp_c: number[];
  acc_temp_c: number[];
  mag: Affine[];
  acc: Affine[];
  dip_deg: number;
  field_ut: number;
  residual_deg: number;
  shots: number;
}

export interface Calibration {
  id: number;
  date_unix: number;
  mag_cycle_count: number;
  n_sets: number;
  sets: SensorSet[];
}

export type EventKind =
  | "Boot"
  | "CalibrationLoaded"
  | "CalibrationMissing"
  | "ShotStarted"
  | "ShotFinished"
  | "StreamStopped"
  | "SensorFault";

export interface DeviceEvent {
  kind: EventKind;
  detail: number;
}

export interface LogLine {
  level: number;
  len: number;
  text: string;
}

export interface FwBegin {
  size: number;
  crc32: number;
  version: [number, number, number];
}

export type DeviceMessage =
  | { kind: "Ack"; data: Ack }
  | { kind: "Info"; data: Info }
  | { kind: "Config"; data: DeviceConfig }
  | { kind: "ShotResult"; data: ShotResult }
  | { kind: "ShotRaw"; data: ShotRaw }
  | { kind: "Sample"; data: Sample }
  | { kind: "Temps"; data: Temps }
  | { kind: "CalShotDone"; data: CalShotReport }
  | { kind: "CalShotData"; data: CalShotReport }
  | { kind: "CalSolved"; data: CalSolved }
  | { kind: "CalData"; data: Calibration }
  | { kind: "CalInfo"; data: CalInfo }
  | { kind: "Log"; data: LogLine }
  | { kind: "Event"; data: DeviceEvent };

export type HostMessage =
  | { kind: "Ping" }
  | { kind: "GetInfo" }
  | { kind: "SetTime"; data: number }
  | { kind: "Reset" }
  | { kind: "Shot"; data: ShotRequest }
  | { kind: "Abort" }
  | { kind: "Stream"; data: StreamRequest }
  | { kind: "GetConfig" }
  | { kind: "SetConfig"; data: DeviceConfig }
  | { kind: "CalBegin" }
  | { kind: "CalShot"; data: { group: number; roll: number } }
  | { kind: "CalRemove"; data: { group: number; roll: number } }
  | { kind: "CalSolve" }
  | { kind: "CalStore"; data: { id: number } }
  | { kind: "CalLoad" }
  | { kind: "CalGet" }
  | { kind: "CalSet"; data: Calibration }
  | { kind: "CalClear" }
  | { kind: "CalInfo" }
  | { kind: "CalGetShots" }
  | { kind: "CalEnd" }
  | { kind: "FwBegin"; data: FwBegin }
  | { kind: "FwFinish" }
  | { kind: "FwAbort" };

export type DeviceKind = DeviceMessage["kind"];
export type Extract<K extends DeviceKind> = globalThis.Extract<DeviceMessage, { kind: K }>["data"];

// ---------------------------------------------------------------------------------------------
// Quality flags
// ---------------------------------------------------------------------------------------------

export interface FlagInfo {
  bit: number;
  name: string;
  severe: boolean;
  description: string;
}

export const QUALITY_FLAGS: readonly FlagInfo[] = [
  { bit: 0, name: "NO_CALIBRATION", severe: true, description: "Nominal gains used; angles are rough" },
  { bit: 1, name: "TEMP_EXTRAPOLATED", severe: false, description: "Temperature outside the calibrated range" },
  { bit: 2, name: "MAG_DISAGREE", severe: true, description: "Triads disagree: nearby iron" },
  { bit: 3, name: "ACC_DISAGREE", severe: true, description: "Accelerometers disagree" },
  { bit: 4, name: "MAG_NORM", severe: false, description: "|B| off the reference: distant iron or anomaly" },
  { bit: 5, name: "ACC_NORM", severe: false, description: "|g| off nominal: motion or fault" },
  { bit: 6, name: "DIP", severe: false, description: "Dip differs from the reference" },
  { bit: 7, name: "MOTION", severe: true, description: "Gyro saw motion during the window" },
  { bit: 8, name: "MAG_NOISY", severe: false, description: "A triad was noisy" },
  { bit: 9, name: "ACC_NOISY", severe: false, description: "An accelerometer was noisy" },
  { bit: 10, name: "SENSOR_FAULT", severe: true, description: "A sensor delivered no data" },
  { bit: 11, name: "QUIET_SKIPPED", severe: false, description: "QUIET was not asserted" },
  { bit: 12, name: "SHORT", severe: false, description: "Fewer samples than requested" },
  { bit: 13, name: "CYCLE_COUNT_MISMATCH", severe: false, description: "Shot cycle count ≠ calibration cycle count" },
];

export function flagsOf(bits: number): FlagInfo[] {
  return QUALITY_FLAGS.filter((f) => (bits & (1 << f.bit)) !== 0);
}

export function isSevere(bits: number): boolean {
  return flagsOf(bits).some((f) => f.severe);
}

// ---------------------------------------------------------------------------------------------
// Sensors
// ---------------------------------------------------------------------------------------------

export const SENSOR_NAMES: readonly string[] = ["Mag 1", "Mag 2", "Mag 3", "Acc 1", "Acc 2", "IMU"];
export const SAMPLE_SOURCE_NAMES: readonly string[] = ["Mag 1", "Mag 2", "Mag 3", "Acc 1", "Acc 2", "IMU gyro", "IMU acc"];

export function sensorName(tag: number): string {
  return SENSOR_NAMES[tag] ?? `sensor ${tag}`;
}

export function sampleSourceName(tag: number): string {
  return SAMPLE_SOURCE_NAMES[tag] ?? `source ${tag}`;
}

export const BOOT_STATES: Record<number, string> = {
  0: "confirmed",
  1: "trial boot (unconfirmed)",
  2: "reverted after failed trial",
  3: "DFU detach requested",
  255: "unknown",
};

export const CAL_GROUPS = 14;
export const CAL_ROLLS = 4;

/** Rough wall-clock duration of a shot, ms (see PROTOCOL.md, `ShotRequest`). */
export function shotDurationMs(magSamples: number, cycleCount: number, quietLeadMs: number): number {
  const rateHz = 88_000 / 3 / Math.max(1, cycleCount);
  return (magSamples / rateHz) * 1000 + quietLeadMs;
}

export function describeMessage(m: DeviceMessage): string {
  switch (m.kind) {
    case "Ack":
      return `Ack ${m.data.status} to 0x${m.data.request.toString(16).padStart(2, "0")} (detail ${m.data.detail})`;
    case "Info":
      return `Info fw ${m.data.firmware.join(".")} sensors 0x${m.data.sensors_ok.toString(16)} boot ${m.data.boot_state}`;
    case "Config":
      return `Config cc ${m.data.mag_cycle_count} samples ${m.data.mag_samples}`;
    case "ShotResult":
      return `Shot #${m.data.shot_id} az ${m.data.azimuth_deg.toFixed(2)} inc ${m.data.inclination_deg.toFixed(2)} flags 0x${m.data.flags.toString(16)}`;
    case "ShotRaw":
      return `ShotRaw #${m.data.shot_id} ${sensorName(m.data.sensor)} n=${m.data.count}`;
    case "Sample":
      return `Sample ${sampleSourceName(m.data.sensor)} ${m.data.raw.join(", ")}`;
    case "Temps":
      return `Temps mag ${m.data.mag_temp_c.map((t) => (t ?? NaN).toFixed(1)).join("/")} vdda ${m.data.vdda_mv} mV`;
    case "CalShotDone":
    case "CalShotData":
      return `${m.kind} g${m.data.shot.group} r${m.data.shot.roll} (${m.data.count} buffered)`;
    case "CalSolved":
      return m.data.error ? `CalSolved error ${m.data.error}` : `CalSolved residual ${m.data.report.residual_deg.toFixed(3)}°`;
    case "CalData":
      return `CalData id ${m.data.id} sets ${m.data.n_sets}`;
    case "CalInfo":
      return `CalInfo id ${m.data.id} sets ${m.data.n_sets} buffered ${m.data.buffered_shots}`;
    case "Log":
      return `Log[${m.data.level}] ${m.data.text}`;
    case "Event":
      return `Event ${m.data.kind} (${m.data.detail})`;
  }
}
