/**
 * The turn panel's buttons (0.3.2): every action a combatant has, grouped by what it spends, with its numbers on the
 * button ("Scimitar +4 · 1d6+2", "Fire Breath 5–6", "Fireball L3+ · 2 slots"), and why it can't be used now. The
 * engine is the judge; this only says what the DM can tap.
 */
import type { AbilityName, DamageType } from "./common";
import { abilityStatus, incapacitated, isAlive, isDefeated } from "./derive";
import { evalFormula, varsFor } from "./formula";
import { castingOf, coverageOf, isSpellAbility, kindOfCast, mechOf, spellOf, type Coverage, type StandardAction } from "./spellTypes";
import { featureFor, spellLists } from "./statRef";
import type { Ability, CastSpec, Combatant, CombatState, Edition, MonsterFeature, StatBlock } from "./types";

export type TurnGroup = "action" | "bonus_action" | "reaction" | "legendary" | "free";

export interface TurnAttack { toHit: number; damage: { dice: string; type?: DamageType }[]; melee: boolean }
export interface TurnSave { ability: AbilityName; dc: number; half: boolean; damage: { dice: string; type?: DamageType }[] }

export interface TurnButton {
  key: string;
  group: TurnGroup;
  label: string;
  /** The numbers after the name. */
  detail: string;
  kind: "attack" | "save" | "spell" | "standard" | "use";
  ability?: Ability;
  attack?: TurnAttack;
  save?: TurnSave;
  standard?: StandardAction;
  coverage?: Coverage;
  /** Why it can't be used now, or null. */
  reason: string | null;
}

const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);

export const GROUP_WORD: Record<TurnGroup, string> = {
  action: "Action", bonus_action: "Bonus action", reaction: "Reaction", legendary: "Legendary", free: "Free / lair",
};

/** The damage dice of a compiled definition at its own level ("2d6+3 slashing"). */
function specDamage(a: Ability): { dice: string; type?: DamageType }[] {
  const spec = a.mech;
  if (!spec) return [];
  const vars = varsFor(spec.mech, undefined, spec.cantrip_tier ?? 1, spec.mod ?? 0);
  return (spec.mech.on_land ?? []).flatMap((p) => (p.kind === "damage" ? [{ dice: evalFormula(p.dice, vars), type: p.damage_type }] : []));
}

function attackOf(a: Ability, f: MonsterFeature | undefined): TurnAttack | undefined {
  const spec = a.mech;
  if (spec?.origin.kind === "statblock" && spec.mech.delivery.kind === "attack") {
    return { toHit: spec.attack ?? 0, damage: specDamage(a), melee: spec.mech.delivery.attack === "melee" };
  }
  if (!f?.attack) return undefined;
  return {
    toHit: f.attack.to_hit,
    damage: (f.attack.damage ?? []).map((d) => ({ dice: d.dice ?? String(d.average), type: d.type })),
    melee: /melee/.test(f.attack.kind),
  };
}

/** "18d6" out of "taking 63 (18d6) fire damage": the stat block prints the dice only in its text. */
const diceInText = (text: string): { dice: string; type?: DamageType }[] =>
  [...text.matchAll(/\((\d+d\d+(?:\s*[+-]\s*\d+)?)\)\s*([a-z]+)?\s*damage/gi)].map((m) => ({ dice: m[1]!.replace(/\s+/g, ""), type: m[2]?.toLowerCase() as DamageType | undefined }));

function saveOf(a: Ability, f: MonsterFeature | undefined): TurnSave | undefined {
  const spec = a.mech;
  if (spec?.origin.kind === "statblock" && spec.mech.delivery.kind === "save") {
    const d = spec.mech.delivery;
    return { ability: d.ability, dc: spec.dc ?? 0, half: d.success === "half", damage: specDamage(a) };
  }
  if (!f?.save) return undefined;
  return { ability: f.save.ability, dc: f.save.dc, half: Boolean(f.save.half_on_success), damage: diceInText(f.text) };
}

/** Slots of this level or higher left, for "· 2 slots". */
export function slotsFrom(c: Combatant, level: number): number[] {
  return Object.entries(c.spell_slots ?? {}).filter(([l, p]) => Number(l) >= level && p.remaining > 0).map(([l]) => Number(l)).sort((x, y) => x - y);
}

/** Why a spell can't be cast now (slot, economy, the engine's ability rules), or null. */
export function castReason(state: CombatState, c: Combatant, a: Ability): string | null {
  const mech = mechOf(a);
  const level = mech?.level ?? spellOf(a)?.level ?? 0;
  const kind = mech ? kindOfCast(mech.cast) : a.kind;
  const why = abilityStatus(state, c, { ...a, kind });
  if (why) return why;
  const freq = spellOf(a)?.frequency;
  const bySlot = level > 0 && (freq === undefined || freq === "slot") && !a.uses && spellOf(a)?.fixed_slot == null;
  if (bySlot && slotsFrom(c, level).length === 0) return `No slot of level ${level}+ left`;
  return null;
}

function spellDetail(c: Combatant, a: Ability): string {
  const mech = mechOf(a);
  const link = spellOf(a);
  const level = mech?.level ?? link?.level ?? 0;
  const bits: string[] = [];
  const dmg = specDamage(a)[0];
  if (dmg) bits.push(dmg.dice);
  if (level === 0) bits.push("cantrip");
  else if (link?.frequency === "at_will") bits.push("at will");
  else if (a.uses && c.resources?.[a.uses]) bits.push(`${c.resources[a.uses]!.pool.remaining}/day left`);
  else if (link?.frequency === "per_day") bits.push("per day");
  else {
    const n = slotsFrom(c, level).reduce((sum, l) => sum + (c.spell_slots?.[String(l)]?.remaining ?? 0), 0);
    bits.push(`${n} slot${n === 1 ? "" : "s"}`);
  }
  return bits.join(" · ");
}

const STANDARD: { key: StandardAction; label: string; detail: string }[] = [
  { key: "dodge", label: "Dodge", detail: "attacks vs it at disadvantage" },
  { key: "dash", label: "Dash", detail: "double move" },
  { key: "disengage", label: "Disengage", detail: "no opportunity attacks" },
  { key: "help", label: "Help", detail: "an ally gets advantage" },
];

/** Every button of a combatant's turn panel, in its groups. */
/**
 * A stat block entry that is a feature, not something the creature uses (user, 0.3.5: "Multiattack is not an
 * action"): Multiattack says how many attacks an Attack action makes; the attacks it names are the buttons. Matched by
 * name, so it holds for any installed content (SRD or imported).
 */
export const isPassiveFeature = (a: Pick<Ability, "name" | "kind">): boolean =>
  a.kind === "passive" || /^multiattack\b/i.test(a.name.trim());

/** The passive lines shown above the action buttons: each feature's name and its stat block text. */
export function passiveLines(c: Combatant, block: StatBlock | undefined): { key: string; name: string; text: string }[] {
  return (c.abilities ?? []).filter((a) => a.kind !== "passive" && isPassiveFeature(a)).map((a) => ({
    key: a.key, name: a.name, text: featureFor(block, a)?.text ?? a.text ?? "",
  }));
}

export function turnButtons(state: CombatState, c: Combatant, block: StatBlock | undefined, suggested: ReadonlySet<string> = new Set()): TurnButton[] {
  const listed = new Set(spellLists(block).flatMap((l) => l.lists.flatMap((x) => x.spells.map((s) => s.replace(/\s*\(.*\)$/, "").toLowerCase()))));
  const out: TurnButton[] = [];
  for (const a of (c.abilities ?? []).filter((x) => !isPassiveFeature(x))) {
    const f = featureFor(block, a);
    if (isSpellAbility(a, listed)) {
      const mech = mechOf(a);
      const level = mech?.level ?? spellOf(a)?.level ?? 0;
      out.push({
        key: a.key, group: mech ? kindOfCast(mech.cast) as TurnGroup : a.kind as TurnGroup, ability: a, kind: "spell",
        label: level > 0 ? `${a.name} L${level}${slotsFrom(c, level + 1).length ? "+" : ""}` : a.name,
        detail: spellDetail(c, a), coverage: coverageOf(a, suggested.has(spellOf(a)?.key ?? "")), reason: castReason(state, c, a),
      });
      continue;
    }
    const attack = attackOf(a, f);
    const save = attack ? undefined : saveOf(a, f);
    const recharge = a.recharge ? ` ${a.recharge.min === 6 ? "6" : `${a.recharge.min}–6`}` : "";
    let detail = "";
    if (attack) detail = [signed(attack.toHit), attack.damage.map((d) => d.dice).join(" + ")].filter(Boolean).join(" · ");
    else if (save) detail = `DC ${save.dc} ${save.ability.toUpperCase()}${save.damage.length ? ` · ${save.damage.map((d) => d.dice).join(" + ")}` : ""}`;
    else if (a.kind === "legendary") detail = `${a.legendary_cost ?? 1} LA`;
    out.push({
      key: a.key, group: a.kind as TurnGroup, ability: a, kind: attack ? "attack" : save ? "save" : "use",
      label: `${a.name}${recharge}`, detail, attack, save, reason: abilityStatus(state, c, a),
    });
  }
  const e = c.economy ?? { action: false, bonus_action: false, reaction: false };
  const down = isDefeated(c) ? "Defeated" : !isAlive(c) ? "Down" : incapacitated(state, c.id) ? "Incapacitated" : null;
  for (const s of STANDARD) {
    out.push({ key: `std-${s.key}`, group: "action", label: s.label, detail: s.detail, kind: "standard", standard: s.key, reason: down ?? (e.action ? "Action spent" : null) });
  }
  const melee = out.find((b) => b.kind === "attack" && b.attack?.melee && b.group === "action");
  if (melee) {
    out.push({
      key: `oa-${melee.key}`, group: "reaction", label: "Opportunity attack", detail: `${melee.ability!.name} ${melee.detail}`, kind: "attack",
      ability: melee.ability, attack: melee.attack, reason: down ?? (e.reaction ? "Reaction spent" : null),
    });
  }
  return out;
}

/** The sticky strip while the reaction is unspent: reaction spells (Shield, Counterspell), Parry, opportunity attacks. */
export const reactionButtons = (buttons: TurnButton[]): TurnButton[] => buttons.filter((b) => b.group === "reaction" && !b.reason);

/** The caster's DC and spell attack: the cast spec's, else the block's Spellcasting, else the engine's casting[]. */
export function casterNumbers(c: Combatant, block: StatBlock | undefined, a?: Ability): { dc?: number; attack?: number } {
  const spec = a?.mech;
  if (spec?.dc != null || spec?.attack != null) return { dc: spec.dc, attack: spec.attack };
  const list = spellLists(block)[0];
  if (list) return { dc: list.dc, attack: list.attack };
  const k = castingOf(c)[0];
  return { dc: k?.dc, attack: k?.attack };
}

/**
 * A stat block action's definition when the engine sent none on the ability (`perform_statblock` takes it as
 * `spec`): its printed attack (to hit and damage) or save (DC, ability, half, the dice its text prints).
 */
export function statblockSpec(edition: Edition, a: Ability, f: MonsterFeature | undefined): CastSpec | null {
  const base = { v: 1 as const, key: a.key, edition, name: a.name, level: 0, cast: "action" as const, tier: "auto" as const };
  const attack = attackOf(a, f);
  if (attack) {
    return { v: 1, origin: { kind: "statblock" }, attack: attack.toHit, mech: { ...base, targeting: { kind: "creatures", max: "1" },
      delivery: { kind: "attack", attack: attack.melee ? "melee" : "ranged" },
      on_land: attack.damage.map((d) => ({ kind: "damage" as const, dice: d.dice, ...(d.type ? { damage_type: d.type } : {}) })) } };
  }
  const save = saveOf(a, f);
  if (save) {
    return { v: 1, origin: { kind: "statblock" }, dc: save.dc, mech: { ...base, targeting: { kind: "area", shape: "cone", size_ft: 0 },
      delivery: { kind: "save", ability: save.ability, success: save.half ? "half" : "none" },
      on_land: save.damage.map((d) => ({ kind: "damage" as const, dice: d.dice, ...(d.type ? { damage_type: d.type } : {}) })) } };
  }
  return null;
}
