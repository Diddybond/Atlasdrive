import { useEffect, useState } from "react";
import { api, ArchiveEvent, SplitPoint } from "../api";
import type { SearchContext } from "../App";

/// A proposal is described by its size until someone names it.
function describe(e: ArchiveEvent): string {
  if (e.name && e.name.trim()) return e.name;
  return `${e.photo_count} photograph${e.photo_count === 1 ? "" : "s"}`;
}

/// When it happened, in one readable phrase.
///
/// Kept separate from the name so the two never repeat each other, and written
/// out rather than shown as raw ISO timestamps — "Sat 30 May 2026, 13:02 → 01:30"
/// is what tells you it was a wedding day running into the night.
function when(e: ArchiveEvent): string | null {
  if (!e.earliest_date) return null;
  const start = new Date(e.earliest_date);
  const end = e.latest_date ? new Date(e.latest_date) : null;
  if (Number.isNaN(start.getTime())) return e.earliest_date.slice(0, 10);

  const day = (d: Date) =>
    d.toLocaleDateString(undefined, {
      weekday: "short",
      day: "numeric",
      month: "long",
      year: "numeric",
    });
  const time = (d: Date) =>
    d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });

  const hasTime = e.earliest_date.length >= 16;
  if (!end || Number.isNaN(end.getTime())) return day(start);
  if (!hasTime) {
    return start.toDateString() === end.toDateString()
      ? day(start)
      : `${day(start)} → ${day(end)}`;
  }
  // Same calendar day: no need to repeat the date at the far end.
  if (start.toDateString() === end.toDateString()) {
    return `${day(start)}, ${time(start)} → ${time(end)}`;
  }
  // Ran past midnight: say so, because that is the interesting part.
  const short = (d: Date) =>
    d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
  return `${day(start)}, ${time(start)} → ${short(end)} ${time(end)}`;
}

export function EventsScreen({
  onSearchWithin,
}: {
  onSearchWithin?: (context: SearchContext) => void;
}) {
  const [events, setEvents] = useState<ArchiveEvent[]>([]);
  const [proposal, setProposal] = useState<ArchiveEvent | null>(null);
  const [clients, setClients] = useState<[string, number][]>([]);
  const [name, setName] = useState("");
  const [client, setClient] = useState("");
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [adjusting, setAdjusting] = useState<string | null>(null);
  const [splits, setSplits] = useState<SplitPoint[]>([]);
  const [mergeFrom, setMergeFrom] = useState("");

  async function refresh() {
    setEvents(await api.listEvents());
    setProposal(await api.nextEventProposal());
    setClients(await api.eventClients());
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function propose() {
    setBusy(true);
    setNote(null);
    try {
      const r = await api.proposeEvents();
      const bits = [`Found ${r.proposed} event${r.proposed === 1 ? "" : "s"} in ${r.photos_grouped} photographs.`];
      if (r.photos_skipped) bits.push(`${r.photos_skipped} were too few to be a shoot.`);
      if (r.photos_imprecise)
        bits.push(
          `${r.photos_imprecise} are dated only to a wide range — scanned prints, most likely — ` +
            `and were left alone rather than grouped by a guess.`,
        );
      if (r.photos_undated) bits.push(`${r.photos_undated} have no usable date.`);
      setNote(bits.join(" "));
      await refresh();
    } finally {
      setBusy(false);
    }
  }

  async function confirm() {
    if (!proposal || !name.trim()) return;
    setBusy(true);
    try {
      await api.nameEvent(proposal.id, name.trim(), client.trim() || undefined);
      setName("");
      setClient("");
      await refresh();
    } finally {
      setBusy(false);
    }
  }

  async function skip() {
    if (!proposal) return;
    setBusy(true);
    try {
      await api.forgetEvent(proposal.id);
      setName("");
      setClient("");
      await refresh();
    } finally {
      setBusy(false);
    }
  }

  /// Open the correction panel for one event, loading where it could be split.
  async function adjust(eventId: string) {
    if (adjusting === eventId) {
      setAdjusting(null);
      return;
    }
    setAdjusting(eventId);
    setMergeFrom("");
    setSplits(await api.eventSplitPoints(eventId, 3));
  }

  async function doSplit(eventId: string, at: string) {
    setBusy(true);
    try {
      await api.splitEvent(eventId, at);
      setAdjusting(null);
      setNote("Split. The later photographs are now their own event, waiting to be named.");
      await refresh();
    } finally {
      setBusy(false);
    }
  }

  async function doMerge(into: string) {
    if (!mergeFrom) return;
    setBusy(true);
    try {
      const moved = await api.mergeEvents(into, mergeFrom);
      setAdjusting(null);
      setNote(`Merged ${moved} photograph${moved === 1 ? "" : "s"}.`);
      await refresh();
    } finally {
      setBusy(false);
    }
  }

  const named = events.filter((e) => e.status === "named");
  const proposedCount = events.filter((e) => e.status === "proposed").length;

  return (
    <section aria-labelledby="events-heading">
      <h1 id="events-heading">Events</h1>
      <p className="lede">
        Your archive is made of shoots, not dates. AtlasDrive groups photographs taken together and
        you give each group a name — a wedding, a christening, a session for a client.
      </p>

      <div className="card">
        <div className="row-between">
          <h2>Find events</h2>
          <button onClick={propose} disabled={busy}>
            {busy ? "Working…" : "Find events"}
          </button>
        </div>
        <p className="lede">
          Photographs taken within ten hours of each other are treated as one shoot, so an evening
          reception stays with the ceremony it followed. Nothing already named is disturbed.
        </p>
        {note && (
          <p className="check-detail" role="status">
            {note}
          </p>
        )}
      </div>

      {proposal && (
        <div className="card">
          <div className="row-between">
            <h2>Name this one</h2>
            <span className="check-detail">{proposedCount} waiting</span>
          </div>
          <p className="lede">
            <strong>{describe(proposal)}</strong>
            {when(proposal) ? ` · ${when(proposal)}` : ""}
          </p>

          <div className="form">
            <label>
              What was it?
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Aimee &amp; Kent wedding"
                onKeyDown={(e) => e.key === "Enter" && void confirm()}
              />
            </label>
            <label>
              Client (optional)
              <input
                value={client}
                onChange={(e) => setClient(e.target.value)}
                placeholder="Aimee Kanovan"
                list="known-clients"
                onKeyDown={(e) => e.key === "Enter" && void confirm()}
              />
              <datalist id="known-clients">
                {clients.map(([c]) => (
                  <option key={c} value={c} />
                ))}
              </datalist>
            </label>
          </div>
          <p className="check-detail">
            Adding a client gathers several shoots for the same people, without merging them into
            one event.
          </p>

          <div className="row-between">
            <button onClick={confirm} disabled={busy || !name.trim()}>
              Save
            </button>
            <button className="ghost" onClick={skip} disabled={busy}>
              Not an event
            </button>
          </div>
        </div>
      )}

      {clients.length > 0 && (
        <div className="card">
          <h2>Clients</h2>
          <ul className="tag-cloud">
            {clients.map(([c, n]) => (
              <li key={c}>
                <button
                  className="tag-chip"
                  onClick={() => onSearchWithin?.({ client: c, label: `everything for ${c}` })}
                  title={`Show every photograph shot for ${c}`}
                >
                  {c} <span className="tag-count">{n}</span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="card">
        <h2>Named events</h2>
        {named.length === 0 ? (
          <p className="lede">
            None yet. Use <strong>Find events</strong> above, then name what it proposes.
          </p>
        ) : (
          <ul className="check-list">
            {named.map((e) => (
              <li key={e.id} className="event-row">
                <span className="check-name">{describe(e)}</span>
                <span className="check-detail">
                  {e.photo_count} photographs
                  {when(e) ? ` · ${when(e)}` : ""}
                  {e.client ? ` · ${e.client}` : ""}
                </span>
                <span>
                  {onSearchWithin && (
                    <button
                      className="ghost"
                      onClick={() =>
                        onSearchWithin({ eventId: e.id, label: describe(e) })
                      }
                    >
                      Show photographs
                    </button>
                  )}{" "}
                  <button
                    className="ghost"
                    onClick={() => void adjust(e.id)}
                    aria-expanded={adjusting === e.id}
                  >
                    Adjust…
                  </button>{" "}
                  <button className="ghost" onClick={() => void api.forgetEvent(e.id).then(refresh)}>
                    Remove
                  </button>
                </span>
                {adjusting === e.id && (
                  <div className="adjust-panel">
                    <h3>Split it in two</h3>
                    {splits.length === 0 ? (
                      <p className="check-detail">
                        No obvious break — the camera ran continuously, so this looks like one
                        shoot.
                      </p>
                    ) : (
                      <ul className="check-list">
                        {splits.map((p) => (
                          <li key={p.at} className="check-row">
                            <span className="check-name">
                              After a {p.gap_hours.toFixed(1)}-hour pause
                            </span>
                            <span className="check-detail">
                              {p.photos_after} photographs would move to a new event
                            </span>
                            <button onClick={() => void doSplit(e.id, p.at)} disabled={busy}>
                              Split here
                            </button>
                          </li>
                        ))}
                      </ul>
                    )}

                    <h3>Or fold another event into this one</h3>
                    <div className="row-between">
                      <select
                        aria-label="Event to fold in"
                        value={mergeFrom}
                        onChange={(ev) => setMergeFrom(ev.target.value)}
                      >
                        <option value="">Choose an event…</option>
                        {events
                          .filter((o) => o.id !== e.id)
                          .map((o) => (
                            <option key={o.id} value={o.id}>
                              {/* describe() already gives the count for an
                                  unnamed event, so pair it with the date
                                  rather than repeating the count. */}
                              {describe(o)}
                              {when(o) ? ` · ${when(o)}` : ""}
                            </option>
                          ))}
                      </select>
                      <button onClick={() => void doMerge(e.id)} disabled={busy || !mergeFrom}>
                        Merge in
                      </button>
                    </div>
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
