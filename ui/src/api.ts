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
    call<SearchResult[]>("search_catalogue", { query, ...opts }),
  startIndex: (input: { drive: number; path: string; dryRun: boolean }) =>
    call<Progress>("start_index", input),
  getProgress: () => call<Progress | null>("get_progress"),
  runVerifier: () => call<VerifierCheck[]>("run_verifier"),
  prepareReview: (limit: number) => call<ClusterSummary[]>("prepare_review", { limit }),
  doctor: () => call<Record<string, string>>("doctor"),
};

// ---- browser mock -------------------------------------------------------

const mockDrives: Drive[] = [
  { id: "d-14", drive_number: 14, friendly_name: "Family Archive A", status: "online", physical_location: "Studio shelf B", last_scan_at: "2026-07-24", image_count: 4213 },
  { id: "d-07", drive_number: 7, friendly_name: "Holidays 2004-2011", status: "offline", physical_location: "Drawer 2", last_scan_at: "2026-06-30", image_count: 8891 },
  { id: "d-22", drive_number: 22, friendly_name: "Scanned prints", status: "offline", physical_location: "Box A", last_scan_at: "2026-05-12", image_count: 1502 },
];

const mockResults: SearchResult[] = [
  { file_id: "f1", filename: "beach_1998.jpg", relative_path: "holiday/beach_1998.jpg", drive_number: 14, drive_name: "Family Archive A", online: true, date_label: "Taken on 12 Aug 1998", matched: ["text", "visual"], score: 0.91 },
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
      return Promise.resolve(results as unknown as T);
    }
    case "start_index":
      return Promise.resolve({ runId: "mock-run", driveNumber: Number(args?.drive ?? 0), filesDiscovered: 4213, filesDone: 4213, filesFailed: 2, filesQueued: 0, currentBatch: 66, status: "complete" } as unknown as T);
    case "get_progress":
      return Promise.resolve(null as unknown as T);
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
    default:
      return Promise.reject(new Error(`unknown command: ${cmd}`));
  }
}

export const runningInTauri = hasTauri;
