import { serverFetch, type ServerEvent } from "../http";
import { liveEvents } from "../liveEvents";
import { loadSession, type ServerSession } from "../session";
import type { Command, DmCombat, DmEvent, LogEntry } from "../../combat/dm/types";
import { override } from "./registry";

/**
 * Combat for the DM (plan 4.4, 4.4b; release 0.3.0): the combat server's routes and its account event, as
 * party/docs/combat-api.md describes them. The tracker (../../combat/dm) keeps the last state and a short write
 * queue on top of these (D44). The player's calls are in ../../combat/player (feat/player-combat).
 *
 * Every call goes through {@link combatDeps}, so tests (and the rig, before a server has the routes) answer them from
 * the mock server in ../../combat/dm/mockServer.ts.
 */

/** A change on the account's event stream: refetch `combat` when `seq` is newer than what is shown. */
export interface CombatEvent {
  party: string;
  combat: string;
  seq: number;
}

export interface CombatDeps {
  fetch: (session: Pick<ServerSession, "baseUrl" | "token">, method: string, path: string, body?: unknown) => Promise<unknown>;
  /** The account's event stream: events, opens (`again` after a drop), and its pings and drops. Answers a stop. */
  events: (onEvent: (e: ServerEvent) => void, onOpen: (again: boolean) => void, status?: StreamStatus) => () => void;
}

/** The stream's health, for the tracker's "Reconnecting…" chip. */
export interface StreamStatus {
  /** Anything arrived: an event or the server's ping. */
  onBeat?: () => void;
  /** The stream dropped; it reopens by itself. */
  onDrop?: () => void;
}

const DEFAULT_DEPS: CombatDeps = {
  fetch: (session, method, path, body) => serverFetch(session, method, path, body),
  events: (onEvent, onOpen, status) => {
    const session = loadSession();
    return session ? liveEvents(session, onEvent, { onOpen, onBeat: status?.onBeat, onDrop: status?.onDrop }) : () => {};
  },
};

export const combatDeps: CombatDeps = { ...DEFAULT_DEPS };

export function resetCombatDeps(): void {
  Object.assign(combatDeps, DEFAULT_DEPS);
}

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject({ code: "unauthorized", message: "Not signed in." });
  return combatDeps.fetch(session, method, path, body) as Promise<T>;
}

const seg = (id: string) => encodeURIComponent(id);
let requests = 0;
/** One id per user action: the server does a write at most once per id (retries, the offline buffer). */
export const newRequestId = (): string =>
  `${Date.now().toString(36)}-${(++requests).toString(36)}-${Math.random().toString(36).slice(2, 10)}`.slice(0, 64);

/** The tracker's address for a combat. */
export const combatPath = (id: string) => `/combat/${seg(id)}`;

/** "Start combat" on an encounter (part B's builder): the new combat's DM view (status `setup`). */
export const startCombat = (encounterId: string) =>
  call<DmCombat>("POST", `/api/encounters/${seg(encounterId)}/start`, { requestId: newRequestId() });
/** The party's live combat id, or null. */
export const partyCombat = (partyId: string) =>
  call<{ combat: string | null }>("GET", `/api/parties/${seg(partyId)}/combat`).then((r) => r?.combat ?? null);
export const loadCombat = (id: string) => call<DmCombat>("GET", `/api/combats/${seg(id)}`);

/** The engine command's tag is the action; its other fields are `args`. */
export function commandArgs(command: Command): { action: string; args: Record<string, unknown> } {
  const { command: action, ...args } = command as { command: string } & Record<string, unknown>;
  return { action, args };
}

/**
 * One write: an engine command or a server action (`add_creature`, `undo`, `export`). `expectSeq` makes it refuse
 * (409 `stale`) when the combat changed since; without it the write applies to the newest state, which is what a
 * queued intent ("7 slashing to X") wants.
 */
export const combatAction = <T = DmCombat>(id: string, action: string, args: Record<string, unknown>, opts: { requestId?: string; expectSeq?: number } = {}) =>
  call<T>("POST", `/api/combats/${seg(id)}/${seg(action)}`, {
    requestId: opts.requestId ?? newRequestId(), ...(opts.expectSeq !== undefined ? { expectSeq: opts.expectSeq } : {}), args,
  });

export const sendCommand = (id: string, command: Command, requestId?: string) => {
  const { action, args } = commandArgs(command);
  return combatAction(id, action, args, { requestId });
};

/** C8: revert the last write, only if nothing changed since this view (`expectSeq`). */
export const undoCombat = (id: string, seq: number, force = false) => combatAction(id, "undo", force ? { force: true } : {}, { expectSeq: seq });

/** C7: new copies of a creature in the fight. */
export const addCreature = (id: string, args: { copyOf: string; count?: number; rollInitiative?: boolean; hidden?: boolean }) =>
  combatAction(id, "add_creature", args);

/** The DM log, oldest first (the server sends newest first). */
export async function loadLog(id: string, limit = 200): Promise<DmEvent[]> {
  const out = await call<{ entries: LogEntry[] }>("GET", `/api/combats/${seg(id)}/log?limit=${limit}`);
  return (out?.entries ?? []).map((e) => e.event).reverse();
}

/** D45: the versioned JSON, DM only. */
export const exportCombat = async (id: string): Promise<string> =>
  JSON.stringify(await combatAction<unknown>(id, "export", {}), null, 2);

/** D45: an export becomes a NEW combat in the party; nothing is overwritten. */
export function importCombat(partyId: string, text: string): Promise<DmCombat> {
  let file: unknown;
  try { file = JSON.parse(text); } catch { return Promise.reject({ code: "invalid", message: "That isn't a combat export (not JSON)." }); }
  return call<DmCombat>("POST", `/api/parties/${seg(partyId)}/combats/import`, { requestId: newRequestId(), file });
}

const COMBAT_EVENT = "party://combat";
const RESYNC_EVENT = "party://resync";

/** Combat changes on the account's stream; a reopened stream or a resync is `{combat: "*"}` (reload everything). */
export function watchCombats(listener: (e: CombatEvent) => void, status: StreamStatus & { onOpen?: (again: boolean) => void } = {}): () => void {
  return combatDeps.events((e) => {
    const p = (e.payload ?? {}) as Record<string, unknown>;
    if (e.event === COMBAT_EVENT && typeof p.combat === "string") listener({ party: String(p.party ?? ""), combat: p.combat, seq: Number(p.seq ?? p.pseq ?? 0) });
    else if (e.event === RESYNC_EVENT) listener({ party: "", combat: "*", seq: 0 });
  }, (again) => {
    status.onOpen?.(again);
    if (again) listener({ party: "", combat: "*", seq: 0 });
  }, status);
}

/**
 * The DM's own live combats, for the Live chip in the top chrome (feat/player-combat owns the chip; its
 * `registerDmLiveCombats` takes this object): the parties this account runs that have a live combat, a watcher
 * for combat starts, ends and changes, and the tracker's address.
 */
export const dmLiveCombats = {
  list: async (): Promise<{ id: string; partyName?: string }[]> => {
    const out = await call<{ parties: { id: string; name: string; role: string }[] }>("GET", "/api/parties").catch(() => ({ parties: [] }));
    const mine = (out?.parties ?? []).filter((p) => p.role === "dm");
    const live = await Promise.all(mine.map((p) => partyCombat(p.id).then((id) => (id ? { id, partyName: p.name } : null), () => null)));
    return live.filter((c): c is { id: string; partyName: string } => c !== null);
  },
  watch: (onChange: () => void): (() => void) => combatDeps.events((e) => {
    const p = (e.payload ?? {}) as Record<string, unknown>;
    if (e.event === COMBAT_EVENT || e.event === RESYNC_EVENT || (e.event === "party://party" && p.kind === "combat")) onChange();
  }, (again) => { if (again) onChange(); }),
  path: combatPath,
};

/** A failure that means "no answer" (D44: queue and retry), not "the server refused". */
export function isNetworkFailure(e: unknown): boolean {
  const code = (e as { code?: string } | null)?.code;
  return code === "network" || code === "unavailable" || code === "timeout";
}

/** The engine refused because the last write was a player's: undo needs `force`. */
export const needsForce = (e: unknown): boolean =>
  /player's/i.test(String((e as { message?: string } | null)?.message ?? ""));

override("combat", () => {
  // Nothing of upstream's commands changes: combat has its own routes. The module registers so http.ts lists it
  // with the other overrides (registry.test.ts).
});
