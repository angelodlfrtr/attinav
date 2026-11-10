import type { ReactNode } from "react";
import { flagsOf, type FlagInfo } from "../protocol";

export function Card({ title, children, className = "", actions }: { title?: string; children: ReactNode; className?: string; actions?: ReactNode }) {
  return (
    <section className={`card ${className}`}>
      {(title ?? actions) && (
        <div className="mb-3 flex items-center justify-between gap-2">
          {title && <h2 className="card-title mb-0">{title}</h2>}
          {actions && <div className="flex items-center gap-2">{actions}</div>}
        </div>
      )}
      {children}
    </section>
  );
}

export type Tone = "ok" | "warn" | "bad" | "neutral" | undefined;

export function Stat({ label, value, unit, sub, tone }: { label: string; value: ReactNode; unit?: string | undefined; sub?: ReactNode; tone?: Tone }) {
  const color = tone === "bad" ? "text-bad" : tone === "warn" ? "text-warn" : tone === "ok" ? "text-ok" : "text-ink";
  return (
    <div className="min-w-0">
      <div className="text-[11px] uppercase tracking-wider text-ink-3">{label}</div>
      <div className={`num truncate text-xl ${color}`}>
        {value}
        {unit && <span className="ml-1 text-sm text-ink-3">{unit}</span>}
      </div>
      {sub && <div className="text-xs text-ink-3">{sub}</div>}
    </div>
  );
}

export function Field({ label, children, hint, invalid }: { label: string; children: ReactNode; hint?: string; invalid?: boolean }) {
  return (
    <label className="block">
      <span className={`label ${invalid ? "text-bad" : ""}`}>{label}</span>
      {children}
      {hint && <span className="mt-0.5 block text-[11px] text-ink-3">{hint}</span>}
    </label>
  );
}

export function NumberInput({
  value,
  onChange,
  step,
  min,
  max,
  invalid,
  disabled,
}: {
  value: number;
  onChange: (v: number) => void;
  step?: number;
  min?: number;
  max?: number;
  invalid?: boolean;
  disabled?: boolean;
}) {
  return (
    <input
      type="number"
      className={`input num ${invalid ? "input-invalid" : ""}`}
      value={Number.isFinite(value) ? value : ""}
      step={step}
      min={min}
      max={max}
      disabled={disabled}
      onChange={(e) => {
        onChange(e.target.value === "" ? NaN : Number(e.target.value));
      }}
    />
  );
}

export function Toggle({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <label className={`flex items-center gap-2 text-sm ${disabled ? "opacity-40" : "cursor-pointer"}`}>
      <input
        type="checkbox"
        className="h-4 w-4 accent-accent"
        checked={checked}
        disabled={disabled}
        onChange={(e) => {
          onChange(e.target.checked);
        }}
      />
      {label}
    </label>
  );
}

export function Badge({ children, tone = "neutral", title }: { children: ReactNode; tone?: "neutral" | "ok" | "warn" | "bad" | "accent"; title?: string }) {
  const cls =
    tone === "ok"
      ? "bg-ok/15 text-ok"
      : tone === "warn"
        ? "bg-warn/15 text-warn"
        : tone === "bad"
          ? "bg-bad/15 text-bad"
          : tone === "accent"
            ? "bg-accent/15 text-accent"
            : "bg-line text-ink-2";
  return (
    <span className={`badge ${cls}`} title={title}>
      {children}
    </span>
  );
}

export function FlagChips({ bits, emptyText = "clean" }: { bits: number; emptyText?: string }) {
  const flags: FlagInfo[] = flagsOf(bits);
  if (flags.length === 0) return <Badge tone="ok">{emptyText}</Badge>;
  return (
    <span className="flex flex-wrap gap-1">
      {flags.map((f) => (
        <Badge key={f.bit} tone={f.severe ? "bad" : "warn"} title={f.description}>
          {f.name}
        </Badge>
      ))}
    </span>
  );
}

export function SensorBits({ mask }: { mask: number }) {
  const names = ["M1", "M2", "M3", "A1", "A2", "IMU"];
  return (
    <span className="flex gap-1">
      {names.map((n, i) => (
        <Badge key={n} tone={(mask & (1 << i)) !== 0 ? "ok" : "bad"}>
          {n}
        </Badge>
      ))}
    </span>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <div className="py-8 text-center text-sm text-ink-3">{children}</div>;
}

export function Spinner() {
  return <span className="inline-block h-3.5 w-3.5 animate-spin rounded-full border-2 border-ink-3 border-t-accent" />;
}
