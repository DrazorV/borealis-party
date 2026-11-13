import { useEffect, useState } from "react";
import { loadLog } from "../../api/overrides/combat";
import type { DmEvent } from "./types";

/**
 * The DM's event log by round (C8, D32), newest first, read from the server's DM-only log route while it is shown
 * and again after every change (`seq`). It names hidden creatures, so it is never shown to players.
 */
export function EventLog({ combat, seq }: { combat: string; seq: number }) {
  const [log, setLog] = useState<DmEvent[] | null>(null);
  useEffect(() => {
    let live = true;
    loadLog(combat).then((l) => { if (live) setLog(l); }, () => { /* offline: keep what is shown */ });
    return () => { live = false; };
  }, [combat, seq]);
  if (log === null) return <p className="muted cb-small">Loading the log…</p>;
  return <LogList log={log} />;
}

export function LogList({ log, limit = 120 }: { log: DmEvent[]; limit?: number }) {
  const recent = log.slice(-limit).reverse();
  if (recent.length === 0) return <p className="muted cb-small">Nothing has happened yet.</p>;
  const rounds: { round: number; events: DmEvent[] }[] = [];
  for (const e of recent) {
    const last = rounds[rounds.length - 1];
    if (last && last.round === e.round) last.events.push(e);
    else rounds.push({ round: e.round, events: [e] });
  }
  return (
    <div className="cb-log" aria-label="Event log">
      {rounds.map((r, i) => (
        <section key={`${r.round}-${i}`}>
          <h4 className="cb-log-round">{r.round === 0 ? "Before the fight" : `Round ${r.round}`}</h4>
          <ul>{r.events.map((e, j) => <li key={j} className={`cb-log-${e.event}`}>{e.text}</li>)}</ul>
        </section>
      ))}
    </div>
  );
}
