import { serverFetch } from "../http";
import { loadSession, type ServerSession, type ServerUser } from "../session";
import { override } from "./registry";

/**
 * Parties (plan 3.3-3.5, 4.1; release 0.2.0): the server's `/api/parties`, `/api/invites` and `/api/join` routes, and
 * the DM switch. Characters never move: a party is rows on the server, and a member character stays in its owner's
 * folder. A DM sees members through the server's own read-only projection (`card`), never the owner's files.
 *
 * The routes are the server's (party/docs/parties-api.md). Every call goes through {@link partiesDeps}, so tests answer
 * them from a fake server (../../test/partiesServer.ts).
 */

/** The account as `/api/me` answers it, with the DM switch (plan D2). */
export type PartyUser = ServerUser & { dm?: boolean };

/** The read-only numbers the server projects from a member's saved character (plan 3.5). */
export interface MemberCard {
  ac?: number | null;
  hpMax?: number | null;
  hpCurrent?: number | null;
  passivePerception?: number | null;
  spellDc?: number | null;
}

export interface PartyMember {
  ownerId: string;
  ownerName: string;
  characterId: string;
  name: string;
  /** e.g. "Fighter 3". */
  classLevel: string;
  joinedAt: number;
  /** One of the signed-in account's own characters. */
  mine: boolean;
  /** The file is gone or can't be read. */
  unreadable?: boolean;
  /** Only for the DM, and for the owner. */
  card?: MemberCard | null;
}

/** An open invitation, as its party's DM sees it. */
export interface PartyInvite {
  id: string;
  kind: "username" | "code";
  username?: string | null;
  usesLeft?: number;
  expiresAt: number;
}

export interface Party {
  id: string;
  name: string;
  ruleset: string;
  /** "dm": the signed-in account runs it. "player": one of its characters is a member. */
  role: "dm" | "player";
  dmName: string;
  members: PartyMember[];
  /** The DM's view only. */
  invites?: PartyInvite[];
  createdAt: number;
}

/** A join code just made. The code itself is shown this once: the server keeps only its hash. */
export interface JoinCode extends PartyInvite {
  kind: "code";
  code: string;
}

/** An invitation the signed-in account can take up: by its username, or a join code it entered. */
export interface MyInvite {
  id: string;
  partyId: string;
  partyName: string;
  dmName: string;
  expiresAt: number;
  via: "username" | "code";
}

export interface PartiesDeps {
  fetch: (session: Pick<ServerSession, "baseUrl" | "token">, method: string, path: string, body?: unknown) => Promise<unknown>;
}

const DEFAULT_DEPS: PartiesDeps = {
  // Late-bound: ../http.ts imports this module before its own exports are set.
  fetch: (session, method, path, body) => serverFetch(session, method, path, body),
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

/** Ids go into paths only as one encoded segment each. */
const seg = (id: string) => encodeURIComponent(id);

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

/** Days a join code works for, unless the DM picks fewer (plan 3.4). */
export const CODE_DAYS = 7;
export const MAX_CODE_USES = 20;
/** Plan 3.3: at most 20 parties per account. */
export const MAX_PARTIES = 20;

export const setDungeonMaster = (on: boolean) => call<PartyUser>("PUT", "/api/me/roles/dm", { on });

export const listParties = () => call<{ parties: Party[] }>("GET", "/api/parties").then((r) => r.parties);
export const createParty = (name: string) => changing(call<Party>("POST", "/api/parties", { name }));
export const renameParty = (id: string, name: string) => changing(call<Party>("PATCH", `/api/parties/${seg(id)}`, { name }));
export const deleteParty = (id: string) => changing(call<void>("DELETE", `/api/parties/${seg(id)}`));

/** The DM always hears "sent", whether that account exists or not (plan 3.4). */
export const inviteUsername = (partyId: string, username: string) =>
  changing(call<PartyInvite>("POST", `/api/parties/${seg(partyId)}/invites`, { username: username.trim() }));
export const makeJoinCode = (partyId: string, uses: number, expiresInDays: number) =>
  changing(call<JoinCode>("POST", `/api/parties/${seg(partyId)}/invites`, { uses, expiresInDays }));
export const cancelInvite = (partyId: string, inviteId: string) =>
  changing(call<void>("DELETE", `/api/parties/${seg(partyId)}/invites/${seg(inviteId)}`));

/** The DM's own character joins a party they run (plan D4); the server checks the DM owns it. */
export const addOwnCharacter = (partyId: string, characterId: string) =>
  changing(call<Party>("POST", `/api/parties/${seg(partyId)}/members`, { characterId }));

/** The DM takes a character out, or its owner leaves. */
export const removeMember = (partyId: string, ownerId: string, characterId: string) =>
  changing(call<void>("DELETE", `/api/parties/${seg(partyId)}/members/${seg(ownerId)}/${seg(characterId)}`));

export const listMyInvites = () => call<{ invites: MyInvite[] }>("GET", "/api/invites").then((r) => r.invites);
/** A typed join code becomes one of this account's invites, so the party's name shows before a character is picked. */
export const redeemJoinCode = (code: string) =>
  changing(call<{ invite: MyInvite }>("POST", "/api/join", { code: normaliseCode(code) }).then((r) => r.invite));
export const acceptInvite = (inviteId: string, characterId: string) =>
  changing(call<Party>("POST", `/api/invites/${seg(inviteId)}/accept`, { characterId }));
export const declineInvite = (inviteId: string) => changing(call<void>("POST", `/api/invites/${seg(inviteId)}/decline`));

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
export function charactersInParties(parties: Party[]): Map<string, Party> {
  const taken = new Map<string, Party>();
  for (const p of parties) for (const m of p.members) if (m.mine) taken.set(m.characterId, p);
  return taken;
}

/** The code the server answers when a character is already in a party. */
export const CHARACTER_IN_PARTY = "character_in_party";

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
