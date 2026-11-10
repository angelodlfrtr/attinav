/** Compass rose and inclinometer for one shot result. Pure SVG, no state. */
export function Attitude({ azimuth, inclination, roll, sigmaAz, size = 180 }: { azimuth: number; inclination: number; roll: number; sigmaAz: number; size?: number }) {
  const r = size / 2;
  const c = r;
  const toXY = (deg: number, radius: number) => {
    const a = ((deg - 90) * Math.PI) / 180;
    return { x: c + radius * Math.cos(a), y: c + radius * Math.sin(a) };
  };
  const needle = toXY(azimuth, r - 22);
  const tail = toXY(azimuth + 180, 24);
  const sigma = Math.min(Math.max(sigmaAz, 0.2), 45);
  const arc = (from: number, to: number, radius: number) => {
    const a = toXY(from, radius);
    const b = toXY(to, radius);
    const large = Math.abs(to - from) > 180 ? 1 : 0;
    return `M ${a.x} ${a.y} A ${radius} ${radius} 0 ${large} 1 ${b.x} ${b.y}`;
  };
  const inclY = (deg: number) => c - (deg / 90) * (r - 24);
  return (
    <div className="flex items-center gap-6">
      <svg width={size} height={size} className="shrink-0">
        <circle cx={c} cy={c} r={r - 4} fill="#0b0f17" stroke="#243047" strokeWidth="2" />
        {Array.from({ length: 72 }, (_, i) => {
          const major = i % 18 === 0;
          const mid = i % 6 === 0;
          const a = toXY(i * 5, r - 6);
          const b = toXY(i * 5, r - (major ? 18 : mid ? 13 : 9));
          return <line key={i} x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke={major ? "#9fb0c8" : "#3b4a66"} strokeWidth={major ? 2 : 1} />;
        })}
        {["N", "E", "S", "W"].map((l, i) => {
          const p = toXY(i * 90, r - 30);
          return (
            <text key={l} x={p.x} y={p.y + 4} textAnchor="middle" fill={i === 0 ? "#f87171" : "#9fb0c8"} fontSize="12" fontWeight="600">
              {l}
            </text>
          );
        })}
        <path d={arc(azimuth - sigma * 3, azimuth + sigma * 3, r - 8)} stroke="#38bdf8" strokeOpacity="0.5" strokeWidth="4" fill="none" />
        <line x1={tail.x} y1={tail.y} x2={needle.x} y2={needle.y} stroke="#38bdf8" strokeWidth="3" strokeLinecap="round" />
        <circle cx={c} cy={c} r="4" fill="#e6edf7" />
        <text x={c} y={c + 44} textAnchor="middle" fill="#e6edf7" fontSize="16" fontFamily="ui-monospace, monospace">
          {azimuth.toFixed(1)}°
        </text>
      </svg>
      <svg width={72} height={size} className="shrink-0">
        <rect x="30" y="4" width="12" height={size - 8} rx="6" fill="#0b0f17" stroke="#243047" />
        <line x1="20" y1={c} x2="52" y2={c} stroke="#3b4a66" />
        {[-90, -60, -30, 30, 60, 90].map((d) => (
          <g key={d}>
            <line x1="26" y1={inclY(d)} x2="46" y2={inclY(d)} stroke="#3b4a66" />
            <text x="50" y={inclY(d) + 3} fill="#66778f" fontSize="9">
              {d}
            </text>
          </g>
        ))}
        <circle cx="36" cy={inclY(Math.max(-90, Math.min(90, inclination)))} r="6" fill="#34d399" />
        <text x="36" y={size - 2} textAnchor="middle" fill="#e6edf7" fontSize="11" fontFamily="ui-monospace, monospace">
          {inclination.toFixed(1)}°
        </text>
      </svg>
      <svg width={size / 2} height={size / 2} className="shrink-0">
        <circle cx={size / 4} cy={size / 4} r={size / 4 - 4} fill="#0b0f17" stroke="#243047" />
        <g transform={`rotate(${roll} ${size / 4} ${size / 4})`}>
          <line x1={8} y1={size / 4} x2={size / 2 - 8} y2={size / 4} stroke="#fbbf24" strokeWidth="3" strokeLinecap="round" />
          <circle cx={size / 2 - 10} cy={size / 4} r="3" fill="#fbbf24" />
        </g>
        <text x={size / 4} y={size / 2 - 4} textAnchor="middle" fill="#9fb0c8" fontSize="10" fontFamily="ui-monospace, monospace">
          roll {roll.toFixed(1)}°
        </text>
      </svg>
    </div>
  );
}
