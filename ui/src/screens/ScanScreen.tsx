import { useEffect, useRef, useState } from "react";
import { api, Progress } from "../api";

// The scan screen only ever *observes*. Indexing is started deliberately from
// the Drives screen or the CLI — visiting this tab must never begin a scan.
const POLL_MS = 1500;

export function ScanScreen() {
  const [progress, setProgress] = useState<Progress | null>(null);
  const [loaded, setLoaded] = useState(false);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function poll() {
      const live = await api.getProgress();
      if (cancelled) return;
      setProgress(live);
      setLoaded(true);
      // Keep polling only while a run is actually in flight.
      if (live && live.status === "running") {
        timer.current = window.setTimeout(poll, POLL_MS);
      }
    }
    void poll();

    return () => {
      cancelled = true;
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, []);

  if (!loaded) return <p>Loading…</p>;

  if (!progress) {
    return (
      <section aria-labelledby="scan-heading">
        <h1 id="scan-heading">Scan activity</h1>
        <p className="lede">
          Indexing reads your photographs without ever changing them, and can be safely paused at
          any batch boundary.
        </p>
        <p className="empty">
          No scan has run yet. Go to <strong>Drives</strong> to register a drive and start indexing.
        </p>
      </section>
    );
  }

  const total = progress.filesDiscovered || 1;
  const pct = Math.min(100, Math.round((progress.filesDone / total) * 100));
  const running = progress.status === "running";

  return (
    <section aria-labelledby="scan-heading">
      <h1 id="scan-heading">Scan activity</h1>
      <p className="lede">
        Indexing reads your photographs without ever changing them, and can be safely paused at any
        batch boundary.
      </p>

      <div className="card">
        <div className="row-between">
          <h2>Drive {progress.driveNumber}</h2>
          <span className={running ? "status online" : "status offline"}>
            {statusLabel(progress.status)}
          </span>
        </div>
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
        {progress.status === "interrupted" && (
          <p className="offline-note">
            This scan was interrupted. Nothing was lost — start it again to pick up where it left
            off.
          </p>
        )}
        <p className="footnote">
          Original files are opened read-only. If anything about an original changes during a scan,
          the app stops immediately and writes a safety report.
        </p>
      </div>
    </section>
  );
}

function statusLabel(status: string): string {
  switch (status) {
    case "running":
      return "In progress";
    case "complete":
      return "Finished";
    case "interrupted":
      return "Paused";
    case "halted":
      return "Stopped for safety";
    default:
      return status;
  }
}
