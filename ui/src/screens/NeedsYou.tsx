import { useEffect, useState } from "react";
import { api, DriveCoverage, Settings } from "../api";
import { backupWarning } from "../lib/backupWarning";

export type Place = "drives" | "settings";

interface Item {
  key: string;
  text: string;
  action: string;
  to: Place;
}

/// The few things only the owner can do, in one short list.
///
/// Only real problems belong here: the catalogue is not backed up, a drive was
/// left half-scanned, a scan stopped. Naming people and events is optional —
/// the owner names the ones that matter — so they never appear as chores.
/// When there is nothing to do, nothing is shown.
export function NeedsYou({ onGo }: { onGo: (place: Place) => void }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [coverage, setCoverage] = useState<DriveCoverage[]>([]);
  const [scanError, setScanError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);

  useEffect(() => {
    void api.getSettings().then(setSettings, () => undefined);
    void api.driveCoverage().then(setCoverage, () => undefined);
    void api.lastScanError().then(setScanError, () => undefined);
    void api.isIndexing().then(setScanning, () => undefined);
  }, []);

  const items: Item[] = [];
  const backup = backupWarning(settings);
  if (backup) {
    items.push({
      key: "backup",
      text: backup,
      action: settings?.backup_destination ? "Back up now" : "Choose a backup folder",
      to: "settings",
    });
  }
  if (scanError) {
    items.push({ key: "scan-error", text: `The last scan stopped: ${scanError}`, action: "See drives", to: "drives" });
  }
  if (!scanning) {
    for (const c of coverage.filter((c) => c.outstanding > 0)) {
      items.push({
        key: `drive-${c.drive_number}`,
        text: `Drive ${c.drive_number} is not fully scanned — ${c.outstanding.toLocaleString()} photographs still to read. Plug it in and scan it to finish.`,
        action: "Go to drives",
        to: "drives",
      });
    }
  }

  if (items.length === 0) return null;
  return (
    <section className="card needs-you" aria-labelledby="needs-you-heading">
      <h2 id="needs-you-heading">Needs you</h2>
      <ul>
        {items.map((i) => (
          <li key={i.key}>
            <span>{i.text}</span>
            <button onClick={() => onGo(i.to)}>{i.action}</button>
          </li>
        ))}
      </ul>
    </section>
  );
}
