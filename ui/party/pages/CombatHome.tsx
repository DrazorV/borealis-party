import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router";
import { Alert, Card, Chip, EmptyState } from "../../src/components/Card";
import { errorMessage } from "../../src/state/useAsync";
import { combatPath, partyCombat } from "../api/overrides/combat";
import { listEncounters, watchEncounters, type EncounterSummary } from "../api/overrides/encounters";
import { listParties, type PartySummary } from "../api/overrides/parties";
import { StartCombatButton } from "../combat/dm/StartCombatButton";
import { encounterPath } from "./EncountersPage";
import "./party.css";
import "./encounters.css";

interface PartyRun {
  party: PartySummary;
  encounters: EncounterSummary[];
  /** The party's live combat, if any (one at a time per party). */
  combat: string | null;
}

/** Every party this account runs, with its encounters and its live combat. */
async function loadRuns(): Promise<PartyRun[]> {
  const list = await listParties();
  const mine = (list?.parties ?? []).filter((p) => p.role === "dm");
  return Promise.all(mine.map(async (party) => {
    const l = await listEncounters(party.id).catch(() => null);
    const encounters = l?.role === "dm" ? l.encounters : [];
    const combat = encounters.some((e) => e.status === "live") ? await partyCombat(party.id).catch(() => null) : null;
    return { party, encounters, combat };
  }));
}

/**
 * The Combat main tab (0.3.1, plan 4.5b): where the DM runs the encounters built on the Encounters tab, with no menu
 * in a menu. Live combats come first (Resume), then every ready encounter (Start combat), party by party; drafts are
 * named with a way back to the builder. Picking one opens the tracker (`/combat/<id>`), full width on a big screen.
 * Players run their side from the Play tab's Combat sub-tab.
 */
export function CombatHome() {
  const [runs, setRuns] = useState<PartyRun[] | undefined>(undefined);
  const [error, setError] = useState("");
  const load = useCallback(async () => {
    try {
      setRuns(await loadRuns());
      setError("");
    } catch (e) { setError(errorMessage(e)); }
  }, []);
  useEffect(() => { void load(); }, [load]);
  const ids = (runs ?? []).map((r) => r.party.id).join(",");
  useEffect(() => {
    const stops = ids ? ids.split(",").map((id) => watchEncounters(id, () => { void load(); })) : [];
    return () => { for (const stop of stops) stop(); };
  }, [ids, load]);

  if (error) return <div className="party enc-page"><Alert tone="danger">{error}</Alert></div>;
  if (!runs) return <div className="party enc-page"><p className="muted">Loading your combats…</p></div>;
  if (runs.length === 0) {
    return (
      <div className="party enc-page">
        <EmptyState title="No party to run a combat for yet">
          Combats run encounters of a party you run. <Link to="/parties">Make a party</Link>, then build its encounters on the Encounters tab.
        </EmptyState>
      </div>
    );
  }
  const live = runs.flatMap((r) => r.encounters.filter((e) => e.status === "live").map((e) => ({ ...r, encounter: e })));
  const ready = runs.flatMap((r) => r.encounters.filter((e) => e.status === "ready").map((e) => ({ ...r, encounter: e })));
  const drafts = runs.flatMap((r) => r.encounters.filter((e) => e.status === "draft").map((e) => ({ ...r, encounter: e })));
  const many = runs.length > 1;
  const sub = (party: PartySummary, e: EncounterSummary) => [
    many ? party.name : null,
    e.creatureCount === 1 ? "1 creature" : `${e.creatureCount} creatures`,
    e.visibleToPlayers ? "shown to players" : "hidden from players",
  ].filter(Boolean).join(" · ");

  return (
    <div className="party enc-page cbh">
      <Card title={`Live now (${live.length})`} aria-label="Live now" className="enc-card cbh-live">
        {live.length === 0
          ? <p className="muted party-note">No combat is running. Start one from a ready encounter below.</p>
          : (
            <ul className="party-list cbh-grid">
              {live.map(({ party, encounter: e, combat }) => (
                <li key={e.id} className="cbh-item is-live" aria-label={e.name}>
                  <span className="enc-row-text grow">
                    <strong>{e.name}</strong>
                    <span className="muted party-note">{sub(party, e)}</span>
                  </span>
                  <Chip tone="danger">Live</Chip>
                  {combat
                    ? <Link className="btn btn-primary" to={combatPath(combat)} aria-label={`Resume ${e.name}`}>Resume</Link>
                    : <span className="muted party-note">Its combat isn't there yet.</span>}
                </li>
              ))}
            </ul>
          )}
      </Card>
      <Card title={`Ready to run (${ready.length})`} aria-label="Ready to run" className="enc-card">
        {ready.length === 0
          ? <p className="muted party-note">Set an encounter's status to Ready on the Encounters tab to run it here.</p>
          : (
            <ul className="party-list cbh-grid">
              {ready.map(({ party, encounter: e, combat }) => (
                <li key={e.id} className="cbh-item" aria-label={e.name}>
                  <span className="enc-row-text grow">
                    <strong>{e.name}</strong>
                    <span className="muted party-note">{sub(party, e)}</span>
                  </span>
                  {combat
                    ? <span className="muted party-note">{party.name} has a combat running.</span>
                    : <StartCombatButton encounterId={e.id} />}
                </li>
              ))}
            </ul>
          )}
      </Card>
      {drafts.length > 0 && (
        <Card title={`Drafts (${drafts.length})`} aria-label="Drafts" className="enc-card">
          <ul className="party-list cbh-grid">
            {drafts.map(({ party, encounter: e }) => (
              <li key={e.id} className="cbh-item is-draft">
                <span className="enc-row-text grow">
                  <strong>{e.name}</strong>
                  <span className="muted party-note">{sub(party, e)}</span>
                </span>
                <Link className="btn btn-sm" to={encounterPath(party.id, e.id)} aria-label={`Finish ${e.name} in Encounters`}>Finish it</Link>
              </li>
            ))}
          </ul>
        </Card>
      )}
    </div>
  );
}
