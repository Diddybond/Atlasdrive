import { useEffect, useState } from "react";
import { api, NamedPerson, PhotoFace } from "../api";

interface Props {
  fileId: string;
  filename: string;
  driveNumber: number;
  online: boolean;
  dateLabel?: string | null;
  /// The small preview already on screen, shown until the larger one arrives.
  preview?: string;
  onClose: () => void;
}

/// One photograph, large, with a box on every face in it.
///
/// Click a box and type a name. Only that face is named: the people in a
/// wedding group shot are different people even when the grouping thinks two
/// of them look alike. Look-alikes elsewhere come back as suggestions on the
/// People screen, to accept or refuse.
export function PhotoViewer({ fileId, filename, driveNumber, online, dateLabel, preview, onClose }: Props) {
  const [src, setSrc] = useState<string | undefined>(preview);
  const [faces, setFaces] = useState<PhotoFace[] | null>(null);
  const [people, setPeople] = useState<NamedPerson[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function loadFaces() {
    setFaces(await api.facesInPhoto(fileId).catch(() => []));
  }

  useEffect(() => {
    void loadFaces();
    void api.listPeople().then(setPeople, () => undefined);
    // The larger picture comes from the original when its drive is plugged in,
    // which can take a moment; the preview stands in until then.
    void api.photoView(fileId).then((big) => big && setSrc(big), () => undefined);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fileId]);

  function pick(f: PhotoFace) {
    setSelected(f.face_id);
    setName(f.person_name ?? "");
    setNote(null);
  }

  async function save(e: React.FormEvent) {
    e.preventDefault();
    if (!selected || !name.trim()) return;
    setSaving(true);
    try {
      const r = await api.nameFaceInPhoto(selected, name.trim());
      setNote(
        `Tagged as ${r.person.display_name}.` +
          (r.suggested > 0
            ? ` ${r.suggested} more possible ${r.suggested === 1 ? "photograph" : "photographs"} of ${r.person.display_name} to review on People.`
            : ""),
      );
      setSelected(null);
      setName("");
      await loadFaces();
      void api.listPeople().then(setPeople, () => undefined);
    } catch (err) {
      setNote(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function notAFace() {
    if (!selected) return;
    await api.notAFace(selected);
    setSelected(null);
    setName("");
    setNote("Removed. It will no longer appear as a face.");
    await loadFaces();
  }

  const index = (id: string) => (faces ?? []).findIndex((f) => f.face_id === id) + 1;

  return (
    <div className="viewer-backdrop" onClick={onClose}>
      <div
        className="viewer"
        role="dialog"
        aria-modal="true"
        aria-label={`Photograph ${filename}`}
        onClick={(e) => e.stopPropagation()}
      >
        <header className="viewer-head">
          <span className="drive-badge big">Drive {driveNumber}</span>
          <span className="viewer-title" title={filename}>
            {filename}
          </span>
          <span className="subtle">{dateLabel ?? "Date unknown"}</span>
          <button className="ghost" onClick={onClose} aria-label="Close photograph">
            Close
          </button>
        </header>

        <div className="viewer-stage">
          <div className="viewer-frame">
            {src ? <img src={src} alt={filename} /> : <div className="viewer-empty">No preview</div>}
            {src &&
              (faces ?? []).map((f, i) => (
                <button
                  key={f.face_id}
                  className={
                    "face-box" +
                    (f.person_name ? " named" : "") +
                    (selected === f.face_id ? " selected" : "")
                  }
                  style={{
                    left: `${f.x * 100}%`,
                    top: `${f.y * 100}%`,
                    width: `${f.w * 100}%`,
                    height: `${f.h * 100}%`,
                  }}
                  onClick={() => pick(f)}
                  aria-label={f.person_name ? `Face ${i + 1}: ${f.person_name}` : `Face ${i + 1}: not named`}
                >
                  <span className="face-label">{f.person_name ?? `${i + 1}`}</span>
                </button>
              ))}
          </div>
        </div>

        <div className="viewer-panel">
          {faces === null ? (
            <p className="subtle">Looking for faces…</p>
          ) : faces.length === 0 ? (
            <p className="subtle">No faces were found in this photograph.</p>
          ) : selected ? (
            <form className="viewer-form" onSubmit={(e) => void save(e)}>
              <label>
                Who is face {index(selected)}?
                <input
                  autoFocus
                  list="viewer-people"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="Type a name"
                />
              </label>
              <datalist id="viewer-people">
                {people.map((p) => (
                  <option key={p.id} value={p.display_name} />
                ))}
              </datalist>
              <div className="viewer-actions">
                <button type="submit" disabled={!name.trim() || saving}>
                  {saving ? "Saving…" : "Save name"}
                </button>
                <button type="button" className="ghost" onClick={() => void notAFace()}>
                  Not a face
                </button>
                <button type="button" className="ghost" onClick={() => setSelected(null)}>
                  Cancel
                </button>
              </div>
            </form>
          ) : (
            <p className="subtle">
              {faces.length === 1 ? "1 face" : `${faces.length} faces`} in this photograph. Click one to
              name it.
              {!online && ` Plug in Drive ${driveNumber} for a sharper picture.`}
            </p>
          )}
          {note && (
            <p className="search-note" role="status">
              {note}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
