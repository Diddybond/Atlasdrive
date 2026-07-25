import { useEffect, useState } from "react";
import { api, ExportSummary, GalleryFace, NamedPerson, PersonFolder } from "../api";

/// Browsing faces, not names.
///
/// The gallery leads because you often do not know who someone is — you
/// recognise them. A name is only ever attached when you type one.
export function ReviewScreen() {
  const [faces, setFaces] = useState<GalleryFace[]>([]);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [people, setPeople] = useState<NamedPerson[]>([]);
  const [selected, setSelected] = useState<GalleryFace | null>(null);
  const [name, setName] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [gathering, setGathering] = useState<NamedPerson | null>(null);
  const [destination, setDestination] = useState("");
  const [exported, setExported] = useState<ExportSummary | null>(null);
  const [folders, setFolders] = useState<Record<string, PersonFolder[]>>({});
  const [renaming, setRenaming] = useState<NamedPerson | null>(null);
  const [newName, setNewName] = useState("");

  async function showFolders(person: NamedPerson) {
    setFolders({ ...folders, [person.id]: await api.personFolders(person.id) });
  }

  async function forget(person: NamedPerson) {
    await api.forgetPerson(person.id);
    setStatus(
      `Removed ${person.display_name}. Their faces are kept and are unnamed again.`,
    );
    await load();
  }

  async function rename(person: NamedPerson) {
    if (!newName.trim()) return;
    await api.renamePerson(person.id, newName.trim());
    setRenaming(null);
    setNewName("");
    await load();
  }

  async function load() {
    const gallery = await api.faceGallery(200);
    setFaces(gallery);
    setPeople(await api.listPeople());
    // Fetch crops individually so a large gallery paints as it arrives.
    const loaded: Record<string, string> = {};
    await Promise.all(
      gallery.map(async (f) => {
        const src = await api.faceThumbnail(f.face_id);
        if (src) loaded[f.face_id] = src;
      }),
    );
    setThumbs(loaded);
  }
  useEffect(() => {
    void load();
  }, []);

  async function tag() {
    if (!selected || !name.trim()) return;
    setBusy(true);
    try {
      const result = await api.tagFace(selected.face_id, name.trim());
      const who = result.person.display_name;
      setStatus(
        result.suggested > 0
          ? `Tagged as ${who}. ${result.suggested} other face${result.suggested === 1 ? "" : "s"} look like ${who} — check the ones marked "Is this ${who}?" below.`
          : `Tagged as ${who}. AtlasDrive will suggest ${who} when it sees a similar face on a later scan.`,
      );
      setSelected(null);
      setName("");
      await load();
    } finally {
      setBusy(false);
    }
  }

  /// Take a wrong name off a face without deleting the face or the person.
  async function untag() {
    if (!selected?.cluster_id) return;
    setBusy(true);
    try {
      await api.resolveSuggestion(selected.cluster_id, false);
      setStatus("Name removed. The face is unnamed again.");
      setSelected(null);
      await load();
    } finally {
      setBusy(false);
    }
  }

  async function gather(person: NamedPerson) {
    if (!destination.trim()) return;
    setBusy(true);
    try {
      setExported(await api.copyPersonPhotos(person.id, destination.trim()));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section aria-labelledby="review-heading">
      <h1 id="review-heading">People</h1>
      <p className="lede">
        Every face AtlasDrive has found. You do not need to know who anyone is to browse — click a
        face you recognise and give it a name. Nothing is ever named automatically.
      </p>

      {status && (
        <p className="search-note" role="status">
          {status}
        </p>
      )}

      {faces.length === 0 ? (
        <p className="empty">
          No faces yet. Scan a drive and any faces found will appear here.
        </p>
      ) : (
        <ul className="face-grid" aria-label="Faces found">
          {faces.map((f) => (
            <li key={f.face_id}>
              <button
                className={selected?.face_id === f.face_id ? "face-cell selected" : "face-cell"}
                onClick={() => {
                  setSelected(f);
                  setName(f.person_name ?? "");
                }}
                aria-label={
                  f.person_name
                    ? `${f.person_name}, ${f.group_size} photograph${f.group_size === 1 ? "" : "s"}`
                    : `Unnamed face, ${f.group_size} photograph${f.group_size === 1 ? "" : "s"}`
                }
              >
                {thumbs[f.face_id] ? (
                  <img src={thumbs[f.face_id]} alt="" width={80} height={80} />
                ) : (
                  <span className="face-cell-empty" aria-hidden>
                    🙂
                  </span>
                )}
                <span className="face-cell-label">
                  {f.person_name ?? "Who is this?"}
                  {f.group_size > 1 && <> · {f.group_size}</>}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}

      {selected && (
        <div className="card">
          <h2>{selected.person_name ? "Change this name" : "Name this face"}</h2>
          {selected.person_name && (
            <p className="drive-meta subtle">
              Currently tagged as {selected.person_name}. Typing a different name moves this group
              to that person; the faces themselves are never lost.
            </p>
          )}
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
            {selected.group_size === 1 ? "" : "s"} and teaches AtlasDrive to suggest them next time.
          </p>
          <div className="review-actions">
            <button onClick={() => void tag()} disabled={!name.trim() || busy}>
              {busy ? "Saving…" : "Save name"}
            </button>
            {selected.person_name && (
              <button
                className="ghost"
                onClick={() => void untag()}
                aria-label={`Remove the name from this face`}
              >
                Remove name
              </button>
            )}
            <button className="ghost" onClick={() => setSelected(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {people.length > 0 && (
        <div className="card">
          <h2>People you have named</h2>
          <ul className="people-list">
            {people.map((p) => (
              <li key={p.id}>
                <span className="person-name">{p.display_name}</span>
                <span className="person-counts">
                  {p.confirmed_faces} confirmed
                  {p.suggested_faces > 0 && <> · {p.suggested_faces} awaiting confirmation</>}
                </span>
                <button
                  className="ghost"
                  onClick={() => {
                    setGathering(p);
                    setExported(null);
                  }}
                  aria-label={`Gather ${p.display_name}'s photographs`}
                >
                  Gather their photographs
                </button>
                <button
                  className="ghost"
                  onClick={() => void showFolders(p)}
                  aria-label={`Show where ${p.display_name}'s photographs are`}
                >
                  Where are they?
                </button>
                <button
                  className="ghost"
                  onClick={() => {
                    setRenaming(p);
                    setNewName(p.display_name);
                  }}
                  aria-label={`Rename ${p.display_name}`}
                >
                  Rename
                </button>
                <button
                  className="ghost"
                  onClick={() => void forget(p)}
                  aria-label={`Remove ${p.display_name}`}
                >
                  Remove
                </button>

                {renaming?.id === p.id && (
                  <span className="rename-row">
                    <input
                      aria-label={`New name for ${p.display_name}`}
                      value={newName}
                      onChange={(e) => setNewName(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") void rename(p);
                      }}
                    />
                    <button onClick={() => void rename(p)} disabled={!newName.trim()}>
                      Save
                    </button>
                    <button className="ghost" onClick={() => setRenaming(null)}>
                      Cancel
                    </button>
                  </span>
                )}

                {folders[p.id] && (
                  <ul className="folder-list">
                    {folders[p.id].map((f) => (
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
              </li>
            ))}
          </ul>
        </div>
      )}

      {gathering && (
        <div className="card">
          <h2>Gather {gathering.display_name}&rsquo;s photographs</h2>
          <p className="drive-meta subtle">
            Copies the originals into a folder you choose. Nothing is moved, and nothing is ever
            written to the drive itself.
          </p>
          <label>
            Copy into
            <input
              placeholder="/Users/you/Desktop/Aimee"
              value={destination}
              onChange={(e) => setDestination(e.target.value)}
            />
          </label>
          <div className="review-actions">
            <button onClick={() => void gather(gathering)} disabled={!destination.trim() || busy}>
              {busy ? "Copying…" : "Copy photographs"}
            </button>
            <button className="ghost" onClick={() => setGathering(null)}>
              Cancel
            </button>
          </div>
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

      {/* Typing an existing name reuses that person rather than duplicating. */}
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
