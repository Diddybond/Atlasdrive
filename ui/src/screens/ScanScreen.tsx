import { useEffect, useRef, useState } from "react";
import { api, FailureReason, Progress, ScanStats } from "../api";
import { Gauge } from "./scan/Gauge";
import { Donut } from "./scan/Donut";
import { AreaChart } from "./scan/AreaChart";
import { record, ratePerSecond, secondsRemaining, type Sample } from "./scan/rate";
import { plainReason } from "./scan/reasons";

// The scan screen only ever *observes*. Indexing is started deliberately from
// the Drives screen or the CLI — visiting this tab must never begin a scan.
const POLL_MS = 1500;

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
  const byteSamples = useRef<Sample[]>([]);
  const [mbPerSec, setMbPerSec] = useState<number | null>(null);
  // The gauge face is sized from the fastest reading seen, so a slow drive
  // still swings the needle. Kept separately from the current value because a
  // dial that rescaled downwards would make a steady drive look like it was
  // speeding up.
  const [peakMb, setPeakMb] = useState(0);
  const [mbHistory, setMbHistory] = useState<number[]>([]);
  const [failures, setFailures] = useState<FailureReason[] | null>(null);
  const [retryNote, setRetryNote] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function poll() {
      const live = await api.getProgress();
      if (cancelled) return;
      setProgress(live);
      setLoaded(true);

      if (live) {
        const now = Date.now();
        samples.current = record(samples.current, now, live.filesDone);
        setRate(ratePerSecond(samples.current, now));
      }

      // The catalogue counts are read whether or not a run is in flight: they
      // are what the progress bar is measured against, so a finished or paused
      // scan must still show where the drive stands.
      if (live) {
        const s = await api.scanStats(live.driveNumber, 12);
        if (cancelled) return;
        setStats(s);

        if (live.status === "running") {
          // Read throughput in megabytes, from bytes of original actually
          // catalogued. Photographs vary from 2MB to 80MB, so a count per
          // minute says little about how hard the drive is working.
          const now = Date.now();
          byteSamples.current = record(byteSamples.current, now, s.bytes);
          const bytesPerSec = ratePerSecond(byteSamples.current, now);
          const mbps = bytesPerSec === null ? null : bytesPerSec / 1_048_576;
          setMbPerSec(mbps);
          if (mbps !== null) {
            setPeakMb((p) => Math.max(p, mbps));
            setMbHistory((h) => [...h, mbps].slice(-140));
          }
        }
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

  // Progress is reported for the *drive*, not for this session's run.
  //
  // These were mixed before: the bar counted only what this run had done while
  // the panels below counted the whole catalogue, so a scan resumed after a
  // restart read "4 / 8,333" above "19.6 GB read" and 432 faces. The owner
  // leaves a drive running for two days across several sessions; the question
  // being asked is "is this drive finished", not "how has this process done".
  const running = progress.status === "running";
  const catalogued = stats ? stats.files : progress.filesDone;
  // Still queued is the durable truth about what is left: a resumed run
  // re-walks files it has already catalogued, so discovered minus done counts
  // finished work as outstanding.
  const driveTotal = Math.max(progress.filesDiscovered, catalogued + progress.filesQueued, 1);
  const pct = Math.min(100, Math.round((catalogued / driveTotal) * 100));
  const remaining = progress.filesQueued || Math.max(0, driveTotal - catalogued);
  const secondsLeft = secondsRemaining(remaining, rate);

  return (
    <section aria-labelledby="scan-heading">
      <h1 id="scan-heading">Scan activity</h1>
      <p className="lede">
        Indexing reads your photographs without ever changing them, and can be safely paused at any
        batch boundary.
      </p>

      <div className="console">
        <div className="console-head">
          <div>
            <h2>Drive {progress.driveNumber}</h2>
            <p className="console-sub">
              {running ? "Reading photographs from this drive" : statusLabel(progress.status)}
            </p>
          </div>
          <span className={running ? "pill-live" : "pill-idle"}>
            {running && <span className="live-dot" aria-hidden />}
            {statusLabel(progress.status)}
          </span>
        </div>

        <div className="console-top">
          <section className="panel gauge-panel">
            <h3>Read speed</h3>
            <Gauge value={mbPerSec} peak={peakMb} />
            <p className="panel-note">
              {mbPerSec === null ? "Measuring…" : running ? "Sustained" : "Stopped"}
            </p>
          </section>

          <div className="tiles">
            <Tile
              tone="blue"
              icon="▦"
              label="Photographs found"
              value={driveTotal.toLocaleString()}
              sub="On this drive"
            />
            <Tile
              tone="violet"
              icon="◷"
              label="Left to read"
              value={remaining.toLocaleString()}
              sub="Remaining"
            />
            <Tile
              tone="green"
              icon="◎"
              label="Finishes in"
              value={running && secondsLeft !== null ? duration(secondsLeft) : "—"}
              sub={running && secondsLeft !== null ? finishTime(secondsLeft) : "Estimating"}
            />
            <Tile
              tone="amber"
              icon="⧗"
              label="Been running for"
              value={elapsed(progress.startedAt)}
              sub={rate ? `${(rate * 60).toFixed(1)} photographs/min` : "Elapsed"}
            />
          </div>
        </div>

        {mbHistory.length > 3 && (
          <section className="panel">
            <div className="row-between">
              <h3>Read activity</h3>
              <span className="realtime">
                Real-time <span className="live-dot" aria-hidden />
              </span>
            </div>
            <AreaChart values={mbHistory} unit="MB/s" />
          </section>
        )}

        <div className="console-bottom">
          <section className="panel">
            <div className="row-between">
              <h3>Overall progress</h3>
              <span className="big-pct">{pct}%</span>
            </div>
            <p className="panel-note">
              Reading JPEG, PNG, TIFF and PSD — RAW files are skipped unless you ask for them.
            </p>
            <div
              className="bar"
              role="progressbar"
              aria-valuenow={pct}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-label={`Indexing progress: ${pct}%`}
            >
              <div className="bar-fill" style={{ width: `${pct}%` }} />
            </div>
            <div className="bar-legend">
              <span>
                <strong>{catalogued.toLocaleString()}</strong> / {driveTotal.toLocaleString()} photographs
                catalogued
              </span>
              {stats && <span>{gb(stats.bytes)} read</span>}
            </div>
            <p className="panel-note">
              {progress.filesDone.toLocaleString()} of those were read in this session, which began{" "}
              {elapsed(progress.startedAt)} ago.
            </p>
          </section>

          {stats && stats.by_extension.length > 0 && (
            <section className="panel">
              <h3>File types</h3>
              <Donut slices={stats.by_extension.slice(0, 5)} />
            </section>
          )}
        </div>

        {stats && stats.files > 0 && (
          <div className="console-bottom">
            <section className="panel">
              <h3>What it has found</h3>
              <dl className="found">
                <div><dt>Faces</dt><dd>{stats.faces.toLocaleString()}</dd></div>
                <div><dt>Tags applied</dt><dd>{stats.tags.toLocaleString()}</dd></div>
                <div><dt>People known</dt><dd>{stats.people_recognised.toLocaleString()}</dd></div>
                <div>
                  <dt>Given up on</dt>
                  <dd className={progress.filesFailed > 0 ? "bad" : undefined}>
                    {progress.filesFailed.toLocaleString()}
                  </dd>
                </div>
              </dl>
            </section>

            <section className="panel">
              <div className="row-between">
                <h3>Live feed</h3>
                {running && <span className="live-dot" aria-label="Live" role="img" />}
              </div>
              <ul className="feed">
                {stats.recent.map((f) => (
                  <li key={f.file_id}>
                    <span className="feed-name" title={f.relative_path}>{f.filename}</span>
                    <span className="feed-meta">
                      {f.top_tag && <span className="feed-tag">{f.top_tag}</span>}
                      {f.faces > 0 && <span className="feed-faces">{f.faces} {f.faces === 1 ? "face" : "faces"}</span>}
                      <span className="feed-size">{mb(f.size_bytes)}</span>
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          </div>
        )}

        {progress.filesFailed > 0 && (
          <section className="panel">
            <div className="row-between">
              <h3>{progress.filesFailed.toLocaleString()} photographs were given up on</h3>
              <button
                className="ghost"
                onClick={() =>
                  void (failures === null
                    ? api.scanFailures(progress.driveNumber).then(setFailures)
                    : setFailures(null))
                }
              >
                {failures === null ? "Show why" : "Hide"}
              </button>
            </div>
            <p className="panel-note">
              AtlasDrive tried each of these three times and could not read them. They are not in
              the catalogue. The originals were not touched or changed.
            </p>
            {failures !== null && (
              <>
                <ul className="failure-list">
                  {failures.map((f) => (
                    <li key={f.code + f.message}>
                      <span className="failure-count">{f.files.toLocaleString()}</span>
                      <span className="failure-body">
                        <strong>{plainReason(f.message)}</strong>
                        {f.example && <span className="failure-example">for example {f.example}</span>}
                      </span>
                    </li>
                  ))}
                </ul>
                <button
                  className="secondary"
                  onClick={() =>
                    void api.retryFailedFiles(progress.driveNumber).then((n) => {
                      setRetryNote(
                        n === 0
                          ? "Nothing to retry."
                          : `${n.toLocaleString()} put back in the queue. Start the scan again and AtlasDrive will have another go at them.`,
                      );
                      setFailures(null);
                    })
                  }
                >
                  Try these again
                </button>
                <p className="panel-note">
                  Worth doing after AtlasDrive has been updated: a file it could not read before may
                  read perfectly now. Nothing already catalogued is redone.
                </p>
              </>
            )}
            {retryNote && (
              <p className="search-note" role="status">
                {retryNote}
              </p>
            )}
          </section>
        )}

        <footer className="console-foot">
          <span>Originals opened read-only</span>
          <span>This Mac stays awake while scanning</span>
          <span>Interrupting loses nothing</span>
          {progress.status === "interrupted" && <span className="warn">Paused — start it again to continue</span>}
        </footer>
      </div>
    </section>
  );
}

/// One statistic, as a card with a coloured icon tile.
function Tile({
  tone,
  icon,
  label,
  value,
  sub,
}: {
  tone: string;
  icon: string;
  label: string;
  value: string;
  sub: string;
}) {
  return (
    <div className="tile">
      <span className={`tile-icon ${tone}`} aria-hidden>
        {icon}
      </span>
      <span className="tile-body">
        <span className="tile-label">{label}</span>
        <strong className="tile-value">{value}</strong>
        <span className="tile-sub">{sub}</span>
      </span>
    </div>
  );
}

function mb(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  // Below a megabyte, rounding to whole megabytes prints "0 MB" — which reads
  // as a failure rather than as a small file. This archive holds 333MB scans
  // and sub-megabyte web exports side by side.
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
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
