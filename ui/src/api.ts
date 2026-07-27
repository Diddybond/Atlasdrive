// Typed bridge to the Rust core via Tauri commands.
//
// When running inside the Tauri shell, calls go to the real backend. In a plain
// browser (dev / tests) they fall back to deterministic mock data so the UI is
// demonstrable and testable without the native layer. The shapes mirror the
// serde types in family-archive-core.

export interface Volume {
  name: string;
  path: string;
  is_startup_disk: boolean;
  registered_as?: number | null;
  is_read_only: boolean;
}

export interface DriveCoverage {
  drive_number: number;
  drive_name?: string | null;
  discovered: number;
  complete: number;
  outstanding: number;
  failed: number;
  last_outcome?: string | null;
  last_scan_at?: string | null;
  /// The sentence to display. Computed in Rust so the rule exists once —
  /// reimplementing it here is what produced "Finished — all 0 photographs
  /// indexed. Safe to unplug." on a drive that had never been scanned.
  summary: string;
  can_unplug: boolean;
}

export interface IndexEstimate {
  files: number;
  files_per_second: number;
  hours: number;
  exceeds_one_night: boolean;
  summary: string;
}

export interface ArchiveEvent {
  id: string;
  name?: string | null;
  client?: string | null;
  earliest_date?: string | null;
  latest_date?: string | null;
  status: "proposed" | "named" | "rejected";
  photo_count: number;
}

export interface SplitPoint {
  at: string;
  gap_hours: number;
  photos_after: number;
}

export interface ProposeReport {
  proposed: number;
  photos_grouped: number;
  photos_skipped: number;
  photos_undated: number;
  photos_imprecise: number;
}

export interface Settings {
  backup_destination?: string | null;
  backup_include_key: boolean;
  backup_keep?: number | null;
  backup_after_indexing: boolean;
  last_backup_at?: string | null;
}

export interface BackupReport {
  bundle: string;
  db_bytes: number;
  thumbnails_copied: number;
  thumbnails_present: number;
  thumbnail_bytes_copied: number;
  key_included: boolean;
  pruned: number;
}

export interface BackupManifest {
  created_at: string;
  app_version: string;
  db_bytes: number;
  key_included: boolean;
  counts: {
    drives: number;
    files: number;
    faces: number;
    people_named: number;
    thumbnails: number;
  };
}

export interface BackupInfo {
  path: string;
  name: string;
  manifest?: BackupManifest | null;
}

export interface RestoreReport {
  restored_from: string;
  previous_catalogue?: string | null;
  counts: BackupManifest["counts"];
  thumbnails_restored: number;
  key_restored: boolean;
}

export interface Drive {
  id: string;
  drive_number: number;
  friendly_name?: string | null;
  status: "online" | "offline" | "changed" | "conflict" | "retired";
  physical_location?: string | null;
  categories?: string[];
  last_scan_at?: string | null;
  image_count?: number;
  /// Worth saying, but not a failure — an identity file that could not be
  /// written to a read-only drive, for instance.
  note?: string | null;
}

export interface SearchResult {
  file_id: string;
  filename: string;
  relative_path: string;
  drive_number: number;
  drive_name?: string | null;
  online: boolean;
  thumbnail_rel_path?: string | null;
  date_label?: string | null;
  matched: string[];
  score: number;
}

export interface DriveMatch {
  drive_number: number;
  drive_name?: string | null;
  online: boolean;
  physical_location?: string | null;
  match_count: number;
  examples: string[];
}

export interface TagCount {
  tag: string;
  count: number;
}

/// What is stored on a drive — readable with the drive disconnected.
export interface DriveContents {
  drive_number: number;
  drive_name?: string | null;
  status: string;
  online: boolean;
  physical_location?: string | null;
  categories: string[];
  last_scan_at?: string | null;
  photo_count: number;
  missing_count: number;
  earliest_date?: string | null;
  latest_date?: string | null;
  top_tags: TagCount[];
  with_text_count: number;
  people_count: number;
}

export interface SearchResponse {
  results: SearchResult[];
  /// Terms the local text encoder recognised in the query.
  understood: string[];
  /// True when the query had no visual meaning and only text was searched.
  text_only: boolean;
  /// Which drives hold the matches, most matches first.
  drives: DriveMatch[];
  /// One line answering "which drive do I need to connect?".
  where_to_look: string;
}

export interface Progress {
  runId: string;
  driveNumber: number;
  scanRoot?: string;
  startedAt: string;
  updatedAt?: string;
  filesDiscovered: number;
  filesDone: number;
  filesFailed: number;
  filesQueued: number;
  currentBatch: number;
  /// The photograph most recently finished. Shown so a long run visibly moves
  /// even between whole-number changes in the counter.
  lastCompletedFile?: string | null;
  status: string;
}

export interface RecentFile {
  file_id: string;
  filename: string;
  relative_path: string;
  size_bytes: number;
  faces: number;
  top_tag?: string | null;
}

export interface ScanStats {
  drive_number: number;
  files: number;
  bytes: number;
  faces: number;
  tags: number;
  people_recognised: number;
  by_extension: [string, number][];
  recent: RecentFile[];
}

export interface VerifierCheck {
  name: string;
  status: "Pass" | "Warn" | "Fail" | "Halt";
  detail: string;
}

export interface ClusterSummary {
  cluster_id: string;
  status: string;
  face_count: number;
  label?: string | null;
  person_id?: string | null;
}

/// One face in the gallery — a picture first, a name only if you gave it one.
export interface GalleryFace {
  face_id: string;
  cluster_id?: string | null;
  file_id: string;
  quality?: number | null;
  person_name?: string | null;
  cluster_status?: string | null;
  group_size: number;
  drive_number: number;
  drive_name?: string | null;
}

export interface TagCount {
  tag: string;
  count: number;
}

export interface SuggestedFace {
  face_id: string;
  cluster_id: string;
  /// Similarity to that person's closest confirmed face, 0–1.
  score: number;
  group_size: number;
}

export interface PersonFolder {
  drive_number: number;
  drive_name?: string | null;
  online: boolean;
  relative_folder: string;
  absolute_path?: string | null;
  photo_count: number;
}

export interface PersonPhoto {
  file_id: string;
  filename: string;
  relative_path: string;
  drive_number: number;
  drive_name?: string | null;
  online: boolean;
}

export interface ExportSummary {
  copied: number;
  skipped_existing: number;
  skipped_offline: number;
  missing: number;
  drives_to_connect: number[];
  destination: string;
}

export interface SidecarSummary {
  written: number;
  skipped_offline: number;
  skipped_nothing_to_say: number;
  paths: string[];
}

/// Someone you have named. Confirmed faces are what future scans match against.
export interface NameScan {
  examined: number;
  tagged: number;
  names: TagCount[];
}

export interface FailureReason {
  code: string;
  message: string;
  files: number;
  example: string | null;
}

export interface NamedPerson {
  id: string;
  display_name: string;
  relationship?: string | null;
  confirmed_faces: number;
  suggested_faces: number;
}

/// Set by tests to simulate a background run that died. Nothing in the app
/// writes it; the real value comes from the backend.
/// Whether the mock backend has a scan in flight.
///
/// Defaults to true because the scan console needs one to exercise. Tests about
/// what the Drives screen offers when the machine is idle turn it off, rather
/// than each quietly assuming the opposite of the other.
export let mockScanRunning = true;
export function setMockScanning(v: boolean) {
  mockScanRunning = v;
}

export let mockStopping = false;

export let mockScanError: string | null = null;
export function setMockStopping(v: boolean) {
  mockStopping = v;
}
export function setMockScanError(v: string | null) {
  mockScanError = v;
}

// Detect the Tauri runtime.
function hasTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (hasTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  return mock<T>(cmd, args);
}

// ---- public API ---------------------------------------------------------

export const api = {
  listDrives: () => call<Drive[]>("list_drives"),
  registerDrive: (input: { number: number; path: string; name?: string; writeManifest: boolean }) =>
    call<Drive>("register_drive", input),
  search: (
    query: string,
    opts: {
      drive?: number;
      includeOffline: boolean;
      eventId?: string;
      client?: string;
      tags?: string[];
    },
  ) => call<SearchResponse>("search_catalogue", { query, ...opts }),
  // Starts a background run and returns immediately; poll getProgress().
  startIndex: (input: { drive: number; path: string; dryRun: boolean; resume: boolean }) =>
    call<void>("start_index", input),
  cancelIndex: () => call<void>("cancel_index"),
  isIndexing: () => call<boolean>("is_indexing"),
  getProgress: () => call<Progress | null>("get_progress"),
  lastScanError: () => call<string | null>("last_scan_error"),
  stopScan: () => call<string>("stop_scan"),
  stopPending: () => call<boolean>("stop_pending"),
  scanFailures: (driveNumber: number) =>
    call<FailureReason[]>("scan_failures", { driveNumber }),
  retryFailedFiles: (driveNumber: number, code?: string) =>
    call<number>("retry_failed_files", { driveNumber, code }),
  scanStats: (driveNumber: number, recent?: number) =>
    call<ScanStats>("scan_stats", { driveNumber, recent }),
  runVerifier: () => call<VerifierCheck[]>("run_verifier"),
  prepareReview: (limit: number) => call<ClusterSummary[]>("prepare_review", { limit }),
  doctor: () => call<Record<string, string>>("doctor"),
  driveCoverage: () => call<DriveCoverage[]>("drive_coverage"),
  connectedVolumes: () => call<Volume[]>("connected_volumes"),
  likelyPhotoFolders: (path: string) => call<string[]>("likely_photo_folders", { path }),
  estimateIndex: (path: string) => call<IndexEstimate>("estimate_index", { path }),
  similarPhotographs: (fileId: string, limit?: number) =>
    call<SearchResult[]>("similar_photographs", { fileId, limit }),
  proposeEvents: (gapHours?: number) => call<ProposeReport>("propose_events", { gapHours }),
  listEvents: (status?: string) => call<ArchiveEvent[]>("list_events", { status }),
  nextEventProposal: () => call<ArchiveEvent | null>("next_event_proposal"),
  nameEvent: (eventId: string, name: string, client?: string) =>
    call<void>("name_event", { eventId, name, client }),
  forgetEvent: (eventId: string) => call<void>("forget_event", { eventId }),
  eventClients: () => call<[string, number][]>("event_clients"),
  eventFiles: (eventId: string) => call<string[]>("event_files", { eventId }),
  eventSplitPoints: (eventId: string, limit?: number) =>
    call<SplitPoint[]>("event_split_points", { eventId, limit }),
  splitEvent: (eventId: string, at: string) => call<string>("split_event", { eventId, at }),
  mergeEvents: (into: string, from: string) => call<number>("merge_events", { into, from }),
  chooseFolder: (prompt?: string) => call<string | null>("choose_folder", { prompt }),
  getSettings: () => call<Settings>("get_settings"),
  saveSettings: (settings: Settings) => call<void>("save_settings", { settings }),
  describeBackupDestination: (path: string) =>
    call<string | null>("describe_backup_destination", { path }),
  backupNow: (destination?: string) => call<BackupReport>("backup_now", { destination }),
  listBackups: (destination?: string) => call<BackupInfo[]>("list_backups", { destination }),
  restoreBackup: (bundle: string) => call<RestoreReport>("restore_backup", { bundle }),
  compactCatalogue: () => call<string>("compact_catalogue"),
  exportDiagnostics: () => call<string>("export_diagnostics"),
  tagFaceCluster: (clusterId: string, name: string) =>
    call<{ id: string; display_name: string }>("tag_face_cluster", { clusterId, name }),
  listPeople: () => call<NamedPerson[]>("list_people"),
  setPersonRelationship: (personId: string, relationship?: string) =>
    call<void>("set_person_relationship", { personId, relationship }),
  personRelationships: () => call<[string, number][]>("person_relationships"),
  faceGallery: (limit?: number, driveNumber?: number) =>
    call<GalleryFace[]>("face_gallery", { limit, driveNumber }),
  faceThumbnail: (faceId: string) => call<string | null>("face_thumbnail", { faceId }),
  tagFace: (faceId: string, name: string) =>
    call<{ person: { id: string; display_name: string }; suggested: number }>("tag_face", {
      faceId,
      name,
    }),
  photosOfPerson: (personId: string) => call<PersonPhoto[]>("photos_of_person", { personId }),
  findNames: (driveNumber?: number) => call<NameScan>("find_names", { driveNumber }),
  catalogueTags: (limit?: number, driveNumber?: number) =>
    call<TagCount[]>("catalogue_tags", { limit, driveNumber }),
  photoThumbnail: (fileId: string, maxEdge?: number) =>
    call<string | null>("photo_thumbnail", { fileId, maxEdge }),
  pendingSuggestions: (personId: string, limit?: number) =>
    call<SuggestedFace[]>("pending_suggestions", { personId, limit }),
  confirmSuggestions: (personId: string) => call<number>("confirm_suggestions", { personId }),
  rejectSuggestions: (personId: string) => call<number>("reject_suggestions", { personId }),
  resolveSuggestion: (clusterId: string, isThem: boolean) =>
    call<void>("resolve_suggestion", { clusterId, isThem }),
  personFolders: (personId: string) => call<PersonFolder[]>("person_folders", { personId }),
  openFolder: (path: string) => call<void>("open_folder", { path }),
  forgetPerson: (personId: string) => call<void>("forget_person", { personId }),
  renamePerson: (personId: string, name: string) =>
    call<{ id: string; display_name: string }>("rename_person", { personId, name }),
  rescanDrive: (driveNumber: number) => call<string>("rescan_drive", { driveNumber }),
  copyPersonPhotos: (personId: string, destination: string) =>
    call<ExportSummary>("copy_person_photos", { personId, destination }),
  writeSidecarsForPerson: (personId: string) =>
    call<SidecarSummary>("write_sidecars_for_person", { personId }),
  rejectFaceCluster: (clusterId: string) => call<void>("reject_face_cluster", { clusterId }),
  renameDrive: (driveNumber: number, name: string) =>
    call<void>("rename_drive", { driveNumber, name }),
  driveContents: (driveNumber?: number) =>
    call<DriveContents[]>("drive_contents", { driveNumber }),
  updateDriveDetails: (input: {
    driveNumber: number;
    physicalLocation?: string;
    categories?: string[];
  }) => call<Drive>("update_drive_details", input),
  // Returns a plain-language message: either "Showing … in Finder." or
  // "Connect Drive N to open the original."
  revealInFinder: (fileId: string) => call<string>("reveal_in_finder", { fileId }),
  // Returns the phrasing to display, e.g. "Taken on 1998-08-12".
  setDateOverride: (input: { fileId: string; earliest: string; latest?: string }) =>
    call<string>("set_date_override", input),
  clearDateOverride: (fileId: string) => call<void>("clear_date_override", { fileId }),
};

// ---- browser mock -------------------------------------------------------

const mockDrives: Drive[] = [
  { id: "d-14", drive_number: 14, friendly_name: "AtlasDrive A", status: "online", physical_location: "Studio shelf B", categories: ["family", "holidays"], last_scan_at: "2026-07-24", image_count: 4213 },
  { id: "d-07", drive_number: 7, friendly_name: "Holidays 2004-2011", status: "offline", physical_location: "Drawer 2", categories: ["holidays"], last_scan_at: "2026-06-30", image_count: 8891 },
  { id: "d-22", drive_number: 22, friendly_name: "Scanned prints", status: "offline", physical_location: "Box A", categories: ["scanned prints"], last_scan_at: "2026-05-12", image_count: 1502 },
];

const mockResults: SearchResult[] = [
  { file_id: "f1", filename: "beach_1998.jpg", relative_path: "holiday/beach_1998.jpg", drive_number: 14, drive_name: "AtlasDrive A", online: true, date_label: "Taken on 12 Aug 1998", matched: ["text", "visual"], score: 0.91 },
  { file_id: "f2", filename: "portrait.jpg", relative_path: "family/portrait.jpg", drive_number: 7, drive_name: "Holidays 2004-2011", online: false, date_label: "Likely taken between 1985 and 1989", matched: ["visual"], score: 0.82 },
  { file_id: "f3", filename: "old_scan.jpg", relative_path: "scans/old_scan.jpg", drive_number: 22, drive_name: "Scanned prints", online: false, date_label: "Scanned in 2022, original date unknown", matched: ["visual"], score: 0.66 },
];

function mockFaceImage(seed: number): string {
  const hue = (seed * 57) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="80" height="80"><rect width="80" height="80" fill="hsl(${hue},45%,70%)"/><circle cx="40" cy="34" r="18" fill="hsl(${hue},40%,85%)"/></svg>`;
  return `data:image/svg+xml;base64,${btoa(svg)}`;
}

const mockGallery: GalleryFace[] = [
  { face_id: "fa-1", cluster_id: "c-a1b2c3", file_id: "f1", quality: 0.9, group_size: 34, drive_number: 14, drive_name: "AtlasDrive A" },
  { face_id: "fa-2", cluster_id: "c-d4e5f6", file_id: "f2", quality: 0.8, group_size: 12, drive_number: 7, drive_name: "Holidays 2004-2011" },
  { face_id: "fa-3", cluster_id: null, file_id: "f3", quality: 0.7, group_size: 1, drive_number: 14, drive_name: "AtlasDrive A" },
];

const mockClusters: ClusterSummary[] = [
  { cluster_id: "c-a1b2c3", status: "unnamed", face_count: 34 },
  { cluster_id: "c-d4e5f6", status: "unnamed", face_count: 12 },
];
const mockPeople: NamedPerson[] = [];

let mockSettings: Settings = {
  backup_destination: null,
  backup_include_key: true,
  backup_keep: 7,
  backup_after_indexing: true,
  last_backup_at: null,
};
const mockBackups: BackupInfo[] = [];
// Real bundle names are unique — backup::free_bundle_path guarantees it even
// for two backups in the same second. The mock must not be sloppier than the
// thing it stands in for, or the UI gets exercised against duplicate keys it
// would never see.
let mockBackupSeq = 0;

// A scan that started a little while ago and is still going, at roughly the
// speed measured on a real NTFS drive.
// Varied rather than constant: photographs differ in size and in how many
// faces they contain, so real throughput wanders. A perfectly flat mock would
// have hidden the chart drawing a solid block instead of a line.
//
// The variation is applied to the *speed*, integrated over elapsed time, so the
// running total only ever increases. An earlier version multiplied the total by
// the jitter, which let the count go backwards — something real progress never
// does, and which correctly caused the rate to refuse to quote a figure.
const MOCK_FILES_PER_SEC = 0.22;
function mockDone(): number {
  const elapsed = (Date.now() - mockRunStarted) / 1000;
  // Integral of 0.22 * (1 + 0.28 sin(t/9)) dt, which is monotonic in t.
  const wobble = 0.28 * 9 * (1 - Math.cos(elapsed / 9));
  return Math.min(8333, Math.floor(MOCK_FILES_PER_SEC * (elapsed + wobble)));
}
const mockRunStarted = Date.now() - 20 * 60 * 1000;

let mockEvents: ArchiveEvent[] = [];

function mock<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  switch (cmd) {
    case "list_drives":
      return Promise.resolve(mockDrives as unknown as T);
    case "register_drive": {
      const n = Number(args?.number ?? 0);
      const d: Drive = { id: `d-${n}`, drive_number: n, friendly_name: String(args?.name ?? ""), status: "online", image_count: 0 };
      return Promise.resolve(d as unknown as T);
    }
    case "search_catalogue": {
      // Honour the event/client scope, or a test asserting that scoping works
      // would pass against a mock that ignores it.
      if (args?.eventId || args?.client) {
        return Promise.resolve({
          results: mockResults.slice(0, 1),
          understood: ["scoped"],
          text_only: true,
          drives: [{ drive_number: 14, drive_name: "AtlasDrive A", online: true, count: 1 }],
          where_to_look: "Found on Drive 14.",
        } as unknown as T);
      }
      const q = String(args?.query ?? "").toLowerCase();
      const includeOffline = Boolean(args?.includeOffline);
      let results = mockResults.filter((r) => q === "" || r.filename.toLowerCase().includes(q) || r.matched.join(" ").includes(q) || r.relative_path.toLowerCase().includes(q));
      if (!includeOffline) results = results.filter((r) => r.online);
      if (args?.drive) results = results.filter((r) => r.drive_number === args.drive);
      // The real backend embeds the query locally; the mock just echoes any
      // term it happens to know so the UI's explanation line is exercised.
      const understood = ["beach", "portrait", "old"].filter((t) => q.includes(t));
      const byDrive = new Map<number, DriveMatch>();
      for (const r of results) {
        const e = byDrive.get(r.drive_number) ?? {
          drive_number: r.drive_number,
          drive_name: r.drive_name ?? null,
          online: r.online,
          physical_location:
            mockDrives.find((d) => d.drive_number === r.drive_number)?.physical_location ?? null,
          match_count: 0,
          examples: [],
        };
        e.match_count += 1;
        if (e.examples.length < 3) e.examples.push(r.filename);
        byDrive.set(r.drive_number, e);
      }
      const drives = [...byDrive.values()].sort((a, b) => b.match_count - a.match_count);
      const numbers = drives.map((d) => d.drive_number).sort((a, b) => a - b);
      const list =
        numbers.length === 1
          ? `Drive ${numbers[0]}`
          : `Drives ${numbers.slice(0, -1).join(", ")} and ${numbers[numbers.length - 1]}`;
      const offline = drives.filter((d) => !d.online);
      let where = drives.length === 0 ? "Not found on any indexed drive." : `Found on ${list}.`;
      if (drives.length > 1) where += ` Drive ${drives[0].drive_number} has the most (${drives[0].match_count}).`;
      if (offline.length > 0) {
        where += ` Connect ${offline
          .map((d) => (d.physical_location ? `Drive ${d.drive_number} (${d.physical_location})` : `Drive ${d.drive_number}`))
          .join(", ")} to open the originals.`;
      }
      const response: SearchResponse = { results, understood, text_only: understood.length === 0, drives, where_to_look: where };
      return Promise.resolve(response as unknown as T);
    }
    case "start_index":
      return Promise.resolve(undefined as unknown as T);
    case "cancel_index":
      return Promise.resolve(undefined as unknown as T);
    case "is_indexing":
      return Promise.resolve(mockScanRunning as unknown as T);
    case "get_progress": {
      // A run that actually advances, so the rate and finish-time working is
      // exercised rather than assumed. A frozen mock would have let a broken
      // estimate look perfectly healthy.
      const done = mockDone();
      return Promise.resolve({
        runId: "mock-run",
        // Must be a drive that is actually in `mockDrives`: a run on a drive
        // the list has never heard of is not a state the app can ever be in,
        // and a fixture like that hides every bug about the two agreeing.
        driveNumber: 14,
        scanRoot: "/Volumes/AtlasDrive A",
        startedAt: new Date(mockRunStarted).toISOString(),
        updatedAt: new Date().toISOString(),
        filesDiscovered: 8333,
        filesDone: done,
        filesFailed: 0,
        filesQueued: 8333 - done,
        currentBatch: Math.floor(done / 64) + 1,
        lastCompletedFile: `Weddings/2019/IMG_${String(4000 + done).padStart(4, "0")}.jpg`,
        status: !mockScanRunning || done >= 8333 ? "complete" : "running",
      } as unknown as T);
    }
    case "scan_stats": {
      // Derived from the same simulated clock as get_progress, so the feed and
      // the counter cannot tell different stories.
      const done = mockDone();
      const subjects = ["bride", "confetti", "speech", "cake", "dance floor", "church", "bouquet"];
      return Promise.resolve({
        drive_number: 2,
        files: done,
        bytes: done * 14_500_000,
        faces: Math.floor(done * 1.6),
        tags: Math.floor(done * 9.1),
        people_recognised: Math.min(4, Math.floor(done / 400)),
        by_extension: [
          ["jpg", Math.floor(done * 0.94)],
          ["psd", Math.floor(done * 0.04)],
          ["png", Math.floor(done * 0.02)],
        ].filter(([, n]) => (n as number) > 0) as [string, number][],
        recent: Array.from({ length: Math.min(12, done) }, (_, i) => ({
          file_id: `f${done - i}`,
          filename: `IMG_${String(4000 + done - i).padStart(4, "0")}.jpg`,
          relative_path: `Weddings/2019/IMG_${String(4000 + done - i).padStart(4, "0")}.jpg`,
          size_bytes: 12_000_000 + ((done - i) % 7) * 900_000,
          faces: (done - i) % 5,
          top_tag: subjects[(done - i) % subjects.length],
        })),
      } as unknown as T);
    }
    case "run_verifier":
      return Promise.resolve([
        { name: "db_integrity", status: "Pass", detail: "integrity_check and foreign_key_check ok" },
        { name: "originals_unchanged", status: "Pass", detail: "verified 4213 originals unchanged" },
        { name: "network_isolation", status: "Pass", detail: "no network access during indexing" },
        { name: "thumbnail_files", status: "Pass", detail: "4213 thumbnails decode and match" },
        { name: "face_pipeline", status: "Pass", detail: "812 embeddings, dim 32, finite" },
      ] as unknown as T);
    case "prepare_review":
      return Promise.resolve(mockClusters.filter((c) => c.status === "unnamed") as unknown as T);
    case "tag_face_cluster": {
      const id = String(args?.clusterId);
      const name = String(args?.name ?? "").trim();
      const cluster = mockClusters.find((c) => c.cluster_id === id);
      if (cluster) cluster.status = "confirmed";
      let person = mockPeople.find((p) => p.display_name.toLowerCase() === name.toLowerCase());
      if (!person) {
        person = { id: `p-${mockPeople.length + 1}`, display_name: name, confirmed_faces: 0, suggested_faces: 0 };
        mockPeople.push(person);
      }
      person.confirmed_faces += cluster ? cluster.face_count : 0;
      return Promise.resolve(person as unknown as T);
    }
    case "face_gallery": {
      const only = args?.driveNumber as number | undefined;
      return Promise.resolve(
        (only ? mockGallery.filter((f) => f.drive_number === only) : mockGallery) as unknown as T,
      );
    }
    case "face_thumbnail": {
      const idx = mockGallery.findIndex((f) => f.face_id === args?.faceId);
      return Promise.resolve((idx >= 0 ? mockFaceImage(idx + 1) : null) as unknown as T);
    }
    case "tag_face": {
      const face = mockGallery.find((f) => f.face_id === args?.faceId);
      const name = String(args?.name ?? "").trim();
      if (face) face.person_name = name;
      let person = mockPeople.find((p) => p.display_name.toLowerCase() === name.toLowerCase());
      if (!person) {
        person = { id: `p-${mockPeople.length + 1}`, display_name: name, confirmed_faces: 0, suggested_faces: 2 };
        mockPeople.push(person);
      }
      person.confirmed_faces += face ? face.group_size : 1;
      return Promise.resolve({ person, suggested: 2 } as unknown as T);
    }
    case "catalogue_tags":
      return Promise.resolve([
        { tag: "people", count: 708 },
        { tag: "adult", count: 694 },
        { tag: "clothing", count: 588 },
        { tag: "suit", count: 411 },
        { tag: "outdoor", count: 283 },
        { tag: "wedding", count: 131 },
      ] as unknown as T);
    case "photo_thumbnail": {
      const seed = String(args?.fileId ?? "").length * 47;
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="120" height="90"><rect width="120" height="90" fill="hsl(${seed % 360},40%,70%)"/></svg>`;
      return Promise.resolve(`data:image/svg+xml;base64,${btoa(svg)}` as unknown as T);
    }
    case "pending_suggestions":
      return Promise.resolve([
        { face_id: "fa-2", cluster_id: "c-d4e5f6", score: 0.94, group_size: 12 },
        { face_id: "fa-3", cluster_id: "c-x1", score: 0.89, group_size: 3 },
      ] as unknown as T);
    case "confirm_suggestions":
    case "reject_suggestions":
      return Promise.resolve(0 as unknown as T);
    case "resolve_suggestion":
      return Promise.resolve(undefined as unknown as T);
    case "person_folders":
      return Promise.resolve([
        { drive_number: 14, drive_name: "AtlasDrive A", online: true, relative_folder: "Aimee and Kent/edits", absolute_path: "/Volumes/AtlasDriveA/Aimee and Kent/edits", photo_count: 34 },
        { drive_number: 7, drive_name: "Holidays", online: false, relative_folder: "family", absolute_path: null, photo_count: 8 },
      ] as unknown as T);
    case "open_folder":
      return Promise.resolve(undefined as unknown as T);
    case "forget_person": {
      const i = mockPeople.findIndex((p) => p.id === args?.personId);
      if (i >= 0) mockPeople.splice(i, 1);
      return Promise.resolve(undefined as unknown as T);
    }
    case "rename_person": {
      const p = mockPeople.find((x) => x.id === args?.personId);
      if (p) p.display_name = String(args?.name ?? "");
      return Promise.resolve(p as unknown as T);
    }
    case "rescan_drive":
      return Promise.resolve(`Looking for new photographs on Drive ${args?.driveNumber}.` as unknown as T);
    case "photos_of_person":
      return Promise.resolve([
        { file_id: "f1", filename: "beach_1998.jpg", relative_path: "holiday/beach_1998.jpg", drive_number: 14, drive_name: "AtlasDrive A", online: true },
        { file_id: "f2", filename: "portrait.jpg", relative_path: "family/portrait.jpg", drive_number: 7, drive_name: "Holidays", online: false },
      ] as unknown as T);
    case "copy_person_photos":
      return Promise.resolve({
        copied: 1, skipped_existing: 0, skipped_offline: 1, missing: 0,
        drives_to_connect: [7], destination: String(args?.destination ?? "~/Desktop"),
      } as unknown as T);
    case "write_sidecars_for_person":
      return Promise.resolve({ written: 1, skipped_offline: 1, skipped_nothing_to_say: 0, paths: [] } as unknown as T);
    case "set_person_relationship": {
      const p = mockPeople.find((x) => x.id === args?.personId);
      if (p) p.relationship = (args?.relationship as string) || null;
      return Promise.resolve(undefined as unknown as T);
    }
    case "person_relationships": {
      const counts = new Map<string, number>();
      for (const p of mockPeople) if (p.relationship) counts.set(p.relationship, (counts.get(p.relationship) ?? 0) + 1);
      return Promise.resolve([...counts.entries()] as unknown as T);
    }
    case "find_names":
      return Promise.resolve({
        examined: 695,
        tagged: 11,
        names: [{ tag: "asda", count: 2 }],
      } as unknown as T);
    case "stop_scan":
      mockStopping = true;
      return Promise.resolve("Stopping after the current batch." as unknown as T);
    case "stop_pending":
      return Promise.resolve(mockStopping as unknown as T);
    case "last_scan_error":
      return Promise.resolve(mockScanError as unknown as T);
    case "scan_failures":
      return Promise.resolve([
        {
          code: "PROCESS",
          message: "decode failed: Memory limit exceeded",
          files: 232,
          example: "2011/Wedding/_MG_4471.tif",
        },
      ] as unknown as T);
    case "retry_failed_files":
      return Promise.resolve(232 as unknown as T);
    case "list_people":
      return Promise.resolve(mockPeople as unknown as T);
    case "reject_face_cluster": {
      const c = mockClusters.find((x) => x.cluster_id === String(args?.clusterId));
      if (c) c.status = "rejected";
      return Promise.resolve(undefined as unknown as T);
    }
    case "rename_drive": {
      const d = mockDrives.find((x) => x.drive_number === args?.driveNumber);
      if (d) d.friendly_name = String(args?.name ?? "");
      return Promise.resolve(undefined as unknown as T);
    }
    case "doctor":
      return Promise.resolve({ keystore: "file-fallback-dev", archive_integrity: "ok", ai_offline: "true" } as unknown as T);
    case "connected_volumes":
      return Promise.resolve([
        { name: "Late 25 A", path: "/Volumes/Late 25 A", is_startup_disk: false, registered_as: 1, is_read_only: false },
        { name: "Late 25 B", path: "/Volumes/Late 25 B", is_startup_disk: false, registered_as: null, is_read_only: false },
        { name: "New Volume", path: "/Volumes/New Volume", is_startup_disk: false, registered_as: null, is_read_only: true },
        { name: "Macintosh HD", path: "/Volumes/Macintosh HD", is_startup_disk: true, registered_as: null, is_read_only: false },
      ] as unknown as T);
    case "likely_photo_folders":
      return Promise.resolve(
        (String(args?.path ?? "").includes("Late 25 B")
          ? ["/Volumes/Late 25 B/Photos", "/Volumes/Late 25 B/Weddings"]
          : []) as unknown as T,
      );
    case "drive_coverage":
      return Promise.resolve([
        { drive_number: 7, drive_name: "Holidays 2004-2011", discovered: 15000, complete: 11000, outstanding: 4000, failed: 0, last_outcome: "cancelled", last_scan_at: "2026-07-20", can_unplug: false, summary: "11,000 of 15,000 indexed (73%). 4,000 still to do — leave this drive connected." },
        { drive_number: 14, drive_name: "AtlasDrive A", discovered: 4213, complete: 4213, outstanding: 0, failed: 0, last_outcome: "ok", last_scan_at: "2026-07-24", can_unplug: true, summary: "Finished — all 4,213 photographs indexed. Safe to unplug." },
        { drive_number: 22, drive_name: "Scanned prints", discovered: 0, complete: 0, outstanding: 0, failed: 0, last_outcome: null, last_scan_at: null, can_unplug: false, summary: "Never indexed — press Scan this drive to start." },
      ] as unknown as T);
    case "estimate_index": {
      const files = 14320;
      const hours = files / 0.3 / 3600;
      return Promise.resolve({
        files, files_per_second: 0.3, hours, exceeds_one_night: hours > 10,
        summary: `${files} photographs, about ${hours.toFixed(0)} hours — more than one night. Leave the drive connected until it finishes.`,
      } as unknown as T);
    }
    case "similar_photographs":
      return Promise.resolve(
        mockResults.filter((r) => r.file_id !== args?.fileId) as unknown as T,
      );
    case "propose_events": {
      mockEvents = [
        { id: "ev-wedding", name: null, client: null, earliest_date: "2026-05-30T13:02:00", latest_date: "2026-05-31T01:30:00", status: "proposed", photo_count: 758 },
        { id: "ev-crown", name: null, client: null, earliest_date: "2026-03-14T09:15:00", latest_date: "2026-03-14T16:40:00", status: "proposed", photo_count: 212 },
      ];
      return Promise.resolve({ proposed: 2, photos_grouped: 970, photos_skipped: 6, photos_undated: 3, photos_imprecise: 41 } as unknown as T);
    }
    case "list_events": {
      const want = args?.status as string | undefined;
      return Promise.resolve((want ? mockEvents.filter((e) => e.status === want) : mockEvents) as unknown as T);
    }
    case "next_event_proposal":
      return Promise.resolve((mockEvents.find((e) => e.status === "proposed") ?? null) as unknown as T);
    case "name_event": {
      const target = mockEvents.find((e) => e.id === args?.eventId);
      if (target) {
        target.name = String(args?.name ?? "");
        target.client = (args?.client as string) || null;
        target.status = "named";
      }
      return Promise.resolve(undefined as unknown as T);
    }
    case "forget_event": {
      mockEvents = mockEvents.filter((e) => e.id !== args?.eventId);
      return Promise.resolve(undefined as unknown as T);
    }
    case "event_clients": {
      const counts = new Map<string, number>();
      for (const e of mockEvents) if (e.client) counts.set(e.client, (counts.get(e.client) ?? 0) + 1);
      return Promise.resolve([...counts.entries()] as unknown as T);
    }
    case "event_split_points":
      return Promise.resolve(
        (args?.eventId === "ev-wedding"
          ? [{ at: "2026-05-30T19:40:00", gap_hours: 3.5, photos_after: 210 }]
          : []) as unknown as T,
      );
    case "split_event": {
      const src = mockEvents.find((e) => e.id === args?.eventId);
      if (src) {
        src.photo_count -= 210;
        mockEvents.push({
          id: `${src.id}-split`, name: null, client: null,
          earliest_date: String(args?.at), latest_date: src.latest_date,
          status: "proposed", photo_count: 210,
        });
      }
      return Promise.resolve("new-event" as unknown as T);
    }
    case "merge_events": {
      const into = mockEvents.find((e) => e.id === args?.into);
      const from = mockEvents.find((e) => e.id === args?.from);
      if (into && from) into.photo_count += from.photo_count;
      mockEvents = mockEvents.filter((e) => e.id !== args?.from);
      return Promise.resolve((from?.photo_count ?? 0) as unknown as T);
    }
    case "event_files":
      return Promise.resolve(["f1", "f2", "f3"] as unknown as T);
    case "choose_folder":
      return Promise.resolve("/Users/you/Library/CloudStorage/GoogleDrive-you@example.com/My Drive/AtlasDrive" as unknown as T);
    case "get_settings":
      return Promise.resolve(mockSettings as unknown as T);
    case "save_settings": {
      mockSettings = { ...(args?.settings as Settings) };
      return Promise.resolve(undefined as unknown as T);
    }
    case "describe_backup_destination": {
      const p = String(args?.path ?? "");
      const which = p.includes("GoogleDrive") || p.includes("Google Drive")
        ? "Google Drive"
        : p.includes("Dropbox")
          ? "Dropbox"
          : null;
      return Promise.resolve(which as unknown as T);
    }
    case "backup_now": {
      mockBackupSeq += 1;
      const name =
        new Date().toISOString().replace(/[:.]/g, "").slice(0, 15) + "Z" +
        (mockBackupSeq > 1 ? `-${mockBackupSeq}` : "");
      mockBackups.unshift({
        path: `${mockSettings.backup_destination ?? "/tmp"}/catalogue/${name}`,
        name,
        manifest: {
          created_at: new Date().toISOString(),
          app_version: "0.1.0",
          db_bytes: 36_000_000,
          key_included: mockSettings.backup_include_key,
          counts: { drives: 3, files: 14606, faces: 2126, people_named: 2, thumbnails: 14606 },
        },
      });
      mockSettings.last_backup_at = new Date().toISOString();
      return Promise.resolve({
        bundle: mockBackups[0].path,
        db_bytes: 36_000_000,
        thumbnails_copied: 128,
        thumbnails_present: 14478,
        thumbnail_bytes_copied: 5_600_000,
        key_included: mockSettings.backup_include_key,
        pruned: 0,
      } as unknown as T);
    }
    case "list_backups":
      return Promise.resolve(mockBackups as unknown as T);
    case "restore_backup":
      return Promise.resolve({
        restored_from: String(args?.bundle ?? ""),
        previous_catalogue: "/data/archive.db.replaced-2026-07-25T193000Z",
        counts: { drives: 3, files: 14606, faces: 2126, people_named: 2, thumbnails: 14606 },
        thumbnails_restored: 14606,
        key_restored: true,
      } as unknown as T);
    case "compact_catalogue":
      return Promise.resolve("758 thumbnails re-encoded (182 MB saved); catalogue 160 MB -> 37 MB" as unknown as T);
    case "update_drive_details": {
      const target = mockDrives.find((d) => d.drive_number === args?.driveNumber);
      if (target) {
        if (args?.physicalLocation !== undefined) {
          target.physical_location = String(args.physicalLocation) || null;
        }
        if (Array.isArray(args?.categories)) target.categories = args.categories as string[];
      }
      return Promise.resolve(target as unknown as T);
    }
    case "set_date_override": {
      const from = String(args?.earliest ?? "");
      const to = String(args?.latest ?? from);
      if (!/^\d{4}-\d{2}-\d{2}$/.test(from)) {
        return Promise.reject(new Error(`date must be YYYY-MM-DD, got "${from}"`));
      }
      const label = from === to ? `Taken on ${from}` : `Taken between ${from} and ${to}`;
      const hit = mockResults.find((r) => r.file_id === args?.fileId);
      if (hit) hit.date_label = label;
      return Promise.resolve(label as unknown as T);
    }
    case "clear_date_override":
      return Promise.resolve(undefined as unknown as T);
    case "drive_contents": {
      const contents: DriveContents[] = mockDrives.map((d) => ({
        drive_number: d.drive_number,
        drive_name: d.friendly_name ?? null,
        status: d.status,
        online: d.status === "online",
        physical_location: d.physical_location ?? null,
        categories: d.categories ?? [],
        last_scan_at: d.last_scan_at ?? null,
        photo_count: d.image_count ?? 0,
        missing_count: 0,
        earliest_date: "1998-01-01",
        latest_date: "2011-12-31",
        top_tags: [
          { tag: "beach", count: 900 },
          { tag: "wedding", count: 120 },
          { tag: "dog", count: 80 },
        ],
        with_text_count: 12,
        people_count: 340,
      }));
      const n = args?.driveNumber as number | undefined;
      return Promise.resolve((n ? contents.filter((c) => c.drive_number === n) : contents) as unknown as T);
    }
    case "export_diagnostics":
      return Promise.resolve(
        "~/Library/Application Support/AtlasDrive/reports/diagnostics-sample.json" as unknown as T,
      );
    case "reveal_in_finder": {
      const hit = mockResults.find((r) => r.file_id === args?.fileId);
      return Promise.resolve(
        (hit && !hit.online
          ? `Connect Drive ${hit.drive_number} to open the original.`
          : "Showing the original in Finder.") as unknown as T,
      );
    }
    default:
      return Promise.reject(new Error(`unknown command: ${cmd}`));
  }
}

export const runningInTauri = hasTauri;
