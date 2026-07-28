import { useState } from "react";
import { useEffect } from "react";
import { api, Drive, DriveMatch, SearchResult, TagCount } from "../api";
import type { SearchContext } from "../App";

export function SearchScreen({
  context,
  onClearContext,
}: {
  context?: SearchContext | null;
  onClearContext?: () => void;
}) {
  const [query, setQuery] = useState("");
  const [includeOffline, setIncludeOffline] = useState(true);
  const [results, setResults] = useState<SearchResult[]>([]);
  const [understood, setUnderstood] = useState<string[]>([]);
  const [textOnly, setTextOnly] = useState(false);
  const [drives, setDrives] = useState<DriveMatch[]>([]);
  const [whereToLook, setWhereToLook] = useState("");
  const [loading, setLoading] = useState(false);
  const [searched, setSearched] = useState(false);
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [tags, setTags] = useState<TagCount[]>([]);
  // Which drive is being browsed, and which subjects have been picked. Both
  // narrow the search rather than replacing the typed query, so they can be
  // combined: "children, at weddings, on Drive 2".
  const [driveFilter, setDriveFilter] = useState<number | null>(null);
  const [pickedTags, setPickedTags] = useState<string[]>([]);
  const [allDrives, setAllDrives] = useState<Drive[]>([]);
  const [nameNote, setNameNote] = useState<string | null>(null);
  const [findingNames, setFindingNames] = useState(false);

  useEffect(() => {
    void api.listDrives().then(setAllDrives);
  }, []);

  // The subject list follows the selected drive, so every chip on screen leads
  // to photographs on the disk being browsed rather than to an empty result.
  useEffect(() => {
    void api.catalogueTags(60, driveFilter ?? undefined).then(setTags);
  }, [driveFilter]);

  // Arriving from Events with a filter should show that shoot immediately —
  // landing on an empty search box having just asked to see something would be
  // a dead end.
  useEffect(() => {
    if (context) void search(query || "photograph");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [context?.eventId, context?.client]);
  const [similarTo, setSimilarTo] = useState<string | null>(null);
  const [correcting, setCorrecting] = useState<string | null>(null);
  const [dateError, setDateError] = useState<string | null>(null);

  /// Find photographs that look like this one.
  ///
  /// Distinct from a text search, and labelled as such: this asks the visual
  /// index, which is the one thing it is genuinely good at.
  async function findSimilar(fileId: string, filename: string) {
    setLoading(true);
    setSimilarTo(filename);
    try {
      const hits = await api.similarPhotographs(fileId, 24);
      setResults(hits);
      setUnderstood([]);
      setDrives([]);
      setWhereToLook("");
      setSearched(true);
    } finally {
      setLoading(false);
    }
  }

  async function reveal(fileId: string) {
    const message = await api.revealInFinder(fileId);
    setRevealed((prev) => ({ ...prev, [fileId]: message }));
  }

  async function saveDate(e: React.FormEvent<HTMLFormElement>, fileId: string) {
    e.preventDefault();
    setDateError(null);
    const form = new FormData(e.currentTarget);
    const earliest = String(form.get("earliest") ?? "").trim();
    const latest = String(form.get("latest") ?? "").trim();
    try {
      const label = await api.setDateOverride({
        fileId,
        earliest,
        latest: latest || undefined,
      });
      setResults((prev) =>
        prev.map((r) => (r.file_id === fileId ? { ...r, date_label: label } : r)),
      );
      setCorrecting(null);
    } catch (err) {
      setDateError(
        "Please enter the date as YYYY-MM-DD, for example 1998-08-12.",
      );
      void err;
    }
  }

  async function run(e: React.FormEvent) {
    e.preventDefault();
    await search(query);
  }

  /// Shared by the form and the tag chips, so clicking a subject is exactly the
  /// same operation as typing it.
  async function search(term: string, picked?: string[]) {
    setQuery(term);
    setSimilarTo(null);
    setLoading(true);
    try {
      const r = await api.search(term, {
        includeOffline,
        drive: driveFilter ?? undefined,
        tags: picked ?? pickedTags,
        eventId: context?.eventId,
        client: context?.client,
      });
      setResults(r.results);
      setUnderstood(r.understood);
      setTextOnly(r.text_only);
      setDrives(r.drives);
      setWhereToLook(r.where_to_look);
      setSearched(true);

      // Thumbnails come from the local catalogue, so they appear whether or not
      // the drive is connected.
      const loaded: Record<string, string> = {};
      await Promise.all(
        r.results.map(async (x) => {
          const src = await api.photoThumbnail(x.file_id, 240);
          if (src) loaded[x.file_id] = src;
        }),
      );
      setThumbs(loaded);
    } finally {
      setLoading(false);
    }
  }

  return (
    <section aria-labelledby="search-heading">
      <h1 id="search-heading">Search</h1>
      <p className="lede">
        Search by what a photo shows, who is in it, or where it lives — even while the drive is
        disconnected.
      </p>

      {similarTo && (
        <p className="scope-bar" role="status" aria-label="Result scope">
          Photographs that look like <strong>{similarTo}</strong>
          <button className="ghost" onClick={() => void search(query)}>
            Back to search
          </button>
        </p>
      )}

      {context && (
        <p className="scope-bar" role="status" aria-label="Search scope">
          Searching within <strong>{context.label}</strong>
          <button
            className="ghost"
            onClick={() => {
              onClearContext?.();
              void search(query);
            }}
          >
            Search everything instead
          </button>
        </p>
      )}

      <form className="search-bar" onSubmit={run} role="search">
        <input
          type="search"
          aria-label="Search photographs"
          placeholder="Try: bikes, Christmas, family wedding, photos from the 1980s"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button type="submit" disabled={loading}>
          {loading ? "Searching…" : "Search"}
        </button>
      </form>

      <label className="checkbox">
        <input
          type="checkbox"
          checked={includeOffline}
          onChange={(e) => setIncludeOffline(e.target.checked)}
        />
        Include photographs on disconnected drives
      </label>

      {allDrives.length > 0 && (
        <div className="drive-filter">
          <span className="filter-label">Look on</span>
          <button
            className={driveFilter === null ? "chip selected" : "chip"}
            onClick={() => {
              setDriveFilter(null);
              setPickedTags([]);
            }}
          >
            Every drive
          </button>
          {allDrives.map((d) => (
            <button
              key={d.id}
              className={driveFilter === d.drive_number ? "chip selected" : "chip"}
              onClick={() => {
                setDriveFilter(d.drive_number);
                // Subjects belong to the drive they were picked from; keeping
                // them would silently search for something the new drive may
                // not have and return nothing for no visible reason.
                setPickedTags([]);
              }}
              title={d.friendly_name ?? undefined}
            >
              Drive {d.drive_number}
              <span className="chip-count">{(d.image_count ?? 0).toLocaleString()}</span>
            </button>
          ))}
        </div>
      )}

      {tags.length > 0 && (
        <div className="card">
          <div className="row-between">
            <h2>What is in your photographs</h2>
            {pickedTags.length > 0 && (
              <button
                className="ghost"
                onClick={() => {
                  setPickedTags([]);
                  void search(query, []);
                }}
              >
                Clear {pickedTags.length} selected
              </button>
            )}
          </div>
          <p className="drive-meta subtle">
            {driveFilter === null
              ? "Everything AtlasDrive recognised across your drives."
              : `Everything AtlasDrive recognised on Drive ${driveFilter}.`}{" "}
            Click to add a subject — each one you add narrows the search further.
          </p>
          <ul className="tag-cloud">
            {tags.map((t) => {
              const on = pickedTags.includes(t.tag);
              return (
                <li key={t.tag}>
                  <button
                    className={on ? "tag-chip selected" : "tag-chip"}
                    aria-pressed={on}
                    onClick={() => {
                      const next = on
                        ? pickedTags.filter((x) => x !== t.tag)
                        : [...pickedTags, t.tag];
                      setPickedTags(next);
                      // Subjects are filters, not text. Clicking one used to
                      // write it into the search box, and the leftover words
                      // were then silently intersected with the next click —
                      // "jeans" answered through the lens of "likely-scan".
                      // The box now belongs to the owner's own typing.
                      void search(query, next);
                    }}
                    aria-label={
                      on
                        ? `Stop narrowing to ${t.tag}`
                        : `Narrow to the ${t.count} photographs showing ${t.tag}`
                    }
                  >
                    {t.tag}
                    <span className="tag-count">{t.count.toLocaleString()}</span>
                  </button>
                </li>
              );
            })}
          </ul>
          <div className="row-between name-row">
            <p className="panel-note">
              Names come from text AtlasDrive read on things in the picture — a bottle, a shop
              front, a van, a magazine. They are never guessed from the image itself.
            </p>
            <button
              className="ghost"
              disabled={findingNames}
              onClick={() => {
                setFindingNames(true);
                setNameNote(null);
                void api
                  .findNames(driveFilter ?? undefined)
                  .then((r) => {
                    setNameNote(
                      r.tagged === 0
                        ? `Read the text of ${r.examined.toLocaleString()} photographs and found no names.`
                        : `Found names in ${r.tagged.toLocaleString()} of ${r.examined.toLocaleString()} photographs: ${r.names
                            .slice(0, 8)
                            .map((b) => b.tag)
                            .join(", ")}${r.names.length > 8 ? "…" : ""}`,
                    );
                    return api.catalogueTags(60, driveFilter ?? undefined).then(setTags);
                  })
                  .finally(() => setFindingNames(false));
              }}
            >
              {findingNames ? "Reading…" : "Find names in photographs"}
            </button>
          </div>
          {nameNote && (
            <p className="search-note" role="status">
              {nameNote}
            </p>
          )}

          {pickedTags.length > 1 && (
            <p className="panel-note">
              Showing only photographs that contain <strong>all</strong> of these:{" "}
              {pickedTags.join(", ")}.
            </p>
          )}
        </div>
      )}

      {searched && understood.length > 0 && (
        <p className="search-note" role="status">
          Looking for photographs that appear to show: {understood.join(", ")}. These are visual
          guesses.
        </p>
      )}
      {searched && textOnly && (
        <p className="search-note" role="status">
          No visual terms recognised in that wording — searched names, folders and tags only.
        </p>
      )}

      {searched && drives.length > 0 && (
        <div className="card where-to-look" role="status">
          <h2>{whereToLook}</h2>
          <ul className="drive-hits">
            {drives.map((d) => (
              <li key={d.drive_number}>
                <span className="drive-badge">Drive {d.drive_number}</span>
                <span className="drive-hit-count">
                  {d.match_count} photograph{d.match_count === 1 ? "" : "s"}
                </span>
                {d.drive_name && <span className="drive-hit-name">{d.drive_name}</span>}
                <span className={d.online ? "status online" : "status offline"}>
                  {d.online ? "Connected" : "Disconnected"}
                </span>
                {!d.online && d.physical_location && (
                  <span className="drive-hit-where">Kept in {d.physical_location}</span>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}

      {searched && results.length === 0 && (
        <p className="empty">No matching photographs yet. Try a broader description.</p>
      )}

      <ul className="results-grid" aria-label="Search results">
        {results.map((r) => (
          <li key={r.file_id} className="result-card">
            <div className="thumb">
              {thumbs[r.file_id] ? (
                <img src={thumbs[r.file_id]} alt="" loading="lazy" />
              ) : (
                <span className="thumb-mark" aria-hidden>
                  🖼
                </span>
              )}
            </div>
            <div className="result-body">
              <div className="result-top">
                <span className="drive-badge">Drive {r.drive_number}</span>
                <span className={r.online ? "status online" : "status offline"}>
                  {r.online ? "Online" : "Offline"}
                </span>
              </div>
              <p className="filename">{r.filename}</p>
              <p className="date">{r.date_label ?? "Date uncertain"}</p>
              {correcting === r.file_id ? (
                <form className="form date-form" onSubmit={(e) => void saveDate(e, r.file_id)}>
                  <label>
                    Date taken (YYYY-MM-DD)
                    <input name="earliest" placeholder="1998-08-12" required />
                  </label>
                  <label>
                    If unsure, latest it could be
                    <input name="latest" placeholder="1998-12-31" />
                  </label>
                  {dateError && (
                    <p className="error" role="alert">
                      {dateError}
                    </p>
                  )}
                  <button type="submit">Save date</button>
                  <button type="button" className="ghost" onClick={() => setCorrecting(null)}>
                    Cancel
                  </button>
                </form>
              ) : (
                <button
                  className="ghost"
                  onClick={() => {
                    setDateError(null);
                    setCorrecting(r.file_id);
                  }}
                  aria-label={`Correct the date for ${r.filename}`}
                >
                  Correct this date
                </button>
              )}
              <p className="matched">
                Matched: {r.matched.join(", ")} · {(r.score * 100).toFixed(0)}% match
              </p>
              <button
                className="ghost"
                onClick={() => void findSimilar(r.file_id, r.filename)}
                aria-label={`Find photographs that look like ${r.filename}`}
              >
                More like this
              </button>
              {r.online ? (
                <button
                  className="ghost"
                  onClick={() => void reveal(r.file_id)}
                  aria-label={`Show ${r.filename} in Finder`}
                >
                  Show in Finder
                </button>
              ) : (
                <p className="offline-note">Connect Drive {r.drive_number} to open the original.</p>
              )}
              {revealed[r.file_id] && (
                <p className="check-detail" role="status">
                  {revealed[r.file_id]}
                </p>
              )}
            </div>
          </li>
        ))}
      </ul>
      {results.length > 0 && (
        <p className="footnote">
          Visual matches are best guesses, not certainties. Confirmed names and tags always take
          priority.
        </p>
      )}
    </section>
  );
}
