import { type FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { Link, Navigate, useParams } from "react-router";
import type { CharacterSummary } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip, EmptyState } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { useToast } from "../../src/components/Toast";
import { useApi } from "../../src/state/ApiContext";
import { errorMessage } from "../../src/state/useAsync";
import {
  acceptInvite, addOwnCharacter, cancelInvite, CONFLICT, charactersInParties, createParty, declineInvite, deleteParty,
  keepJoinCode, keptJoinCode, listMyInvites, loadParties, MAX_PARTIES, onPartiesChanged, redeemJoinCode, removeMember,
  renameParty, type MyInvite, type Party, type PartyMember, type Ruleset,
} from "../api/overrides/parties";
import { InviteDialog, NameDialog, PickCharacterDialog } from "./PartiesDialogs";
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
  | { kind: "invite"; party: Party }
  | { kind: "pick"; invite: MyInvite }
  | { kind: "add-own"; party: Party };

const day = (at: number) => new Date(at).toLocaleDateString();
const EDITION: Record<Ruleset, string> = { dnd2024: "2024 rules", dnd2014: "2014 rules" };
const memberName = (m: PartyMember) => m.name || (m.state === "missing" ? "A deleted character" : "An unreadable character");

/**
 * A member's roster card: what the server reads from the saved file (plan 3.5), read-only. Class and the numbers
 * (AC, HP, passive Perception, spell DC) come with 0.2.1's card cache.
 */
function MemberRow({ member, dm, onRemove, onLeave }: { member: PartyMember; dm: boolean; onRemove: () => void; onLeave: () => void }) {
  const who = member.mine ? "your character" : member.dmCharacter ? `the DM's character (${member.ownerName})` : member.ownerName;
  const facts = [member.build, member.level != null ? `Level ${member.level}` : null, member.ruleset ? EDITION[member.ruleset] : null, who]
    .filter(Boolean).join(" · ");
  return (
    <li className="party-member" aria-label={memberName(member)}>
      <div className="party-member-head">
        <span className="grow">
          <strong>{memberName(member)}</strong>
          <span className="muted party-note">{facts}</span>
        </span>
        {member.state === "unreadable" && <Chip tone="warning" title="The file is there but can't be read">Unreadable</Chip>}
        {member.state === "missing" && <Chip tone="warning" title="The character's file is gone">Missing</Chip>}
        {member.mine && member.ownerId && <Button small onClick={onLeave}>Leave</Button>}
        {dm && !member.mine && member.ownerId && <Button small variant="danger" onClick={onRemove}>Remove</Button>}
      </div>
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
  return (
    <Card className={`party-card ${dm ? "party-card-dm" : "party-card-player"}`} aria-label={party.name} title={party.name}
      actions={<Chip tone={dm ? "gold" : "accent"}>{dm ? "DM" : "Player"}</Chip>}>
      <p className="muted party-note">{dm ? EDITION[party.ruleset] : `DM: ${party.dmName} · ${EDITION[party.ruleset]}`}</p>
      {dm && (
        <div className="party-actions">
          <Button small icon="send" variant="primary" onClick={() => open({ kind: "invite", party })}>Invite</Button>
          <Button small icon="plus" onClick={() => open({ kind: "add-own", party })}>Add my character</Button>
          <Button small icon="pen" onClick={() => open({ kind: "rename", party })}>Rename</Button>
          <Button small icon="trash" variant="danger" onClick={() => void destroy()}>Delete</Button>
        </div>
      )}
      <h3 className="party-sub">Characters</h3>
      {party.members.length === 0
        ? <p className="muted">{dm ? "No characters yet. Invite your players." : "No characters yet."}</p>
        : (
          <ul className="party-list">
            {party.members.map((m) => (
              <MemberRow key={`${m.ownerName}/${m.characterId}`} member={m} dm={dm} onRemove={() => void remove(m)} onLeave={() => void leave(m)} />
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
  // No live roster before 0.2.1: catch up when the page is looked at again.
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

      {dialog?.kind === "create" && (
        <NameDialog title="New party" action="Create" onClose={() => setDialog(null)}
          onSave={async (name) => { const p = await createParty(name); toast.push(`${p.name} was created.`, "info"); }} />
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
