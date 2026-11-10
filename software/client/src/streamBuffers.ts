/**
 * Ring buffers for raw streaming. Samples arrive at up to 500 Hz per sensor; routing each one
 * through Redux would make the UI unusable, so the buffers live here and the chart components
 * read them directly on an animation frame. Redux only carries counters (see `streamSlice`).
 */
import type { Sample, Temps } from "./protocol";

export class Ring {
  readonly t: Float64Array;
  readonly v: Float64Array;
  private head = 0;
  private count = 0;

  constructor(readonly capacity: number) {
    this.t = new Float64Array(capacity);
    this.v = new Float64Array(capacity);
  }

  push(t: number, v: number): void {
    this.t[this.head] = t;
    this.v[this.head] = v;
    this.head = (this.head + 1) % this.capacity;
    if (this.count < this.capacity) this.count += 1;
  }

  get length(): number {
    return this.count;
  }

  clear(): void {
    this.head = 0;
    this.count = 0;
  }

  /** Calls `f` for every stored point, oldest first. */
  forEach(f: (t: number, v: number) => void): void {
    const start = (this.head - this.count + this.capacity) % this.capacity;
    for (let i = 0; i < this.count; i += 1) {
      const idx = (start + i) % this.capacity;
      f(this.t[idx] ?? 0, this.v[idx] ?? 0);
    }
  }

  last(): { t: number; v: number } | null {
    if (this.count === 0) return null;
    const idx = (this.head - 1 + this.capacity) % this.capacity;
    return { t: this.t[idx] ?? 0, v: this.v[idx] ?? 0 };
  }
}

export interface SensorRings {
  x: Ring;
  y: Ring;
  z: Ring;
}

const CAPACITY = 5000;

class StreamBuffers {
  private readonly rings = new Map<number, SensorRings>();
  temps: Temps | null = null;
  total = 0;
  private windowCount = 0;
  private windowStart = performance.now();
  rateHz = 0;

  sensor(tag: number): SensorRings {
    let r = this.rings.get(tag);
    if (!r) {
      r = { x: new Ring(CAPACITY), y: new Ring(CAPACITY), z: new Ring(CAPACITY) };
      this.rings.set(tag, r);
    }
    return r;
  }

  has(tag: number): boolean {
    return this.rings.has(tag);
  }

  push(s: Sample): void {
    const r = this.sensor(s.sensor);
    r.x.push(s.t_ms, s.raw[0]);
    r.y.push(s.t_ms, s.raw[1]);
    r.z.push(s.t_ms, s.raw[2]);
    this.total += 1;
    this.windowCount += 1;
    const now = performance.now();
    if (now - this.windowStart >= 1000) {
      this.rateHz = (this.windowCount * 1000) / (now - this.windowStart);
      this.windowCount = 0;
      this.windowStart = now;
    }
  }

  clear(): void {
    this.rings.clear();
    this.temps = null;
    this.total = 0;
    this.windowCount = 0;
    this.windowStart = performance.now();
    this.rateHz = 0;
  }
}

export const streamBuffers = new StreamBuffers();
