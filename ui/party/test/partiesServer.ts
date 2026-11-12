import type { JoinCode, MyInvite, Party, PartyInvite, PartyMember, PartyUser } from "../api/overrides/parties";

/**
 * An in-memory parties server for tests, answering the routes of ../api/overrides/parties.ts the way
 * party/docs/parties-api.md says: outsiders get 404, a username invite is always "sent", a character is in at most
 * one party (409 `character_in_party`), a code with limited uses runs out.
 */

interface Row { id: string; name: string; dmId: string; createdAt: number }
interface Invite { id: string; partyId: string; username?: string; code?: string; usesLeft: number; expiresAt: number }
interface Member { ownerId: string; characterId: string; partyId: string; joinedAt: number }
interface Account { id: string; username: string; dm: boolean; characters: { id: string; name: string; classLevel: string }[] }

export interface FakeServer {
  accounts: Account[];
  parties: Row[];
  invites: Invite[];
  /** Personal invites made from a redeemed code: invite id -> account id. */
  redeemed: Map<string, string>;
  members: Member[];
  /** Who is signed in. */
  me: string;
  calls: { method: string; path: string; body?: unknown }[];
  fetch: (session: unknown, method: string, path: string, body?: unknown) => Promise<unknown>;
}

const err = (code: string, message: string) => ({ code, message });
const notFound = () => err("not_found", "Not found.");
let next = 1;
const id = (p: string) => `${p}${next++}`;
const DAY = 86_400_000;

export function fakePartiesServer(accounts: Account[], me: string): FakeServer {
  const s: FakeServer = { accounts, parties: [], invites: [], redeemed: new Map(), members: [], me, calls: [], fetch: async () => null };
  const account = (uid: string) => s.accounts.find((a) => a.id === uid)!;
  const user = (): PartyUser => {
    const a = account(s.me);
    return { id: a.id, username: a.username, email: null, admin: false, mustChangePassword: false, dm: a.dm };
  };
  const memberView = (m: Member, viewerIsDm: boolean): PartyMember => {
    const owner = account(m.ownerId);
    const c = owner.characters.find((x) => x.id === m.characterId);
    const mine = m.ownerId === s.me;
    return {
      ownerId: m.ownerId, ownerName: owner.username, characterId: m.characterId, name: c?.name ?? "", classLevel: c?.classLevel ?? "",
      joinedAt: m.joinedAt, mine, ...(c ? {} : { unreadable: true }),
      ...(viewerIsDm || mine ? { card: { ac: 15, hpMax: 24, hpCurrent: 20, passivePerception: 13, spellDc: null } } : {}),
    };
  };
  const view = (p: Row): Party => {
    const dm = p.dmId === s.me;
    return {
      id: p.id, name: p.name, ruleset: "dnd2024", role: dm ? "dm" : "player", dmName: account(p.dmId).username, createdAt: p.createdAt,
      members: s.members.filter((m) => m.partyId === p.id).map((m) => memberView(m, dm)),
      ...(dm ? { invites: s.invites.filter((i) => i.partyId === p.id).map((i): PartyInvite => (
        i.username ? { id: i.id, kind: "username", username: i.username, expiresAt: i.expiresAt } : { id: i.id, kind: "code", usesLeft: i.usesLeft, expiresAt: i.expiresAt })) } : {}),
    };
  };
  const visible = (p: Row) => p.dmId === s.me || s.members.some((m) => m.partyId === p.id && m.ownerId === s.me);
  const myInvite = (i: Invite): MyInvite => {
    const p = s.parties.find((x) => x.id === i.partyId)!;
    return { id: i.id, partyId: p.id, partyName: p.name, dmName: account(p.dmId).username, expiresAt: i.expiresAt, via: i.username ? "username" : "code" };
  };
  const mineToTake = (i: Invite) =>
    (i.username && i.username === account(s.me).username.toLowerCase()) || s.redeemed.get(i.id) === s.me;
  const join = (partyId: string, characterId: unknown): Party => {
    const c = String(characterId ?? "");
    if (!account(s.me).characters.some((x) => x.id === c)) throw notFound();
    if (s.members.some((m) => m.ownerId === s.me && m.characterId === c)) throw err("character_in_party", "That character is already in a party.");
    s.members.push({ ownerId: s.me, characterId: c, partyId, joinedAt: Date.now() });
    return view(s.parties.find((p) => p.id === partyId)!);
  };

  s.fetch = async (_session, method, path, body) => {
    s.calls.push({ method, path, ...(body === undefined ? {} : { body }) });
    const b = (body ?? {}) as Record<string, unknown>;
    const parts = path.split("/").slice(2).map(decodeURIComponent); // after "/api"
    const route = `${method} /${parts.map((p, i) => (i > 0 && !["roles", "dm", "invites", "members", "accept", "decline"].includes(p) ? ":" : p)).join("/")}`;
    switch (route) {
      case "PUT /me/roles/dm": account(s.me).dm = Boolean(b.on); return user();
      case "GET /parties": return { parties: s.parties.filter(visible).map(view) };
      case "POST /parties": {
        if (!account(s.me).dm) throw err("forbidden", "Turn on Dungeon Master tools first.");
        const row = { id: id("p"), name: String(b.name), dmId: s.me, createdAt: Date.now() };
        s.parties.push(row);
        return view(row);
      }
    }
    const party = s.parties.find((p) => p.id === parts[1]);
    const isDm = party?.dmId === s.me;
    switch (route) {
      case "PATCH /parties/:": if (!isDm) throw notFound(); party!.name = String(b.name); return view(party!);
      case "DELETE /parties/:":
        if (!isDm) throw notFound();
        s.parties = s.parties.filter((p) => p !== party);
        s.members = s.members.filter((m) => m.partyId !== party!.id);
        s.invites = s.invites.filter((i) => i.partyId !== party!.id);
        return null;
      case "POST /parties/:/invites": {
        if (!isDm) throw notFound();
        if (typeof b.username === "string") {
          const inv = { id: id("i"), partyId: party!.id, username: b.username.toLowerCase(), usesLeft: 1, expiresAt: Date.now() + 7 * DAY };
          s.invites.push(inv);
          return { id: inv.id, kind: "username", username: b.username, expiresAt: inv.expiresAt };
        }
        const code = `CODE${next++}ABCD`;
        const inv = { id: id("i"), partyId: party!.id, code, usesLeft: Number(b.uses), expiresAt: Date.now() + Number(b.expiresInDays) * DAY };
        s.invites.push(inv);
        return { id: inv.id, kind: "code", code, usesLeft: inv.usesLeft, expiresAt: inv.expiresAt } satisfies JoinCode;
      }
      case "DELETE /parties/:/invites/:": if (!isDm) throw notFound(); s.invites = s.invites.filter((i) => i.id !== parts[3]); return null;
      case "POST /parties/:/members": if (!isDm) throw notFound(); return join(party!.id, b.characterId);
      case "DELETE /parties/:/members/:/:": {
        const m = s.members.find((x) => x.partyId === parts[1] && x.ownerId === parts[3] && x.characterId === parts[4]);
        if (!m || !(isDm || m.ownerId === s.me)) throw notFound();
        s.members = s.members.filter((x) => x !== m);
        return null;
      }
      case "GET /invites": return { invites: s.invites.filter(mineToTake).map(myInvite) };
      case "POST /join": {
        const inv = s.invites.find((i) => i.code === b.code && i.usesLeft > 0);
        if (!inv) throw err("not_found", "That code isn't valid, or it has run out.");
        inv.usesLeft -= 1;
        const personal = { id: id("i"), partyId: inv.partyId, usesLeft: 1, expiresAt: inv.expiresAt };
        s.invites.push(personal);
        s.redeemed.set(personal.id, s.me);
        return { invite: myInvite(personal) };
      }
      case "POST /invites/:/accept": {
        const inv = s.invites.find((i) => i.id === parts[1]);
        if (!inv || !mineToTake(inv)) throw notFound();
        const out = join(inv.partyId, b.characterId);
        s.invites = s.invites.filter((i) => i !== inv);
        return out;
      }
      case "POST /invites/:/decline": s.invites = s.invites.filter((i) => i.id !== parts[1]); return null;
      default: throw err("not_found", "The server does not offer that.");
    }
  };
  return s;
}
