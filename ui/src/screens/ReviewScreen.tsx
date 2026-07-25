import { useEffect, useState } from "react";
import { api, ClusterSummary } from "../api";

export function ReviewScreen() {
  const [clusters, setClusters] = useState<ClusterSummary[]>([]);
  const [names, setNames] = useState<Record<string, string>>({});

  useEffect(() => {
    void (async () => setClusters(await api.prepareReview(20)))();
  }, []);

  return (
    <section aria-labelledby="review-heading">
      <h1 id="review-heading">Review</h1>
      <p className="lede">
        The app groups faces that look alike, but never puts a name to anyone on its own. You decide
        who each group is.
      </p>

      {clusters.length === 0 ? (
        <p className="empty">No groups are waiting for review right now.</p>
      ) : (
        <ul className="review-list">
          {clusters.map((c) => (
            <li key={c.cluster_id} className="card review-card">
              <div className="face-tiles" aria-hidden>
                {Array.from({ length: Math.min(4, c.face_count) }).map((_, i) => (
                  <span key={i} className="face-tile">🙂</span>
                ))}
              </div>
              <div className="review-body">
                <p className="review-title">Unnamed group · {c.face_count} photos</p>
                <label className="review-name">
                  Who is this?
                  <input
                    placeholder="Add a name to confirm"
                    value={names[c.cluster_id] ?? ""}
                    onChange={(e) => setNames({ ...names, [c.cluster_id]: e.target.value })}
                  />
                </label>
                <div className="review-actions">
                  <button disabled={!names[c.cluster_id]}>Confirm name</button>
                  <button className="ghost">Not a person</button>
                  <button className="ghost">Review later</button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}
      <p className="footnote">
        Face data is encrypted on this Mac and never leaves it. A name is only ever set when you
        confirm it.
      </p>
    </section>
  );
}
