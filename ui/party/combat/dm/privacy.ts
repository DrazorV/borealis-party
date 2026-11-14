/**
 * The DM screen never knows a player's HP or resources (user, 2026-11-14: "DM should never know players HP and
 * resources"). The server leaves them out of the DM view (party/docs/role-views.md); this module makes sure of it on
 * the screen too, whatever a server sends: every player-side combatant (a PC, or a creature its player controls) is
 * stripped to what the DM may see, before anything renders. What stays: name, initiative, AC, conditions, effects and
 * concentration.
 *
 * The DM still damages and heals them: "Send damage to Aria" (./SendDamage.tsx) asks the player, who applies it on
 * their own Play tab (D7 a).
 */
import { byId } from "./derive";
import type { Combatant, CombatState, DmCombat, PlayerPrompt } from "./types";

/** A PC, or a creature a player controls (their summon, pet or companion): its HP and resources are the player's. */
export function playerSide(c: Combatant): boolean {
  return c.kind === "character" || c.controller?.kind === "player";
}

/** What the DM screen keeps of a player-side combatant's HP: nothing (a zero the screen never shows). */
const NO_HP = Object.freeze({ current: 0, max: 0, temp: 0 });

/**
 * The health band the DM sees for a player's combatant (user, 2026-11-14: "DM can have the same information as
 * players, bloodied, healthy, down"): never a number. The server's own `band` when it sends one, else read from what it
 * sent, before that is stripped.
 */
export type DmBand = "healthy" | "bloodied" | "down" | "stable" | "dead";

export const DM_BAND_LABEL: Record<DmBand, string> = { healthy: "Healthy", bloodied: "Bloodied", down: "Down", stable: "Stable", dead: "Dead" };

const SERVER_BAND: Record<string, DmBand> = { unhurt: "healthy", healthy: "healthy", bloodied: "bloodied", down: "down", stable: "stable", dead: "dead", dying: "down" };

function bandOf(c: Combatant): DmBand | undefined {
  // A copy of a combatant this module already stripped keeps the band it read then.
  const kept = (c as { dmBand?: DmBand }).dmBand;
  if (kept) return kept;
  // The server's `life_state` (alive | dying | stable | dead) says Stable and Dead, which its `band` can't.
  const state = (c as { life_state?: unknown }).life_state;
  if (state === "dead" || state === "stable") return state;
  if (state === "dying") return "down";
  const sent = (c as { band?: unknown }).band;
  if (typeof sent === "string" && SERVER_BAND[sent]) return SERVER_BAND[sent];
  const life = c.life?.state;
  if (life === "dead") return "dead";
  if (life === "stable") return "stable";
  if (life === "dying" || (c as { down?: boolean }).down === true) return "down";
  if (!c.hp || !(c.hp.max > 0)) return undefined;
  if (c.hp.current <= 0) return "down";
  return c.hp.current * 2 <= c.hp.max ? "bloodied" : "healthy";
}

/** The band kept on a stripped player's combatant (see {@link redactCombatant}). */
export const dmBand = (c: Combatant): DmBand | undefined => (c as Combatant & { dmBand?: DmBand }).dmBand;

/** What a player-side combatant never brings to the DM screen besides its HP: death saves, slots, resources, economy. */
const PRIVATE = ["life", "zero_hp", "spell_slots", "resources", "economy", "pending_concentration", "legendary_resistance", "spell_turn"] as const;

/**
 * A player-side combatant as the DM may see it. The server (role-views) sends its concentration as `true`/`false`
 * only; the screen names it from the concentration effect it put on someone (Bless), else "a spell".
 */
export function redactCombatant(c: Combatant, state?: CombatState): Combatant {
  if (!playerSide(c)) return c;
  // Already stripped here (the band is read once, from what the server sent).
  if (STRIPPED.has(c)) return c;
  const band = bandOf(c);
  const kept = { ...c, hp: { ...NO_HP }, ...(band ? { dmBand: band } : {}) } as Combatant;
  delete (kept as { band?: unknown }).band;
  for (const k of PRIVATE) delete kept[k];
  const conc = c.concentration as unknown;
  if (conc === true || (conc !== null && typeof conc === "object" && !(conc as { name?: unknown }).name)) {
    const held = state?.effects.find((e) => e.source === c.id && e.concentration);
    kept.concentration = { name: held?.name ?? "a spell" };
  } else if (conc === false || conc === null) delete kept.concentration;
  STRIPPED.add(kept);
  return kept;
}

/** The combatants this module made, so stripping twice (the mock server and the screen) changes nothing. */
const STRIPPED = new WeakSet<Combatant>();

/** The whole state, with every player-side combatant stripped. */
export function redactState(state: CombatState): CombatState {
  if (!state.combatants.some(playerSide)) return state;
  return { ...state, combatants: state.combatants.map((c) => redactCombatant(c, state)) };
}


/**
 * Damage the DM sent a player and what became of it. A server that keeps answered prompts says so with `status`;
 * one that drops them on an answer leaves the client to see it go (./SendDamage.tsx).
 */
export type SentStatus = "delivered" | "accepted" | "declined";

export interface SentPrompt extends PlayerPrompt {
  status?: SentStatus | "open";
  /** Seconds since the epoch, once answered. */
  resolvedAt?: number | null;
}

/** The DM view as the screen draws it: player-side combatants stripped. Prompts stay (the DM sent their amounts). */
export function redactDmView(view: DmCombat): DmCombat {
  const state = redactState(view.state);
  return state === view.state ? view : { ...view, state };
}

/** The prompts sent to this combatant, newest first. */
export function promptsFor(view: Pick<DmCombat, "prompts">, id: string): SentPrompt[] {
  return ((view.prompts ?? []) as SentPrompt[]).filter((p) => p.combatantId === id).sort((a, b) => (b.at ?? 0) - (a.at ?? 0));
}

/** "Aria", or the owner's name for a creature a player controls ("Owl (Aria's)"). */
export function playerName(state: CombatState, c: Combatant): string {
  if (c.kind === "character") return c.label;
  const pc = c.controller?.kind === "player" ? byId(state, c.controller.pc) : byId(state, c.owner_id);
  return pc ? pc.label : c.label;
}
