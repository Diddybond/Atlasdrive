import { useEffect, useState } from "react";
import { api, Drive, DriveContents, DriveCoverage } from "../api";

export function DrivesScreen() {
  const [coverage, setCoverage] = useState<Record<number, DriveCoverage>>({});
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
  const [rescanNote, setRescanNote] = useState<string | null>(null);

  async function rescan(drive: Drive) {
    setRescanning(drive.drive_number);
    try {
      setRescanNote(await api.rescanDrive(drive.drive_number));
    } catch (err) {
      setRescanNote(String(err));
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
  useEffect(() => {
    void load();
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
      await api.registerDrive({ number: n, path, name, writeManifest });
      setShowForm(false);
      setNumber("");
      setName("");
      setPath("");
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
            Drive location on this Mac
            <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="/Volumes/AtlasDriveA" />
          </label>
          <label className="checkbox">
            <input type="checkbox" checked={writeManifest} onChange={(e) => setWriteManifest(e.target.checked)} />
            Save a small identity file on the drive so it is recognised next time (nothing else on the
            drive is changed)
          </label>
          {error && <p className="error" role="alert">{error}</p>}
          <button type="submit">Register drive</button>
        </form>
      )}

      {rescanNote && (
        <p className="search-note" role="status">
          {rescanNote}
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
                  className={
                    coverage[d.drive_number].outstanding > 0
                      ? "coverage working"
                      : "coverage done"
                  }
                  role="status"
                >
                  {coverage[d.drive_number].outstanding > 0
                    ? `${coverage[d.drive_number].complete.toLocaleString()} of ` +
                      `${coverage[d.drive_number].discovered.toLocaleString()} indexed ` +
                      `(${Math.round(
                        (coverage[d.drive_number].complete /
                          Math.max(1, coverage[d.drive_number].discovered)) *
                          100,
                      )}%) — ${coverage[d.drive_number].outstanding.toLocaleString()} still to do. ` +
                      `Leave this drive connected.`
                    : `Finished — all ${coverage[d.drive_number].complete.toLocaleString()} ` +
                      `photographs indexed. Safe to unplug.`}
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
                    aria-label={`Check Drive ${d.drive_number} for new photographs`}
                  >
                    {rescanning === d.drive_number ? "Checking…" : "Check for new photographs"}
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
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
