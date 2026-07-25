import { useEffect, useState } from "react";
import { api, Drive, DriveContents, DriveCoverage, Volume } from "../api";

export function DrivesScreen() {
  const [coverage, setCoverage] = useState<Record<number, DriveCoverage>>({});
  const [volumes, setVolumes] = useState<Volume[]>([]);
  const [registerNote, setRegisterNote] = useState<string | null>(null);
  const [driveNotes, setDriveNotes] = useState<Record<number, string>>({});
  const [folders, setFolders] = useState<string[]>([]);
  const [drives, setDrives] = useState<Drive[]>([]);
  const [showForm, setShowForm] = useState(false);
  const [number, setNumber] = useState("");
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [writeManifest, setWriteManifest] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [contents, setContents] = useState<Record<number, DriveContents>>({});
  const [rescanning, setRescanning] = useState<number | null>(null);

  /// Scan a drive, or check an already-scanned one for new photographs.
  ///
  /// The outcome is shown against the drive it concerns. It used to go to a
  /// note at the top of the page, far enough from the button that pressing it
  /// looked like nothing had happened.
  async function rescan(drive: Drive) {
    setRescanning(drive.drive_number);
    setDriveNotes((n) => ({ ...n, [drive.drive_number]: "" }));
    try {
      const message = await api.rescanDrive(drive.drive_number);
      setDriveNotes((n) => ({ ...n, [drive.drive_number]: message }));
    } catch (err) {
      setDriveNotes((n) => ({ ...n, [drive.drive_number]: String(err) }));
    } finally {
      setRescanning(null);
    }
  }

  async function load() {
    setDrives(await api.listDrives());
    // What is actually stored on each drive, readable with them all unplugged.
    const all = await api.driveContents();
    setContents(Object.fromEntries(all.map((c) => [c.drive_number, c])));
  }

  async function saveDetails(e: React.FormEvent<HTMLFormElement>, drive: Drive) {
    e.preventDefault();
    const form = new FormData(e.currentTarget);
    const categories = String(form.get("categories") ?? "")
      .split(",")
      .map((c) => c.trim())
      .filter(Boolean);
    await api.updateDriveDetails({
      driveNumber: drive.drive_number,
      physicalLocation: String(form.get("location") ?? ""),
      categories,
    });
    setEditing(null);
    await load();
  }
  /// A drive was picked from the list: use it, and offer the folders on it
  /// where photographs usually live.
  /// Whether the chosen path sits on a read-only volume. Matched on the volume
  /// root so it still holds when a folder inside the drive is chosen.
  const chosenIsReadOnly = volumes.some(
    (v) => v.is_read_only && (path === v.path || path.startsWith(`${v.path}/`)),
  );

  async function pickVolume(chosen: string) {
    setPath(chosen);
    setFolders(chosen ? await api.likelyPhotoFolders(chosen) : []);
    const v = volumes.find((x) => x.path === chosen);
    // Pre-fill the name from the disk's own label, which is nearly always what
    // is wanted and is otherwise typed out again by hand.
    if (v && !name.trim()) setName(v.name);
  }

  /// For anything the list cannot offer — a folder inside a drive, a network
  /// share, a disk image.
  async function browse() {
    const picked = await api.chooseFolder("Choose the folder to index");
    if (picked) {
      setPath(picked);
      setFolders([]);
    }
  }

  useEffect(() => {
    void load();
    void api.connectedVolumes().then(setVolumes);
    // Coverage is what tells you whether a drive can be unplugged, so it is
    // loaded whenever this screen is, not behind a button.
    void api.driveCoverage().then((rows) => {
      setCoverage(Object.fromEntries(rows.map((c) => [c.drive_number, c])));
    });
  }, []);

  async function register(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    const n = parseInt(number, 10);
    if (!Number.isInteger(n) || n <= 0) {
      setError("Enter a positive whole number, matching the label on the drive.");
      return;
    }
    try {
      const drive = await api.registerDrive({ number: n, path, name, writeManifest });
      // Registering can succeed and still have something worth saying — a
      // read-only drive that could not take the identity file, most often.
      setRegisterNote(drive.note ?? null);
      setShowForm(false);
      setNumber("");
      setName("");
      setPath("");
      setFolders([]);
      await load();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section aria-labelledby="drives-heading">
      <div className="row-between">
        <h1 id="drives-heading">Drives</h1>
        <button onClick={() => setShowForm((s) => !s)}>
          {showForm ? "Cancel" : "Register a drive"}
        </button>
      </div>
      <p className="lede">
        Give each physical drive a memorable number, matching the label you write on it. The drive
        stays searchable even after you disconnect it.
      </p>

      {showForm && (
        <form className="card form" onSubmit={register}>
          <h2>Register a drive</h2>
          <label>
            Physical drive number
            <input
              inputMode="numeric"
              value={number}
              onChange={(e) => setNumber(e.target.value)}
              placeholder="e.g. 14"
            />
          </label>
          <label>
            Friendly name
            <input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. AtlasDrive A" />
          </label>
          <label>
            Which drive?
            <select
              value={volumes.some((v) => v.path === path) ? path : ""}
              onChange={(e) => void pickVolume(e.target.value)}
            >
              <option value="">Choose a connected drive…</option>
              {volumes.map((v) => (
                <option key={v.path} value={v.path} disabled={v.registered_as != null}>
                  {v.registered_as != null
                    ? `${v.name} — already Drive ${v.registered_as}`
                    : v.is_startup_disk
                      ? `${v.name} — this Mac's startup disk`
                      : v.is_read_only
                        ? `${v.name} — read-only`
                        : v.name}
                </option>
              ))}
            </select>
          </label>

          {folders.length > 0 && (
            <div className="folder-hint">
              <p className="check-detail">
                Photographs are usually in one of these. Pick one to index just that folder, or
                leave it to index the whole drive.
              </p>
              <div className="folder-choices">
                {folders.map((f) => (
                  <button
                    key={f}
                    type="button"
                    className={path === f ? "chip selected" : "chip"}
                    onClick={() => setPath(f)}
                  >
                    {f.split("/").pop()}
                  </button>
                ))}
              </div>
            </div>
          )}

          <label>
            Folder to index
            <span className="row-between">
              <input
                value={path}
                onChange={(e) => setPath(e.target.value)}
                placeholder="Choose a drive above"
                aria-label="Folder to index"
              />
              <button type="button" className="ghost" onClick={browse}>
                Browse…
              </button>
            </span>
          </label>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={writeManifest && !chosenIsReadOnly}
              disabled={chosenIsReadOnly}
              onChange={(e) => setWriteManifest(e.target.checked)}
            />
            Save a small identity file on the drive so it is recognised next time (nothing else on the
            drive is changed)
          </label>
          {chosenIsReadOnly && (
            <p className="check-detail">
              This drive is read-only, so nothing can be written to it — which is exactly how
              AtlasDrive reads your photographs anyway. It will be recognised by its name and
              contents instead.
            </p>
          )}
          {error && <p className="error" role="alert">{error}</p>}
          <button type="submit">Register drive</button>
        </form>
      )}

      {registerNote && (
        <p className="search-note" role="status">
          {registerNote}
        </p>
      )}

      <ul className="drive-list">
        {drives.map((d) => (
          <li key={d.id} className="card drive-card">
            <div className="drive-number" aria-hidden>{d.drive_number}</div>
            <div className="drive-info">
              <p className="drive-name">{d.friendly_name || `Drive ${d.drive_number}`}</p>
              <p className="drive-meta">
                <span className={d.status === "online" ? "status online" : "status offline"}>
                  {d.status === "online" ? "Connected" : "Disconnected"}
                </span>
                {d.image_count != null && <> · {d.image_count.toLocaleString()} photographs</>}
                {d.physical_location && <> · {d.physical_location}</>}
              </p>
              {coverage[d.drive_number] && (
                <p
                  // The wording and the verdict both come from the backend.
                  // Deciding here is what produced "Safe to unplug" on a drive
                  // that had never been scanned.
                  className={coverage[d.drive_number].can_unplug ? "coverage done" : "coverage working"}
                  role="status"
                >
                  {coverage[d.drive_number].summary}
                </p>
              )}
              {d.categories && d.categories.length > 0 && (
                <p className="drive-meta subtle">What's on it: {d.categories.join(", ")}</p>
              )}
              {contents[d.drive_number] && (
                <p className="drive-meta subtle">
                  {contents[d.drive_number].earliest_date && contents[d.drive_number].latest_date && (
                    <>
                      {contents[d.drive_number].earliest_date!.slice(0, 4)}–
                      {contents[d.drive_number].latest_date!.slice(0, 4)}
                      {" · "}
                    </>
                  )}
                  {contents[d.drive_number].top_tags.length > 0 ? (
                    <>
                      Pictures of{" "}
                      {contents[d.drive_number].top_tags
                        .slice(0, 5)
                        .map((t) => `${t.tag} (${t.count})`)
                        .join(", ")}
                    </>
                  ) : (
                    <>Nothing recognised yet — scan this drive to find out what is on it</>
                  )}
                </p>
              )}
              <p className="drive-meta subtle">
                Last scanned: {d.last_scan_at ?? "never"}
              </p>

              {editing === d.id ? (
                <form className="form drive-edit" onSubmit={(e) => void saveDetails(e, d)}>
                  <label>
                    Where this drive is kept
                    <input
                      name="location"
                      defaultValue={d.physical_location ?? ""}
                      placeholder="Drawer 2, studio shelf B…"
                    />
                  </label>
                  <label>
                    What's on it (comma separated)
                    <input
                      name="categories"
                      defaultValue={(d.categories ?? []).join(", ")}
                      placeholder="holidays, scanned prints"
                    />
                  </label>
                  <button type="submit">Save</button>
                  <button type="button" className="ghost" onClick={() => setEditing(null)}>
                    Cancel
                  </button>
                </form>
              ) : (
                <>
                  <button
                    onClick={() => void rescan(d)}
                    disabled={rescanning === d.drive_number}
                    aria-label={
                      coverage[d.drive_number] && coverage[d.drive_number].discovered === 0
                        ? `Scan Drive ${d.drive_number}`
                        : `Check Drive ${d.drive_number} for new photographs`
                    }
                  >
                    {rescanning === d.drive_number
                      ? "Starting…"
                      : coverage[d.drive_number] && coverage[d.drive_number].discovered === 0
                        ? "Scan this drive"
                        : "Check for new photographs"}
                  </button>
                  <button
                    className="ghost"
                    onClick={() => setEditing(d.id)}
                    aria-label={`Edit location and categories for Drive ${d.drive_number}`}
                  >
                    Edit location &amp; categories
                  </button>
                </>
              )}
              {driveNotes[d.drive_number] && (
                <p className="drive-note" role="status">
                  {driveNotes[d.drive_number]}
                </p>
              )}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
