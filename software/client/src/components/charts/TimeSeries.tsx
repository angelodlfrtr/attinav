import { useEffect, useRef } from "react";
import type { Ring } from "../../streamBuffers";

export interface Series {
  label: string;
  color: string;
  ring: () => Ring;
}

/**
 * Canvas strip chart over ring buffers. Redraws on every animation frame while mounted; the data
 * is read straight from the buffers (no React state), so hundreds of samples per second cost
 * nothing on the React side.
 */
export function TimeSeries({ series, windowSeconds, height = 160, title }: { series: Series[]; windowSeconds: number; height?: number; title: string }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let frame = 0;
    const draw = () => {
      frame = requestAnimationFrame(draw);
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      const dpr = window.devicePixelRatio || 1;
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      if (canvas.width !== Math.floor(w * dpr) || canvas.height !== Math.floor(h * dpr)) {
        canvas.width = Math.floor(w * dpr);
        canvas.height = Math.floor(h * dpr);
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);

      // Time window: from the newest sample backwards.
      let tMax = -Infinity;
      let vMin = Infinity;
      let vMax = -Infinity;
      for (const s of series) {
        const last = s.ring().last();
        if (last) tMax = Math.max(tMax, last.t);
      }
      if (!Number.isFinite(tMax)) {
        ctx.fillStyle = "#66778f";
        ctx.font = "12px sans-serif";
        ctx.fillText("no data", 8, 16);
        return;
      }
      const tMin = tMax - windowSeconds * 1000;
      for (const s of series) {
        s.ring().forEach((t, v) => {
          if (t >= tMin) {
            if (v < vMin) vMin = v;
            if (v > vMax) vMax = v;
          }
        });
      }
      if (vMin === vMax) {
        vMin -= 1;
        vMax += 1;
      }
      const pad = (vMax - vMin) * 0.08;
      vMin -= pad;
      vMax += pad;

      const left = 56;
      const right = 8;
      const top = 6;
      const bottom = 18;
      const pw = w - left - right;
      const ph = h - top - bottom;
      const xOf = (t: number) => left + ((t - tMin) / (tMax - tMin || 1)) * pw;
      const yOf = (v: number) => top + (1 - (v - vMin) / (vMax - vMin)) * ph;

      // Grid and axes.
      ctx.strokeStyle = "#243047";
      ctx.lineWidth = 1;
      ctx.fillStyle = "#66778f";
      ctx.font = "10px ui-monospace, monospace";
      ctx.textAlign = "right";
      for (let i = 0; i <= 4; i += 1) {
        const v = vMin + ((vMax - vMin) * i) / 4;
        const y = yOf(v);
        ctx.beginPath();
        ctx.moveTo(left, y);
        ctx.lineTo(w - right, y);
        ctx.stroke();
        ctx.fillText(formatTick(v), left - 4, y + 3);
      }
      ctx.textAlign = "center";
      for (let i = 0; i <= 5; i += 1) {
        const t = tMin + ((tMax - tMin) * i) / 5;
        const x = xOf(t);
        ctx.fillText(`${((t - tMax) / 1000).toFixed(1)}s`, x, h - 4);
      }

      // Series.
      for (const s of series) {
        ctx.strokeStyle = s.color;
        ctx.lineWidth = 1.25;
        ctx.beginPath();
        let started = false;
        s.ring().forEach((t, v) => {
          if (t < tMin) return;
          const x = xOf(t);
          const y = yOf(v);
          if (started) ctx.lineTo(x, y);
          else {
            ctx.moveTo(x, y);
            started = true;
          }
        });
        ctx.stroke();
      }

      // Legend with the latest values.
      ctx.textAlign = "left";
      ctx.font = "11px ui-monospace, monospace";
      let lx = left + 6;
      for (const s of series) {
        const last = s.ring().last();
        const text = `${s.label} ${last ? formatTick(last.v) : "—"}`;
        ctx.fillStyle = s.color;
        ctx.fillText(text, lx, top + 11);
        lx += ctx.measureText(text).width + 14;
      }
    };
    frame = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(frame);
    };
  }, [series, windowSeconds]);

  return (
    <div className="rounded-md border border-line bg-ground">
      <div className="px-2 pt-1 text-[11px] uppercase tracking-wider text-ink-3">{title}</div>
      <canvas ref={canvasRef} className="block w-full" style={{ height }} />
    </div>
  );
}

function formatTick(v: number): string {
  const a = Math.abs(v);
  if (a >= 100000) return v.toExponential(2);
  if (a >= 1000) return v.toFixed(0);
  if (a >= 10) return v.toFixed(1);
  return v.toFixed(3);
}
