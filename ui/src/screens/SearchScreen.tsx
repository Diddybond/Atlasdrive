import { useState } from "react";
import { api, SearchResult } from "../api";

export function SearchScreen() {
  const [query, setQuery] = useState("");
  const [includeOffline, setIncludeOffline] = useState(true);
  const [results, setResults] = useState<SearchResult[]>([]);
  const [understood, setUnderstood] = useState<string[]>([]);
  const [textOnly, setTextOnly] = useState(false);
  const [loading, setLoading] = useState(false);
  const [searched, setSearched] = useState(false);
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const [correcting, setCorrecting] = useState<string | null>(null);
  const [dateError, setDateError] = useState<string | null>(null);

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
    setLoading(true);
    try {
      const r = await api.search(query, { includeOffline });
      setResults(r.results);
      setUnderstood(r.understood);
      setTextOnly(r.text_only);
      setSearched(true);
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

      {searched && results.length === 0 && (
        <p className="empty">No matching photographs yet. Try a broader description.</p>
      )}

      <ul className="results-grid" aria-label="Search results">
        {results.map((r) => (
          <li key={r.file_id} className="result-card">
            <div className="thumb" aria-hidden>
              <span className="thumb-mark">🖼</span>
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
