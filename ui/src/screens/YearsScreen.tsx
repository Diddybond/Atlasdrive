import { useEffect, useState } from "react";
import { api, YearRow } from "../api";
import type { SearchContext } from "../App";

/// Every year of the archive and which drives hold it (D-107).
///
/// "Which drive is 2016 on?" answered at a glance. The year comes from the
/// camera's date, or a date you corrected, or an estimate narrow enough to name
/// one year; anything vaguer is counted as undated rather than guessed.
export function YearsScreen({ onSearchWithin }: { onSearchWithin?: (c: SearchContext) => void }) {
  const [years, setYears] = useState<YearRow[] | null>(null);

  useEffect(() => {
    void api.yearsOverview().then(setYears, () => setYears([]));
  }, []);

  return (
    <section aria-labelledby="years-heading">
      <h1 id="years-heading">Years</h1>
      <p className="lede">
        Each year of your archive and the drives it is on. A photograph on two drives is counted
        once.
      </p>
      {years === null ? (
        <p className="subtle">Counting…</p>
      ) : years.length === 0 ? (
        <p className="empty">No photographs catalogued yet.</p>
      ) : (
        <ul className="years-list">
          {years.map((y) => (
            <li key={y.year ?? "undated"} className="year-row">
              <span className="year-label">{y.year ?? "Undated"}</span>
              <span className="year-count">
                {y.photographs.toLocaleString()} photograph{y.photographs === 1 ? "" : "s"}
              </span>
              <span className="year-drives">
                {y.drives.map((d) => (
                  <span
                    key={d.drive_number}
                    className="drive-badge"
                    title={`${d.drive_name ?? `Drive ${d.drive_number}`}: ${d.photographs.toLocaleString()} photographs`}
                  >
                    Drive {d.drive_number}
                    <span className="badge-count"> {d.photographs.toLocaleString()}</span>
                  </span>
                ))}
              </span>
              {y.year !== null && onSearchWithin && (
                <button
                  className="ghost"
                  onClick={() => onSearchWithin({ year: y.year!, label: `photographs from ${y.year}` })}
                  aria-label={`Show the photographs from ${y.year}`}
                >
                  Show photographs
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
