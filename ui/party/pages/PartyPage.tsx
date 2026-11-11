import { type FormEvent, useCallback, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip, EmptyState, ProgressBar } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { useToast } from "../../src/components/Toast";
import {
  acceptInvitation, bringCharacter, cancelInvite, changeEmail, changePassword, createGroup, declineInvitation, deleteGroup,
  groupDetail, invitePlayer, listGroups, myCharacters, myInvitations, partyOf, removeMember, renameGroup, withdrawCharacter,
  type GroupDetail, type GroupSummary, type Invitation, type OwnedCharacter, type PartyRow,
} from "../api/party";
import { homeScope, libraryPlace, loadSession, moveTo, updateUser } from "../api/session";
import "./party.css";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

function useLoad<T>(load: () => Promise<T>) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState("");
  const refresh = useCallback(() => {
    load().then((d) => { setData(d); setError(""); }, (e) => setError(message(e)));
  }, [load]);
  useEffect(refresh, [refresh]);
  return { data, error, refresh };
}

export function PartyPage() {
  const { groupId } = useParams();
  return groupId ? <DmGroup groupId={groupId} /> : <Home />;
}

/** Every account's home: the groups it runs, then its own characters and the groups it plays in. */
function Home() {
  const toast = useToast();
  const { data: invitations, refresh: refreshInvitations } = useLoad(myInvitations);
  const { data: groups, error: groupsError, refresh: refreshGroups } = useLoad(listGroups);
  const { data: characters, error, refresh: refreshCharacters } = useLoad(myCharacters);
  const refresh = () => { refreshInvitations(); refreshGroups(); refreshCharacters(); };
  const answer = async (invitation: Invitation, accept: boolean) => {
    try {
      if (accept) await acceptInvitation(invitation.id);
      else await declineInvitation(invitation.id);
      refresh();
    } catch (e) { toast.push(message(e), "error"); }
  };
  const running = groups?.filter((g) => g.isDm) ?? null;
  const playing = groups?.filter((g) => !g.isDm) ?? null;
  return (
    <div className="party">
      {(error || groupsError) && <Alert tone="danger">{error || groupsError}</Alert>}
      {(invitations?.length ?? 0) > 0 && (
        <Card title="Invitations">
          <ul className="party-list">
            {invitations?.map((i) => (
              <li key={i.id} className="party-line">
                <span><strong>{i.groupName}</strong> <span className="muted">from {i.dmName}</span></span>
                <span className="spacer" />
                <Button small variant="primary" onClick={() => void answer(i, true)}>Join</Button>
                <Button small onClick={() => void answer(i, false)}>Decline</Button>
              </li>
            ))}
          </ul>
        </Card>
      )}
      <h2 className="party-half">Groups I run</h2>
      <GroupsIRun groups={running} refresh={refreshGroups} />
      <h2 className="party-half">My characters and groups I play in</h2>
      <MyCharacters characters={characters} />
      <Card title="Groups I play in">
        {playing?.length === 0 && <EmptyState title="No groups yet">When a Dungeon Master invites you, the invitation appears above.</EmptyState>}
        <ul className="party-list">
          {playing?.map((g) => <PlayerGroup key={g.id} group={g} spare={characters?.filter((c) => c.scope === "personal") ?? []} refresh={refresh} />)}
        </ul>
      </Card>
      <MyAccount />
    </div>
  );
}

function GroupsIRun({ groups, refresh }: { groups: GroupSummary[] | null; refresh: () => void }) {
  const nav = useNavigate();
  const toast = useToast();
  const [name, setName] = useState("");

  const create = async (e: FormEvent) => {
    e.preventDefault();
    try {
      const made = await createGroup(name);
      setName("");
      refresh();
      nav(`/party/${made.id}`);
    } catch (err) { toast.push(message(err), "error"); }
  };

  return (
    <Card title="My groups" actions={<Button icon="box" onClick={() => moveTo(libraryPlace, "/content")}>Content library</Button>}>
      {groups?.length === 0 && <EmptyState title="You run no groups yet">Create one below, then invite your players by username. Content you add to your Library can be chosen per group.</EmptyState>}
      <ul className="party-list">
        {groups?.map((g) => (
          <li key={g.id}>
            <button type="button" className="party-row-button" onClick={() => nav(`/party/${g.id}`)}>
              <strong>{g.name}</strong>
              <span className="muted">{g.memberCount} {g.memberCount === 1 ? "player" : "players"}</span>
              <span className="spacer" />
              <Chip tone="gold">Dungeon Master</Chip>
            </button>
          </li>
        ))}
      </ul>
      <form className="party-inline" onSubmit={(e) => void create(e)}>
        <input className="aur-text" aria-label="New group name" placeholder="New group name" value={name} onChange={(e) => setName(e.target.value)} required maxLength={80} />
        <Button type="submit" variant="primary">Create group</Button>
      </form>
    </Card>
  );
}

function MyCharacters({ characters }: { characters: OwnedCharacter[] | null }) {
  const open = (c: OwnedCharacter) => moveTo({ scope: c.scope, scopeLabel: c.groupName ?? "My characters", asDm: false }, "/play", c.id);
  return (
    <Card title="My characters" actions={<Button icon="plus" variant="primary" onClick={() => moveTo(homeScope(), "/characters?new=1")}>New character</Button>}>
      {characters?.length === 0 && <EmptyState title="No characters yet">Characters you build here use the free SRD rules until they join a group.</EmptyState>}
      <ul className="party-list">
        {characters?.map((c) => (
          <li key={c.id}>
            <button type="button" className="party-row-button" onClick={() => open(c)}>
              <strong>{c.name}</strong>
              <span className="muted">Level {c.level}{c.build ? ` ${c.build}` : ""}</span>
              <span className="spacer" />
              {c.groupName ? <Chip tone="accent">{c.groupName}</Chip> : <Chip>Not in a group</Chip>}
            </button>
          </li>
        ))}
      </ul>
    </Card>
  );
}

function MyAccount() {
  const toast = useToast();
  const user = loadSession()?.user;
  const [email, setEmail] = useState(user?.email ?? "");
  const [current, setCurrent] = useState("");
  const [password, setPassword] = useState("");

  const saveEmail = async (e: FormEvent) => {
    e.preventDefault();
    try {
      updateUser(await changeEmail(email));
      toast.push("Email address saved.", "info");
    } catch (err) { toast.push(message(err), "error"); }
  };
  const savePassword = async (e: FormEvent) => {
    e.preventDefault();
    try {
      updateUser(await changePassword(current, password));
      setCurrent("");
      setPassword("");
      toast.push("Password changed. Your other devices are signed out.", "info");
    } catch (err) { toast.push(message(err), "error"); }
  };

  return (
    <Card title="My account">
      <p className="muted">Signed in as <strong>{user?.username}</strong>{user?.admin ? " · admin" : ""}</p>
      {!user?.email && <Alert tone="info">Add an email address, so an admin can reach you.</Alert>}
      <form className="party-inline" onSubmit={(e) => void saveEmail(e)}>
        <input className="aur-text" type="email" aria-label="Email address" placeholder="Email address" autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} required />
        <Button type="submit">Save email</Button>
      </form>
      <form className="party-inline" onSubmit={(e) => void savePassword(e)}>
        <input className="aur-text" type="password" aria-label="Current password" placeholder="Current password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} required />
        <input className="aur-text" type="password" aria-label="New password" placeholder="New password" autoComplete="new-password" minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} required />
        <Button type="submit">Change password</Button>
      </form>
    </Card>
  );
}

function xpProgress(row: PartyRow) {
  const level = row.level;
  if (!level || level.nextLevelXp === null) return null;
  return { value: level.xp - level.currentLevelXp, max: level.nextLevelXp - level.currentLevelXp };
}

function classLine(row: PartyRow): string {
  const classes = row.level?.classes.filter((c) => c.name) ?? [];
  if (classes.length === 0) return "No class yet";
  return classes.map((c) => `${c.name}${c.subclass ? ` (${c.subclass})` : ""} ${c.level}`).join(" / ");
}

function PartyMember({ row, group, onRemove }: { row: PartyRow; group: GroupDetail; onRemove: () => void }) {
  const progress = xpProgress(row);
  const view = () => {
    if (!row.characterId) return;
    moveTo(
      { scope: `group-${group.id}`, scopeLabel: group.name, asDm: true, view: { groupId: group.id, playerId: row.playerId, characterName: row.name ?? row.username } },
      "/play",
      row.characterId,
    );
  };
  return (
    <li className="party-member">
      <div className="party-member-head">
        <div>
          <strong>{row.name ?? "No character yet"}</strong>
          <div className="muted">{row.username}{row.level ? ` · ${classLine(row)}` : ""}</div>
        </div>
        <span className="spacer" />
        {row.level?.levelUpAvailable && <Chip tone="gold">Level up ready</Chip>}
        {row.characterId && !row.unreadable && <Button small onClick={view}>View</Button>}
        <Button small onClick={onRemove}>Remove</Button>
      </div>
      {row.unreadable && <Alert tone="warning">This character's file could not be read.</Alert>}
      {row.level && row.stats && (
        <>
          <dl className="party-stats">
            <div><dt>Level</dt><dd>{row.level.total}</dd></div>
            <div><dt>HP</dt><dd>{row.hitPoints?.current ?? row.stats.maxHp}/{row.hitPoints?.max ?? row.stats.maxHp}{row.hitPoints?.temp ? ` +${row.hitPoints.temp}` : ""}</dd></div>
            <div><dt>AC</dt><dd>{row.stats.armorClass}</dd></div>
            <div><dt>Init</dt><dd>{row.stats.initiative >= 0 ? `+${row.stats.initiative}` : row.stats.initiative}</dd></div>
            <div><dt>Speed</dt><dd>{row.stats.speed}</dd></div>
            <div><dt>Passive Perc.</dt><dd>{row.stats.passivePerception}</dd></div>
          </dl>
          {progress && (
            <div className="party-xp">
              <ProgressBar value={progress.value} max={progress.max} label={`${row.name ?? row.username} experience`} />
              <span className="muted">{row.level.xp} / {row.level.nextLevelXp} XP</span>
            </div>
          )}
          {(row.progress?.remaining ?? 0) > 0 && <p className="muted">{row.progress?.remaining} choices still to make</p>}
        </>
      )}
    </li>
  );
}

/** A group this account runs. */
function DmGroup({ groupId }: { groupId: string }) {
  const nav = useNavigate();
  const toast = useToast();
  const confirm = useConfirm();
  const loadGroup = useCallback(() => groupDetail(groupId), [groupId]);
  const loadParty = useCallback(() => partyOf(groupId), [groupId]);
  const { data: group, error, refresh: refreshGroup } = useLoad(loadGroup);
  const { data: party, refresh: refreshParty } = useLoad(loadParty);
  const [username, setUsername] = useState("");
  const refresh = () => { refreshGroup(); refreshParty(); };
  const guarded = async (work: () => Promise<unknown>) => {
    try { await work(); refresh(); } catch (e) { toast.push(message(e), "error"); }
  };

  if (error) return <div className="party"><Alert tone="danger">{error}</Alert></div>;
  if (!group) return null;
  if (!group.isDm) {
    return (
      <div className="party">
        <Button onClick={() => nav("/party")}>All groups</Button>
        <Alert tone="info">{group.dmName} runs {group.name}. Your character in it is on the party page.</Alert>
      </div>
    );
  }

  const invite = (e: FormEvent) => {
    e.preventDefault();
    void guarded(async () => { await invitePlayer(group.id, username); setUsername(""); });
  };
  const remove = async (row: PartyRow) => {
    const answer = await confirm({ title: "Remove from group", body: `Remove ${row.username} from ${group.name}? Their character goes back to their own list.`, confirmLabel: "Remove", danger: true });
    if (answer === "confirm") void guarded(() => removeMember(group.id, row.playerId));
  };
  const destroy = async () => {
    const answer = await confirm({ title: "Delete group", body: `Delete ${group.name}? Every character goes back to its player.`, confirmLabel: "Delete", danger: true });
    if (answer !== "confirm") return;
    try { await deleteGroup(group.id); nav("/party"); } catch (e) { toast.push(message(e), "error"); }
  };
  const rename = () => {
    const name = window.document.querySelector<HTMLInputElement>("#party-group-name")?.value.trim();
    if (name && name !== group.name) void guarded(() => renameGroup(group.id, name));
  };

  return (
    <div className="party">
      <Button onClick={() => nav("/party")}>All groups</Button>
      <Card
        title={group.name}
        actions={<Button icon="box" onClick={() => moveTo({ scope: `group-${group.id}`, scopeLabel: group.name, asDm: true }, "/content/sources")}>Choose content</Button>}
      >
        {party?.length === 0 && <EmptyState title="Nobody here yet">Invite your players by their username.</EmptyState>}
        <ul className="party-list">
          {party?.map((row) => <PartyMember key={row.playerId} row={row} group={group} onRemove={() => void remove(row)} />)}
        </ul>
      </Card>
      <Card title="Invitations">
        <form className="party-inline" onSubmit={invite}>
          <input className="aur-text" aria-label="Player username" placeholder="Player username" autoCapitalize="none" spellCheck={false} value={username} onChange={(e) => setUsername(e.target.value)} required />
          <Button type="submit" variant="primary">Invite</Button>
        </form>
        <ul className="party-list">
          {group.invites.map((i) => (
            <li key={i.id} className="party-line">
              <span>{i.username}</span><span className="muted">Waiting for an answer</span><span className="spacer" />
              <Button small onClick={() => void guarded(() => cancelInvite(group.id, i.id))}>Cancel</Button>
            </li>
          ))}
        </ul>
      </Card>
      <Card title="Group settings">
        <div className="party-inline">
          <input id="party-group-name" className="aur-text" aria-label="Group name" defaultValue={group.name} maxLength={80} />
          <Button onClick={rename}>Rename</Button>
          <span className="spacer" />
          <Button onClick={() => void destroy()}>Delete group</Button>
        </div>
      </Card>
    </div>
  );
}

/** A group this account plays in. */
function PlayerGroup({ group, spare, refresh }: { group: GroupSummary; spare: OwnedCharacter[]; refresh: () => void }) {
  const toast = useToast();
  const confirm = useConfirm();
  const player = loadSession()?.user.id ?? "";
  const [chosen, setChosen] = useState("");
  const place = { scope: `group-${group.id}`, scopeLabel: group.name, asDm: false };
  const guarded = async (work: () => Promise<unknown>) => {
    try { await work(); refresh(); } catch (e) { toast.push(message(e), "error"); }
  };
  const leave = async () => {
    const answer = await confirm({ title: "Leave group", body: `Leave ${group.name}? Your character goes back to your own list.`, confirmLabel: "Leave", danger: true });
    if (answer === "confirm") void guarded(() => removeMember(group.id, player));
  };
  return (
    <li className="party-member">
      <div className="party-member-head">
        <div><strong>{group.name}</strong><div className="muted">Dungeon Master: {group.dmName}</div></div>
        <span className="spacer" />
        <Button small onClick={() => void leave()}>Leave</Button>
      </div>
      {group.characterId ? (
        <div className="party-inline">
          <Button variant="primary" onClick={() => moveTo(place, "/play", group.characterId ?? undefined)}>Open character</Button>
          <Button onClick={() => void guarded(() => withdrawCharacter(group.id))}>Take out of group</Button>
        </div>
      ) : (
        <div className="party-inline">
          <Button variant="primary" onClick={() => moveTo(place, "/characters?new=1")}>Build a character here</Button>
          {spare.length > 0 && (
            <>
              <select className="aur-text" aria-label="Character to bring" value={chosen} onChange={(e) => setChosen(e.target.value)}>
                <option value="">Bring an existing character…</option>
                {spare.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
              </select>
              <Button disabled={!chosen} onClick={() => void guarded(() => bringCharacter(group.id, chosen))}>Bring</Button>
            </>
          )}
        </div>
      )}
    </li>
  );
}
