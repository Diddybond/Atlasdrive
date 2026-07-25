import { useEffect, useState } from "react";
import { api, Progress } from "../api";

export function ScanScreen() {
  const [progress, setProgress] = useState<Progress | null>(null);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    void (async () => {
      // In demo mode start_index returns a completed snapshot; in the real app
      // get_progress reflects the live run written to progress.json.
      const live = await api.getProgress();
      const p = live ?? (await api.startIndex({ drive: 14, path: "/Volumes/FamilyArchiveA", dryRun: false }));
      setProgress(p);
      setLoaded(true);
    })();
  }, []);

  if (!loaded) return <p>Loading…</p>;
  if (!progress) return <p className="empty">No scan has run yet.</p>;

  const total = progress.filesDiscovered || 1;
  const pct = Math.min(100, Math.round((progress.filesDone / total) * 100));

  return (
    <section aria-labelledby="scan-heading">
      <h1 id="scan-heading">Scan activity</h1>
      <p className="lede">
        Indexing reads your photographs without ever changing them, and can be safely paused at any
        batch boundary.
      </p>

      <div className="card">
        <h2>Drive {progress.driveNumber}</h2>
        <div
          className="progress"
          role="progressbar"
          aria-valuenow={pct}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-label={`Indexing progress: ${pct}%`}
        >
          <div className="progress-fill" style={{ width: `${pct}%` }} />
        </div>
        <dl className="stats">
          <div><dt>Discovered</dt><dd>{progress.filesDiscovered.toLocaleString()}</dd></div>
          <div><dt>Completed</dt><dd>{progress.filesDone.toLocaleString()}</dd></div>
          <div><dt>Remaining</dt><dd>{progress.filesQueued.toLocaleString()}</dd></div>
          <div><dt>Failed</dt><dd>{progress.filesFailed.toLocaleString()}</dd></div>
          <div><dt>Batch</dt><dd>{progress.currentBatch}</dd></div>
          <div><dt>Status</dt><dd className="capitalize">{progress.status}</dd></div>
        </dl>
        <p className="footnote">
          Original files are opened read-only. If anything about an original changes during a scan,
          the app stops immediately and writes a safety report.
        </p>
      </div>
    </section>
  );
}
