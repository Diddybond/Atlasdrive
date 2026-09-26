import { useEffect, useState } from "react";
import { api, Drive, DriveCoverage, Settings } from "../api";
import { backupWarning } from "../lib/backupWarning";

export type Place = "drives" | "settings";

/// A plugged-in drive not checked for this long is worth offering a check.
const RECHECK_AFTER_DAYS = 7;

interface Item {
  key: string;
  text: string;
  action: string;
  run: () => void;
}

/// The few things only the owner can do, in one short list.
///
/// Only real problems and ready-to-go jobs belong here: the catalogue is not
/// backed up, a scan stopped, a drive was left half-scanned — and, when a drive
/// is plugged in, the one click that deals with it. Naming people and events is
/// optional — the owner names the ones that matter — so they never appear as
/// chores. When there is nothing to do, nothing is shown.
export function NeedsYou({ onGo }: { onGo: (place: Place) => void }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [coverage, setCoverage] = useState<DriveCoverage[]>([]);
  const [drives, setDrives] = useState<Drive[]>([]);
  const [scanError, setScanError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(true);
  const [started, setStarted] = useState<string | null>(null);
  const [due, setDue] = useState<[number, number][]>([]);
  const [checking, setChecking] = useState<number | null>(null);

  useEffect(() => {
    void api.getSettings().then(setSettings, () => undefined);
    void api.driveCoverage().then(setCoverage, () => undefined);
    void api.listDrives().then(setDrives, () => undefined);
    void api.lastScanError().then(setScanError, () => undefined);
    void api.isIndexing().then(setScanning, () => undefined);
    void api.healthDue().then(setDue, () => undefined);
  }, []);

  async function check(number: number) {
    setChecking(number);
    try {
      setStarted(await api.spotCheckDrive(number));
      setDue((d) => d.filter(([n]) => n !== number));
    } catch (err) {
      setStarted(String(err));
    } finally {
      setChecking(null);
    }
  }

  async function scan(number: number) {
    try {
      setStarted(`${await api.rescanDrive(number)} Progress is under Drives → Scan activity.`);
      setScanning(true);
    } catch (err) {
      setStarted(String(err));
    }
  }

  const items: Item[] = [];
  const backup = backupWarning(settings);
  if (backup) {
    items.push({
      key: "backup",
      text: backup,
      action: settings?.backup_destination ? "Back up now" : "Choose a backup folder",
      run: () => onGo("settings"),
    });
  }
  if (scanError) {
    items.push({
      key: "scan-error",
      text: `The last scan stopped: ${scanError}`,
      action: "See drives",
      run: () => onGo("drives"),
    });
  }
  if (!scanning) {
    const now = Date.now();
    for (const d of drives) {
      const c = coverage.find((x) => x.drive_number === d.drive_number);
      const plugged = d.status === "online";
      if (c && c.outstanding > 0) {
        items.push(
          plugged
            ? {
                key: `drive-${d.drive_number}`,
                text: `Drive ${d.drive_number} is plugged in and ${c.outstanding.toLocaleString()} of its photographs are still to be read.`,
                action: "Finish scanning",
                run: () => void scan(d.drive_number),
              }
            : {
                key: `drive-${d.drive_number}`,
                text: `Drive ${d.drive_number} is not fully scanned — ${c.outstanding.toLocaleString()} photographs still to read.${d.physical_location ? ` It is kept in ${d.physical_location}.` : ""} Plug it in to finish.`,
                action: "See drives",
                run: () => onGo("drives"),
              },
        );
        continue;
      }
      const last = d.last_scan_at ? new Date(d.last_scan_at).getTime() : NaN;
      if (plugged && c && c.discovered > 0 && (Number.isNaN(last) || now - last > RECHECK_AFTER_DAYS * 86_400_000)) {
        items.push({
          key: `recheck-${d.drive_number}`,
          text: `Drive ${d.drive_number} is plugged in. Check it for photographs added since it was last scanned?`,
          action: "Check for new photographs",
          run: () => void scan(d.drive_number),
        });
      }
    }
  }

  if (!scanning) {
    for (const [number, count] of due) {
      if (items.some((i) => i.key === `drive-${number}` || i.key === `recheck-${number}`)) continue;
      items.push({
        key: `health-${number}`,
        text: `Drive ${number} is plugged in. ${count.toLocaleString()} of its photographs have not been checked for damage in six months.`,
        action: checking === number ? "Checking…" : "Check 200 now",
        run: () => void check(number),
      });
    }
  }

  if (items.length === 0 && !started) return null;
  return (
    <section className="card needs-you" aria-labelledby="needs-you-heading">
      <h2 id="needs-you-heading">Needs you</h2>
      {started && (
        <p className="search-note" role="status">
          {started}
        </p>
      )}
      <ul>
        {items.map((i) => (
          <li key={i.key}>
            <span>{i.text}</span>
            <button onClick={i.run}>{i.action}</button>
          </li>
        ))}
      </ul>
    </section>
  );
}
