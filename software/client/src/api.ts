/**
 * Typed wrappers around the Tauri commands and events exposed by `src-tauri/src/commands.rs`.
 * Everything that crosses the IPC boundary is cast exactly once, here.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { DeviceConfig, DeviceKind, DeviceMessage, Extract, HostMessage } from "./protocol";

export interface PortInfo {
  path: string;
  kind: string;
  vid: number | null;
  pid: number | null;
  manufacturer: string | null;
  product: string | null;
  serial: string | null;
  is_attinav: boolean;
}

export type ConnectTarget = { kind: "serial"; path: string; baud: number } | { kind: "simulator" };

export interface LinkStatus {
  connected: boolean;
  description: string | null;
  simulator: boolean;
}

export interface AppInfo {
  version: string;
  protocol_version: number;
  core_firmware_version: [number, number, number];
  cal_groups: number;
  cal_rolls: number;
  max_sets: number;
  fw_chunk_max: number;
}

export interface FirmwareFile {
  size: number;
  crc32: number;
}

export interface FwProgress {
  stage: "preflight" | "begin" | "transfer" | "verify" | "done" | "failed";
  sent: number;
  total: number;
  message: string | null;
}

export interface SimControl {
  azimuth_deg: number;
  inclination_deg: number;
  roll_deg: number;
  field_ut: number;
  dip_deg: number;
  mag_noise_counts: number;
  acc_noise_counts: number;
  motion_dps: number;
  temp_c: number;
  nearby_iron_ut: number;
  sensors_ok: number;
  auto_pose_for_calibration: boolean;
}

export interface MessageEvent {
  seq: number;
  message: DeviceMessage;
}

export interface LinkEvent {
  connected: boolean;
  description: string;
  simulator: boolean;
  reason: string | null;
}

/** Error shape produced by `ClientError`'s `Serialize` impl. */
export interface ClientErrorPayload {
  kind: string;
  message: string;
  request?: number;
  status?: string;
  detail?: number;
}

export class ClientError extends Error {
  readonly kind: string;
  readonly status: string | undefined;
  readonly detail: number | undefined;
  constructor(p: ClientErrorPayload) {
    super(p.message);
    this.kind = p.kind;
    this.status = p.status;
    this.detail = p.detail;
  }
}

function isPayload(e: unknown): e is ClientErrorPayload {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/** Normalises anything thrown across the IPC boundary into a `ClientError`. */
export function toError(e: unknown): ClientError {
  if (e instanceof ClientError) return e;
  if (isPayload(e)) return new ClientError(e);
  if (e instanceof Error) return new ClientError({ kind: "js", message: e.message });
  return new ClientError({ kind: "unknown", message: String(e) });
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toError(e);
  }
}

async function callVoid(cmd: string, args?: Record<string, unknown>): Promise<void> {
  await call<unknown>(cmd, args);
}

export const api = {
  appInfo: () => call<AppInfo>("app_info"),
  listPorts: () => call<PortInfo[]>("list_ports"),
  connect: (target: ConnectTarget) => call<LinkStatus>("connect", { target }),
  disconnect: () => callVoid("disconnect"),
  linkStatus: () => call<LinkStatus>("link_status"),
  request: (message: HostMessage, timeoutMs?: number) =>
    call<DeviceMessage[]>("request", { message, timeoutMs: timeoutMs ?? null }),
  firmwareInspect: (path: string) => call<FirmwareFile>("firmware_inspect", { path }),
  firmwareUpdate: (path: string, version: [number, number, number]) => callVoid("firmware_update", { path, version }),
  readJson: (path: string) => call<unknown>("read_json", { path }),
  writeJson: (path: string, value: unknown) => callVoid("write_json", { path, value }),
  writeText: (path: string, text: string) => callVoid("write_text", { path, text }),
  simGet: () => call<SimControl>("sim_get"),
  simSet: (control: SimControl) => callVoid("sim_set", { control }),
};

/**
 * Sends a request and returns the data of the first reply of the wanted kind. An `Ack` whose
 * status is not `Ok` becomes a `ClientError` of kind `nack`; an `Ack Ok` where another kind was
 * expected becomes `unexpected`.
 */
export async function expect<K extends DeviceKind>(message: HostMessage, kind: K, timeoutMs?: number): Promise<Extract<K>> {
  const replies = await api.request(message, timeoutMs);
  for (const r of replies) {
    if (r.kind === kind) return r.data as Extract<K>;
    if (r.kind === "Ack" && r.data.status !== "Ok") {
      throw new ClientError({
        kind: "nack",
        message: `device answered ${r.data.status} to ${message.kind} (detail ${r.data.detail})`,
        request: r.data.request,
        status: r.data.status,
        detail: r.data.detail,
      });
    }
  }
  throw new ClientError({ kind: "unexpected", message: `no ${kind} in the reply to ${message.kind}` });
}

/** Sends a request that is answered by an `Ack`; returns its detail. */
export async function ack(message: HostMessage, timeoutMs?: number): Promise<number> {
  const a = await expect(message, "Ack", timeoutMs);
  if (a.status !== "Ok") {
    throw new ClientError({
      kind: "nack",
      message: `device answered ${a.status} to ${message.kind} (detail ${a.detail})`,
      request: a.request,
      status: a.status,
      detail: a.detail,
    });
  }
  return a.detail;
}

export function onDeviceMessage(cb: (e: MessageEvent) => void): Promise<UnlistenFn> {
  return listen<MessageEvent>("device:message", (ev) => {
    cb(ev.payload);
  });
}

export function onLink(cb: (e: LinkEvent) => void): Promise<UnlistenFn> {
  return listen<LinkEvent>("device:link", (ev) => {
    cb(ev.payload);
  });
}

export function onFwProgress(cb: (e: FwProgress) => void): Promise<UnlistenFn> {
  return listen<FwProgress>("fw:progress", (ev) => {
    cb(ev.payload);
  });
}

/** Timeout generous enough for a shot with the given parameters. */
export function shotTimeout(magSamples: number, cycleCount: number, config: DeviceConfig): number {
  const samples = magSamples === 0 ? config.mag_samples : magSamples;
  const cc = cycleCount === 0 ? config.mag_cycle_count : cycleCount;
  const rateHz = 88_000 / 3 / Math.max(1, cc);
  return Math.ceil((samples / rateHz) * 1000 + config.quiet_lead_ms + 4000);
}
