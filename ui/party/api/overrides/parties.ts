import type { CharacterSnapshot } from "../../../src/api/contract";
import { serverFetch, type ServerEvent } from "../http";
import { liveEvents } from "../liveEvents";
import { rememberDmRole } from "../dmRole";
import { loadSession, type ServerSession } from "../session";
import { override } from "./registry";

/**
 * Parties (plan 3.3-3.7, 4.1; releases 0.2.0 and 0.2.1): the server's `/api/parties`, `/api/invites` and `/api/join`
 * routes, the DM switch, the DM's read-only view of a member character and the party events on the account's event
 * stream, as party/docs/parties-api.md describes them. Characters never move: a party is rows on the server, and a
 * member character stays in its owner's folder. What anyone sees of a member is the server's projection of its card
 * for them (`card_view_for`): the DM and the owner get the numbers, other members a name, class and level, and during
 * a combat only (0.4.x), a health band.
 *
 * Every call goes through {@link partiesDeps}, so tests answer them from a fake server (../../test/partiesServer.ts).
 * Times from the server are seconds since the epoch; the functions here turn them into milliseconds.
 */

export type Ruleset = "dnd2024" | "dnd2014";

/** A party in the list (`GET /api/parties`). */
export interface PartySummary {
  id: string;
  name: string;
  ruleset: Ruleset;
  /** "dm": the signed-in account runs it (also when its own character is in it). "player": one of its characters is. */
  role: "dm" | "player";
  dmName: string;
  memberCount: number;
  /** The signed-in account's own characters in it. */
  myCharacters: string[];
  createdAt: number;
  updatedAt: number;
}

/** One class of a member character. */
export interface ClassLine {
  name: string;
  level: number;
}

/** What the DM (and the owner) see of a member character at the table. */
export interface CardStats {
  hp: number;
  hpMax: number;
  tempHp: number;
  armorClass: number;
  initiative: number;
  speed: number;
  passivePerception: number;
  spellDcs: { class: string; dc: number }[];
  exhaustion: number;
  concentration: string | null;
  inspiration: boolean;
}

/** How hurt a member looks to the rest of the party (plan D6). */
export type HealthBand = "unhurt" | "bloodied" | "down";

/** A roster card, as the server projects it for the signed-in account. */
export interface PartyMember {
  characterId: string;
  ownerName: string;
  /** Only for the party's DM, and for one's own characters. */
  ownerId?: string;
  mine: boolean;
  /** Owned by the party's DM (plan D4). */
  dmCharacter: boolean;
  /** Null when the file can't be read. */
  name: string | null;
  level: number | null;
  ruleset: Ruleset | null;
  /** Main class first; empty until the server read the character with its content. */
  classes?: ClassLine[];
  /** The DM and the owner only: null until the owner played it or the DM viewed it. */
  stats?: CardStats | null;
  /** When `stats` were read (ms). */
  readAt?: number | null;
  /** Other members only. */
  /** Other members only, and only during a combat: the server sends no band otherwise (plan D6). */
  band?: HealthBand | null;
  /** The DM only: the owner has the app open. */
  online?: boolean;
  build: string | null;
  state: "ok" | "unreadable" | "missing";
  joinedAt: number;
}

/** An open invitation, as its party's DM sees it. */
export interface PartyInvite {
  id: string;
  kind: "username" | "code";
  username?: string;
  usesLeft?: number;
  createdAt: number;
  expiresAt: number;
}

/** A party with its roster; invites only for its DM. */
export interface Party extends PartySummary {
  members: PartyMember[];
  invites?: PartyInvite[];
  /** How new this view is: a party event with a higher `pseq` means it changed since. */
  pseq?: number;
}

/** The DM's read-only view of a member character: upstream's snapshot as its owner has it now, and its card. */
export interface MemberView {
  snapshot: CharacterSnapshot;
  card: PartyMember;
  pseq: number;
}

/** A party event from the account's event stream (party/docs/parties-api.md). */
export type PartyEvent =
  | { type: "party"; party: string; pseq: number; kind: string; id: string | null }
  | { type: "mine"; kind: string }
  /** Events may have been missed (a resync, or the stream reopened after a drop): reload everything shown. */
  | { type: "resync" };

/** A join code just made. The code itself is shown this once: the server keeps only its hash. */
export interface JoinCode {
  invite: PartyInvite;
  code: string;
  /** The server's web link (`…/join#CODE`), or null when it has no public address. */
  link: string | null;
}

/** An invitation the signed-in account can take up: to its username, or a join code it entered. */
export interface MyInvite {
  id: string;
  kind: "username" | "code";
  party: { name: string; dmName: string; ruleset: Ruleset };
  expiresAt: number;
}

/** Which party a code is for, asked without signing in (the sign-up, plan D39). */
export interface CodePreview {
  party: { name: string; dmName: string; ruleset: Ruleset };
  expiresAt: number;
}

export interface PartiesDeps {
  fetch: (session: Pick<ServerSession, "baseUrl" | "token">, method: string, path: string, body?: unknown) => Promise<unknown>;
  /** The account's event stream: events, and opens (`again` after a drop). Answers a stop. */
  events: (onEvent: (e: ServerEvent) => void, onOpen: (again: boolean) => void) => () => void;
}

const DEFAULT_DEPS: PartiesDeps = {
  // Late-bound: ../http.ts imports this module before its own exports are set.
  fetch: (session, method, path, body) => serverFetch(session, method, path, body),
  events: (onEvent, onOpen) => {
    const session = loadSession();
    return session ? liveEvents(session, onEvent, { onOpen }) : () => {};
  },
};

export const partiesDeps: PartiesDeps = { ...DEFAULT_DEPS };

/** Tests: start over. */
export function resetParties(): void {
  Object.assign(partiesDeps, DEFAULT_DEPS);
  listeners.clear();
}

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject({ code: "unauthorized", message: "Not signed in." });
  return partiesDeps.fetch(session, method, path, body) as Promise<T>;
}

const notFound = (e: unknown) => (e as { code?: string } | null)?.code === "not_found";

/** Ids go into paths only as one encoded segment each. */
const seg = (id: string) => encodeURIComponent(id);
const ms = (seconds: number) => seconds * 1000;

const summary = <T extends PartySummary>(p: T): T => ({ ...p, createdAt: ms(p.createdAt), updatedAt: ms(p.updatedAt) });
const member = (m: PartyMember): PartyMember => ({
  ...m, joinedAt: ms(m.joinedAt), ...(m.readAt != null ? { readAt: ms(m.readAt) } : {}),
});
const invite = (i: PartyInvite): PartyInvite => ({ ...i, createdAt: ms(i.createdAt), expiresAt: ms(i.expiresAt) });
const detail = (p: Party): Party => ({
  ...summary(p), members: (p.members ?? []).map(member), ...(p.invites ? { invites: p.invites.map(invite) } : {}),
});
const myInvite = (i: MyInvite): MyInvite => ({ ...i, expiresAt: ms(i.expiresAt) });

// What changed: the Parties page reloads (a deleted character leaves its party on the server too).
const listeners = new Set<() => void>();
export function onPartiesChanged(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
export function partiesChanged(): void {
  for (const l of [...listeners]) l();
}

async function changing<T>(work: Promise<T>): Promise<T> {
  const out = await work;
  partiesChanged();
  return out;
}

/** The longest a join code or invite works, in days (plan 3.4). */
export const CODE_DAYS = 7;
export const MAX_CODE_USES = 50;
/** Plan 3.3: at most 20 parties per account. */
export const MAX_PARTIES = 20;
export const MAX_NAME = 60;

/** The server's answer about the DM switch, also kept for the top bar's Encounters tab (../dmRole.ts). */
const known = (dm: boolean) => { rememberDmRole(dm); return dm; };

export const dungeonMaster = () => call<{ dm: boolean }>("GET", "/api/me/roles").then((r) => known(r.dm));
/** Off is refused (409 `conflict`) while the account still runs parties. */
export const setDungeonMaster = (on: boolean) =>
  (on ? call<{ dm: boolean }>("POST", "/api/me/roles/dm", {}) : call<{ dm: boolean }>("DELETE", "/api/me/roles/dm")).then((r) => known(r.dm));

/** The parties in the list, without their rosters: the Encounters tab's party strip. Null when parties are off. */
export async function listParties(): Promise<{ dm: boolean; parties: PartySummary[] } | null> {
  try {
    const list = await call<{ dm: boolean; parties: PartySummary[] }>("GET", "/api/parties");
    known(list.dm);
    return { dm: list.dm, parties: list.parties.map(summary) };
  } catch (e) {
    if (notFound(e)) return null;
    throw e;
  }
}

/**
 * The parties this account runs or has a character in, each with its roster, and whether it has DM tools on.
 * `off`: the server answers 404 here when parties are switched off in its config.
 */
export async function loadParties(): Promise<{ dm: boolean; parties: Party[]; off?: boolean }> {
  let list: { dm: boolean; parties: PartySummary[] };
  try {
    list = await call<{ dm: boolean; parties: PartySummary[] }>("GET", "/api/parties");
    known(list.dm);
  } catch (e) {
    if (notFound(e)) return { dm: false, parties: [], off: true };
    throw e;
  }
  const parties = await Promise.all(list.parties.map((p) => loadParty(p.id)));
  return { dm: list.dm, parties: parties.filter((p): p is Party => p !== null) };
}

/** One party with its roster, or null when it's gone or no longer yours to see. */
export async function loadParty(id: string): Promise<Party | null> {
  try {
    return detail(await call<Party>("GET", `/api/parties/${seg(id)}`));
  } catch (e) {
    if (notFound(e)) return null;
    throw e;
  }
}

/** The DM's read-only view of a member character. Null when it isn't in the party (any more). */
export async function viewMember(partyId: string, ownerId: string, characterId: string): Promise<MemberView | null> {
  try {
    const out = await call<MemberView>("POST", `/api/parties/${seg(partyId)}/members/${seg(ownerId)}/${seg(characterId)}/view/snapshot`, {});
    return { ...out, card: member({ ...out.card, characterId, joinedAt: 0 } as PartyMember) };
  } catch (e) {
    if (notFound(e)) return null;
    throw e;
  }
}

const PARTY_EVENT = "party://party";
const MINE_EVENT = "party://mine";
const RESYNC_EVENT = "party://resync";

/** Party events, as they come; a reopened stream counts as a resync. Answers a stop. */
export function watchParties(listener: (e: PartyEvent) => void): () => void {
  return partiesDeps.events((e) => {
    const p = (e.payload ?? {}) as Record<string, unknown>;
    if (e.event === PARTY_EVENT && typeof p.party === "string") {
      listener({ type: "party", party: p.party, pseq: Number(p.pseq ?? 0), kind: String(p.kind ?? ""), id: typeof p.id === "string" ? p.id : null });
    } else if (e.event === MINE_EVENT) {
      listener({ type: "mine", kind: String(p.kind ?? "") });
    } else if (e.event === RESYNC_EVENT) {
      listener({ type: "resync" });
    }
  }, (again) => { if (again) listener({ type: "resync" }); });
}

/** Human words for a health band. */
export const BAND_LABEL: Record<HealthBand, string> = { unhurt: "Unhurt", bloodied: "Bloodied", down: "Down" };

/** "Fighter 3 / Wizard 2", or "" when the classes aren't known. */
export function classLine(classes: ClassLine[] | undefined): string {
  return (classes ?? []).map((c) => `${c.name} ${c.level}`).join(" / ");
}

export const createParty = (name: string) => changing(call<Party>("POST", "/api/parties", { name }).then(detail));
export const renameParty = (id: string, name: string) => changing(call<Party>("PATCH", `/api/parties/${seg(id)}`, { name }).then(detail));
export const deleteParty = (id: string) => changing(call<unknown>("DELETE", `/api/parties/${seg(id)}`));

/** The DM always hears "sent", whether that account exists or not (plan 3.4). */
export const inviteUsername = (partyId: string, username: string) =>
  changing(call<{ invite: PartyInvite }>("POST", `/api/parties/${seg(partyId)}/invites`, { kind: "username", username: username.trim() })
    .then((r) => invite(r.invite)));
export const makeJoinCode = (partyId: string, uses: number, days: number) =>
  changing(call<JoinCode>("POST", `/api/parties/${seg(partyId)}/invites`, { kind: "code", uses, days })
    .then((r) => ({ ...r, invite: invite(r.invite) })));
export const cancelInvite = (partyId: string, inviteId: string) =>
  changing(call<unknown>("DELETE", `/api/parties/${seg(partyId)}/invites/${seg(inviteId)}`));

/** The DM's own character joins a party they run, with no invite (plan D4). */
export const addOwnCharacter = (partyId: string, characterId: string) =>
  changing(call<PartyMember>("POST", `/api/parties/${seg(partyId)}/members`, { characterId }).then(member));

/** The DM takes a character out, or its owner withdraws it. */
export const removeMember = (partyId: string, ownerId: string, characterId: string) =>
  changing(call<unknown>("DELETE", `/api/parties/${seg(partyId)}/members/${seg(ownerId)}/${seg(characterId)}`));

export const listMyInvites = () => call<{ invites: MyInvite[] }>("GET", "/api/invites").then((r) => r.invites.map(myInvite));
/** A typed join code is kept on the account as one of its invites, so the party's name shows before a character is picked. */
export const redeemJoinCode = (code: string) =>
  changing(call<{ invite: MyInvite }>("POST", "/api/join", { code: normaliseCode(code) }).then((r) => myInvite(r.invite)));
export const acceptInvite = (inviteId: string, characterId: string) =>
  changing(call<{ party: PartySummary }>("POST", `/api/invites/${seg(inviteId)}/accept`, { characterId }).then((r) => summary(r.party)));
export const declineInvite = (inviteId: string) => changing(call<unknown>("POST", `/api/invites/${seg(inviteId)}/decline`, {}));

/** Which party a code is for, without a session (the sign-up). A code that isn't valid is refused with 404. */
export const previewJoinCode = (baseUrl: string, code: string): Promise<CodePreview> =>
  partiesDeps.fetch({ baseUrl, token: "" }, "POST", "/api/join/preview", { code: normaliseCode(code) })
    .then((r) => ({ ...(r as CodePreview), expiresAt: ms((r as CodePreview).expiresAt) }));

/** A code as people paste it: spaces, dashes and case don't matter; a `…/join#CODE` link gives its code. */
export function normaliseCode(typed: string): string {
  const hash = typed.indexOf("#");
  const raw = (hash >= 0 ? typed.slice(hash + 1) : typed).replace(/^\/join\//i, "");
  return raw.replace(/[\s-]/g, "").toUpperCase();
}

/** A code split in groups of four, for reading out at the table. */
export function formatCode(code: string): string {
  return normaliseCode(code).replace(/(.{4})(?=.)/g, "$1-");
}

/** The signed-in account's characters that are in a party, and which one (plan D19: one party per character). */
export function charactersInParties(parties: PartySummary[]): Map<string, PartySummary> {
  const taken = new Map<string, PartySummary>();
  for (const p of parties) for (const c of p.myCharacters) taken.set(c, p);
  return taken;
}

/** The code the server answers when a character is already in a party (or the DM tools can't be switched off yet). */
export const CONFLICT = "conflict";

// Join links. The code is after the `#`, so it never reaches the server's logs: `…/#/join/CODE` (the link this app
// makes) or `…/join#CODE` (plan 3.4). It is kept on the device until used, so it survives signing up (plan D39).
const JOIN_KEY = "party.joinCode";
const HASH_ROUTE = /^#\/join\/([^/?#]+)/;

/** The link players open to join with `code`. */
export function joinLink(code: string, origin = window.location.origin): string {
  return `${origin}/#/join/${normaliseCode(code)}`;
}

/** A join code kept on this device, or "". */
export function keptJoinCode(): string {
  try { return localStorage.getItem(JOIN_KEY) ?? ""; } catch { return ""; }
}

export function keepJoinCode(code: string | null): void {
  try {
    if (code) localStorage.setItem(JOIN_KEY, normaliseCode(code));
    else localStorage.removeItem(JOIN_KEY);
  } catch { /* a private window: the code is typed again */ }
}

/**
 * A page opened from a join link: keep its code and go on to the Parties page (signed in) or the sign-in. Runs before
 * the router reads the address. Answers the code, or null.
 */
export function takeJoinLink(location: Pick<Location, "pathname" | "hash"> = window.location): string | null {
  const route = HASH_ROUTE.exec(location.hash);
  const raw = route ? decodeURIComponent(route[1] ?? "") : /\/join\/?$/.test(location.pathname) && location.hash.length > 1 ? location.hash.slice(1) : "";
  const code = normaliseCode(raw);
  if (!code) return null;
  keepJoinCode(code);
  try {
    const base = location.pathname.replace(/\/join\/?$/, "/");
    window.history.replaceState(null, "", `${base}#/parties`);
  } catch { /* tests, or an odd host: the router still shows the code */ }
  return code;
}

if (typeof window !== "undefined") takeJoinLink();

override("parties", (methods) => {
  // The server drops a deleted character's membership; the Parties page shows that at once.
  const remove = methods.deleteCharacter;
  methods.deleteCharacter = async (args?: Record<string, unknown>) => {
    const out = await remove(args);
    partiesChanged();
    return out;
  };
});
