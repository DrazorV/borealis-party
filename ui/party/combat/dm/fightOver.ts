/**
 * Whether the fight looks over (0.3.2): every hostile creature down, or every player character down. Only a hint for
 * the "All enemies are down" banner; the DM ends a combat, never the app.
 */
import { byId, isCharacter, isDead } from "./derive";
import { playerSide } from "./privacy";
import type { Combatant, CombatState } from "./types";

/** The PC at the top of a creature's owner chain, if any (a PC's pet or summon fights for the party). */
function partyOwner(state: CombatState, c: Combatant): Combatant | undefined {
  let at: Combatant | undefined = c;
  for (let depth = 0; at && depth < 8; depth++) {
    if (isCharacter(at)) return at;
    at = byId(state, at.owner_id);
  }
  return undefined;
}

/** A creature fighting against the party: not a PC, not a PC's creature, not one a player controls. */
export const isHostile = (state: CombatState, c: Combatant): boolean =>
  !isCharacter(c) && c.controller?.kind !== "player" && !partyOwner(state, c);

/**
 * Out of the fight: removed, withdrawn, dead, or at 0 HP (dying or stable). A player's combatant only when removed or
 * withdrawn: the DM screen doesn't know its HP (./privacy.ts), so "Every player character is down" is the table's call.
 */
export const isOut = (c: Combatant): boolean =>
  Boolean(c.removed) || Boolean(c.absent) ||
  (!playerSide(c) && (isDead(c) || (c.life?.state ?? "alive") !== "alive" || c.hp.current <= 0));

export type FightOver = "enemies" | "party";

export function fightOver(state: CombatState): FightOver | null {
  if (state.status !== "active") return null;
  const hostile = state.combatants.filter((c) => isHostile(state, c));
  if (hostile.length > 0 && hostile.every(isOut)) return "enemies";
  const pcs = state.combatants.filter((c) => isCharacter(c) && !c.removed && !c.absent);
  if (pcs.length > 0 && pcs.every(isOut)) return "party";
  return null;
}

/** Who is down, as a key: the banner the DM waved off comes back only when that changes. */
export const outKey = (state: CombatState): string => state.combatants.filter(isOut).map((c) => c.id).sort().join(",");
