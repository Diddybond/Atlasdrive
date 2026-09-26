import type { Settings } from "../api";

/// Days without a backup before AtlasDrive says so. A fortnight: long enough
/// not to nag between ordinary scans, short enough that a backup which quietly
/// stopped working is noticed before months of names and events are at risk.
export const STALE_BACKUP_DAYS = 14;

/// What to tell the owner about the catalogue's backup, or `null` when there is
/// nothing to say.
///
/// The catalogue — which drive holds what, names, events, dates — exists only
/// on this Mac. The photographs are safe on their drives; the map of them is
/// not, until it is backed up. That warning lived in a card at the bottom of
/// Settings, and the owner's real catalogue of ~218,000 photographs had never
/// been backed up.
export function backupWarning(settings: Settings | null, now: Date = new Date()): string | null {
  if (!settings) return null;
  if (!settings.backup_destination) {
    return "Your catalogue has never been backed up. The photographs are safe on their drives, but the record of which drive holds each one — and every name and event — exists only on this Mac.";
  }
  if (!settings.last_backup_at) {
    return "A backup folder is chosen, but no backup has been made yet.";
  }
  const last = new Date(settings.last_backup_at);
  if (Number.isNaN(last.getTime())) return null;
  const days = Math.floor((now.getTime() - last.getTime()) / 86_400_000);
  if (days < STALE_BACKUP_DAYS) return null;
  return `The catalogue was last backed up ${days} days ago. Anything named or scanned since then exists only on this Mac.`;
}
