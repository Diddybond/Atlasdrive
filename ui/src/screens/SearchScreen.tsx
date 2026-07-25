import { useState } from "react";
import { api, SearchResult } from "../api";

export function SearchScreen() {
  const [query, setQuery] = useState("");
  const [includeOffline, setIncludeOffline] = useState(true);
  const [results, setResults] = useState<SearchResult[]>([]);
  const [loading, setLoading] = useState(false);
  const [searched, setSearched] = useState(false);

  async function run(e: React.FormEvent) {
    e.preventDefault();
    setLoading(true);
    try {
      const r = await api.search(query, { includeOffline });
      setResults(r);
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
              <p className="matched">
                Matched: {r.matched.join(", ")} · {(r.score * 100).toFixed(0)}% match
              </p>
              {!r.online && (
                <p className="offline-note">Connect Drive {r.drive_number} to open the original.</p>
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
