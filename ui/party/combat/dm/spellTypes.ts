/**
 * Spells and mechanics in combat (0.3.2): the JSON the tracker reads and the commands it sends for casts, statblock
 * actions and the standard actions, as the UI expects them from the engine (party-combat `spell.rs`, `command.rs`).
 *
 * PROVISIONAL: written before the engine's contract commit (shared note feat-spell-ui-contract-asks.md). When the
 * engine's types.ts mirror lands, these become re-exports of it and the casts below go away. Everything here is read
 * through the helpers at the bottom, so the tracker keeps working on a 0.3.1 state that has none of these fields.
 */
import type { AbilityName, D20Input, D20Result, DamageType, DiceInput } from "./common";
import type { Ability, Combatant, Command, CombatState, Effect, EffectSpec } from "./types";

export type CastTime = "action" | "bonus" | "reaction" | "minute";
/** How well the app knows a spell's mechanics (the Spells tab's badge). */
export type Coverage = "auto" | "partial" | "ruling" | "unlinked" | "suggested";
export type Tier = "auto" | "partial" | "utility" | "ruling" | "uncurated";

/** A small grammar over dice: "8d6+(slot_above)d6", "1+slot_above", "mod". */
export type Formula = string | number;

export type TargetFilter =
  | { kind: "any" } | { kind: "willing" } | { kind: "ally" } | { kind: "enemy" }
  | { kind: "type"; types: string[] };

export type Targeting =
  | { kind: "self" }
  | { kind: "creatures"; max: Formula; filter?: TargetFilter; range_ft?: number }
  | { kind: "area"; shape: string; size_ft: number };

export type Delivery =
  | { kind: "auto" }
  | { kind: "attack"; attack_kind?: "melee" | "ranged"; rays?: Formula }
  | { kind: "save"; ability: AbilityName; success: "half" | "none" };

export interface SummonOption { cr: number; count: number }

export type Primitive =
  | { kind: "damage"; dice: Formula; type?: DamageType; choice?: DamageType[] }
  | { kind: "heal"; dice: Formula; add_mod?: boolean }
  | { kind: "temp_hp"; dice: Formula }
  | { kind: "condition"; condition: string; repeat_save?: { ability: AbilityName; at: "start" | "end" }; ends_on_damage?: boolean }
  | { kind: "ac_base"; base: number; add_dex: boolean }
  | { kind: "ac_bonus"; value: number }
  | { kind: "ac_min"; value: number }
  | { kind: "roll_dice"; on: "attack" | "save"; term: string; sign?: 1 | -1 }
  | { kind: "roll_mode"; on: string; mode: "adv" | "dis" }
  | { kind: "speed"; op: "add" | "mult" | "zero"; value?: number }
  | { kind: "zone"; ability: AbilityName; damage?: { dice: Formula; type?: DamageType }; half?: boolean }
  /** 0.3.2: no spawning (user decision); the tracker shows the options and opens Add creature. */
  | { kind: "summon_info"; options: SummonOption[]; creature_type?: string; per_slot?: Record<string, number>; note?: string }
  | { kind: "ruling"; question: string; buttons: string[] }
  | { kind: "note"; key: string; text?: string };

/** A spell's mechanics (the engine's SpellMech, compiled for this caster as the CastSpec template). */
export interface SpellMech {
  key: string;
  name: string;
  edition?: "2014" | "2024";
  level: number;
  concentration: boolean;
  cast: CastTime;
  targeting: Targeting;
  delivery: Delivery;
  on_land?: Primitive[];
  on_resist?: Primitive[];
  always?: Primitive[];
  /** In the caster's turns; null = until removed. */
  duration_turns?: number | null;
  tier: Tier;
  /** The 2014 bonus-action rule and others, as short words. */
  notes?: string[];
  /** It ends when its target attacks / takes damage (Invisibility, Sanctuary): the "It attacked: end" reminder. */
  ends_on?: ("attack" | "damage" | "cast")[];
  dc?: number;
  attack?: number;
  mod?: number;
}

export interface SpellLink {
  key: string;
  element_id?: string;
  name: string;
  level: number;
  ruleset?: string;
  status: "linked" | "ambiguous" | "unlinked";
  coverage?: Coverage;
  /** At will, N per day, or a slot (Overrides.spells frequency). */
  frequency?: "at_will" | { per_day: number } | "slot";
}

/** A statblock action's numbers (Feature.attack / Feature.save), when the engine copies them onto the ability. */
export interface StatAttack { to_hit: number; damage?: { dice: string; type?: DamageType }[]; reach?: number; range?: number }
export interface StatSave { ability: AbilityName; dc: number; half_on_success?: boolean; damage?: { dice: string; type?: DamageType }[] }

export type SpellAbility = Ability & { spell?: SpellLink; mech?: SpellMech; attack?: StatAttack; save?: StatSave };

export interface Casting { ability?: AbilityName; dc?: number; attack?: number; caster_level?: number; cantrip_tier?: number }
export type SpellCombatant = Combatant & {
  stats?: { mods: number[]; saves: number[]; magic_resistance?: boolean };
  casting?: Casting[];
};

export type RowOutcome = "failed" | "saved" | "hit" | "miss" | "crit";

export interface CastRow {
  target: string;
  kind: "save" | "attack";
  ability?: AbilityName;
  outcome?: RowOutcome;
  roll?: D20Result;
  lr_used?: boolean;
}

/** A cast or statblock action waiting on rolls (the "1 pending" badge). */
export interface PendingCast {
  id: string;
  caster: string;
  ability?: string;
  name: string;
  slot?: number;
  dc?: number;
  attack?: number;
  delivery: Delivery;
  rows: CastRow[];
  damage?: { dice: string; type?: DamageType; half_on_save: boolean };
  heal?: { dice: string };
  status: "open" | "applied" | "cancelled";
}

export type StandardAction = "dodge" | "dash" | "disengage" | "help";

/** The new commands (`party_combat::Command`, tagged by `command`). */
export type SpellCommand =
  | { command: "cast_spell"; caster: string; ability?: string; adhoc?: SpellMech; slot?: number; targets: string[];
      rolls?: { target: string; roll: D20Input }[]; damage?: DiceInput; choices?: { damage_type?: DamageType }; precast?: boolean }
  | { command: "resolve_cast_row"; cast: string; target: string; roll?: D20Input; outcome?: RowOutcome; use_lr?: boolean }
  | { command: "apply_cast"; cast: string; damage?: DiceInput }
  | { command: "cancel_cast"; cast: string; refund?: boolean }
  | { command: "apply_zone"; effect: number; targets: string[] }
  | { command: "perform_statblock"; id: string; ability: string; targets: string[]; as_reaction?: boolean }
  | { command: "standard_action"; id: string; action: StandardAction; target?: string };

export type AnyCommand = Command | SpellCommand;

/** The tracker's `send` takes the engine's Command; the new ones go through it until the mirror has them. */
export const asCommand = (c: SpellCommand): Command => c as unknown as Command;

/** The effect fields 0.3.2 adds (EffectSpec.spell, dismiss_on_end, ends_on, zone). */
export type SpellEffect = Effect & {
  spell?: { key: string; name: string; cast?: string };
  dismiss_on_end?: boolean;
  ends_on?: ("attack" | "damage" | "cast")[];
  zone?: { ability: AbilityName; dc: number; damage?: { dice: string; type?: DamageType }; half?: boolean };
};
export type SpellEffectSpec = EffectSpec & Partial<Pick<SpellEffect, "spell" | "dismiss_on_end" | "ends_on" | "zone">>;

// --- readers that tolerate a state without the new fields ---

export const castsOf = (state: CombatState): PendingCast[] => ((state as CombatState & { casts?: PendingCast[] }).casts ?? []);
export const openCasts = (state: CombatState): PendingCast[] => castsOf(state).filter((c) => c.status === "open");
export const spellOf = (a: Ability): SpellLink | undefined => (a as SpellAbility).spell;
export const mechOf = (a: Ability): SpellMech | undefined => (a as SpellAbility).mech;
export const castingOf = (c: Combatant): Casting[] => (c as SpellCombatant).casting ?? [];
export const asSpellEffect = (e: Effect): SpellEffect => e as SpellEffect;

/** Whether an ability is a spell: linked, with mechanics, or named in the block's spell lists. */
export const isSpellAbility = (a: Ability, listed: Set<string>): boolean =>
  Boolean(spellOf(a) || mechOf(a)) || listed.has(a.name.toLowerCase());

export function coverageOf(a: Ability): Coverage {
  const link = spellOf(a);
  if (link?.coverage) return link.coverage;
  if (link?.status === "unlinked") return "unlinked";
  const tier = mechOf(a)?.tier;
  if (tier === "auto") return "auto";
  if (tier === "partial" || tier === "utility") return "partial";
  if (tier) return "ruling";
  return link ? "ruling" : "unlinked";
}

export const COVERAGE_WORD: Record<Coverage, string> = {
  auto: "Auto", partial: "Partial", ruling: "Ruling", unlinked: "Unlinked", suggested: "Suggested",
};
export const COVERAGE_HELP: Record<Coverage, string> = {
  auto: "The app applies all of it.",
  partial: "The app applies part of it; the rest is a note for you.",
  ruling: "The app spends the slot and the action; you rule what happens.",
  unlinked: "Not linked to a spell in your content yet.",
  suggested: "Drafted from your own spell data's tags: check it once and confirm.",
};

export const castWord = (t: CastTime): string => (t === "bonus" ? "Bonus" : t === "reaction" ? "Reaction" : t === "minute" ? "1 min" : "Action");
export const kindOfCast = (t: CastTime): Ability["kind"] => (t === "bonus" ? "bonus_action" : t === "reaction" ? "reaction" : "action");
