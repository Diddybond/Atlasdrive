import { FaceUpgradeCard } from "./FaceUpgradeCard";
import { useEffect, useState } from "react";
import { api, ExportSummary, GalleryFace, NamedPerson, PersonFolder, SuggestedFace, UnnamedOnDrive } from "../api";

/// People, in three clearly separate parts.
///
/// The first version of this screen mixed them, and a face the app *guessed*
/// looked identical to a name the user *gave*. Those are different things and
/// must never share a presentation. So:
///
///   1. People you have named — facts, plus the actions for one person.
///   2. Faces that might be someone — guesses, asked as questions.
///   3. Faces nobody has claimed — the gallery to browse and name.
export function ReviewScreen({ onFind }: { onFind?: (query: string) => void } = {}) {
  // Which drive's faces to show. A wall of unnamed faces from twenty disks is
  // not reviewable; "who is this?" is a far easier question when you know the
  // photograph came off the 2019 weddings drive.
  const [driveFilter, setDriveFilter] = useState<number | null>(null);
  // Which drive the faces on screen actually came from. Distinct from
  // `driveFilter`, which changes the instant the chip is clicked: reading the
  // heading off the chip meant "12 faces on Drive 2" was printed above Drive
  // 1's faces for as long as the fetch took.
  const [shownDrive, setShownDrive] = useState<number | null>(null);
  const [loadingFaces, setLoadingFaces] = useState(false);
  const [drives, setDrives] = useState<{ number: number; name: string; faces: number }[]>([]);
  const [revealNote, setRevealNote] = useState<string | null>(null);
  // Which relationship the named list is showing. Most of a wedding
  // photographer's archive is clients and guests; family is the handful still
  // being searched for in ten years' time.
  const [peopleFilter, setPeopleFilter] = useState<string | null>(null);

  const [faces, setFaces] = useState<GalleryFace[]>([]);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  // Why face pictures are not showing, when they are not — never a silent 🙂.
  const [thumbNote, setThumbNote] = useState<string | null>(null);
  const [people, setPeople] = useState<NamedPerson[]>([]);
  // The suggestion being answered "someone else", and the name typed for it.
  const [otherFor, setOtherFor] = useState<string | null>(null);
  const [otherName, setOtherName] = useState("");
  const [selected, setSelected] = useState<GalleryFace | null>(null);
  const [name, setName] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Reviewing one person's proposals.
  const [reviewing, setReviewing] = useState<NamedPerson | null>(null);
  const [queue, setQueue] = useState<SuggestedFace[]>([]);

  // Per-person actions, shown only for the person being managed.
  const [managing, setManaging] = useState<string | null>(null);
  const [folders, setFolders] = useState<PersonFolder[]>([]);
  const [destination, setDestination] = useState("");
  const [exported, setExported] = useState<ExportSummary | null>(null);
  const [newName, setNewName] = useState("");

  async function loadThumbs(ids: string[], into: Record<string, string>) {
    const loaded = { ...into };
    let failure: string | null = null;
    let missing = 0;
    await Promise.all(
      ids.map(async (id) => {
        if (loaded[id]) return;
        try {
          const src = await api.faceThumbnail(id);
          if (src) loaded[id] = src;
          else missing += 1;
        } catch (err) {
          failure ??= String(err);
        }
      }),
    );
    if (failure) {
      setThumbNote(`Face pictures cannot be shown: ${failure}`);
    } else if (missing > 0 && missing === ids.length) {
      setThumbNote(
        "No pictures are stored for these faces yet, so they show as 🙂. " +
          "The faces are still there and can be named and searched.",
      );
    } else {
      setThumbNote(null);
    }
    return loaded;
  }

  /// Open the photograph this face came from in Finder.
  ///
  /// The face is a crop; the thing worth opening is the original it was cut
  /// from, on whichever drive holds it. The backend says plainly when that
  /// drive is not connected rather than failing silently.
  async function reveal(f: GalleryFace) {
    try {
      setRevealNote(await api.revealInFinder(f.file_id));
    } catch (err) {
      setRevealNote(String(err));
    }
  }

  async function load(drive?: number) {
    const gallery = await api.faceGallery(200, drive);
    setFaces(gallery);
    setShownDrive(drive ?? null);
    setPeople(await api.listPeople());
    setThumbs(await loadThumbs(gallery.map((f) => f.face_id), {}));
  }
  // Faces nobody has named, per drive, counted in the catalogue. These used to
  // be counted within a sample of the first thousand faces, so the chips added
  // up to exactly 1,000 and the heading said 197 on an archive with tens of
  // thousands.
  const [unnamedCounts, setUnnamedCounts] = useState<UnnamedOnDrive[]>([]);
  async function refreshCounts() {
    const counts = await api.unnamedFaceCounts();
    setUnnamedCounts(counts);
    setDrives(
      counts.map((c) => ({
        number: c.drive_number,
        name: c.drive_name ?? `Drive ${c.drive_number}`,
        faces: c.faces,
      })),
    );
  }
  useEffect(() => {
    void refreshCounts();
  }, []);

  const [grouping, setGrouping] = useState(false);
  const [groupNote, setGroupNote] = useState<string | null>(null);
  async function groupLookAlikes() {
    setGrouping(true);
    setGroupNote(null);
    try {
      const r = await api.groupFaces();
      setGroupNote(
        (r.groups_created === 0
          ? "No new groups — every face that looks like another is already grouped."
          : `Put ${r.faces_grouped.toLocaleString()} faces into ${r.groups_created.toLocaleString()} groups. Name one face and its whole group is named.`) +
          (r.groups_merged
            ? ` Joined ${r.groups_merged.toLocaleString()} groups that were the same person on different drives.`
            : ""),
      );
      await refreshCounts();
      await load(driveFilter ?? undefined);
    } catch (err) {
      setGroupNote(String(err));
    } finally {
      setGrouping(false);
    }
  }
  const shownCounts = unnamedCounts.filter((c) => shownDrive === null || c.drive_number === shownDrive);
  const totalUnnamed = shownCounts.reduce((n, c) => n + c.faces, 0);
  const totalTiles = shownCounts.reduce((n, c) => n + c.groups, 0);

  useEffect(() => {
    // Clicking through drives faster than they load must not leave an earlier
    // drive's faces on screen because its reply arrived last.
    let current = true;
    setLoadingFaces(true);
    void (async () => {
      const drive = driveFilter ?? undefined;
      const gallery = await api.faceGallery(200, drive);
      if (!current) return;
      setFaces(gallery);
      setShownDrive(drive ?? null);
      const named = await api.listPeople();
      if (!current) return;
      setPeople(named);
      const loaded = await loadThumbs(gallery.map((f) => f.face_id), {});
      if (!current) return;
      setThumbs(loaded);
      setLoadingFaces(false);
    })();
    return () => {
      current = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [driveFilter]);

  async function openReview(person: NamedPerson) {
    setReviewing(person);
    setManaging(null);
    const pending = await api.pendingSuggestions(person.id, 200);
    setQueue(pending);
    setThumbs(await loadThumbs(pending.map((s) => s.face_id), thumbs));
  }

  /// Answer one proposal and drop it from the queue immediately, so the next
  /// face lands in the same place — this is a fast, repetitive task.
  async function answer(s: SuggestedFace, isThem: boolean) {
    await api.resolveSuggestion(s.cluster_id, isThem);
    setQueue((q) => q.filter((x) => x.cluster_id !== s.cluster_id));
    setPeople(await api.listPeople());
  }

  /// "No — this is someone else": refuse the guess, then name this face.
  ///
  /// Only this face is named, as when naming from a photograph: its group was
  /// just shown to be wrong about one person, so it is not trusted about the
  /// next.
  async function answerSomeoneElse(s: SuggestedFace, name: string) {
    const who = name.trim();
    if (!who) return;
    await api.resolveSuggestion(s.cluster_id, false);
    const r = await api.nameFaceInPhoto(s.face_id, who);
    setQueue((q) => q.filter((x) => x.cluster_id !== s.cluster_id));
    setOtherFor(null);
    setOtherName("");
    setStatus(
      `Tagged as ${r.person.display_name}.` +
        (r.suggested > 0 ? ` ${r.suggested} more possible to review for ${r.person.display_name}.` : ""),
    );
    setPeople(await api.listPeople());
  }

  async function answerAll(person: NamedPerson, isThem: boolean) {
    setBusy(true);
    try {
      const n = isThem
        ? await api.confirmSuggestions(person.id)
        : await api.rejectSuggestions(person.id);
      setQueue([]);
      setStatus(
        isThem
          ? `Confirmed ${n} group${n === 1 ? "" : "s"} as ${person.display_name}.`
          : `Cleared ${n} guess${n === 1 ? "" : "es"}. Those faces are unnamed again.`,
      );
      await load();
    } finally {
      setBusy(false);
    }
  }

  async function tag() {
    if (!selected || !name.trim()) return;
    setBusy(true);
    try {
      const result = await api.tagFace(selected.face_id, name.trim());
      const who = result.person.display_name;
      setStatus(
        result.suggested > 0
          ? `Tagged as ${who}. ${result.suggested} other face${result.suggested === 1 ? "" : "s"} might also be ${who} — review them above.`
          : `Tagged as ${who}.`,
      );
      setSelected(null);
      setName("");
      await load();
    } finally {
      setBusy(false);
    }
  }

  const unnamed = faces.filter((f) => !f.person_name);
  const familyCount = people.filter((p) => p.relationship === "family").length;
  const shownPeople = peopleFilter
    ? people.filter((p) => p.relationship === peopleFilter)
    : people;

  /// Mark someone as family, or take the mark away.
  async function setFamily(p: NamedPerson, isFamily: boolean) {
    await api.setPersonRelationship(p.id, isFamily ? "family" : undefined);
    setPeople(await api.listPeople());
  }

  return (
    <section aria-labelledby="review-heading">
      <h1 id="review-heading">People</h1>
      <p className="lede">
        Name only the people you want to find — family, friends, the couple at a wedding. Everyone
        else can stay unnamed. Once someone is named, search for them on Find like anything else.
      </p>

      <FaceUpgradeCard onFinished={() => void load()} />

      {status && (
        <p className="search-note" role="status">
          {status}
        </p>
      )}

      {/* 1. Facts. */}
      {people.length > 0 && (
        <div className="card">
          <div className="row-between">
            <h2>People you have named</h2>
            {familyCount > 0 && (
              <span className="people-filter">
                <button
                  className={peopleFilter === null ? "chip selected" : "chip"}
                  onClick={() => setPeopleFilter(null)}
                >
                  Everyone <span className="chip-count">{people.length}</span>
                </button>
                <button
                  className={peopleFilter === "family" ? "chip selected" : "chip"}
                  onClick={() => setPeopleFilter("family")}
                >
                  Family <span className="chip-count">{familyCount}</span>
                </button>
              </span>
            )}
          </div>
          <ul className="people-list">
            {shownPeople.map((p) => (
              <li key={p.id} className="person-row">
                <span className="person-name">
                  {p.display_name}
                  {p.relationship === "family" && (
                    <span className="family-badge" title="Family">
                      family
                    </span>
                  )}
                </span>
                <span className="person-counts">
                  {p.confirmed_faces} photograph{p.confirmed_faces === 1 ? "" : "s"}
                </span>
                {onFind && (
                  <button
                    onClick={() => onFind(p.display_name)}
                    aria-label={`Find photographs of ${p.display_name}`}
                  >
                    Find their photographs
                  </button>
                )}
                {p.suggested_faces > 0 && (
                  <button
                    className="ghost"
                    onClick={() => void openReview(p)}
                    aria-label={`Review ${p.suggested_faces} possible matches for ${p.display_name}`}
                  >
                    Review {p.suggested_faces} possible
                  </button>
                )}
                <button
                  className="ghost"
                  onClick={() => {
                    setManaging(managing === p.id ? null : p.id);
                    setReviewing(null);
                    setNewName(p.display_name);
                    setExported(null);
                    setFolders([]);
                  }}
                  aria-label={`More actions for ${p.display_name}`}
                >
                  {managing === p.id ? "Done" : "Manage"}
                </button>

                {managing === p.id && (
                  <div className="person-manage">
                    <label className="checkbox">
                      <input
                        type="checkbox"
                        checked={p.relationship === "family"}
                        onChange={(e) => void setFamily(p, e.target.checked)}
                      />
                      {p.display_name} is family
                    </label>
                    <p className="check-detail">
                      Family are the people you will still be looking for in ten years. Marking
                      them lets you see just them, without wading through clients and guests.
                    </p>

                    <label>
                      Name
                      <input
                        aria-label={`New name for ${p.display_name}`}
                        value={newName}
                        onChange={(e) => setNewName(e.target.value)}
                      />
                    </label>
                    <div className="review-actions">
                      <button
                        onClick={async () => {
                          await api.renamePerson(p.id, newName.trim());
                          setStatus(`Renamed to ${newName.trim()}.`);
                          await load();
                        }}
                        disabled={!newName.trim() || newName === p.display_name}
                      >
                        Save name
                      </button>
                      <button
                        className="ghost"
                        onClick={async () => setFolders(await api.personFolders(p.id))}
                        aria-label={`Show where ${p.display_name}'s photographs are`}
                      >
                        Where are they?
                      </button>
                      <button
                        className="ghost"
                        onClick={async () => {
                          await api.forgetPerson(p.id);
                          setStatus(
                            `Removed ${p.display_name}. Their faces are kept and are unnamed again.`,
                          );
                          setManaging(null);
                          await load();
                        }}
                        aria-label={`Remove ${p.display_name}`}
                      >
                        Remove person
                      </button>
                    </div>

                    {folders.length > 0 && (
                      <ul className="folder-list">
                        {folders.map((f) => (
                          <li key={`${f.drive_number}-${f.relative_folder}`}>
                            <span className="drive-badge">Drive {f.drive_number}</span>
                            <span className="folder-path">{f.relative_folder}</span>
                            <span className="person-counts">{f.photo_count} photographs</span>
                            {f.online && f.absolute_path ? (
                              <button
                                className="ghost"
                                onClick={() => void api.openFolder(f.absolute_path!)}
                                aria-label={`Open ${f.relative_folder} in Finder`}
                              >
                                Open folder
                              </button>
                            ) : (
                              <span className="drive-hit-where">
                                Connect Drive {f.drive_number} to open
                              </span>
                            )}
                          </li>
                        ))}
                      </ul>
                    )}

                    <label>
                      Copy their photographs into
                      <input
                        placeholder="/Users/you/Desktop/Exports"
                        value={destination}
                        onChange={(e) => setDestination(e.target.value)}
                      />
                    </label>
                    <button
                      onClick={async () =>
                        setExported(await api.copyPersonPhotos(p.id, destination.trim()))
                      }
                      disabled={!destination.trim()}
                      aria-label={`Gather ${p.display_name}'s photographs`}
                    >
                      Copy photographs
                    </button>
                    {exported && (
                      <p className="check-detail" role="status">
                        Copied {exported.copied} photograph{exported.copied === 1 ? "" : "s"} to{" "}
                        {exported.destination}.
                        {exported.skipped_offline > 0 && (
                          <>
                            {" "}
                            {exported.skipped_offline} more are on Drive{" "}
                            {exported.drives_to_connect.join(", ")} — connect and run this again.
                          </>
                        )}
                      </p>
                    )}
                  </div>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* 2. Guesses — asked as questions, never shown as names. */}
      {reviewing && (
        <div className="card">
          <div className="row-between">
            <h2>Is this {reviewing.display_name}?</h2>
            <button className="ghost" onClick={() => setReviewing(null)}>
              Close
            </button>
          </div>
          {queue.length === 0 ? (
            <p className="empty">Nothing left to review for {reviewing.display_name}.</p>
          ) : (
            <>
              <p className="drive-meta subtle">
                Strongest matches first — stop whenever they start looking wrong.
              </p>
              <datalist id="review-people">
                {people.map((p) => (
                  <option key={p.id} value={p.display_name} />
                ))}
              </datalist>
              <ul className="suggestion-list">
                {queue.map((s) => (
                  <li key={s.cluster_id} className="suggestion">
                    {thumbs[s.face_id] ? (
                      <img src={thumbs[s.face_id]} alt="" width={96} height={96} />
                    ) : (
                      <span className="face-cell-empty" aria-hidden>
                        🙂
                      </span>
                    )}
                    <span className="person-counts">
                      {(s.score * 100).toFixed(0)}% match
                      {s.group_size > 1 && <> · {s.group_size} photographs</>}
                    </span>
                    <button
                      onClick={() => void answer(s, true)}
                      aria-label={`Yes, this is ${reviewing.display_name}`}
                    >
                      Yes
                    </button>
                    <button
                      className="ghost"
                      onClick={() => void answer(s, false)}
                      aria-label={`No, this is not ${reviewing.display_name}`}
                    >
                      No
                    </button>
                    {otherFor === s.cluster_id ? (
                      <form
                        className="someone-else"
                        onSubmit={(e) => {
                          e.preventDefault();
                          void answerSomeoneElse(s, otherName);
                        }}
                      >
                        <input
                          autoFocus
                          list="review-people"
                          value={otherName}
                          onChange={(e) => setOtherName(e.target.value)}
                          placeholder="Who is it?"
                          aria-label="Who is it?"
                        />
                        <button type="submit" disabled={!otherName.trim()}>
                          Save
                        </button>
                        <button type="button" className="ghost" onClick={() => setOtherFor(null)}>
                          Cancel
                        </button>
                      </form>
                    ) : (
                      <button
                        className="ghost"
                        onClick={() => {
                          setOtherFor(s.cluster_id);
                          setOtherName("");
                        }}
                        aria-label={`This is someone else, not ${reviewing.display_name}`}
                      >
                        Someone else…
                      </button>
                    )}
                  </li>
                ))}
              </ul>
              <div className="review-actions">
                <button onClick={() => void answerAll(reviewing, true)} disabled={busy}>
                  Yes to all
                </button>
                <button
                  className="ghost"
                  onClick={() => void answerAll(reviewing, false)}
                  disabled={busy}
                >
                  No to all
                </button>
              </div>
            </>
          )}
        </div>
      )}

      {/* 3. Faces to choose from. Naming is optional; this is a place to
          pick out the people who matter, not a list to get through. */}
      <h2>Add someone</h2>
      <p className="panel-note">
        Faces AtlasDrive found, biggest groups first. Click a face you know and type their name —
        the whole group is named at once.
        {totalUnnamed > 0 && (
          <span className="subtle">
            {" "}
            {totalUnnamed.toLocaleString()} unnamed faces in {totalTiles.toLocaleString()} groups
            {shownDrive !== null && ` on Drive ${shownDrive}`}.
          </span>
        )}
      </p>
      <div className="row-between face-tools">
        {drives.length > 1 ? (
          <label className="inline-select">
            Faces from
            <select
              aria-label="Faces from"
              value={driveFilter ?? ""}
              onChange={(e) => setDriveFilter(e.target.value === "" ? null : Number(e.target.value))}
            >
              <option value="">Every drive</option>
              {drives.map((d) => (
                <option key={d.number} value={d.number}>
                  Drive {d.number} — {d.name} ({d.faces.toLocaleString()})
                </option>
              ))}
            </select>
          </label>
        ) : (
          <span />
        )}
        <button
          className="ghost"
          onClick={() => void groupLookAlikes()}
          disabled={grouping}
          title="Scans do this themselves. Use it once for drives scanned before grouping existed."
        >
          {grouping ? "Grouping…" : "Group look-alike faces"}
        </button>
      </div>
      {groupNote && (
        <p className="search-note" role="status">
          {groupNote}
        </p>
      )}
      {revealNote && (
        <p className="search-note" role="status">
          {revealNote}
        </p>
      )}
      {thumbNote && (
        <p className="search-note" role="status">
          {thumbNote}
        </p>
      )}

      {selected && (
        <div className="card naming-card">
          <h2>Who is this?</h2>
          <label className="review-name">
            Name
            <input
              autoFocus
              list="known-people"
              placeholder="Type a name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void tag();
              }}
            />
          </label>
          <p className="drive-meta subtle">
            Names {selected.group_size} photograph{selected.group_size === 1 ? "" : "s"}. AtlasDrive
            will then show you other faces that might be them, to confirm or not.
          </p>
          <div className="review-actions">
            <button onClick={() => void tag()} disabled={!name.trim() || busy}>
              Save name
            </button>
            <button className="ghost" onClick={() => setSelected(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {loadingFaces && shownDrive !== driveFilter ? (
        <p className="empty">Loading faces{driveFilter !== null && ` from Drive ${driveFilter}`}…</p>
      ) : unnamed.length === 0 ? (
        <p className="empty">No faces yet. Scan a drive and any faces found will appear here.</p>
      ) : (
        <ul className="face-grid" aria-label="Faces found">
          {unnamed.map((f) => (
            <li key={f.face_id}>
              <button
                className={selected?.face_id === f.face_id ? "face-cell selected" : "face-cell"}
                onClick={() => {
                  setSelected(f);
                  setName("");
                }}
                aria-label={`Unnamed face, ${f.group_size} photograph${f.group_size === 1 ? "" : "s"}`}
              >
                {thumbs[f.face_id] ? (
                  <img src={thumbs[f.face_id]} alt="" width={80} height={80} />
                ) : (
                  <span className="face-cell-empty" aria-hidden>
                    🙂
                  </span>
                )}
                <span className="face-cell-label">
                  {f.group_size > 1 ? `${f.group_size} photos` : "1 photo"}
                </span>
              </button>
              <button
                className="face-reveal"
                onClick={() => void reveal(f)}
                aria-label={`Show the photograph containing this face in Finder, on Drive ${f.drive_number}`}
                title={`Drive ${f.drive_number}${f.drive_name ? ` — ${f.drive_name}` : ""}`}
              >
                Drive {f.drive_number} ›
              </button>
            </li>
          ))}
        </ul>
      )}

      <datalist id="known-people">
        {people.map((p) => (
          <option key={p.id} value={p.display_name} />
        ))}
      </datalist>

      <p className="footnote">
        Face pictures and face data are encrypted on this Mac and never leave it. A name is only ever
        set when you type one.
      </p>
    </section>
  );
}
