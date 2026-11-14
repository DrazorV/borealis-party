import { type FormEvent, type ReactNode, useCallback, useEffect, useState } from "react";
import { Link, Navigate, useNavigate, useParams } from "react-router";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip, EmptyState } from "../../src/components/Card";
import { Modal } from "../../src/components/Modal";
import { errorMessage } from "../../src/state/useAsync";
import {
  createEncounter, LIMITS, listEncounters, seeEncounter, STATUS_WORD, watchEncounters, type EncounterList, type SeenEncounter,
} from "../api/overrides/encounters";
import { loadParty, type Party } from "../api/overrides/parties";
import { EncounterStatBlock } from "./EncounterStatBlock";
import "./party.css";
import "./encounters.css";

/**
 * The DM's encounters live under the Encounters main tab (0.3.1, entry/encountersTab.ts): `/encounters/<party>` and
 * `/encounters/<party>/<encounter>`. The 0.3.0 addresses under `/parties/<party>/encounters` lead there for a DM, and
 * stay the players' page of what their DM showed them.
 */
export const encountersPath = (partyId: string) => `/encounters/${encodeURIComponent(partyId)}`;
export const encounterPath = (partyId: string, id: string) => `${encountersPath(partyId)}/${encodeURIComponent(id)}`;
/** The players' page of a party's shown encounters (and the 0.3.0 address of the DM's list). */
export const shownEncountersPath = (partyId: string) => `/parties/${encodeURIComponent(partyId)}/encounters`;

/** 0.3.0's address of one encounter in the builder: only a DM ever had it, and it now lives under the Encounters tab. */
export function OldEncounterRoute() {
  const { party = "", encounter = "" } = useParams();
  return <Navigate to={encounterPath(party, encounter)} replace />;
}

/** A new encounter: its name for the DM, and the name players will see. */
function NewEncounterDialog({ partyId, onClose }: { partyId: string; onClose: () => void }) {
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [publicName, setPublicName] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    setError("");
    try {
      const made = await createEncounter(partyId, { name: name.trim(), ...(publicName.trim() ? { publicName: publicName.trim() } : {}) });
      onClose();
      void navigate(encounterPath(partyId, made.id));
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };
  return (
    <Modal title="New encounter" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || !name.trim()} onClick={() => void submit()}>Create</Button>
      </>}>
      <form className="party-form" onSubmit={(e) => void submit(e)}>
        <label className="party-field">
          Name (only you see it)
          <input className="input" aria-label="Encounter name" maxLength={LIMITS.name} value={name} onChange={(e) => setName(e.target.value)} required />
        </label>
        <label className="party-field">
          Name players see
          <input className="input" aria-label="Name players see" maxLength={LIMITS.publicName} placeholder="The same name" value={publicName} onChange={(e) => setPublicName(e.target.value)} />
        </label>
        <p className="muted party-note">New encounters are drafts: players see nothing until you show it to them.</p>
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
    </Modal>
  );
}

/**
 * A party's encounters for its DM (plan 4.3): drafts and ready ones, how many creatures each has and how many are
 * hidden, and a button for a new one. No XP or difficulty (the user's D16). `current`: the one open beside it (the
 * Encounters tab's left column on a big screen).
 */
export function DmEncounterList({ partyId, list, current, onNew, tools }: {
  partyId: string;
  list: Extract<EncounterList, { role: "dm" }>;
  current?: string;
  onNew: () => void;
  /** More buttons for the heading row, before New encounter (the Assistant). */
  tools?: ReactNode;
}) {
  return (
    <Card title="Encounters" className="enc-list-card" actions={(tools || list.encounters.length < 50) ? (
      <span className="enc-list-actions">
        {tools}
        {list.encounters.length < 50 && <Button small icon="plus" variant="primary" onClick={onNew}>New encounter</Button>}
      </span>) : undefined}>
      {list.encounters.length === 0 && <EmptyState title="No encounters yet">Build one from your monsters, then show it to your players when it's ready.</EmptyState>}
      <ul className="party-list enc-list">
        {list.encounters.map((e) => (
          <li key={e.id}>
            <Link className={`enc-row ${e.id === current ? "is-current" : ""}`} to={encounterPath(partyId, e.id)} aria-label={`Open ${e.name}`}
              aria-current={e.id === current ? "page" : undefined}>
              <span className="grow enc-row-text">
                <strong>{e.name}</strong>
                <span className="muted party-note">
                  {e.creatureCount === 1 ? "1 creature" : `${e.creatureCount} creatures`}
                  {e.hiddenCount > 0 ? ` (${e.hiddenCount} hidden)` : ""}
                  {e.publicName !== e.name ? ` · players see "${e.publicName}"` : ""}
                </span>
              </span>
              <span className="enc-row-chips">
                <Chip tone={e.status === "draft" ? "default" : e.status === "live" ? "danger" : "accent"}>{STATUS_WORD[e.status]}</Chip>
                {e.visibleToPlayers && <Chip tone="gold" title="Players see this encounter">Shown</Chip>}
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </Card>
  );
}

/** A party's encounters with their live updates, for the Encounters tab and the players' page. */
export function usePartyEncounters(partyId: string) {
  const [list, setList] = useState<EncounterList | null | undefined>(undefined);
  const [error, setError] = useState("");
  const load = useCallback(async () => {
    try {
      setList(await listEncounters(partyId));
      setError("");
    } catch (e) { setError(errorMessage(e)); }
  }, [partyId]);
  useEffect(() => { setList(undefined); void load(); }, [load]);
  useEffect(() => watchEncounters(partyId, () => { void load(); }), [partyId, load]);
  return { list, error, reload: load };
}

/** "New encounter", for the Encounters tab. */
export { NewEncounterDialog };

/**
 * A party's shown encounters, as a player sees them (the server decides which). Its DM is sent on to the Encounters tab,
 * where 0.3.0's list now lives.
 */
export function EncountersPage() {
  const { party: partyId = "" } = useParams();
  const [party, setParty] = useState<Party | null | undefined>(undefined);
  const [list, setList] = useState<EncounterList | null | undefined>(undefined);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    try {
      const [p, l] = await Promise.all([loadParty(partyId), listEncounters(partyId)]);
      setParty(p);
      setList(l);
      setError("");
    } catch (e) { setError(errorMessage(e)); }
  }, [partyId]);
  useEffect(() => { void load(); }, [load]);
  useEffect(() => watchEncounters(partyId, () => { void load(); }), [partyId, load]);

  if (party === null || list === null) {
    return <div className="party enc-page"><Alert tone="info">This party isn't there any more, or it isn't yours.</Alert><Link className="btn" to="/parties">Parties</Link></div>;
  }
  if (list?.role === "dm") return <Navigate to={encountersPath(partyId)} replace />;
  return (
    <div className="party enc-page">
      <div className="party-line enc-head">
        <Link className="btn btn-sm" to="/parties" aria-label="Back to parties">Parties</Link>
        <h2 className="grow enc-title">{party ? `${party.name}: encounters` : "Encounters"}</h2>
      </div>
      {error && <Alert tone="danger">{error}</Alert>}
      {list === undefined && !error && <p className="muted">Loading encounters…</p>}
      {list?.role === "player" && (
        <Card title="Encounters">
          {list.encounters.length === 0
            ? <p className="muted">Your DM hasn't shown you an encounter yet.</p>
            : <ul className="party-list">{list.encounters.map((e) => <li key={e.id}><SeenEncounterCard id={e.id} /></li>)}</ul>}
        </Card>
      )}
    </div>
  );
}

/** A shown encounter as a player sees it: its name and the creatures the DM revealed, each with its stat block. */
export function SeenEncounterCard({ id }: { id: string }) {
  const [seen, setSeen] = useState<SeenEncounter | null | undefined>(undefined);
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    seeEncounter(id).then((s) => { if (live) setSeen(s); }, () => { if (live) setSeen(null); });
    return () => { live = false; };
  }, [id]);
  if (!seen) return seen === null ? null : <p className="muted">Loading…</p>;
  const creature = seen.creatures.find((c) => c.id === open);
  return (
    <div className="party-member enc-seen" aria-label={seen.publicName}>
      <div className="party-member-head">
        <strong className="grow">{seen.publicName}</strong>
        <Chip tone={seen.status === "live" ? "danger" : "accent"}>{STATUS_WORD[seen.status]}</Chip>
      </div>
      {seen.creatures.length === 0
        ? <p className="muted party-note">Nothing has shown itself yet.</p>
        : (
          <ul className="party-list">
            {seen.creatures.map((c) => (
              <li key={c.id} className="party-line enc-seen-row">
                <span className="grow">
                  <strong>{c.publicName}</strong>
                  <span className="muted party-note">{[c.size.join(" or "), c.type.toLowerCase()].filter(Boolean).join(" ")}</span>
                </span>
                {c.kind === "companion" && <Chip tone="info">Companion</Chip>}
                <Button small onClick={() => setOpen(c.id)} aria-label={`Stat block of ${c.publicName}`}>Stat block</Button>
              </li>
            ))}
          </ul>
        )}
      {creature && (
        <Modal title={creature.publicName} onClose={() => setOpen(null)} footer={<Button onClick={() => setOpen(null)}>Close</Button>}>
          <EncounterStatBlock title={creature.publicName} block={creature.statBlock} hp={creature.hp} />
        </Modal>
      )}
    </div>
  );
}

/**
 * On a party card: the DM's way into its encounters, and for players, "Next: <name>" once the DM shows one (the
 * server lists only shown encounters to players, live first).
 */
export function EncounterTeaser({ partyId, dm }: { partyId: string; dm: boolean }) {
  const [next, setNext] = useState<{ id: string; publicName: string; status: string } | null>(null);
  const load = useCallback(() => {
    if (dm) return;
    listEncounters(partyId).then((l) => setNext(l?.role === "player" ? l.encounters[0] ?? null : null), () => setNext(null));
  }, [partyId, dm]);
  useEffect(() => { load(); }, [load]);
  useEffect(() => (dm ? undefined : watchEncounters(partyId, () => load())), [partyId, dm, load]);
  if (dm) {
    return <Link className="btn btn-sm enc-link" to={encountersPath(partyId)}>Encounters</Link>;
  }
  if (!next) return null;
  return (
    <Link className="enc-next" to={shownEncountersPath(partyId)} aria-label={`Next: ${next.publicName}`}>
      <span className="enc-next-label">Next:</span> <strong>{next.publicName}</strong>
      {next.status === "live" && <Chip tone="danger">Live</Chip>}
    </Link>
  );
}
