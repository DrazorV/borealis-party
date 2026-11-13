/**
 * The creature card's quick reference (C9), read from the stat block snapshot: attack bonuses, the spell attack, save
 * DCs and saving throws. Nothing is invented: what the block doesn't print is left out.
 */
import { ABILITY_NAMES, type AbilityName } from "./common";
import type { Ability, MonsterFeature, StatBlock } from "./types";

export interface QuickRef {
  attacks: string[];
  spellAttack: string[];
  dcs: string[];
  saves: { ability: AbilityName; bonus: number; proficient: boolean }[];
}

const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);
const mod = (score: number) => Math.floor((score - 10) / 2);
const ABILITY_WORD: Record<AbilityName, string> = { str: "STR", dex: "DEX", con: "CON", int: "INT", wis: "WIS", cha: "CHA" };

export function features(block: StatBlock | undefined): MonsterFeature[] {
  if (!block) return [];
  return [...(block.traits ?? []), ...(block.actions ?? []), ...(block.bonus_actions ?? []), ...(block.reactions ?? []), ...(block.legendary?.actions ?? [])];
}

export function quickRef(block: StatBlock | undefined): QuickRef {
  const all = features(block);
  const attacks = all.filter((f) => f.attack).map((f) => `${f.name} ${signed(f.attack!.to_hit)}`);
  const casting = all.filter((f) => f.spellcasting);
  const spellAttack = casting.filter((f) => f.spellcasting!.attack_bonus != null).map((f) => signed(f.spellcasting!.attack_bonus!));
  const dcs: string[] = [];
  for (const f of casting) if (f.spellcasting!.save_dc != null) dcs.push(`Spell DC ${f.spellcasting!.save_dc}`);
  for (const f of all) if (f.save) dcs.push(`${f.name} DC ${f.save.dc} ${ABILITY_WORD[f.save.ability] ?? f.save.ability.toUpperCase()}`);
  const saves = block?.abilities
    ? ABILITY_NAMES.map((a) => {
      const printed = block.saves?.[a];
      return { ability: a, bonus: printed ?? mod(block.abilities![a] ?? 10), proficient: printed != null };
    })
    : [];
  return { attacks: [...new Set(attacks)], spellAttack: [...new Set(spellAttack)], dcs: [...new Set(dcs)], saves };
}

/** The save bonus for a concentration (CON) save, or 0 when the block has none. */
export function saveBonus(block: StatBlock | undefined, ability: AbilityName): number {
  if (!block?.abilities) return 0;
  return block.saves?.[ability] ?? mod(block.abilities[ability] ?? 10);
}

export function initiativeText(block: StatBlock | undefined, bonus: number | undefined): string {
  if (block?.initiative) return `${signed(block.initiative.modifier)} (${block.initiative.score})`;
  return signed(bonus ?? 0);
}

/** The stat block's text for an ability, matched by name. */
export function featureFor(block: StatBlock | undefined, ability: Ability): MonsterFeature | undefined {
  const name = ability.name.toLowerCase();
  return features(block).find((f) => f.name.toLowerCase() === name);
}

/** The spells a block's Spellcasting lists, with how often (C18). */
export function spellLists(block: StatBlock | undefined): { feature: string; dc?: number; attack?: number; lists: { label: string; spells: string[] }[] }[] {
  return features(block).filter((f) => f.spellcasting).map((f) => ({
    feature: f.name,
    dc: f.spellcasting!.save_dc,
    attack: f.spellcasting!.attack_bonus,
    lists: f.spellcasting!.lists.map((l) => ({
      label: l.frequency === "at_will" ? "At will" : `${l.frequency.per_day.uses}/day${l.frequency.per_day.each ? " each" : ""}`,
      spells: l.spells.map((s) => s.name + (s.note ? ` (${s.note})` : "")),
    })),
  }));
}

export const KIND_WORD: Record<Ability["kind"], string> = {
  action: "Action", bonus_action: "Bonus action", reaction: "Reaction", legendary: "Legendary", free: "Free", passive: "Passive",
};
