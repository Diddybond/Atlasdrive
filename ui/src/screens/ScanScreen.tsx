import { useEffect, useRef, useState } from "react";
import { api, Progress } from "../api";

// The scan screen only ever *observes*. Indexing is started deliberately from
// the Drives screen or the CLI — visiting this tab must never begin a scan.
const POLL_MS = 1500;

/// A sample of how far along a run was at a moment in time.
interface Sample {
  at: number;
  done: number;
}

/// How far back to measure the rate.
///
/// Long enough to be steady — a single photograph can take anywhere from one
/// second to twenty depending on how many faces are in it — and short enough
/// that the figure still reflects the drive being read now rather than an
/// average over the whole night.
const RATE_WINDOW_MS = 3 * 60 * 1000;

export function ScanScreen() {
  const [progress, setProgress] = useState<Progress | null>(null);
  const [loaded, setLoaded] = useState(false);
  const timer = useRef<number | null>(null);
  // Rate is measured here rather than reported by the backend: the interface
  // already sees every update, and a figure derived from what is actually
  // arriving cannot drift from what is on screen.
  const samples = useRef<Sample[]>([]);
  const [rate, setRate] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function poll() {
      const live = await api.getProgress();
      if (cancelled) return;
      setProgress(live);
      setLoaded(true);

      if (live) {
        const now = Date.now();
        const history = samples.current;
        // Only record when the count actually moves, so a stalled run shows a
        // falling rate rather than a confidently wrong one.
        if (history.length === 0 || history[history.length - 1].done !== live.filesDone) {
          history.push({ at: now, done: live.filesDone });
        }
        while (history.length > 2 && now - history[0].at > RATE_WINDOW_MS) history.shift();

        const first = history[0];
        const last = history[history.length - 1];
        const seconds = (last.at - first.at) / 1000;
        setRate(seconds >= 20 && last.done > first.done ? (last.done - first.done) / seconds : null);
      }
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
  const remaining = Math.max(0, progress.filesDiscovered - progress.filesDone);
  const secondsLeft = rate && rate > 0 ? remaining / rate : null;

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
        <p className="counter">
          <strong>{progress.filesDone.toLocaleString()}</strong>
          <span className="counter-of"> of {progress.filesDiscovered.toLocaleString()} photographs</span>
          <span className="counter-pct">{pct}%</span>
        </p>

        <dl className="stats">
          <div>
            <dt>Remaining</dt>
            <dd>{remaining.toLocaleString()}</dd>
          </div>
          <div>
            <dt>Speed</dt>
            <dd>{running ? (rate ? `${(rate * 60).toFixed(1)}/min` : "measuring…") : "—"}</dd>
          </div>
          <div>
            <dt>Time left</dt>
            <dd>{running ? (secondsLeft !== null ? duration(secondsLeft) : "…") : "—"}</dd>
          </div>
          <div>
            <dt>Should finish</dt>
            <dd>{running && secondsLeft !== null ? finishTime(secondsLeft) : "—"}</dd>
          </div>
          <div>
            <dt>Running for</dt>
            <dd>{elapsed(progress.startedAt)}</dd>
          </div>
          <div>
            <dt>Failed</dt>
            <dd className={progress.filesFailed > 0 ? "bad" : undefined}>
              {progress.filesFailed.toLocaleString()}
            </dd>
          </div>
        </dl>

        {running && progress.lastCompletedFile && (
          <p className="now-doing" role="status">
            Just finished <span className="filename">{progress.lastCompletedFile}</span>
          </p>
        )}

        {running && (
          <p className="footnote">
            This Mac will stay awake until the scan finishes. You can close this window — indexing
            carries on, and interrupting it loses nothing.
          </p>
        )}
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

/// "4h 20m", "18m", "45s" — the unit someone would use out loud.
function duration(seconds: number): string {
  if (seconds < 90) return `${Math.round(seconds)}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 90) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours < 24) return rest ? `${hours}h ${rest}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h`;
}

/// The clock time it should finish, which is what a person actually wants to
/// know overnight — "about 09:40 tomorrow" beats "in 34,000 seconds".
function finishTime(secondsLeft: number): string {
  const end = new Date(Date.now() + secondsLeft * 1000);
  const time = end.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  const today = new Date().toDateString() === end.toDateString();
  if (today) return `about ${time}`;
  const day = end.toLocaleDateString(undefined, { weekday: "long" });
  return `about ${time} ${day}`;
}

function elapsed(startedAt: string): string {
  const start = new Date(startedAt).getTime();
  if (Number.isNaN(start)) return "—";
  return duration((Date.now() - start) / 1000);
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
