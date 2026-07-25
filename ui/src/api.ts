// Typed bridge to the Rust core via Tauri commands.
//
// When running inside the Tauri shell, calls go to the real backend. In a plain
// browser (dev / tests) they fall back to deterministic mock data so the UI is
// demonstrable and testable without the native layer. The shapes mirror the
// serde types in family-archive-core.

export interface Drive {
  id: string;
  drive_number: number;
  friendly_name?: string | null;
  status: "online" | "offline" | "changed" | "conflict" | "retired";
  physical_location?: string | null;
  categories?: string[];
  last_scan_at?: string | null;
  image_count?: number;
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
  filesDiscovered: number;
  filesDone: number;
  filesFailed: number;
  filesQueued: number;
  currentBatch: number;
  status: string;
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
  search: (query: string, opts: { drive?: number; includeOffline: boolean }) =>
    call<SearchResponse>("search_catalogue", { query, ...opts }),
  // Starts a background run and returns immediately; poll getProgress().
  startIndex: (input: { drive: number; path: string; dryRun: boolean; resume: boolean }) =>
    call<void>("start_index", input),
  cancelIndex: () => call<void>("cancel_index"),
  isIndexing: () => call<boolean>("is_indexing"),
  getProgress: () => call<Progress | null>("get_progress"),
  runVerifier: () => call<VerifierCheck[]>("run_verifier"),
  prepareReview: (limit: number) => call<ClusterSummary[]>("prepare_review", { limit }),
  doctor: () => call<Record<string, string>>("doctor"),
  exportDiagnostics: () => call<string>("export_diagnostics"),
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
      return Promise.resolve(false as unknown as T);
    case "get_progress":
      return Promise.resolve({
        runId: "mock-run",
        driveNumber: 14,
        filesDiscovered: 4213,
        filesDone: 4213,
        filesFailed: 2,
        filesQueued: 0,
        currentBatch: 66,
        status: "complete",
      } as unknown as T);
    case "run_verifier":
      return Promise.resolve([
        { name: "db_integrity", status: "Pass", detail: "integrity_check and foreign_key_check ok" },
        { name: "originals_unchanged", status: "Pass", detail: "verified 4213 originals unchanged" },
        { name: "network_isolation", status: "Pass", detail: "no network access during indexing" },
        { name: "thumbnail_files", status: "Pass", detail: "4213 thumbnails decode and match" },
        { name: "face_pipeline", status: "Pass", detail: "812 embeddings, dim 32, finite" },
      ] as unknown as T);
    case "prepare_review":
      return Promise.resolve([
        { cluster_id: "c-a1b2c3", status: "unnamed", face_count: 34 },
        { cluster_id: "c-d4e5f6", status: "unnamed", face_count: 12 },
      ] as unknown as T);
    case "doctor":
      return Promise.resolve({ keystore: "file-fallback-dev", archive_integrity: "ok", ai_offline: "true" } as unknown as T);
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
