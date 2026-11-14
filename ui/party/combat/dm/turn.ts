/**
 * The turn panel's buttons (0.3.2): every action a combatant has, grouped by what it spends, with its numbers on the
 * button ("Scimitar +4 · 1d6+2", "Fire Breath 5–6", "Fireball L3+ · 2 slots"), and why it can't be used now. The
 * engine is the judge; this only says what the DM can tap.
 */
import type { AbilityName, DamageType } from "./common";
import { abilityStatus, incapacitated, isAlive, isDefeated } from "./derive";
import { evalFormula, varsFor } from "./formula";
import { castingOf, coverageOf, isSpellAbility, kindOfCast, mechOf, spellOf, type Coverage, type SpellAbility, type StandardAction } from "./spellTypes";
import { featureFor, spellLists } from "./statRef";
import type { Ability, Combatant, CombatState, MonsterFeature, StatBlock } from "./types";

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

function attackOf(a: Ability, f: MonsterFeature | undefined): TurnAttack | undefined {
  const own = (a as SpellAbility).attack;
  if (own) return { toHit: own.to_hit, damage: own.damage ?? [], melee: own.reach != null };
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
  const own = (a as SpellAbility).save;
  if (own) return { ability: own.ability, dc: own.dc, half: Boolean(own.half_on_success), damage: own.damage ?? [] };
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
  const bySlot = level > 0 && (freq === undefined || freq === "slot") && !a.uses;
  if (bySlot && slotsFrom(c, level).length === 0) return `No slot of level ${level}+ left`;
  return null;
}

function spellDetail(c: Combatant, a: Ability): string {
  const mech = mechOf(a);
  const link = spellOf(a);
  const level = mech?.level ?? link?.level ?? 0;
  const bits: string[] = [];
  const dmg = mech?.on_land?.find((p) => p.kind === "damage");
  if (dmg && dmg.kind === "damage") bits.push(evalFormula(dmg.dice, varsFor(mech!, undefined)));
  if (level === 0) bits.push("cantrip");
  else if (link?.frequency === "at_will") bits.push("at will");
  else if (link?.frequency && typeof link.frequency === "object") bits.push(`${link.frequency.per_day}/day`);
  else if (a.uses && c.resources?.[a.uses]) bits.push(`${c.resources[a.uses]!.pool.remaining} left`);
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
export function turnButtons(state: CombatState, c: Combatant, block: StatBlock | undefined): TurnButton[] {
  const listed = new Set(spellLists(block).flatMap((l) => l.lists.flatMap((x) => x.spells.map((s) => s.replace(/\s*\(.*\)$/, "").toLowerCase()))));
  const out: TurnButton[] = [];
  for (const a of (c.abilities ?? []).filter((x) => x.kind !== "passive")) {
    const f = featureFor(block, a);
    if (isSpellAbility(a, listed)) {
      const mech = mechOf(a);
      const level = mech?.level ?? spellOf(a)?.level ?? 0;
      out.push({
        key: a.key, group: mech ? kindOfCast(mech.cast) as TurnGroup : a.kind as TurnGroup, ability: a, kind: "spell",
        label: level > 0 ? `${a.name} L${level}${slotsFrom(c, level + 1).length ? "+" : ""}` : a.name,
        detail: spellDetail(c, a), coverage: coverageOf(a), reason: castReason(state, c, a),
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
  const mech = a ? mechOf(a) : undefined;
  if (mech?.dc != null || mech?.attack != null) return { dc: mech.dc, attack: mech.attack };
  const list = spellLists(block)[0];
  if (list) return { dc: list.dc, attack: list.attack };
  const k = castingOf(c)[0];
  return { dc: k?.dc, attack: k?.attack };
}
