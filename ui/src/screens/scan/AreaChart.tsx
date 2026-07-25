/// Read activity over time, with a labelled axis.
///
/// The reference this follows prints fixed axis labels; these are derived from
/// the data, because a chart whose axis says "900 MB/s" while the line sits at
/// three is decoration rather than instrumentation.
export function AreaChart({ values, unit }: { values: number[]; unit: string }) {
  const w = 1000;
  const h = 180;
  const padLeft = 54;
  const padBottom = 18;

  const peak = niceCeiling(Math.max(...values, 0.001));
  const plotW = w - padLeft;
  const plotH = h - padBottom;
  const step = plotW / Math.max(1, values.length - 1);

  const x = (i: number) => padLeft + i * step;
  const y = (v: number) => plotH - (v / peak) * (plotH - 8);

  const line = values.map((v, i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(" ");
  const area = `${padLeft},${plotH} ${line} ${x(values.length - 1).toFixed(1)},${plotH}`;

  // Four gridlines, including zero, so the eye has something to measure against.
  const rows = [1, 0.66, 0.33, 0];

  return (
    <svg
      className="area-chart"
      viewBox={`0 0 ${w} ${h}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={`Read activity, currently ${values[values.length - 1].toFixed(1)} ${unit}`}
    >
      <defs>
        <linearGradient id="area-grad" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#3da9fc" stopOpacity="0.42" />
          <stop offset="100%" stopColor="#3da9fc" stopOpacity="0.02" />
        </linearGradient>
      </defs>

      {rows.map((f) => (
        <g key={f}>
          <line className="grid-line" x1={padLeft} y1={y(peak * f)} x2={w} y2={y(peak * f)} />
          <text className="grid-label" x={padLeft - 10} y={y(peak * f) + 4}>
            {formatTick(peak * f)}
          </text>
        </g>
      ))}

      <polygon className="area-fill" points={area} />
      <polyline className="area-line" points={line} />
    </svg>
  );
}

/// Round the axis top to something readable rather than the raw maximum.
function niceCeiling(peak: number): number {
  const magnitude = Math.pow(10, Math.floor(Math.log10(peak)));
  const normalised = peak / magnitude;
  const step = normalised <= 1 ? 1 : normalised <= 2 ? 2 : normalised <= 5 ? 5 : 10;
  return step * magnitude * 1.2;
}

function formatTick(v: number): string {
  if (v === 0) return "0";
  if (v >= 100) return v.toFixed(0);
  if (v >= 10) return v.toFixed(0);
  return v.toFixed(1);
}
