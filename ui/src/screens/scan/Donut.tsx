/// File-type breakdown as a ring.
///
/// Percentages are computed here from the counts rather than passed in
/// alongside them, so the ring and the legend cannot disagree — two figures for
/// one fact is how a chart ends up lying.
export function Donut({ slices }: { slices: [string, number][] }) {
  const total = slices.reduce((sum, [, n]) => sum + n, 0) || 1;
  const colours = ["#3da9fc", "#8b7bf7", "#39c07a", "#f0b23c", "#ef7a5a", "#7c8aa0"];

  const r = 52;
  const circumference = 2 * Math.PI * r;
  let offset = 0;

  return (
    <div className="donut">
      <svg viewBox="0 0 140 140" role="img" aria-label="File types found on this drive">
        <g transform="rotate(-90 70 70)">
          {slices.map(([ext, n], i) => {
            const dash = (n / total) * circumference;
            const el = (
              <circle
                key={ext}
                className="donut-slice"
                cx={70}
                cy={70}
                r={r}
                stroke={colours[i % colours.length]}
                strokeDasharray={`${dash} ${circumference - dash}`}
                strokeDashoffset={-offset}
              />
            );
            offset += dash;
            return el;
          })}
        </g>
      </svg>
      <ul className="donut-legend">
        {slices.map(([ext, n], i) => (
          <li key={ext}>
            <span className="dot" style={{ background: colours[i % colours.length] }} />
            <span className="legend-name">{ext.toUpperCase()}</span>
            <span className="legend-pct">{((n / total) * 100).toFixed(1)}%</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
