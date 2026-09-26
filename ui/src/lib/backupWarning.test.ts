import { backupWarning, STALE_BACKUP_DAYS } from "./backupWarning";
import type { Settings } from "../api";

const base = {
  backup_include_key: true,
  backup_after_indexing: true,
} as Settings;
const now = new Date("2026-09-26T12:00:00Z");
const daysAgo = (n: number) => new Date(now.getTime() - n * 86_400_000).toISOString();

describe("backupWarning", () => {
  it("warns when no backup folder was ever chosen", () => {
    expect(backupWarning({ ...base, backup_destination: null }, now)).toMatch(/never been backed up/);
  });
  it("warns when a folder is chosen but nothing was written", () => {
    expect(backupWarning({ ...base, backup_destination: "/Volumes/B", last_backup_at: null }, now)).toMatch(
      /no backup has been made/,
    );
  });
  it("is quiet while backups are recent", () => {
    const s = { ...base, backup_destination: "/Volumes/B", last_backup_at: daysAgo(STALE_BACKUP_DAYS - 1) };
    expect(backupWarning(s, now)).toBeNull();
  });
  it("speaks up when backups have stopped", () => {
    const s = { ...base, backup_destination: "/Volumes/B", last_backup_at: daysAgo(40) };
    expect(backupWarning(s, now)).toMatch(/40 days ago/);
  });
  it("says nothing before settings have loaded", () => {
    expect(backupWarning(null, now)).toBeNull();
  });
});
