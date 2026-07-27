import { useEffect, useRef, useState } from "react";
import { api, Drive, FailureReason, Progress, ScanStats } from "../api";
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
  // Read inside the poll loop, which is created once per selection.
  const pickedRef = useRef<number | null>(null);
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
  // Which drive the screen is showing. Independent of which drive is being
  // read: a scan takes two days, and "how did Drive 1 turn out" is a fair
  // question to ask while Drive 3 is running.
  const [drives, setDrives] = useState<Drive[]>([]);
  const [picked, setPicked] = useState<number | null>(null);
  // Counted from the drive's own queue, not from the running process. A file
  // given up on two sessions ago is still not in the catalogue.
  const [failedCount, setFailedCount] = useState(0);
  const [scanError, setScanError] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);
  const [stopNote, setStopNote] = useState<string | null>(null);

  useEffect(() => {
    void api.listDrives().then(setDrives);
  }, []);
  pickedRef.current = picked;

  useEffect(() => {
    let cancelled = false;

    async function poll() {
      const live = await api.getProgress();
      if (cancelled) return;
      setProgress(live);
      setLoaded(true);
      const shown = pickedRef.current ?? live?.driveNumber ?? null;
      setScanError(await api.lastScanError().catch(() => null));
      setStopping(await api.stopPending().catch(() => false));

      if (live) {
        const now = Date.now();
        samples.current = record(samples.current, now, live.filesDone);
        setRate(ratePerSecond(samples.current, now));
      }

      // The catalogue counts are read whether or not a run is in flight: they
      // are what the progress bar is measured against, so a finished or paused
      // scan must still show where the drive stands.
      if (shown !== null) {
        const s = await api.scanStats(shown, 12);
        if (cancelled) return;
        setStats(s);
        const reasons = await api.scanFailures(shown).catch(() => []);
        if (cancelled) return;
        setFailedCount(reasons.reduce((n, r) => n + r.files, 0));

        if (live && live.status === "running" && live.driveNumber === shown) {
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
  }, [picked]);

  if (!loaded) return <p>Loading…</p>;

  const shownDrive = picked ?? progress?.driveNumber ?? drives[0]?.drive_number ?? null;
  // The live figures belong to the drive being read. Showing them beside
  // another drive's totals would be the same mixing of two things that made
  // "4 / 8,333" appear above "19.6 GB read".
  const isLive = progress !== null && progress.driveNumber === shownDrive;

  const drivePicker =
    drives.length > 1 ? (
      <div className="drive-filter">
        <span className="filter-label">Show</span>
        {drives.map((d) => (
          <button
            key={d.id}
            className={shownDrive === d.drive_number ? "chip selected" : "chip"}
            onClick={() => setPicked(d.drive_number)}
            title={d.friendly_name ?? undefined}
          >
            Drive {d.drive_number}
            {progress?.driveNumber === d.drive_number && progress.status === "running" && (
              <span className="live-dot" aria-label="being read now" />
            )}
          </button>
        ))}
      </div>
    ) : null;

  if (shownDrive === null) {
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
  const running = isLive && progress.status === "running";
  const catalogued = stats ? stats.files : 0;
  // Still queued is the durable truth about what is left: a resumed run
  // re-walks files it has already catalogued, so discovered minus done counts
  // finished work as outstanding.
  const queued = isLive ? progress.filesQueued : 0;
  const discovered = isLive ? progress.filesDiscovered : 0;
  const driveTotal = Math.max(discovered, catalogued + queued, 1);
  const pct = Math.min(100, Math.round((catalogued / driveTotal) * 100));
  const remaining = queued || Math.max(0, driveTotal - catalogued);
  const secondsLeft = secondsRemaining(remaining, rate);

  return (
    <section aria-labelledby="scan-heading">
      <h1 id="scan-heading">Scan activity</h1>
      <p className="lede">
        Indexing reads your photographs without ever changing them, and can be safely paused at any
        batch boundary.
      </p>

      {stopNote && (
        <p className="search-note" role="status">
          {stopNote}
        </p>
      )}

      {scanError && (
        <p className="search-note warn" role="alert">
          The last scan stopped before it finished: {scanError}. Nothing was lost — start it again
          from <strong>Drives</strong> and it will carry on from where it stopped.
        </p>
      )}

      {drivePicker}

      <div className="console">
        <div className="console-head">
          <div>
            <h2>
              Drive {shownDrive}
              {drives.find((d) => d.drive_number === shownDrive)?.friendly_name && (
                <span className="console-name">
                  {" "}
                  {drives.find((d) => d.drive_number === shownDrive)?.friendly_name}
                </span>
              )}
            </h2>
            <p className="console-sub">
              {running
                ? "Reading photographs from this drive"
                : isLive
                  ? statusLabel(progress.status)
                  : "Not being read now — showing what is already catalogued"}
            </p>
          </div>
          <span className="head-actions">
            {running && (
              <button
                className="secondary"
                disabled={stopping}
                onClick={() =>
                  void api.stopScan().then((m) => {
                    setStopping(true);
                    setStopNote(m);
                  })
                }
              >
                {stopping ? "Stopping…" : "Stop scanning"}
              </button>
            )}
            <span className={running ? "pill-live" : "pill-idle"}>
              {running && <span className="live-dot" aria-hidden />}
              {stopping ? "Stopping" : isLive ? statusLabel(progress.status) : "Idle"}
            </span>
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
              label={isLive ? "Been running for" : "Last read"}
              value={
                isLive
                  ? elapsed(progress.startedAt)
                  : stats?.recent[0]
                    ? "—"
                    : "never"
              }
              sub={
                isLive
                  ? rate
                    ? `${(rate * 60).toFixed(1)} photographs/min`
                    : "Elapsed"
                  : "Not being read now"
              }
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
            {isLive && (
              <p className="panel-note">
                {progress.filesDone.toLocaleString()} of those were read in this session, which
                began {elapsed(progress.startedAt)} ago.
              </p>
            )}
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
                  <dd className={failedCount > 0 ? "bad" : undefined}>
                    {failedCount.toLocaleString()}
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

        {failedCount > 0 && (
          <section className="panel">
            <div className="row-between">
              <h3>{failedCount.toLocaleString()} photographs were given up on</h3>
              <button
                className="ghost"
                onClick={() =>
                  void (failures === null
                    ? api.scanFailures(shownDrive).then(setFailures)
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
                    void api.retryFailedFiles(shownDrive).then((n) => {
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
          {isLive && progress.status === "interrupted" && (
            <span className="warn">Paused — start it again to continue</span>
          )}
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
