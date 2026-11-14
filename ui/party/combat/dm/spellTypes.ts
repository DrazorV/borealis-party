/**
 * Spells and mechanics in combat (0.3.2): the tracker's helpers over the engine's JSON (party-combat `spell.rs`,
 * mirrored in ./types.ts). Everything is read through these, so the tracker keeps working on a 0.3.1 state that has
 * none of the new fields.
 */
import type { AbilityName, DamageType } from "./common";
import { evalFormula } from "./formula";
import type {
  Ability, CastSpec, Combatant, Command, CombatState, Effect, FormulaVars, PendingCast, Primitive, RowOutcome, SpellMech, SpellTag,
} from "./types";

export type {
  AmountRoll, CastRow, CastSpec, CastTime, Delivery, Formula, PendingCast, Primitive, RowInput, RowOutcome, RowRoll, SpellLink,
  SpellMech, SpellTag, StandardActionKind as StandardAction, SummonInfo, TargetFilter, Targeting, Tier, ZoneSpec,
} from "./types";

/** How well the app knows a spell's mechanics (the Spells tab's badge). */
export type Coverage = "auto" | "partial" | "ruling" | "unlinked" | "suggested";

/** An effect as the spell chips read it (`spell` tag, `dismiss_on_end`, `ends_on_attack`: all in the engine's mirror). */
export type SpellEffect = Effect;

/** The tracker's `send` takes the engine's Command; kept as a no-op so call sites read the same. */
export const asCommand = (c: Command): Command => c;

// --- abilities ---

/** The compiled definition: a spell's, or a stat block attack or save's. */
export const specOf = (a: Ability): CastSpec | undefined => a.mech;
/** The spell mechanics, for a spell (not a stat block action). */
export const mechOf = (a: Ability): SpellMech | undefined => (a.mech && a.mech.origin.kind !== "statblock" ? a.mech.mech : undefined);
export const spellOf = (a: Ability) => a.spell;
export const isStatblockMech = (a: Ability): boolean => a.mech?.origin.kind === "statblock";
export const castingOf = (c: Combatant) => c.casting ?? [];

/** Whether an ability is a spell: linked, with spell mechanics, or named in the block's spell lists. */
export const isSpellAbility = (a: Ability, listed: Set<string>): boolean =>
  Boolean(a.spell || mechOf(a)) || listed.has(a.name.toLowerCase());

/**
 * The badge. The engine's origin says where the rules come from: the curated SRD table (its tier), the DM's own
 * definition, or none (a ruling). A spell with no key in the DM's content is Unlinked. "Suggested" is the server's
 * (a tag draft waits for the DM), passed in by the caller.
 */
export function coverageOf(a: Ability, suggested = false): Coverage {
  const spec = a.mech;
  if (!spec || spec.origin.kind === "statblock") {
    if (a.spell && !a.spell.key) return "unlinked";
    return suggested ? "suggested" : "ruling";
  }
  if (spec.origin.kind === "ruling") return suggested ? "suggested" : "ruling";
  const tier = spec.mech.tier;
  if (tier === "auto") return "auto";
  if (tier === "partial" || tier === "utility") return "partial";
  return suggested ? "suggested" : "ruling";
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

/** The server's spell list coverage words ("defined", "linked", or a tier) as a badge. */
export function coverageWord(server: string): Coverage {
  if (server === "auto" || server === "defined") return server === "auto" ? "auto" : "partial";
  if (server === "partial" || server === "utility" || server === "linked") return "partial";
  if (server === "suggested") return "suggested";
  return "ruling";
}

export const kindOfCast = (t: SpellMech["cast"]): Ability["kind"] => (t === "bonus" ? "bonus_action" : t === "reaction" ? "reaction" : "action");

// --- pending casts ---

export const castsOf = (state: CombatState): PendingCast[] => state.casts ?? [];
/** The casts waiting on rolls or amounts (applied and cancelled ones leave the list). */
export const openCasts = (state: CombatState): PendingCast[] => castsOf(state);

export const castName = (k: PendingCast): string => k.spec.mech.name;

/** One amount the cast asks for (damage, healing, temp HP), in the definition's order. */
export interface CastAmount { kind: "damage" | "heal" | "temp_hp"; dice: string; type?: DamageType }

const AMOUNT_KINDS = new Set(["damage", "heal", "temp_hp"]);

/** The vars a pending cast was compiled with (the engine sends them; else the slot it used). */
export function castVars(k: PendingCast): FormulaVars {
  if (k.vars) return k.vars;
  const level = k.spec.mech.level;
  const slot = k.slot ?? level;
  return { slot, slot_above: Math.max(0, slot - level), mod: k.spec.mod ?? 0, cantrip_tier: k.spec.cantrip_tier ?? 1 };
}

/** The amounts a pending cast asks for, with its dice at its slot ("9d6 fire"). */
export function castAmounts(k: PendingCast): CastAmount[] {
  const vars = castVars(k);
  return (k.spec.mech.on_land ?? []).filter((p) => AMOUNT_KINDS.has(p.kind)).map((p) => {
    const q = p as Extract<Primitive, { kind: "damage" | "heal" | "temp_hp" }>;
    const dice = evalFormula(q.dice, vars) + ("add_mod" in q && q.add_mod && vars.mod ? `+${vars.mod}` : "");
    return { kind: q.kind, dice: dice.replace(/\+-/g, "-"), ...(q.kind === "damage" ? { type: k.choices?.damage_type ?? q.damage_type } : {}) };
  });
}

export const halfOnSave = (k: PendingCast): boolean => k.spec.mech.delivery.kind === "save" && k.spec.mech.delivery.success === "half";
export const rowKind = (k: PendingCast): "save" | "attack" | "auto" => k.spec.mech.delivery.kind;
export const rowAbility = (k: PendingCast): AbilityName | undefined => (k.spec.mech.delivery.kind === "save" ? k.spec.mech.delivery.ability : undefined);
export const landed = (o: RowOutcome | undefined): boolean => o === "failed" || o === "hit" || o === "crit" || o === "lands";

// --- effects ---

export const asSpellEffect = (e: Effect): SpellEffect => e as SpellEffect;
export const tagOf = (e: Effect): SpellTag | undefined => e.spell;
