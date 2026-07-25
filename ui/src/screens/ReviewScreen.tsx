import { useEffect, useState } from "react";
import { api, ExportSummary, GalleryFace, NamedPerson, PersonFolder, SuggestedFace } from "../api";

/// People, in three clearly separate parts.
///
/// The first version of this screen mixed them, and a face the app *guessed*
/// looked identical to a name the user *gave*. Those are different things and
/// must never share a presentation. So:
///
///   1. People you have named — facts, plus the actions for one person.
///   2. Faces that might be someone — guesses, asked as questions.
///   3. Faces nobody has claimed — the gallery to browse and name.
export function ReviewScreen() {
  const [faces, setFaces] = useState<GalleryFace[]>([]);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [people, setPeople] = useState<NamedPerson[]>([]);
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
    await Promise.all(
      ids.map(async (id) => {
        if (loaded[id]) return;
        const src = await api.faceThumbnail(id);
        if (src) loaded[id] = src;
      }),
    );
    return loaded;
  }

  async function load() {
    const gallery = await api.faceGallery(200);
    setFaces(gallery);
    setPeople(await api.listPeople());
    setThumbs(await loadThumbs(gallery.map((f) => f.face_id), {}));
  }
  useEffect(() => {
    void load();
  }, []);

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

  return (
    <section aria-labelledby="review-heading">
      <h1 id="review-heading">People</h1>
      <p className="lede">
        AtlasDrive groups faces that look alike and can guess who they are, but it never puts a name
        to anyone on its own.
      </p>

      {status && (
        <p className="search-note" role="status">
          {status}
        </p>
      )}

      {/* 1. Facts. */}
      {people.length > 0 && (
        <div className="card">
          <h2>People you have named</h2>
          <ul className="people-list">
            {people.map((p) => (
              <li key={p.id} className="person-row">
                <span className="person-name">{p.display_name}</span>
                <span className="person-counts">
                  {p.confirmed_faces} photograph{p.confirmed_faces === 1 ? "" : "s"}
                </span>
                {p.suggested_faces > 0 && (
                  <button
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
                        placeholder="/Users/you/Desktop/Aimee"
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

      {/* 3. Faces nobody has claimed. */}
      <h2>
        {unnamed.length} face{unnamed.length === 1 ? "" : "s"} nobody has named
      </h2>
      {unnamed.length === 0 ? (
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
                  Who is this?{f.group_size > 1 && <> · {f.group_size}</>}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}

      {selected && (
        <div className="card">
          <h2>Name this face</h2>
          <label className="review-name">
            Who is this?
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
            Naming this confirms {selected.group_size} photograph
            {selected.group_size === 1 ? "" : "s"}, and AtlasDrive will then ask you about any other
            faces that look like them.
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
