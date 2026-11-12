import type { MyInvite, Party, PartyInvite, PartyMember, PartySummary } from "../api/overrides/parties";

/**
 * An in-memory parties server for tests. It answers the routes in party/docs/parties-api.md the way the server does:
 * - outsiders get 404 `not_found`;
 * - a username invite is always "sent";
 * - a character is in at most one party (409 `conflict`);
 * - a code's uses are taken only when a character joins;
 * - times are in seconds.
 */

interface Row { id: string; name: string; dmId: string; createdAt: number }
interface Invite { id: string; partyId: string; username?: string; code?: string; usesLeft: number; expiresAt: number }
interface Member { ownerId: string; characterId: string; partyId: string; joinedAt: number }
interface Account { id: string; username: string; dm: boolean; characters: { id: string; name: string; level: number }[] }

export interface FakeServer {
  accounts: Account[];
  parties: Row[];
  invites: Invite[];
  /** Codes an account entered: invite id -> account ids. */
  redeemed: Map<string, Set<string>>;
  /** Invites an account declined: they leave only that account's list. */
  declined: Set<string>;
  members: Member[];
  /** Who is signed in. */
  me: string;
  /** Parties switched off in the server's config: every route answers 404. */
  off: boolean;
  calls: { method: string; path: string; body?: unknown }[];
  fetch: (session: unknown, method: string, path: string, body?: unknown) => Promise<unknown>;
}

const err = (code: string, message: string) => ({ code, message });
const notFound = () => err("not_found", "Not found.");
let next = 1;
const id = (p: string) => `${p}${next++}`;
const now = () => Math.floor(Date.now() / 1000);
const DAY = 86_400;
export const charId = (n: number) => `c_${String(n).padStart(32, "0")}`;

export function fakePartiesServer(accounts: Account[], me: string): FakeServer {
  const s: FakeServer = {
    accounts, parties: [], invites: [], redeemed: new Map(), declined: new Set(), members: [], me, off: false, calls: [],
    fetch: async () => null,
  };
  const account = (uid: string) => s.accounts.find((a) => a.id === uid)!;
  const memberView = (m: Member, party: Row): PartyMember => {
    const owner = account(m.ownerId);
    const c = owner.characters.find((x) => x.id === m.characterId);
    const mine = m.ownerId === s.me;
    return {
      characterId: m.characterId, ownerName: owner.username, ...(party.dmId === s.me || mine ? { ownerId: m.ownerId } : {}),
      mine, dmCharacter: m.ownerId === party.dmId, name: c?.name ?? null, level: c?.level ?? null, ruleset: c ? "dnd2024" : null,
      build: null, state: c ? "ok" : "missing", joinedAt: m.joinedAt,
    };
  };
  const summary = (p: Row): PartySummary => {
    const members = s.members.filter((m) => m.partyId === p.id);
    return {
      id: p.id, name: p.name, ruleset: "dnd2024", role: p.dmId === s.me ? "dm" : "player", dmName: account(p.dmId).username,
      memberCount: members.length, myCharacters: members.filter((m) => m.ownerId === s.me).map((m) => m.characterId),
      createdAt: p.createdAt, updatedAt: p.createdAt,
    };
  };
  const open = (i: Invite) => i.expiresAt > now() && (i.username !== undefined || i.usesLeft > 0);
  const dmInvite = (i: Invite): PartyInvite => (i.username !== undefined
    ? { id: i.id, kind: "username", username: i.username, createdAt: now(), expiresAt: i.expiresAt }
    : { id: i.id, kind: "code", usesLeft: i.usesLeft, createdAt: now(), expiresAt: i.expiresAt });
  const detail = (p: Row): Party => ({
    ...summary(p),
    members: s.members.filter((m) => m.partyId === p.id).map((m) => memberView(m, p)),
    ...(p.dmId === s.me ? { invites: s.invites.filter((i) => i.partyId === p.id && open(i)).map(dmInvite) } : {}),
  });
  const visible = (p: Row) => p.dmId === s.me || s.members.some((m) => m.partyId === p.id && m.ownerId === s.me);
  const myInvite = (i: Invite): MyInvite => {
    const p = s.parties.find((x) => x.id === i.partyId)!;
    return { id: i.id, kind: i.username !== undefined ? "username" : "code", party: { name: p.name, dmName: account(p.dmId).username, ruleset: "dnd2024" }, expiresAt: i.expiresAt };
  };
  const mineToTake = (i: Invite) => open(i) && !s.declined.has(`${s.me}:${i.id}`)
    && (i.username === account(s.me).username.toLowerCase() || Boolean(s.redeemed.get(i.id)?.has(s.me)));
  const join = (partyId: string, characterId: unknown): Member => {
    const c = String(characterId ?? "");
    if (!/^c_[0-9a-f]{32}$/.test(c)) throw err("invalid_argument", "That isn't a character id.");
    if (!account(s.me).characters.some((x) => x.id === c)) throw notFound();
    if (s.members.some((m) => m.ownerId === s.me && m.characterId === c)) throw err("conflict", "That character is already in a party.");
    const m = { ownerId: s.me, characterId: c, partyId, joinedAt: now() };
    s.members.push(m);
    return m;
  };

  s.fetch = async (_session, method, path, body) => {
    s.calls.push({ method, path, ...(body === undefined ? {} : { body }) });
    if (s.off) throw notFound();
    const b = (body ?? {}) as Record<string, unknown>;
    const parts = path.split("/").slice(2).map(decodeURIComponent); // after "/api"
    const fixed = ["me", "roles", "dm", "parties", "invites", "members", "accept", "decline", "join", "preview"];
    const route = `${method} /${parts.map((p, i) => (i > 0 && !fixed.includes(p) ? ":" : p)).join("/")}`;
    switch (route) {
      case "GET /me/roles": return { dm: account(s.me).dm };
      case "POST /me/roles/dm": account(s.me).dm = true; return { dm: true };
      case "DELETE /me/roles/dm":
        if (s.parties.some((p) => p.dmId === s.me)) throw err("conflict", "Delete the parties you run first.");
        account(s.me).dm = false;
        return { dm: false };
      case "GET /parties": return { dm: account(s.me).dm, parties: s.parties.filter(visible).map(summary).reverse() };
      case "POST /parties": {
        if (!account(s.me).dm) throw err("dm_required", "Turn on Dungeon Master tools first.");
        const row = { id: id("p"), name: String(b.name).trim(), dmId: s.me, createdAt: now() };
        s.parties.push(row);
        return detail(row);
      }
      case "POST /join": {
        const inv = s.invites.find((i) => i.code === b.code && open(i));
        if (!inv) throw notFound();
        s.redeemed.set(inv.id, (s.redeemed.get(inv.id) ?? new Set()).add(s.me));
        return { invite: myInvite(inv) };
      }
      case "POST /join/preview": {
        const inv = s.invites.find((i) => i.code === b.code && open(i));
        if (!inv) throw notFound();
        const p = s.parties.find((x) => x.id === inv.partyId)!;
        return { party: { name: p.name, dmName: account(p.dmId).username, ruleset: "dnd2024" }, expiresAt: inv.expiresAt };
      }
      case "GET /invites": return { invites: s.invites.filter(mineToTake).map(myInvite) };
    }
    if (parts[0] === "invites") {
      const inv = s.invites.find((i) => i.id === parts[1]);
      if (!inv || !mineToTake(inv)) throw notFound();
      if (route === "POST /invites/:/accept") {
        join(inv.partyId, b.characterId);
        inv.usesLeft -= 1;
        if (inv.username !== undefined) s.invites = s.invites.filter((i) => i !== inv);
        return { party: summary(s.parties.find((p) => p.id === inv.partyId)!) };
      }
      if (route === "POST /invites/:/decline") { s.declined.add(`${s.me}:${inv.id}`); return { declined: true }; }
      throw err("not_found", "The server does not offer that.");
    }
    const party = s.parties.find((p) => p.id === parts[1]);
    if (!party || !visible(party)) throw notFound();
    const isDm = party.dmId === s.me;
    switch (route) {
      case "GET /parties/:": return detail(party);
      case "PATCH /parties/:": if (!isDm) throw notFound(); party.name = String(b.name).trim(); return detail(party);
      case "DELETE /parties/:":
        if (!isDm) throw notFound();
        s.parties = s.parties.filter((p) => p !== party);
        s.members = s.members.filter((m) => m.partyId !== party.id);
        s.invites = s.invites.filter((i) => i.partyId !== party.id);
        return { deleted: true };
      case "POST /parties/:/invites": {
        if (!isDm) throw notFound();
        const days = Number(b.days ?? 7);
        if (b.kind === "username") {
          const name = String(b.username).toLowerCase();
          const old = s.invites.find((i) => i.partyId === party.id && i.username === name);
          const inv = old ?? { id: id("i"), partyId: party.id, username: name, usesLeft: 1, expiresAt: now() + days * DAY };
          if (!old) s.invites.push(inv);
          return { sent: true, invite: dmInvite(inv) };
        }
        const code = `K7QM3XTR9BWD${String(next++).padStart(4, "0")}`;
        const inv = { id: id("i"), partyId: party.id, code, usesLeft: Number(b.uses ?? 1), expiresAt: now() + days * DAY };
        s.invites.push(inv);
        return { invite: dmInvite(inv), code: code.replace(/(.{4})(?=.)/g, "$1-"), link: null };
      }
      case "DELETE /parties/:/invites/:":
        if (!isDm) throw notFound();
        s.invites = s.invites.filter((i) => i.id !== parts[3]);
        return { deleted: true };
      case "POST /parties/:/members": {
        if (!isDm) throw notFound();
        return memberView(join(party.id, b.characterId), party);
      }
      case "DELETE /parties/:/members/:/:": {
        const m = s.members.find((x) => x.partyId === party.id && x.ownerId === parts[3] && x.characterId === parts[4]);
        if (!m || !(isDm || m.ownerId === s.me)) throw notFound();
        s.members = s.members.filter((x) => x !== m);
        return { removed: true };
      }
      default: throw err("not_found", "The server does not offer that.");
    }
  };
  return s;
}
