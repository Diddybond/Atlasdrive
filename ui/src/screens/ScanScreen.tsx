import { useEffect, useRef, useState } from "react";
import { api, Progress, ScanStats } from "../api";

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
  const [stats, setStats] = useState<ScanStats | null>(null);
  // Recent throughput readings, for the activity chart. Kept as a plain array
  // rather than a charting library: a hundred numbers drawn as an SVG polyline
  // is a few lines of code and no dependency.
  const [history, setHistory] = useState<number[]>([]);
  const byteSamples = useRef<Sample[]>([]);
  const [mbPerSec, setMbPerSec] = useState<number | null>(null);

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
        const fps = seconds >= 20 && last.done > first.done ? (last.done - first.done) / seconds : null;
        setRate(fps);
        if (fps !== null) setHistory((h) => [...h, fps * 60].slice(-120));
      }

      if (live && live.status === "running") {
        const s = await api.scanStats(live.driveNumber, 12);
        if (cancelled) return;
        setStats(s);

        // Read throughput in megabytes, from bytes of original actually
        // catalogued. Photographs vary from 2MB to 80MB, so a count per minute
        // says little about how hard the drive is working.
        const now = Date.now();
        const bs = byteSamples.current;
        if (bs.length === 0 || bs[bs.length - 1].done !== s.bytes) {
          bs.push({ at: now, done: s.bytes });
        }
        while (bs.length > 2 && now - bs[0].at > RATE_WINDOW_MS) bs.shift();
        const bSeconds = (bs[bs.length - 1].at - bs[0].at) / 1000;
        setMbPerSec(
          bSeconds >= 20 && bs[bs.length - 1].done > bs[0].done
            ? (bs[bs.length - 1].done - bs[0].done) / bSeconds / 1_048_576
            : null,
        );
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

        {running && history.length > 3 && (
          <div className="activity">
            <div className="row-between">
              <h3>Read activity</h3>
              <span className="check-detail">
                {mbPerSec !== null ? `${mbPerSec.toFixed(1)} MB/s` : "measuring…"}
              </span>
            </div>
            <Sparkline values={history} />
          </div>
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

      {stats && stats.files > 0 && (
        <div className="scan-grid">
          <div className="card">
            <h2>What it has found</h2>
            <dl className="stats">
              <div><dt>Faces</dt><dd>{stats.faces.toLocaleString()}</dd></div>
              <div><dt>Tags applied</dt><dd>{stats.tags.toLocaleString()}</dd></div>
              <div><dt>People known</dt><dd>{stats.people_recognised.toLocaleString()}</dd></div>
              <div><dt>Read so far</dt><dd>{gb(stats.bytes)}</dd></div>
            </dl>

            <h3>File types</h3>
            <ul className="type-bars">
              {stats.by_extension.slice(0, 5).map(([ext, n]) => (
                <li key={ext}>
                  <span className="type-name">{ext.toUpperCase()}</span>
                  <span className="type-bar">
                    <span
                      className="type-fill"
                      style={{ width: `${Math.max(2, (n / Math.max(1, stats.files)) * 100)}%` }}
                    />
                  </span>
                  <span className="type-count">
                    {((n / Math.max(1, stats.files)) * 100).toFixed(1)}%
                  </span>
                </li>
              ))}
            </ul>
          </div>

          <div className="card">
            <div className="row-between">
              <h2>Live feed</h2>
              {running && <span className="live-dot" aria-label="Live" role="img" />}
            </div>
            <p className="check-detail">The photographs it has just read, newest first.</p>
            <ul className="feed">
              {stats.recent.map((f) => (
                <li key={f.file_id}>
                  <span className="feed-name" title={f.relative_path}>
                    {f.filename}
                  </span>
                  <span className="feed-meta">
                    {f.top_tag && <span className="feed-tag">{f.top_tag}</span>}
                    {f.faces > 0 && (
                      <span className="feed-faces">
                        {f.faces} {f.faces === 1 ? "face" : "faces"}
                      </span>
                    )}
                    <span className="feed-size">{mb(f.size_bytes)}</span>
                  </span>
                </li>
              ))}
            </ul>
          </div>
        </div>
      )}
    </section>
  );
}

/// A throughput chart, drawn as a plain SVG polyline.
///
/// A charting library would be a dependency and a bundle for one line on one
/// screen. The y-axis is scaled to the data rather than fixed, so a slow drive
/// still shows its shape instead of a flat line along the bottom.
function Sparkline({ values }: { values: number[] }) {
  const w = 600;
  const h = 64;
  // Headroom above the peak, so a perfectly steady rate draws a line across
  // the upper third rather than filling the box to the brim and reading as a
  // solid block. Indexing is often steady for long stretches.
  const peak = Math.max(...values, 1) * 1.35;
  const step = w / Math.max(1, values.length - 1);
  const points = values
    .map((v, i) => `${(i * step).toFixed(1)},${(h - (v / peak) * (h - 6) - 3).toFixed(1)}`)
    .join(" ");
  return (
    <svg
      className="sparkline"
      viewBox={`0 0 ${w} ${h}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={`Read activity, currently ${values[values.length - 1].toFixed(0)} photographs per minute`}
    >
      <polyline className="spark-line" points={points} />
      <polyline className="spark-fill" points={`0,${h} ${points} ${w},${h}`} />
    </svg>
  );
}

function mb(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  return `${Math.round(bytes / 1024 ** 2)} MB`;
}

function gb(bytes: number): string {
  return bytes >= 1024 ** 3 ? `${(bytes / 1024 ** 3).toFixed(1)} GB` : `${Math.round(bytes / 1024 ** 2)} MB`;
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
