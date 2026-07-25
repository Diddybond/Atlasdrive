import { useEffect, useState } from "react";
import { api, Drive } from "../api";

export function DrivesScreen() {
  const [drives, setDrives] = useState<Drive[]>([]);
  const [showForm, setShowForm] = useState(false);
  const [number, setNumber] = useState("");
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [writeManifest, setWriteManifest] = useState(true);
  const [error, setError] = useState<string | null>(null);

  async function load() {
    setDrives(await api.listDrives());
  }
  useEffect(() => {
    void load();
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
            <input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Family Archive A" />
          </label>
          <label>
            Drive location on this Mac
            <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="/Volumes/FamilyArchiveA" />
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
              <p className="drive-meta subtle">
                Last scanned: {d.last_scan_at ?? "never"}
              </p>
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
