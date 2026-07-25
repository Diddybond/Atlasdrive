import { useEffect, useState } from "react";
import { api, ClusterSummary, NamedPerson } from "../api";

export function ReviewScreen() {
  const [clusters, setClusters] = useState<ClusterSummary[]>([]);
  const [people, setPeople] = useState<NamedPerson[]>([]);
  const [names, setNames] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [justTagged, setJustTagged] = useState<string | null>(null);

  async function load() {
    setClusters(await api.prepareReview(20));
    setPeople(await api.listPeople());
  }
  useEffect(() => {
    void load();
  }, []);

  async function tag(cluster: ClusterSummary) {
    const name = (names[cluster.cluster_id] ?? "").trim();
    if (!name) return;
    setBusy(cluster.cluster_id);
    setError(null);
    try {
      const person = await api.tagFaceCluster(cluster.cluster_id, name);
      setJustTagged(person.display_name);
      setNames({ ...names, [cluster.cluster_id]: "" });
      await load();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function reject(cluster: ClusterSummary) {
    setBusy(cluster.cluster_id);
    try {
      await api.rejectFaceCluster(cluster.cluster_id);
      await load();
    } finally {
      setBusy(null);
    }
  }

  return (
    <section aria-labelledby="review-heading">
      <h1 id="review-heading">People</h1>
      <p className="lede">
        AtlasDrive groups faces that look alike, but never puts a name to anyone on its own. Once you
        name someone, it will suggest them when it sees a similar face on a later scan — and still
        ask you to confirm.
      </p>

      {people.length > 0 && (
        <div className="card">
          <h2>People you have named</h2>
          <ul className="people-list">
            {people.map((p) => (
              <li key={p.id}>
                <span className="person-name">{p.display_name}</span>
                <span className="person-counts">
                  {p.confirmed_faces} confirmed
                  {p.suggested_faces > 0 && <> · {p.suggested_faces} awaiting your confirmation</>}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}

      {justTagged && (
        <p className="search-note" role="status">
          Tagged as {justTagged}. AtlasDrive will suggest {justTagged} when it sees a similar face on
          the next scan.
        </p>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      {clusters.length === 0 ? (
        <p className="empty">No groups are waiting for review right now.</p>
      ) : (
        <ul className="review-list">
          {clusters.map((c) => (
            <li key={c.cluster_id} className="card review-card">
              <div className="face-tiles" aria-hidden>
                {Array.from({ length: Math.min(4, c.face_count) }).map((_, i) => (
                  <span key={i} className="face-tile">
                    🙂
                  </span>
                ))}
              </div>
              <div className="review-body">
                <p className="review-title">
                  {c.label ? `Possible match — ${c.label}` : "Unnamed group"} · {c.face_count}{" "}
                  photograph{c.face_count === 1 ? "" : "s"}
                </p>
                <label className="review-name">
                  Who is this?
                  <input
                    placeholder="Type a name to confirm"
                    list="known-people"
                    value={names[c.cluster_id] ?? ""}
                    onChange={(e) => setNames({ ...names, [c.cluster_id]: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void tag(c);
                    }}
                  />
                </label>
                <div className="review-actions">
                  <button
                    onClick={() => void tag(c)}
                    disabled={!names[c.cluster_id]?.trim() || busy === c.cluster_id}
                  >
                    {busy === c.cluster_id ? "Saving…" : "Confirm name"}
                  </button>
                  <button
                    className="ghost"
                    onClick={() => void reject(c)}
                    disabled={busy === c.cluster_id}
                  >
                    Not a person
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}

      {/* Typing an existing name reuses that person rather than creating a duplicate. */}
      <datalist id="known-people">
        {people.map((p) => (
          <option key={p.id} value={p.display_name} />
        ))}
      </datalist>

      <p className="footnote">
        Face data is encrypted on this Mac and never leaves it. A name is only ever set when you
        confirm it.
      </p>
    </section>
  );
}
