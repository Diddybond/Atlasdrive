/// A speedometer for read throughput.
///
/// The scale is *derived*, not fixed. A dial marked 0–1000 MB/s looks handsome
/// and tells you nothing when the drive being read is an NTFS disk sustaining
/// three — the needle would sit pinned at the bottom for ten hours. The range
/// tracks the fastest reading seen so far, rounded up to a number worth printing
/// on a dial, so the needle actually uses the face it is given.
export function Gauge({ value, peak }: { value: number | null; peak: number }) {
  const max = niceMax(Math.max(peak, value ?? 0));
  const shown = value ?? 0;
  const fraction = Math.max(0, Math.min(1, shown / max));

  // A 240° sweep starting at 150°, leaving the gap at the bottom.
  //
  // The pivot sits high in the box on purpose. Centred, the hub and the lower
  // half of the needle land exactly where the reading is printed and cross
  // through the digits. Raising the centre and lengthening the box leaves the
  // dial face below the pivot clear for the number, which is where a real
  // instrument puts it.
  const start = 150;
  const sweep = 240;
  const angle = start + fraction * sweep;
  const cx = 120;
  const cy = 100;
  const r = 84;

  const polar = (deg: number, radius: number): [number, number] => {
    const rad = (deg * Math.PI) / 180;
    return [cx + radius * Math.cos(rad), cy + radius * Math.sin(rad)];
  };
  const arc = (from: number, to: number, radius: number) => {
    const [x1, y1] = polar(from, radius);
    const [x2, y2] = polar(to, radius);
    return `M ${x1} ${y1} A ${radius} ${radius} 0 ${to - from > 180 ? 1 : 0} 1 ${x2} ${y2}`;
  };

  // The needle stops short of the rim and starts away from the hub, so it reads
  // as a pointer rather than a spoke laid across the face.
  const [nx, ny] = polar(angle, r - 16);
  const [tailX, tailY] = polar(angle + 180, 12);
  const [minX, minY] = polar(start, r + 16);
  const [maxX, maxY] = polar(start + sweep, r + 16);

  return (
    <div className="gauge">
      <svg
        viewBox="0 0 240 210"
        role="img"
        aria-label={`Read speed ${value === null ? "not yet measured" : `${shown.toFixed(1)} megabytes per second`}`}
      >
        <defs>
          <linearGradient id="gauge-grad" x1="0" y1="1" x2="1" y2="0">
            <stop offset="0%" stopColor="#3da9fc" />
            <stop offset="55%" stopColor="#6f7bf7" />
            <stop offset="100%" stopColor="#b06ef0" />
          </linearGradient>
        </defs>

        <path className="gauge-track" d={arc(start, start + sweep, r)} />
        {fraction > 0.001 && <path className="gauge-value" d={arc(start, angle, r)} />}

        {Array.from({ length: 25 }, (_, i) => {
          const t = start + (i / 24) * sweep;
          const major = i % 6 === 0;
          const [x1, y1] = polar(t, r - 20);
          const [x2, y2] = polar(t, r - (major ? 32 : 26));
          return (
            <line
              key={i}
              className={major ? "gauge-tick major" : "gauge-tick"}
              x1={x1}
              y1={y1}
              x2={x2}
              y2={y2}
            />
          );
        })}

        <line className="gauge-needle" x1={tailX} y1={tailY} x2={nx} y2={ny} />
        <circle className="gauge-hub" cx={cx} cy={cy} r={6} />

        <text className="gauge-bound" x={minX} y={minY}>
          0
        </text>
        <text className="gauge-bound" x={maxX} y={maxY}>
          {max}
        </text>
      </svg>

      <div className="gauge-readout">
        <strong>{value === null ? "—" : shown.toFixed(1)}</strong>
        <span>MB/s</span>
      </div>
    </div>
  );
}

/// Round a peak up to a number worth printing on a dial: 5, 10, 25, 50, 100…
function niceMax(peak: number): number {
  const candidates = [5, 10, 25, 50, 100, 250, 500, 1000, 2000];
  const target = Math.max(peak * 1.25, 1);
  return candidates.find((c) => c >= target) ?? Math.ceil(target / 1000) * 1000;
}
