export function deg(v: number | null | undefined, digits = 2): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return "—";
  return `${v.toFixed(digits)}°`;
}

export function num(v: number | null | undefined, digits = 2, unit = ""): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return "—";
  return `${v.toFixed(digits)}${unit}`;
}

export function hex(v: number, width = 2): string {
  return `0x${v.toString(16).toUpperCase().padStart(width, "0")}`;
}

export function uidHex(uid: number[]): string {
  return uid.map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function unixTime(t: number): string {
  if (t === 0) return "not set";
  return new Date(t * 1000).toLocaleString();
}

export function uptime(ms: number): string {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return h > 0 ? `${h}h ${m}m ${sec}s` : m > 0 ? `${m}m ${sec}s` : `${sec}s`;
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / 1024 / 1024).toFixed(2)} MiB`;
}

export function clockTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour12: false });
}

/** CSV cell escaping. */
export function csv(v: string | number | null | undefined): string {
  if (v === null || v === undefined) return "";
  const s = String(v);
  return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}
