/**
 * The tracker's basic combat words: damage types, conditions, the health band and the roll inputs, matching the JSON
 * of the `party-combat` and `party-dice` crates (party/docs/combat-engine.md). The player's Combat sub-tab has its own
 * in ../shared (feat/player-combat); the two can be joined once both are merged.
 */

export const DAMAGE_TYPES = [
  "acid", "bludgeoning", "cold", "fire", "force", "lightning", "necrotic",
  "piercing", "poison", "psychic", "radiant", "slashing", "thunder",
] as const;
export type DamageType = (typeof DAMAGE_TYPES)[number];

/** The SRD conditions. Exhaustion has levels, so it is a number on the combatant instead. */
export const CONDITIONS = [
  "blinded", "charmed", "deafened", "frightened", "grappled", "incapacitated", "invisible",
  "paralyzed", "petrified", "poisoned", "prone", "restrained", "stunned", "unconscious",
] as const;
export type Condition = (typeof CONDITIONS)[number];

/** Incapacitated, or a condition that includes it: these end concentration (C10). */
export const INCAPACITATING: readonly Condition[] = ["incapacitated", "paralyzed", "petrified", "stunned", "unconscious"];
/** The condition stops movement (speed 0). */
export const STOPS_MOVEMENT: readonly Condition[] = ["grappled", "restrained", "paralyzed", "petrified", "stunned", "unconscious"];

export type AbilityName = "str" | "dex" | "con" | "int" | "wis" | "cha";
export const ABILITY_NAMES: readonly AbilityName[] = ["str", "dex", "con", "int", "wis", "cha"];

/** The health band players see (plan D6): the same rule as the party card's band. */
export type HealthBand = "unhurt" | "bloodied" | "down";

export interface Hp {
  current: number;
  max: number;
  temp?: number;
}

/** `Hp::band`: null when max HP is unknown. */
export function healthBand(hp: Pick<Hp, "current" | "max">): HealthBand | null {
  if (hp.max <= 0) return null;
  if (hp.current <= 0) return "down";
  if (hp.current * 2 <= hp.max) return "bloodied";
  return "unhurt";
}

export type RollMode = "normal" | "advantage" | "disadvantage";

/** A d20 test (initiative, death save, a save): a typed table roll (D43), or rolled by the server (D21). */
export type D20Input =
  | { kind: "typed"; total: number; natural?: number }
  | { kind: "app"; modifier?: number; mode?: RollMode; crit_range?: number }
  /** 0.3.5: a pass/fail roll's result pressed at the table, with no number (`party_dice::TableOutcome`). */
  | { kind: "outcome"; outcome: TableOutcome };

export type TableOutcome = "pass" | "fail" | "nat20" | "nat1";

/** Damage, healing, recharge: a typed total, or dice the server rolls. */
export type DiceInput =
  | { kind: "typed"; total: number }
  | { kind: "app"; notation: string; critical?: boolean; floor?: number };

/** A d20 roll the server made or took typed (`party_dice::D20Result`). */
export interface D20Result {
  source: "typed" | "app";
  mode: RollMode;
  dice: number[];
  natural: number | null;
  modifier: number | null;
  total: number;
  is_nat20: boolean;
  is_nat1: boolean;
}

/** A dice roll (`party_dice::DiceResult`). */
export interface DiceResult {
  source?: "typed" | "app";
  notation?: string;
  total: number;
  breakdown?: string;
}

/** Human words. */
export const BAND_WORD: Record<HealthBand, string> = { unhurt: "Unhurt", bloodied: "Bloodied", down: "Down" };
export const conditionWord = (c: Condition): string => c.charAt(0).toUpperCase() + c.slice(1);
export const abilityWord = (a: AbilityName): string => a.toUpperCase();
