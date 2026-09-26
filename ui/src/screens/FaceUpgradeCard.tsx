import { useEffect, useRef, useState } from "react";
import { api, FaceIdentityState } from "../api";

/// Better face recognition (D-102): offer it, show it working, say what it did.
///
/// The upgrade re-reads every stored face picture with a model built to tell
/// people apart, then rebuilds the unnamed groups. It needs no drives, can be
/// stopped at any time, and carries on where it left off.
export function FaceUpgradeCard({ onFinished }: { onFinished: () => void }) {
  const [s, setS] = useState<FaceIdentityState | null>(null);
  // Why the state could not be read. Shown rather than hiding the card: a
  // card that silently vanished left the owner with no way to carry on.
  const [problem, setProblem] = useState<string | null>(null);
  const wasRunning = useRef(false);
  // One question at a time: never queue a new one behind a slow answer.
  const asking = useRef(false);

  async function refresh() {
    if (asking.current) return;
    asking.current = true;
    try {
      await ask();
    } finally {
      asking.current = false;
    }
  }

  async function ask() {
    let next: FaceIdentityState | null = null;
    try {
      next = await api.faceIdentityState();
      setProblem(null);
    } catch (e) {
      setProblem(String(e));
      return; // keep showing the last good state
    }
    setS(next);
    if (next) {
      if (wasRunning.current && !next.job.running) onFinished();
      wasRunning.current = next.job.running;
    }
  }

  useEffect(() => {
    void refresh();
    const t = window.setInterval(() => void refresh(), 2000);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!s) {
    return problem ? (
      <div className="card upgrade-card" role="status">
        <h2>Face recognition</h2>
        <p className="error">Could not check the face recognition upgrade: {problem}</p>
        <p className="subtle">Trying again every few seconds.</p>
      </div>
    ) : null;
  }
  if (!s.model_installed) {
    return (
      <div className="card upgrade-card" role="status">
        <h2>Face recognition</h2>
        <p className="subtle">
          The improved face recognition is not installed in this copy of AtlasDrive. Rebuild the app
          (the build fetches it) to carry on.
        </p>
      </div>
    );
  }
  const { job, status } = s;

  if (job.running) {
    const reading = job.phase === "reading";
    const handled = job.done + job.unreadable;
    const pct = job.total > 0 ? Math.min(100, Math.round((handled / job.total) * 100)) : 0;
    const rate = job.elapsed_secs > 20 && handled > 0 ? handled / job.elapsed_secs : 0;
    const left = rate > 0 ? (job.total - handled) / rate : null;
    return (
      <div className="card upgrade-card" role="status">
        <h2>Improving face recognition</h2>
        {reading ? (
          <>
            <p>
              Reading faces: {handled.toLocaleString()} of {job.total.toLocaleString()} ({pct}%)
              {left !== null && <> · about {duration(left)} left</>}
            </p>
            <div className="bar" aria-hidden>
              <span style={{ width: `${pct}%` }} />
            </div>
            <p className="subtle">
              Keep using AtlasDrive as normal. Stopping keeps everything done so far.
            </p>
            <button className="ghost" onClick={() => void api.stopFaceUpgrade().then(refresh)}>
              Stop for now
            </button>
          </>
        ) : (
          <p>Regrouping faces with the new recognition… nearly done.</p>
        )}
      </div>
    );
  }

  if (job.phase === "finished" && job.regroup && !job.error) {
    const r = job.regroup;
    return (
      <div className="card upgrade-card done" role="status">
        <h2>Face recognition improved</h2>
        <p>
          {job.done.toLocaleString()} faces re-read. Unnamed faces are now in{" "}
          {r.groups_created.toLocaleString()} groups of the same person
          {r.suggestions > 0 && (
            <>
              , and {r.suggestions.toLocaleString()} faces are suggested as people you have
              already named — use <b>Review</b> next to each name
            </>
          )}
          .
        </p>
      </div>
    );
  }

  if (status.pending === 0) return null;

  const resuming = status.upgraded > 0;
  return (
    <div className="card upgrade-card">
      <h2>{resuming ? "Finish improving face recognition" : "Better face recognition is ready"}</h2>
      {job.error && <p className="error">Stopped: {job.error}</p>}
      <p>
        AtlasDrive can re-read {status.pending.toLocaleString()} faces with a model built to tell
        people apart, so the same person is grouped together and different people are not. It
        uses the face pictures already stored — no drives needed — and runs in the background.
        Names you have given are kept.
      </p>
      <button onClick={() => void api.startFaceUpgrade().then(refresh)}>
        {resuming ? "Carry on" : "Improve face recognition"}
      </button>
    </div>
  );
}

function duration(secs: number): string {
  const m = Math.round(secs / 60);
  if (m < 2) return "a minute";
  if (m < 60) return `${m} minutes`;
  const h = Math.floor(m / 60);
  const rest = m % 60;
  return rest >= 5 ? `${h} h ${rest} min` : `${h} hour${h === 1 ? "" : "s"}`;
}
