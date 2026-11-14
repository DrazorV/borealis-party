import type { ServerEvent } from "../api/http";
import type { CardStats, ClassLine, MyInvite, Party, PartyInvite, PartyMember, PartySummary, Ruleset } from "../api/overrides/parties";

/**
 * An in-memory parties server for tests. It answers the routes in party/docs/parties-api.md the way the server does:
 * - outsiders get 404 `not_found`;
 * - a username invite is always "sent";
 * - a character is in at most one party (409 `conflict`);
 * - a code's uses are taken only when a character joins;
 * - times are in seconds;
 * - a member's card is projected for the viewer (plan D6): stats for the DM and the owner; for others a health band
 *   during combat only (`inCombat`), and no band field at all otherwise;
 * - `emit` sends an event on the account's stream (`listen` is what `partiesDeps.events` calls).
 */

interface Row { id: string; name: string; dmId: string; createdAt: number; ruleset?: Ruleset }
interface Invite { id: string; partyId: string; username?: string; code?: string; usesLeft: number; expiresAt: number }
interface Member { ownerId: string; characterId: string; partyId: string; joinedAt: number }
interface Character { id: string; name: string; level: number; classes?: ClassLine[]; stats?: CardStats | null; ruleset?: Ruleset; updatedAt?: string }
interface Account { id: string; username: string; dm: boolean; online?: boolean; characters: Character[] }

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
  /** Each party's sequence (the tests move it with the events they send). */
  pseq: Map<string, number>;
  /** The parties are in a combat: other members see the health band. Never before 0.4.x. */
  inCombat: boolean;
  fetch: (session: unknown, method: string, path: string, body?: unknown) => Promise<unknown>;
  /** For `partiesDeps.events`. */
  listen: (onEvent: (e: ServerEvent) => void, onOpen: (again: boolean) => void) => () => void;
  /** Send an event to the listening page. */
  emit: (e: ServerEvent) => void;
  /** The stream dropped and reopened. */
  reopen: () => void;
  /** Streams open now. */
  listening: () => number;
}

/** Stats for a test character. */
export function stats(hp: number, hpMax: number, more: Partial<CardStats> = {}): CardStats {
  return {
    hp, hpMax, tempHp: 0, armorClass: 16, initiative: 2, speed: 30, passivePerception: 13, spellDcs: [], exhaustion: 0,
    concentration: null, inspiration: false, ...more,
  };
}

const band = (st: CardStats | null | undefined) => {
  if (!st || st.hp == null || !st.hpMax) return null;
  return st.hp === 0 ? "down" : st.hp * 2 <= st.hpMax ? "bloodied" : "unhurt";
};

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
    pseq: new Map(), inCombat: false,
    fetch: async () => null, listen: () => () => {}, emit: () => {}, reopen: () => {}, listening: () => 0,
  };
  const streams = new Set<{ onEvent: (e: ServerEvent) => void; onOpen: (again: boolean) => void }>();
  s.listen = (onEvent, onOpen) => {
    const stream = { onEvent, onOpen };
    streams.add(stream);
    onOpen(false);
    return () => { streams.delete(stream); };
  };
  s.emit = (e) => { for (const st of [...streams]) st.onEvent(e); };
  s.reopen = () => { for (const st of [...streams]) st.onOpen(true); };
  s.listening = () => streams.size;
  const account = (uid: string) => s.accounts.find((a) => a.id === uid)!;
  const memberView = (m: Member, party: Row): PartyMember => {
    const owner = account(m.ownerId);
    const c = owner.characters.find((x) => x.id === m.characterId);
    const mine = m.ownerId === s.me;
    const dm = party.dmId === s.me;
    const full = dm || mine;
    return {
      characterId: m.characterId, ownerName: owner.username, ...(full ? { ownerId: m.ownerId } : {}),
      ...(dm ? { online: Boolean(owner.online) } : {}),
      mine, dmCharacter: m.ownerId === party.dmId, name: c?.name ?? null, level: c?.level ?? null, ruleset: c ? (c.ruleset ?? "dnd2024") : null,
      classes: c?.classes ?? [],
      ...(full ? { stats: c?.stats ?? null, readAt: c?.stats ? now() : null } : s.inCombat ? { band: band(c?.stats) } : {}),
      build: null, state: c ? "ok" : "missing", joinedAt: m.joinedAt,
    };
  };
  const summary = (p: Row): PartySummary => {
    const members = s.members.filter((m) => m.partyId === p.id);
    return {
      id: p.id, name: p.name, ruleset: p.ruleset ?? "dnd2024", role: p.dmId === s.me ? "dm" : "player", dmName: account(p.dmId).username,
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
    pseq: s.pseq.get(p.id) ?? 0,
    members: s.members.filter((m) => m.partyId === p.id).map((m) => memberView(m, p)),
    ...(p.dmId === s.me ? { invites: s.invites.filter((i) => i.partyId === p.id && open(i)).map(dmInvite) } : {}),
  });
  const visible = (p: Row) => p.dmId === s.me || s.members.some((m) => m.partyId === p.id && m.ownerId === s.me);
  const myInvite = (i: Invite): MyInvite => {
    const p = s.parties.find((x) => x.id === i.partyId)!;
    return { id: i.id, kind: i.username !== undefined ? "username" : "code", party: { name: p.name, dmName: account(p.dmId).username, ruleset: p.ruleset ?? "dnd2024" }, expiresAt: i.expiresAt };
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
    const fixed = ["me", "roles", "dm", "parties", "invites", "members", "accept", "decline", "join", "preview", "view", "snapshot"];
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
        if (b.ruleset !== undefined && b.ruleset !== "dnd2024" && b.ruleset !== "dnd2014") throw err("invalid_argument", "The ruleset is dnd2024 or dnd2014.");
        // Like the server: no ruleset is 2024 (API clients before 0.3.2).
        const row: Row = { id: id("p"), name: String(b.name).trim(), dmId: s.me, createdAt: now(), ruleset: (b.ruleset as Ruleset | undefined) ?? "dnd2024" };
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
        return { party: { name: p.name, dmName: account(p.dmId).username, ruleset: p.ruleset ?? "dnd2024" }, expiresAt: inv.expiresAt };
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
      case "PATCH /parties/:":
        if (!isDm) throw notFound();
        if (b.ruleset !== undefined && b.ruleset !== "dnd2024" && b.ruleset !== "dnd2014") throw err("invalid_argument", "The ruleset is dnd2024 or dnd2014.");
        if (b.name !== undefined) party.name = String(b.name).trim();
        if (b.ruleset !== undefined) party.ruleset = b.ruleset as Ruleset;
        return detail(party);
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
      case "POST /parties/:/members/:/:/view/snapshot": {
        const m = s.members.find((x) => x.partyId === party.id && x.ownerId === parts[3] && x.characterId === parts[4]);
        if (!m || !isDm) throw notFound();
        const card = memberView(m, party);
        const c = account(m.ownerId).characters.find((x) => x.id === m.characterId)!;
        return { snapshot: fakeSnapshot(c), card, pseq: s.pseq.get(party.id) ?? 0 };
      }
      default: throw err("not_found", "The server does not offer that.");
    }
  };
  return s;
}

/** Enough of upstream's snapshot for the read-only sheet. */
export function fakeSnapshot(c: Character): unknown {
  const st = c.stats ?? stats(10, 10);
  const scores = { str: 16, dex: 14, con: 14, int: 10, wis: 12, cha: 8 };
  const mod = (n: number) => Math.floor((n - 10) / 2);
  const record = <T,>(f: (k: keyof typeof scores) => T) =>
    Object.fromEntries(Object.keys(scores).map((k) => [k, f(k as keyof typeof scores)])) as Record<keyof typeof scores, T>;
  return {
    id: c.id, name: c.name, ruleset: "dnd2024", portrait: null,
    level: { total: c.level, classes: (c.classes ?? []).map((x, i) => ({ slotId: i ? `multi-${i}` : "main", name: x.name, level: x.level, isMain: i === 0, subclass: null })) },
    abilities: { final: scores, modifier: record((k) => mod(scores[k])), save: record((k) => ({ bonus: mod(scores[k]) + (k === "str" ? 2 : 0), proficient: k === "str" })) },
    stats: {
      proficiencyBonus: 2, armorClass: st.armorClass, initiative: st.initiative, speed: st.speed, hpMax: st.hpMax,
      passivePerception: st.passivePerception, languages: ["Common"], proficiencies: [{ label: "Armor", values: ["Light", "Medium"] }],
      features: [{ name: "Second Wind", source: "Fighter" }],
      skills: [{ key: "athletics", name: "Athletics", ability: "str", bonus: 5, proficiency: "proficient" }],
    },
    attacks: [{ id: "a1", name: "Longsword", range: "5 ft.", attackBonus: "+5", damage: "1d8+3 slashing", description: "" }],
    spellcasting: { classes: [], slots: [], multiclassSlots: null },
    inventory: { items: [{ id: "i1", itemId: "x", name: "Longsword", displayName: "Longsword", quantity: 1, equipped: "primary" }], coins: { cp: 0, sp: 0, ep: 0, gp: 15, pp: 0 } },
    play: {
      hitPoints: { current: st.hp, max: st.hpMax, temp: st.tempHp }, hitDice: [], spellSlots: [], pactSlots: null,
      concentration: st.concentration ? { id: "s", name: st.concentration, level: 1 } : null,
      exhaustion: { level: st.exhaustion }, inspiration: st.inspiration, speeds: {},
    },
    details: {},
  };
}
