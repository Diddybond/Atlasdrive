import { useEffect, useRef, useState } from "react";
import { api, Drive, DriveMatch, PlaceCount, SearchResult, subjectLabel, TagCount } from "../api";
import type { SearchContext } from "../App";
import { NeedsYou, Place } from "./NeedsYou";
import { PhotoViewer } from "./PhotoViewer";
import { OnThisDay } from "./OnThisDay";

export function SearchScreen({
  context,
  onClearContext,
  onGo,
  asked,
}: {
  context?: SearchContext | null;
  onClearContext?: () => void;
  onGo?: (place: Place) => void;
  /// A search requested from another screen, run on arrival.
  asked?: { query: string; n: number } | null;
}) {
  const [query, setQuery] = useState("");
  // Every drive is always searched, plugged in or not — that is the point of
  // the app. The switch to leave unplugged drives out was a way to get fewer,
  // less useful answers.
  const includeOffline = true;
  const [results, setResults] = useState<SearchResult[]>([]);
  const [understood, setUnderstood] = useState<string[]>([]);
  const [textOnly, setTextOnly] = useState(false);
  const [drives, setDrives] = useState<DriveMatch[]>([]);
  const [whereToLook, setWhereToLook] = useState("");
  const [loading, setLoading] = useState(false);
  const [searched, setSearched] = useState(false);
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  // The photograph open in the viewer, where its faces can be named.
  const [viewing, setViewing] = useState<SearchResult | null>(null);
  // Places photographs were taken, from their GPS (D-106).
  const [places, setPlaces] = useState<PlaceCount[]>([]);
  useEffect(() => {
    void api.topPlaces(24).then(setPlaces, () => undefined);
  }, []);
  const [tags, setTags] = useState<TagCount[]>([]);
  const [allSubjects, setAllSubjects] = useState(false);
  // Bumped on every search, so thumbnails from an abandoned one are dropped.
  const searchToken = useRef(0);
  // Which drive is being browsed, and which subjects have been picked. Both
  // narrow the search rather than replacing the typed query, so they can be
  // combined: "children, at weddings, on Drive 2".
  const [driveFilter, setDriveFilter] = useState<number | null>(null);
  const [pickedTags, setPickedTags] = useState<string[]>([]);
  const [allDrives, setAllDrives] = useState<Drive[]>([]);
  const [nameNote, setNameNote] = useState<string | null>(null);
  const [findingNames, setFindingNames] = useState(false);

  const [lightroom, setLightroom] = useState(false);
  useEffect(() => {
    void api.listDrives().then(setAllDrives);
    void api.lightroomAvailable().then(setLightroom, () => setLightroom(false));
  }, []);

  async function openWith(fileId: string, app?: "lightroom") {
    const message = await api.openOriginal(fileId, app).catch((e) => String(e));
    setRevealed((prev) => ({ ...prev, [fileId]: message }));
  }

  // The subject list follows the selected drive, so every chip on screen leads
  // to photographs on the disk being browsed rather than to an empty result.
  // A short list of subjects that narrow a search, until asked for all of them.
  useEffect(() => {
    void api
      .catalogueTags(allSubjects ? 60 : 16, driveFilter ?? undefined, !allSubjects)
      .then(setTags);
  }, [driveFilter, allSubjects]);

  // Arriving from Events with a filter should show that shoot immediately —
  // landing on an empty search box having just asked to see something would be
  // a dead end.
  useEffect(() => {
    // Every photograph of that shoot or client: no search words, no subjects
    // left over from an earlier search narrowing it.
    if (context) {
      setPickedTags([]);
      void search("", []);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [context?.eventId, context?.client, context?.year]);
  useEffect(() => {
    if (asked) void search(asked.query);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [asked?.n]);
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

  /// Fill in thumbnails a batch at a time, abandoning the work if the owner
  /// searches again — a slow batch from a discarded search must never paint
  /// over the results now on screen.
  async function loadThumbnailsProgressively(ids: string[], token: number) {
    const BATCH = 60;
    for (let i = 0; i < ids.length; i += BATCH) {
      if (searchToken.current !== token) return;
      const slice = ids.slice(i, i + BATCH);
      const loaded: Record<string, string> = {};
      await Promise.all(
        slice.map(async (id) => {
          const src = await api.photoThumbnail(id, 240);
          if (src) loaded[id] = src;
        }),
      );
      if (searchToken.current !== token) return;
      setThumbs((prev) => ({ ...prev, ...loaded }));
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
    const token = ++searchToken.current;
    void token;
    try {
      const r = await api.search(term, {
        includeOffline,
        drive: driveFilter ?? undefined,
        tags: picked ?? pickedTags,
        eventId: context?.eventId,
        client: context?.client,
        year: context?.year,
      });
      setResults(r.results);
      setUnderstood(r.understood);
      setTextOnly(r.text_only);
      setDrives(r.drives);
      setWhereToLook(r.where_to_look);
      setSearched(true);

      // Thumbnails come from the local catalogue, so they appear whether or not
      // the drive is connected.
      //
      // Fetched in batches rather than all at once. A subject like "people"
      // matches ten thousand photographs, and asking for ten thousand
      // thumbnails in one breath locks the window; asking for sixty at a time
      // fills the page as you read it. Every result is on screen immediately
      // either way — only the pictures arrive progressively.
      setThumbs({});
      void loadThumbnailsProgressively(r.results.map((x) => x.file_id), searchToken.current);
    } finally {
      setLoading(false);
    }
  }

  const driveName = (n: number) => {
    const d = allDrives.find((x) => x.drive_number === n);
    return d?.physical_location ? `kept in ${d.physical_location}` : null;
  };

  return (
    <section aria-labelledby="search-heading">
      <h1 id="search-heading">Find a photograph</h1>
      <p className="lede">
        Describe it, name someone, or type a year. Every drive is searched, plugged in or not, and
        each photograph tells you which drive holds it.
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

      <form className="search-bar big" onSubmit={run} role="search">
        <input
          type="search"
          aria-label="Search photographs"
          placeholder="e.g. bikes, Christmas 1998, a wedding in the rain"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button type="submit" disabled={loading}>
          {loading ? "Searching…" : "Search"}
        </button>
      </form>

      {!searched && onGo && <NeedsYou onGo={onGo} />}

      {allDrives.length > 1 && (
        <label className="inline-select">
          Look on
          <select
            aria-label="Look on"
            value={driveFilter ?? ""}
            onChange={(e) => {
              const v = e.target.value;
              setDriveFilter(v === "" ? null : Number(v));
              // Subjects belong to the drive they were picked from; keeping
              // them would silently search for something the new drive may
              // not have and return nothing for no visible reason.
              setPickedTags([]);
            }}
          >
            <option value="">Every drive</option>
            {allDrives.map((d) => (
              <option key={d.id} value={d.drive_number}>
                Drive {d.drive_number}
                {d.friendly_name ? ` — ${d.friendly_name}` : ""} ({(d.image_count ?? 0).toLocaleString()})
              </option>
            ))}
          </select>
        </label>
      )}

      {!searched && !context && <OnThisDay onOpen={setViewing} />}

      {places.length > 0 && (
        <div className="subjects places">
          <h2>Or pick a place</h2>
          <ul className="tag-cloud">
            {places.map((p) => {
              const on = pickedTags.includes(p.name);
              return (
                <li key={p.name}>
                  <button
                    className={on ? "tag-chip selected" : "tag-chip"}
                    aria-pressed={on}
                    onClick={() => {
                      const next = on ? pickedTags.filter((x) => x !== p.name) : [...pickedTags, p.name];
                      setPickedTags(next);
                      void search(query, next);
                    }}
                    aria-label={
                      on
                        ? `Stop narrowing to ${p.name}`
                        : `Narrow to the ${p.photographs} photographs taken in ${p.name}`
                    }
                  >
                    {p.name}
                    <span className="tag-count">{p.photographs.toLocaleString()}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {tags.length > 0 && (
        <div className="subjects">
          <div className="row-between">
            <h2>{allSubjects ? "All subjects" : "Or pick a subject"}</h2>
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
                      // Subjects are filters, not text: the box belongs to
                      // the owner's own typing (see D-074).
                      void search(query, next);
                    }}
                    aria-label={
                      on
                        ? `Stop narrowing to ${subjectLabel(t.tag)}`
                        : `Narrow to the ${t.count} photographs showing ${subjectLabel(t.tag)}`
                    }
                  >
                    {subjectLabel(t.tag)}
                    <span className="tag-count">{t.count.toLocaleString()}</span>
                  </button>
                </li>
              );
            })}
          </ul>
          {pickedTags.length > 1 && (
            <p className="panel-note">
              Showing only photographs that contain <strong>all</strong> of these:{" "}
              {pickedTags.map(subjectLabel).join(", ")}.
            </p>
          )}
          <div className="row-between name-row">
            <button className="ghost" onClick={() => setAllSubjects((v) => !v)}>
              {allSubjects ? "Show fewer subjects" : "Show all subjects"}
            </button>
            {allSubjects && (
              <button
                className="ghost"
                disabled={findingNames}
                title="Names read on things in the picture — a van, a shop front, a bottle. Never guessed from the image."
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
                      return api.catalogueTags(60, driveFilter ?? undefined, false).then(setTags);
                    })
                    .finally(() => setFindingNames(false));
                }}
              >
                {findingNames ? "Reading…" : "Find names in photographs"}
              </button>
            )}
          </div>
          {nameNote && (
            <p className="search-note" role="status">
              {nameNote}
            </p>
          )}
        </div>
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
                  {d.online ? "Plugged in" : "Not plugged in"}
                </span>
                {!d.online && d.physical_location && (
                  <span className="drive-hit-where">Kept in {d.physical_location}</span>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}

      {searched && results.length > 0 && (
        <p className="search-note" role="status">
          {pickedTags.length > 0
            ? `${results.length.toLocaleString()} photograph${results.length === 1 ? "" : "s"} — every match, not a sample.`
            : `${results.length.toLocaleString()} photograph${results.length === 1 ? "" : "s"} found.`}
          {understood.length > 0 &&
            ` Looking for: ${understood.join(", ")} (a best guess from the pictures).`}
        </p>
      )}

      {searched && results.length === 0 && (
        <p className="empty">Nothing matched. Try fewer words, a year, or pick a subject.</p>
      )}
      {searched && textOnly && (
        <p className="search-note">Searched names, folders and subjects.</p>
      )}

      <ul className="results-grid" aria-label="Search results">
        {results.map((r) => {
          const where = driveName(r.drive_number);
          return (
            <li key={r.file_id} className="result-card">
              <button
                type="button"
                className="thumb thumb-open"
                onClick={() => setViewing(r)}
                aria-label={`View ${r.filename} and name the people in it`}
              >
                {thumbs[r.file_id] ? (
                  <img src={thumbs[r.file_id]} alt="" loading="lazy" />
                ) : (
                  <span className="thumb-mark" aria-hidden>
                    🖼
                  </span>
                )}
              </button>
              <div className="result-body">
                <p className="where">
                  <span className="drive-badge big">Drive {r.drive_number}</span>
                  <span className={r.online ? "status online" : "status offline"}>
                    {r.online ? "Plugged in" : where ?? "Not plugged in"}
                  </span>
                </p>
                {r.also_on && r.also_on.length > 0 && (
                  <p className="also-on">Also on Drive {r.also_on.join(", ")}</p>
                )}
                <p className="filename" title={r.relative_path}>
                  {r.filename}
                </p>
                <p className="date">{r.date_label ?? "Date unknown"}</p>
                {r.online ? (
                  <button
                    onClick={() => void reveal(r.file_id)}
                    aria-label={`Show ${r.filename} in Finder`}
                  >
                    Show in Finder
                  </button>
                ) : (
                  <p className="offline-note">Plug in Drive {r.drive_number} to open the original.</p>
                )}
                {revealed[r.file_id] && (
                  <p className="check-detail" role="status">
                    {revealed[r.file_id]}
                  </p>
                )}
                <details className="more">
                  <summary>More</summary>
                  {r.online && (
                    <button
                      className="ghost"
                      onClick={() => void openWith(r.file_id)}
                      aria-label={`View ${r.filename} and name the people in it`}
                    >
                      Open
                    </button>
                  )}
                  {r.online && lightroom && (
                    <button
                      className="ghost"
                      onClick={() => void openWith(r.file_id, "lightroom")}
                      aria-label={`Open ${r.filename} in Lightroom Classic`}
                    >
                      Open in Lightroom Classic
                    </button>
                  )}
                  <button
                    className="ghost"
                    onClick={() => void findSimilar(r.file_id, r.filename)}
                    aria-label={`Find photographs that look like ${r.filename}`}
                  >
                    More like this
                  </button>
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
                      Correct the date
                    </button>
                  )}
                  <p className="matched">Found by: {r.matched.join(", ")}</p>
                </details>
              </div>
            </li>
          );
        })}
      </ul>
      {viewing && (
        <PhotoViewer
          fileId={viewing.file_id}
          filename={viewing.filename}
          driveNumber={viewing.drive_number}
          online={viewing.online}
          dateLabel={viewing.date_label}
          preview={thumbs[viewing.file_id]}
          onClose={() => setViewing(null)}
        />
      )}
    </section>
  );
}
