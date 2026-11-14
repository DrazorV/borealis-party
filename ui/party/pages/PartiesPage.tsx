import { type FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, Navigate, useParams } from "react-router";
import type { CharacterSummary } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip, EmptyState } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { useToast } from "../../src/components/Toast";
import { useApi } from "../../src/state/ApiContext";
import { errorMessage } from "../../src/state/useAsync";
import {
  acceptInvite, addOwnCharacter, BAND_LABEL, cancelInvite, classLine, CONFLICT, charactersInParties, createParty,
  declineInvite, deleteParty, EDITION_LABEL, editionToSwitchTo, keepJoinCode, keptJoinCode, listMyInvites, loadParties,
  loadParty, MAX_PARTIES, newestEdition, onPartiesChanged, redeemJoinCode, removeMember, renameParty, watchParties,
  type CardStats, type HealthBand, type MyInvite, type Party, type PartyMember,
} from "../api/overrides/parties";
import { EncounterTeaser } from "./EncountersPage";
import { InviteDialog, NameDialog, NewPartyDialog, PickCharacterDialog } from "./PartiesDialogs";
import { ChangeEditionDialog } from "./PartyEdition";
import "./party.css";

/** What the page loaded. */
interface Loaded {
  dm: boolean;
  /** The server has parties switched off (its config). */
  off: boolean;
  parties: Party[];
  invites: MyInvite[];
  characters: CharacterSummary[];
}

/** One dialog at a time. */
type Open =
  | { kind: "create" }
  | { kind: "rename"; party: Party }
  | { kind: "edition"; party: Party }
  | { kind: "invite"; party: Party }
  | { kind: "pick"; invite: MyInvite }
  | { kind: "add-own"; party: Party };

const day = (at: number) => new Date(at).toLocaleDateString();
const EDITION = EDITION_LABEL;
const memberName = (m: PartyMember) => m.name || (m.state === "missing" ? "A deleted character" : "An unreadable character");

const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);
const BAND_TONE: Record<HealthBand, "success" | "warning" | "danger"> = { unhurt: "success", bloodied: "warning", down: "danger" };

/** The band the DM's numbers put a character in, so the DM's card reads like the players' at a glance. */
export function bandOf(stats: CardStats): HealthBand | null {
  if (stats.hpMax === 0) return null;
  if (stats.hp === 0) return "down";
  return stats.hp * 2 <= stats.hpMax ? "bloodied" : "unhurt";
}

/** The numbers a DM needs at the table: hit points first, then AC, initiative, passive Perception, spell DCs. */
function StatLine({ stats }: { stats: CardStats }) {
  const band = bandOf(stats);
  const dcs = stats.spellDcs.map((d) => d.dc);
  return (
    <div className="party-stats" role="list" aria-label="Numbers">
        <span role="listitem" className={`party-stat party-stat-hp ${band ?? ""}`} aria-label={`Hit points ${stats.hp} of ${stats.hpMax}${stats.tempHp ? `, ${stats.tempHp} temporary` : ""}`}>
          <span className="party-stat-label">HP</span>
          <span className="party-stat-value">{stats.hp}<span className="muted">/{stats.hpMax}</span>{stats.tempHp > 0 && <span className="party-stat-temp"> +{stats.tempHp}</span>}</span>
        </span>
        <span role="listitem" className="party-stat" aria-label={`Armor class ${stats.armorClass}`}><span className="party-stat-label">AC</span><span className="party-stat-value">{stats.armorClass}</span></span>
        <span role="listitem" className="party-stat" aria-label={`Initiative ${signed(stats.initiative)}`}><span className="party-stat-label">Init</span><span className="party-stat-value">{signed(stats.initiative)}</span></span>
        <span role="listitem" className="party-stat" aria-label={`Passive Perception ${stats.passivePerception}`}><span className="party-stat-label">PP</span><span className="party-stat-value">{stats.passivePerception}</span></span>
        {dcs.length > 0 && (
          <span role="listitem" className="party-stat" aria-label={`Spell save DC ${dcs.join(", ")}`}><span className="party-stat-label">DC</span><span className="party-stat-value">{dcs.join("/")}</span></span>
        )}
    </div>
  );
}

/** Concentration, exhaustion and inspiration, under the numbers. */
function Conditions({ stats }: { stats: CardStats }) {
  if (!stats.concentration && !(stats.exhaustion > 0) && !stats.inspiration) return null;
  return (
    <div className="party-conditions">
      {stats.concentration && <Chip tone="info">Concentrating: {stats.concentration}</Chip>}
      {stats.exhaustion > 0 && <Chip tone="warning">Exhaustion {stats.exhaustion}</Chip>}
      {stats.inspiration && <Chip tone="gold">Inspiration</Chip>}
    </div>
  );
}

/**
 * A member's roster card, as the server projects it for this account (plan 3.5, D6): the DM and the owner see the
 * numbers, live as the owner plays; another member sees the name, class and level, and during a combat a health
 * band. Read-only.
 */
function MemberRow({ member, dm, partyId, onRemove, onLeave }: {
  member: PartyMember; dm: boolean; partyId: string; onRemove: () => void; onLeave: () => void;
}) {
  const who = member.mine ? "your character" : member.dmCharacter ? `the DM's character (${member.ownerName})` : member.ownerName;
  const classes = classLine(member.classes);
  const facts = [classes || (member.level != null ? `Level ${member.level}` : null), member.ruleset ? EDITION[member.ruleset] : null, who]
    .filter(Boolean).join(" · ");
  const band = member.band ?? (member.stats ? bandOf(member.stats) : null);
  return (
    <li className="party-member" aria-label={memberName(member)}>
      <div className="party-member-head">
        <span className="grow">
          <strong>{memberName(member)}</strong>
          <span className="muted party-note">{facts}</span>
        </span>
        {band && <Chip tone={BAND_TONE[band]}>{BAND_LABEL[band]}</Chip>}
        {dm && !member.mine && member.online === false && <Chip title="The player doesn't have the app open: these are the last numbers seen">Offline</Chip>}
        {member.state === "unreadable" && <Chip tone="warning" title="The file is there but can't be read">Unreadable</Chip>}
        {member.state === "missing" && <Chip tone="warning" title="The character's file is gone">Missing</Chip>}
      </div>
      <div className="party-member-body">
      {member.stats && <StatLine stats={member.stats} />}
      {(member.ownerId && (dm || member.mine)) && (
        <div className="party-member-actions">
          {dm && member.state === "ok" && (
            <Link className="btn btn-sm" to={`/parties/${encodeURIComponent(partyId)}/view/${encodeURIComponent(member.ownerId)}/${encodeURIComponent(member.characterId)}`}
              aria-label={`View ${memberName(member)}`}>View</Link>
          )}
          {member.mine && <Button small onClick={onLeave}>Leave</Button>}
          {dm && !member.mine && <Button small variant="danger" onClick={onRemove}>Remove</Button>}
        </div>
      )}
      </div>
      {member.stats && <Conditions stats={member.stats} />}
    </li>
  );
}

/** One party: the DM's view (manage it) or a player's (see it, leave it). Same page, per party (plan 3.1). */
function PartyCard({ party, act, open }: {
  party: Party;
  act: (work: () => Promise<unknown>, done?: string) => Promise<void>;
  open: (o: Open) => void;
}) {
  const confirm = useConfirm();
  const dm = party.role === "dm";
  const remove = async (m: PartyMember) => {
    const answer = await confirm({
      title: `Remove ${memberName(m)}?`, danger: true, confirmLabel: "Remove",
      body: `${memberName(m)} leaves ${party.name}. ${m.ownerName} keeps the character and can join again with a new invite.`,
    });
    if (answer === "confirm" && m.ownerId) await act(() => removeMember(party.id, m.ownerId!, m.characterId), `${memberName(m)} was removed.`);
  };
  const leave = async (m: PartyMember) => {
    const answer = await confirm({
      title: `Leave ${party.name}?`, danger: true, confirmLabel: "Leave",
      body: `${memberName(m)} leaves the party. You keep the character, and it can join another party.`,
    });
    if (answer === "confirm" && m.ownerId) await act(() => removeMember(party.id, m.ownerId!, m.characterId), `${memberName(m)} left ${party.name}.`);
  };
  const destroy = async () => {
    const others = party.members.filter((m) => !m.mine).length;
    const answer = await confirm({
      title: `Delete ${party.name}?`, danger: true, confirmLabel: "Delete party",
      body: others > 0
        ? `The party ends for its ${others === 1 ? "one other character" : `${others} other characters`}. Everyone keeps their characters. This can't be undone.`
        : "Everyone keeps their characters. This can't be undone.",
    });
    if (answer === "confirm") await act(() => deleteParty(party.id), `${party.name} was deleted.`);
  };
  const invites = party.invites ?? [];
  const mismatch = editionToSwitchTo(party);
  return (
    <Card className={`party-card ${dm ? "party-card-dm" : "party-card-player"}`} aria-label={party.name}>
      {/* One header row: the name, the edition (with Change edition inline for the DM) and the role badge. */}
      <header className="party-card-head">
        <h2 className="party-card-name">{party.name}</h2>
        <span className="party-edition-line">
          <span className="muted">Edition:</span> <strong>{EDITION[party.ruleset]}</strong>
          {dm && <button type="button" className="party-link" onClick={() => open({ kind: "edition", party })}>Change edition</button>}
        </span>
        <span className="party-card-badge"><Chip tone={dm ? "gold" : "accent"}>{dm ? "DM" : "Player"}</Chip></span>
      </header>
      {(mismatch || !dm) && (
        <p className="muted party-note party-card-meta">
          {!dm && <>DM: {party.dmName}</>}
          {mismatch && <>{!dm ? " · " : ""}Most of its characters use {EDITION[mismatch]}.</>}
        </p>
      )}
      {/* The DM's one toolbar: Encounters first, Delete apart on the right. A player sees the next encounter. */}
      {dm ? (
        <div className="party-actions" role="toolbar" aria-label={`${party.name} actions`}>
          <EncounterTeaser partyId={party.id} dm />
          <Button small icon="send" variant="primary" onClick={() => open({ kind: "invite", party })}>Invite</Button>
          <Button small icon="plus" onClick={() => open({ kind: "add-own", party })}>Add my character</Button>
          <Button small icon="pen" onClick={() => open({ kind: "rename", party })}>Rename</Button>
          <Button small icon="trash" variant="danger" className="party-actions-end" onClick={() => void destroy()}>Delete</Button>
        </div>
      ) : <EncounterTeaser partyId={party.id} dm={false} />}
      <h3 className="party-sub">Characters</h3>
      {party.members.length === 0
        ? <p className="muted">{dm ? "No characters yet. Invite your players." : "No characters yet."}</p>
        : (
          <ul className="party-list">
            {party.members.map((m) => (
              <MemberRow key={`${m.ownerName}/${m.characterId}`} member={m} dm={dm} partyId={party.id} onRemove={() => void remove(m)} onLeave={() => void leave(m)} />
            ))}
          </ul>
        )}
      {dm && invites.length > 0 && (
        <>
          <h3 className="party-sub">Open invites</h3>
          <ul className="party-list">
            {invites.map((inv) => (
              <li key={inv.id} className="party-line party-invite">
                <span className="grow">
                  {inv.kind === "username" ? <strong>{inv.username}</strong> : <strong>Join code</strong>}
                  <span className="muted party-note">
                    {inv.kind === "code" ? `${inv.usesLeft ?? 0} ${inv.usesLeft === 1 ? "use" : "uses"} left · ` : ""}until {day(inv.expiresAt)}
                  </span>
                </span>
                <Button small onClick={() => void act(() => cancelInvite(party.id, inv.id), "Invite withdrawn.")}>Withdraw</Button>
              </li>
            ))}
          </ul>
        </>
      )}
    </Card>
  );
}

/**
 * Parties (plan 4.1): the ones this account runs as DM and the ones its characters are in, the invites waiting for it,
 * and a join code box. DM tools appear once the account turns them on (Account page, plan D2). Party data is
 * online-only: the app shows the offline page instead (entry/App.tsx).
 *
 * It is live (plan 3.7): a party event refetches that party when its `pseq` is newer than the one shown, a change to
 * the account's own list or a resync (or a stream that reopened) reloads the page.
 */
export function PartiesPage() {
  const api = useApi();
  const toast = useToast();
  const [data, setData] = useState<Loaded | null>(null);
  const [loadError, setLoadError] = useState("");
  const [dialog, setDialog] = useState<Open | null>(null);
  const [code, setCode] = useState(() => keptJoinCode());
  const [joining, setJoining] = useState(false);

  const load = useCallback(async () => {
    try {
      const [list, characters] = await Promise.all([loadParties(), api.listCharacters()]);
      const invites = list.off ? [] : await listMyInvites();
      setData({ dm: list.dm, off: Boolean(list.off), parties: list.parties, invites, characters });
      setLoadError("");
    } catch (e) { setLoadError(errorMessage(e)); }
  }, [api]);
  useEffect(() => { void load(); }, [load]);
  useEffect(() => onPartiesChanged(() => { void load(); }), [load]);

  const shown = useRef<Loaded | null>(null);
  shown.current = data;
  /** One party read again; an answer older than what is shown is dropped, one for a party now gone removes it. */
  const refreshParty = useCallback(async (id: string) => {
    try {
      const fresh = await loadParty(id);
      setData((d) => {
        if (!d) return d;
        if (!fresh) return { ...d, parties: d.parties.filter((p) => p.id !== id) };
        return { ...d, parties: d.parties.map((p) => (p.id === id && (p.pseq ?? -1) <= (fresh.pseq ?? 0) ? fresh : p)) };
      });
    } catch { /* the next event or a reload catches up */ }
  }, []);
  useEffect(() => watchParties((e) => {
    if (e.type !== "party") { void load(); return; }
    const known = shown.current?.parties.find((p) => p.id === e.party);
    if (!known) { void load(); return; }
    if ((known.pseq ?? -1) < e.pseq) void refreshParty(e.party);
  }), [load, refreshParty]);
  // Catch up when the page is looked at again (a phone that slept may have missed events).
  useEffect(() => {
    const seen = () => { if (document.visibilityState === "visible") void load(); };
    document.addEventListener("visibilitychange", seen);
    return () => document.removeEventListener("visibilitychange", seen);
  }, [load]);

  const act = useCallback(async (work: () => Promise<unknown>, done?: string) => {
    try {
      await work();
      if (done) toast.push(done, "info");
    } catch (e) {
      toast.push(errorMessage(e), "error");
      void load();
    }
  }, [toast, load]);

  const taken = useMemo(() => {
    const names = new Map<string, string>();
    for (const [id, p] of charactersInParties(data?.parties ?? [])) names.set(id, p.name);
    return names;
  }, [data]);

  const join = async (e: FormEvent) => {
    e.preventDefault();
    if (!code.trim()) return;
    setJoining(true);
    try {
      const invite = await redeemJoinCode(code);
      keepJoinCode(null);
      setCode("");
      setDialog({ kind: "pick", invite });
    } catch (err) {
      toast.push((err as { code?: string }).code === "not_found" ? "That code isn't valid, or it has expired or run out." : errorMessage(err), "error");
    } finally { setJoining(false); }
  };

  /** A refusal because the character is in a party already (plan D19) shows in the dialog, and the page catches up. */
  const picked = async (work: () => Promise<unknown>, done: string) => {
    try {
      await work();
      toast.push(done, "info");
    } catch (err) {
      if ((err as { code?: string }).code === CONFLICT) void load();
      throw err;
    }
  };

  if (data?.off) {
    return (
      <div className="party party-parties">
        <Alert tone="info">Parties are switched off on this server.</Alert>
      </div>
    );
  }

  const parties = data?.parties ?? [];
  const running = parties.filter((p) => p.role === "dm");
  const playing = parties.filter((p) => p.role === "player");

  return (
    <div className="party party-parties">
      <Card title="Join a party">
        <form className="party-inline" onSubmit={(e) => void join(e)}>
          <input className="input party-code-input" aria-label="Join code" placeholder="Join code" autoCapitalize="characters" autoComplete="off" spellCheck={false}
            value={code} onChange={(e) => setCode(e.target.value)} />
          <Button type="submit" variant="primary" disabled={joining || !code.trim()}>Join</Button>
        </form>
        <p className="muted party-note">Got a code from your DM? Enter it, then pick the character that joins.</p>
      </Card>

      {loadError && <Alert tone="danger">{loadError}</Alert>}
      {!data && !loadError && <p className="muted">Loading parties…</p>}

      {data && data.invites.length > 0 && (
        <Card title="Invitations">
          <ul className="party-list">
            {data.invites.map((inv) => (
              <li key={inv.id} className="party-line party-invite" aria-label={`Invite to ${inv.party.name}`}>
                <span className="grow">
                  <strong>{inv.party.name}</strong>
                  <span className="muted party-note">
                    DM: {inv.party.dmName} · {inv.kind === "code" ? "your join code" : "sent to you"} · until {day(inv.expiresAt)}
                  </span>
                </span>
                <Button small onClick={() => void act(() => declineInvite(inv.id), "Invite declined.")}>Decline</Button>
                <Button small variant="primary" onClick={() => setDialog({ kind: "pick", invite: inv })}>Accept</Button>
              </li>
            ))}
          </ul>
        </Card>
      )}

      {data?.dm && (
        <Card title="Parties I run"
          actions={running.length < MAX_PARTIES ? <Button small icon="plus" variant="primary" onClick={() => setDialog({ kind: "create" })}>New party</Button> : undefined}>
          {running.length === 0 && <EmptyState title="No parties yet">Create one, then invite players by username or with a join code.</EmptyState>}
          {running.length >= MAX_PARTIES && <Alert tone="info">You run {MAX_PARTIES} parties, the most an account can.</Alert>}
        </Card>
      )}
      {running.map((p) => <PartyCard key={p.id} party={p} act={act} open={setDialog} />)}

      {data && (
        <Card title="My parties">
          {playing.length === 0
            ? <p className="muted">None of your characters is in a party yet.</p>
            : <p className="muted party-note">A character can be in one party at a time.</p>}
        </Card>
      )}
      {playing.map((p) => <PartyCard key={p.id} party={p} act={act} open={setDialog} />)}

      {data && !data.dm && (
        <div className="party-line party-dm-offer">
          <span className="grow muted">Want to run a game? Turn on Dungeon Master tools on your Account page.</span>
          <Link className="btn btn-sm" to="/account">Account</Link>
        </div>
      )}
      {data && <p className="muted party-note">Server admins can read party data on the server.</p>}

      {dialog?.kind === "create" && data && (
        <NewPartyDialog initial={newestEdition(data.characters)} onClose={() => setDialog(null)}
          onSave={async (name, ruleset) => { const p = await createParty(name, ruleset); toast.push(`${p.name} was created.`, "info"); }} />
      )}
      {dialog?.kind === "edition" && (
        <ChangeEditionDialog partyId={dialog.party.id} party={dialog.party} onClose={() => setDialog(null)}
          onChanged={(fresh) => setData((d) => (d ? { ...d, parties: d.parties.map((p) => (p.id === fresh.id ? fresh : p)) } : d))} />
      )}
      {dialog?.kind === "rename" && (
        <NameDialog title={`Rename ${dialog.party.name}`} action="Rename" initial={dialog.party.name} onClose={() => setDialog(null)}
          onSave={async (name) => { await renameParty(dialog.party.id, name); }} />
      )}
      {dialog?.kind === "invite" && <InviteDialog party={dialog.party} onClose={() => setDialog(null)} />}
      {dialog?.kind === "pick" && data && (
        <PickCharacterDialog title={`Join ${dialog.invite.party.name}`} characters={data.characters} taken={taken} onClose={() => setDialog(null)}
          onPick={(id) => picked(() => acceptInvite(dialog.invite.id, id), `You joined ${dialog.invite.party.name}.`)} />
      )}
      {dialog?.kind === "add-own" && data && (
        <PickCharacterDialog title={`Add to ${dialog.party.name}`} characters={data.characters} taken={taken} onClose={() => setDialog(null)}
          onPick={(id) => picked(() => addOwnCharacter(dialog.party.id, id), `Your character joined ${dialog.party.name}.`)} />
      )}
    </div>
  );
}

/** `#/join/CODE` opened inside the app: keep the code and show the Parties page with it filled in. */
export function JoinRoute() {
  const { code = "" } = useParams();
  keepJoinCode(code);
  return <Navigate to="/parties" replace />;
}
