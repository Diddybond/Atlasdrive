import { useEffect, useState } from "react";
import { api, BackupInfo, Settings, VerifierCheck } from "../api";

function mb(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 * 1024) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function when(iso?: string | null): string {
  if (!iso) return "never";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

export function SettingsScreen() {
  const [checks, setChecks] = useState<VerifierCheck[]>([]);
  const [doctor, setDoctor] = useState<Record<string, string>>({});
  const [running, setRunning] = useState(false);
  const [exported, setExported] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);

  const [settings, setSettings] = useState<Settings | null>(null);
  const [cloud, setCloud] = useState<string | null>(null);
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  const [backingUp, setBackingUp] = useState(false);
  const [backupNote, setBackupNote] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<string | null>(null);
  const [confirmRestore, setConfirmRestore] = useState<string | null>(null);
  const [compacting, setCompacting] = useState(false);
  const [compactNote, setCompactNote] = useState<string | null>(null);

  async function refreshBackups(dest?: string | null) {
    if (!dest) {
      setBackups([]);
      setCloud(null);
      return;
    }
    setCloud(await api.describeBackupDestination(dest));
    setBackups(await api.listBackups(dest));
  }

  async function persist(next: Settings) {
    setSettings(next);
    await api.saveSettings(next);
  }

  async function chooseDestination() {
    const picked = await api.chooseFolder("Choose where AtlasDrive should keep backups");
    if (!picked || !settings) return;
    await persist({ ...settings, backup_destination: picked });
    await refreshBackups(picked);
  }

  async function backupNow() {
    setBackingUp(true);
    setBackupNote(null);
    try {
      const r = await api.backupNow();
      setBackupNote(
        `Backed up ${mb(r.db_bytes)}. ${r.thumbnails_copied} new thumbnails ` +
          `(${mb(r.thumbnail_bytes_copied)}), ${r.thumbnails_present} already there.`,
      );
      const s = await api.getSettings();
      setSettings(s);
      await refreshBackups(s.backup_destination);
    } catch (e) {
      setBackupNote(`Backup failed: ${String(e)}`);
    } finally {
      setBackingUp(false);
    }
  }

  async function restore(bundle: string) {
    setRestoring(bundle);
    setConfirmRestore(null);
    try {
      const r = await api.restoreBackup(bundle);
      setBackupNote(
        `Restored ${r.counts.files} files, ${r.counts.faces} faces and ` +
          `${r.counts.people_named} named people. ` +
          (r.previous_catalogue
            ? `The catalogue this replaced was kept at ${r.previous_catalogue}.`
            : ""),
      );
    } catch (e) {
      setBackupNote(`Restore failed: ${String(e)}`);
    } finally {
      setRestoring(null);
    }
  }

  async function compact() {
    setCompacting(true);
    try {
      setCompactNote(await api.compactCatalogue());
    } catch (e) {
      setCompactNote(`Could not compact: ${String(e)}`);
    } finally {
      setCompacting(false);
    }
  }

  async function exportDiagnostics() {
    setExporting(true);
    try {
      setExported(await api.exportDiagnostics());
    } finally {
      setExporting(false);
    }
  }

  async function runChecks() {
    setRunning(true);
    try {
      setChecks(await api.runVerifier());
    } finally {
      setRunning(false);
    }
  }
  useEffect(() => {
    void (async () => {
      setDoctor(await api.doctor());
      const s = await api.getSettings();
      setSettings(s);
      await refreshBackups(s.backup_destination);
      await runChecks();
    })();
  }, []);

  const badge = (s: VerifierCheck["status"]) =>
    s === "Pass" ? "ok" : s === "Warn" ? "warn" : "fail";

  return (
    <section aria-labelledby="settings-heading">
      <h1 id="settings-heading">Settings &amp; diagnostics</h1>
      <p className="lede">
        Everything runs on this Mac. These checks confirm your archive is safe, consistent and fully
        offline.
      </p>

      <div className="card">
        <div className="row-between">
          <h2>Backup</h2>
          <button onClick={backupNow} disabled={backingUp || !settings?.backup_destination}>
            {backingUp ? "Backing up…" : "Back up now"}
          </button>
        </div>
        <p className="lede">
          Your photographs stay on your drives. This backs up the catalogue — which drive holds
          what, your tags, dates, faces and the names you have given them. That is the part that
          exists in only one place.
        </p>

        <dl className="kv">
          <div>
            <dt>Backup folder</dt>
            <dd className="path">
              <span className="path-text">
                {settings?.backup_destination ?? "not chosen yet"}
              </span>
              <button className="ghost" onClick={chooseDestination}>
                {settings?.backup_destination ? "Change…" : "Choose…"}
              </button>
            </dd>
          </div>
          <div>
            <dt>Last backup</dt>
            <dd>{when(settings?.last_backup_at)}</dd>
          </div>
        </dl>

        {settings?.backup_destination &&
          (cloud ? (
            <p className="check-detail" role="status">
              This folder is synchronised by {cloud}, so backups will leave this Mac. AtlasDrive
              itself never connects to the internet — {cloud} does the uploading.
            </p>
          ) : (
            <p className="check-detail" role="status">
              This folder is not synchronised to any cloud service, so backups stay on this Mac. If
              the Mac is lost, so is the backup. Choose a folder inside Google Drive, Dropbox or
              iCloud Drive to keep a copy elsewhere.
            </p>
          ))}

        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings?.backup_include_key ?? true}
            onChange={(e) =>
              settings && void persist({ ...settings, backup_include_key: e.target.checked })
            }
          />
          Include the encryption key in the backup
        </label>
        <p className="check-detail">
          Face data is encrypted inside the catalogue. Without the key, a backup can only be
          restored onto this Mac — so it survives a damaged catalogue but not a lost computer. With
          the key included, anyone who can read the backup folder can read the face data.
        </p>

        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings?.backup_after_indexing ?? true}
            onChange={(e) =>
              settings && void persist({ ...settings, backup_after_indexing: e.target.checked })
            }
          />
          Back up automatically after indexing a drive
        </label>

        {backupNote && (
          <p className="check-detail" role="status">
            {backupNote}
          </p>
        )}

        {backups.length > 0 && (
          <>
            <h3>Available backups</h3>
            <ul className="check-list">
              {backups.map((b) => (
                <li key={b.path} className="check-row">
                  <span className="check-name">{b.name}</span>
                  <span className="check-detail">
                    {b.manifest
                      ? `${b.manifest.counts.files} files, ${b.manifest.counts.faces} faces, ` +
                        `${b.manifest.counts.people_named} named · ${mb(b.manifest.db_bytes)}`
                      : "no manifest"}
                  </span>
                  {confirmRestore === b.path ? (
                    <span>
                      <button onClick={() => void restore(b.path)} disabled={restoring !== null}>
                        {restoring === b.path ? "Restoring…" : "Yes, replace it"}
                      </button>{" "}
                      <button className="ghost" onClick={() => setConfirmRestore(null)}>
                        Cancel
                      </button>
                    </span>
                  ) : (
                    <button className="ghost" onClick={() => setConfirmRestore(b.path)}>
                      Restore…
                    </button>
                  )}
                </li>
              ))}
            </ul>
            {confirmRestore && (
              <p className="check-detail" role="alert">
                Restoring replaces your current catalogue with this one. The catalogue being
                replaced is kept on disk, not deleted, so this can be undone.
              </p>
            )}
          </>
        )}
      </div>

      <div className="card">
        <div className="row-between">
          <h2>Reclaim disk space</h2>
          <button onClick={compact} disabled={compacting}>
            {compacting ? "Working…" : "Compact"}
          </button>
        </div>
        <p className="lede">
          Re-encodes thumbnails written by older versions and compacts the catalogue. Nothing you
          can see changes. On a large archive this can take a while — it is safe to leave running.
        </p>
        {compactNote && (
          <p className="check-detail" role="status">
            {compactNote}
          </p>
        )}
      </div>

      <div className="card">
        <h2>Environment</h2>
        <dl className="kv">
          {Object.entries(doctor).map(([k, v]) => (
            <div key={k}>
              <dt>{k.replace(/_/g, " ")}</dt>
              <dd>{v}</dd>
            </div>
          ))}
        </dl>
      </div>

      <div className="card">
        <div className="row-between">
          <h2>Safety checks</h2>
          <button onClick={runChecks} disabled={running}>
            {running ? "Checking…" : "Run checks"}
          </button>
        </div>
        <ul className="check-list">
          {checks.map((c) => (
            <li key={c.name} className="check-row">
              <span className={`pill ${badge(c.status)}`}>{c.status}</span>
              <span className="check-name">{c.name.replace(/_/g, " ")}</span>
              <span className="check-detail">{c.detail}</span>
            </li>
          ))}
        </ul>
      </div>

      <div className="card">
        <div className="row-between">
          <h2>Share a diagnostics file</h2>
          <button onClick={exportDiagnostics} disabled={exporting}>
            {exporting ? "Writing…" : "Create diagnostics file"}
          </button>
        </div>
        <p className="lede">
          Creates a file you can send with a bug report. It contains counts, version numbers and
          the results of the safety checks — never your file names, folders, dates, tags, people or
          photographs.
        </p>
        {exported && (
          <p className="check-detail" role="status">
            Saved to {exported}
          </p>
        )}
      </div>
    </section>
  );
}
