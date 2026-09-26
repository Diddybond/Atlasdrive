import { useEffect, useState } from "react";
import { api, OnThisDay as Found, SearchResult } from "../api";

/// Photographs of the people who matter, taken on this day in earlier years (D-107).
///
/// Family when anyone is marked as family on People, otherwise anyone named.
/// Hidden entirely on a day with nothing to show.
export function OnThisDay({ onOpen }: { onOpen: (r: SearchResult) => void }) {
  const [found, setFound] = useState<Found | null>(null);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const today = new Date();
  const monthDay = `${String(today.getMonth() + 1).padStart(2, "0")}-${String(today.getDate()).padStart(2, "0")}`;

  useEffect(() => {
    void api.onThisDay(monthDay, today.getFullYear()).then(
      async (f) => {
        setFound(f);
        const loaded: Record<string, string> = {};
        await Promise.all(
          f.memories.map(async (m) => {
            const src = await api
              .photoThumbnail(m.result.file_id, 240)
              .catch(() => null);
            if (src) loaded[m.result.file_id] = src;
          }),
        );
        setThumbs(loaded);
      },
      () => setFound(null),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [monthDay]);

  if (!found || found.memories.length === 0) return null;
  const years = [...new Set(found.memories.map((m) => m.year))];
  const dayName = today.toLocaleDateString(undefined, {
    day: "numeric",
    month: "long",
  });

  return (
    <div className="card on-this-day" aria-labelledby="otd-heading">
      <h2 id="otd-heading">On this day · {dayName}</h2>
      <div className="otd-years">
        {years.map((y) => (
          <div key={y} className="otd-year">
            <p className="otd-label">
              {y} · {today.getFullYear() - y} year
              {today.getFullYear() - y === 1 ? "" : "s"} ago
            </p>
            <ul className="otd-strip">
              {found.memories
                .filter((m) => m.year === y)
                .map((m) => (
                  <li key={m.result.file_id}>
                    <button
                      className="otd-photo"
                      onClick={() => onOpen(m.result)}
                      aria-label={`Open ${m.result.filename} from ${y}, on Drive ${m.result.drive_number}`}
                    >
                      {thumbs[m.result.file_id] ? (
                        <img src={thumbs[m.result.file_id]} alt="" />
                      ) : (
                        <span className="thumb-mark" aria-hidden>
                          🖼
                        </span>
                      )}
                      <span className="otd-drive">
                        Drive {m.result.drive_number}
                      </span>
                    </button>
                  </li>
                ))}
            </ul>
          </div>
        ))}
      </div>
      {!found.family_only && (
        <p className="subtle">
          Showing everyone you have named. Mark people as family on People to
          see just them here.
        </p>
      )}
    </div>
  );
}
