import { serverFetch, type ServerEvent } from "../../api/http";
import { liveEvents } from "../../api/liveEvents";
import { loadSession, type ServerSession } from "../../api/session";
import type { D20Input, StatBlock } from "../shared/types";
import type { HealthBand } from "../../api/overrides/parties";

/**
 * The player's side of a live combat (plan 4.4, 0.3.0 part F): the server's PLAYER projection of a party's combat and
 * the few writes a player may make for their own character (initiative, death saves, concentration, and answering
 * the DM's "Take 7 slashing?" prompt). The server builds the projection (`view_for`), so hidden creatures, their
 * turns and anything about them never reach this module: there is nothing here to filter (plan 3.5, D5).
 *
 * Every call goes through {@link combatPlayerDeps}, so tests answer them from a fake server (../../test/combatServer.ts).
 */

export type { D20Input, StatBlock } from "../shared/types";

/** Whose turn players see (D5): while hidden creatures act, the pointer stays on the last visible turn. */
export type TurnPointer =
  | { kind: "combatant"; id: string; round: number }
  | { kind: "enemy_turn"; round: number }
  | { kind: "waiting"; round: number }
  | { kind: "not_started" }
  | { kind: "ended" };

export interface CombatCondition {
  name: string;
  /** Turns left, when it has a turn count. */
  remaining?: number | null;
  /** The SRD condition text, when the server sends it. */
  text?: string | null;
}

export type Life = "alive" | "dying" | "stable" | "dead";

/** A combatant players can see: a PC, or a creature the DM revealed. */
export interface PlayerCombatant {
  id: string;
  kind: "creature" | "character";
  /** The PC's name, or the creature's public name. */
  name: string;
  /** The signed-in account's own PC. */
  mine: boolean;
  /** PCs: the character's id in its owner's folder. */
  characterId?: string | null;
  /** PCs: the player's account name. */
  ownerName?: string | null;
  initiative: number | null;
  /** Allies (and revealed creatures, when the server sends it): Unhurt / Bloodied / Down (D6, combat only). */
  band?: HealthBand | null;
  conditions: CombatCondition[];
  life?: Life;
  /** Own PC only. */
  deathSaves?: { successes: number; failures: number } | null;
  /** Own PC (and what the server shows of others): the spell or effect held. */
  concentration?: string | null;
  /** Revealed creatures only (D20 b). */
  statBlock?: StatBlock | null;
}

/** Something the server asks of the signed-in player for their own PC. */
export type CombatPrompt =
  /** The DM hit the PC (D7 a): the player applies it to their own character, or not. */
  | { id: string; kind: "damage"; combatant: string; amount: number; damageType?: string | null; text?: string | null }
  | { id: string; kind: "heal"; combatant: string; amount: number; text?: string | null }
  /** The PC took damage while concentrating. */
  | { id: string; kind: "concentration"; combatant: string; dc: number }
  /** The PC is dying and it is their turn. */
  | { id: string; kind: "death_save"; combatant: string };

/** A revealed event, as players may see it (D32: revealed events only). */
export interface CombatLogEntry {
  round: number;
  text: string;
  /** Seconds since the epoch on the wire; milliseconds here. */
  at: number;
}

/** A live combat as the server projects it for the signed-in player. */
export interface PlayerCombat {
  id: string;
  partyId: string;
  partyName?: string;
  /** Moves on with every change players can see. */
  seq: number;
  status: "setup" | "active" | "ended";
  round: number;
  pointer: TurnPointer;
  /** The turn order, visible combatants only. */
  combatants: PlayerCombatant[];
  prompts: CombatPrompt[];
  log: CombatLogEntry[];
}

export interface CombatPlayerDeps {
  fetch: (session: Pick<ServerSession, "baseUrl" | "token">, method: string, path: string, body?: unknown) => Promise<unknown>;
  /** The account's event stream: events, and opens (`again` after a drop). Answers a stop. */
  events: (onEvent: (e: ServerEvent) => void, onOpen: (again: boolean) => void) => () => void;
}

const DEFAULT_DEPS: CombatPlayerDeps = {
  fetch: (session, method, path, body) => serverFetch(session, method, path, body),
  events: (onEvent, onOpen) => {
    const session = loadSession();
    return session ? liveEvents(session, onEvent, { onOpen }) : () => {};
  },
};

export const combatPlayerDeps: CombatPlayerDeps = { ...DEFAULT_DEPS };

/** Tests: start over. */
export function resetCombatPlayer(): void {
  Object.assign(combatPlayerDeps, DEFAULT_DEPS);
}

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject({ code: "unauthorized", message: "Not signed in." });
  return combatPlayerDeps.fetch(session, method, path, body) as Promise<T>;
}

const seg = (id: string) => encodeURIComponent(id);
const notFound = (e: unknown) => (e as { code?: string } | null)?.code === "not_found";

const fromWire = (c: PlayerCombat): PlayerCombat => ({
  ...c,
  combatants: c.combatants ?? [],
  prompts: c.prompts ?? [],
  log: (c.log ?? []).map((l) => ({ ...l, at: l.at * 1000 })),
});

/** The party's live combat as the signed-in player sees it, or null when there is none (or it's not theirs to see). */
export async function liveCombat(partyId: string): Promise<PlayerCombat | null> {
  try {
    const out = await call<{ combat: PlayerCombat | null }>("GET", `/api/parties/${seg(partyId)}/combat`);
    return out.combat ? fromWire(out.combat) : null;
  } catch (e) {
    if (notFound(e)) return null;
    throw e;
  }
}

/** Player writes answer the new projection. `combatant` must be the player's own PC; the server checks. */
const act = (combat: string, action: string, body: Record<string, unknown>) =>
  call<PlayerCombat>("POST", `/api/combats/${seg(combat)}/${action}`, body).then(fromWire);

export const setInitiative = (combat: string, combatant: string, roll: D20Input) => act(combat, "set_initiative", { id: combatant, roll });
export const deathSave = (combat: string, combatant: string, roll: D20Input) => act(combat, "death_save", { id: combatant, roll });
export const resolveConcentration = (combat: string, combatant: string, roll: D20Input) => act(combat, "resolve_concentration", { id: combatant, roll });
/** The damage or healing prompt was applied to the player's own character (by them), or declined. */
export const answerPrompt = (combat: string, prompt: string, applied: boolean) =>
  act(combat, "answer_prompt", { prompt, answer: applied ? "applied" : "declined" });

const COMBAT_EVENT = "party://combat";
const PARTY_EVENT = "party://party";
const RESYNC_EVENT = "party://resync";

export type CombatEvent =
  /** A party's combat changed (started, moved on, ended): fetch it again. */
  | { type: "combat"; party: string; seq: number }
  /** A party changed some other way (members joined or left): its combat may have too. */
  | { type: "party"; party: string }
  /** Events may have been missed: fetch everything again. */
  | { type: "resync" };

/** Combat events, as they come; a reopened stream counts as a resync. Answers a stop. */
export function watchCombats(listener: (e: CombatEvent) => void, onOpen?: () => void): () => void {
  return combatPlayerDeps.events((e) => {
    const p = (e.payload ?? {}) as Record<string, unknown>;
    if (e.event === COMBAT_EVENT && typeof p.party === "string") listener({ type: "combat", party: p.party, seq: Number(p.seq ?? 0) });
    else if (e.event === PARTY_EVENT && typeof p.party === "string") {
      const kind = String(p.kind ?? "");
      listener(kind.startsWith("combat") ? { type: "combat", party: p.party, seq: Number(p.seq ?? 0) } : { type: "party", party: p.party });
    } else if (e.event === RESYNC_EVENT) listener({ type: "resync" });
  }, (again) => {
    onOpen?.();
    if (again) listener({ type: "resync" });
  });
}

/** Whose turn it is, as a combatant id, or null (waiting, an enemy turn, not started, ended). */
export function actingId(c: Pick<PlayerCombat, "pointer">): string | null {
  return c.pointer.kind === "combatant" ? c.pointer.id : null;
}

/** The signed-in player's PC in this combat (the one for `characterId`, when given). */
export function myCombatant(c: Pick<PlayerCombat, "combatants">, characterId?: string | null): PlayerCombatant | null {
  return c.combatants.find((x) => x.mine && (!characterId || x.characterId === characterId)) ?? null;
}
