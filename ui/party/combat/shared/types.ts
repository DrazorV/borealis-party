/** Types the player's Combat sub-tab and the DM tracker share. */

/**
 * One d20 roll as the combat engine takes it (`party-dice` `D20Input`, party/docs/combat-engine.md): a typed result
 * from the table's dice, or an in-app roll the server makes (D21, D43). A typed result never touches the die.
 */
export type D20Input =
  | { kind: "typed"; total: number; natural?: number }
  | { kind: "app"; modifier: number; mode?: "normal" | "advantage" | "disadvantage" }
  /** 0.3.5: a pass/fail roll's result pressed at the table, with no number (`party_dice::TableOutcome`). */
  | { kind: "outcome"; outcome: TableOutcome };

/** What the table's die gave on a pass/fail roll. */
export type TableOutcome = "pass" | "fail" | "nat20" | "nat1";

/** Damage, healing or recharge dice the same way (`party-dice` `DiceInput`). */
export type DiceInput =
  | { kind: "typed"; total: number }
  | { kind: "app"; notation: string; critical?: boolean };

/** A creature's printed stat block: upstream's `MonsterData` (docs/monster-model.md), as far as the combat views read it. */
export interface StatBlock {
  size?: string[];
  creature_type?: string;
  tags?: string[];
  alignment?: string;
  ac?: { value: number; source?: string; condition?: string }[];
  ac_note?: string;
  hp?: { average: number; formula?: string };
  hp_note?: string;
  speed?: Partial<Record<"walk" | "burrow" | "climb" | "fly" | "swim", number>> & { hover?: boolean; note?: string };
  abilities?: Partial<Record<"str" | "dex" | "con" | "int" | "wis" | "cha", number>>;
  saves?: Partial<Record<string, number>>;
  skills?: Partial<Record<string, number>>;
  damage_vulnerabilities?: { types: string[]; text?: string }[];
  damage_resistances?: { types: string[]; text?: string }[];
  damage_immunities?: { types: string[]; text?: string }[];
  condition_immunities?: string[];
  senses?: Partial<Record<string, number | string | boolean>> & { passive_perception?: number; notes?: string };
  languages?: string[];
  cr?: string;
  cr_note?: string;
  traits?: { name: string; text: string }[];
  actions?: { name: string; text: string }[];
  bonus_actions?: { name: string; text: string }[];
  reactions?: { name: string; text: string }[];
  legendary?: { preamble?: string; actions: { name: string; text: string }[] };
}

