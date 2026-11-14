import type { CompanionFeature } from "../../src/api/contract";
import { type DiceExpr, damageDice } from "../../src/lib/dice";

/** An attack a companion's stat block prints, as the Play tab's companion card rolls it (0.3.6). */
export interface CompanionAttack {
  name: string;
  /** The attack roll's bonus. */
  toHit: number;
  /** The Hit line's dice ("2d4 + 2"), null when it prints none. */
  damage: DiceExpr | null;
  /** The Hit line's damage as printed ("7 (2d4 + 2) piercing"), for the button's title. */
  damageText: string | null;
}

// 2014: "Melee Weapon Attack: +4 to hit"; 2024: "Melee Attack Roll: +4". A spell attack reads the same.
const TO_HIT = /\bAttack(?: Roll)?:\s*([+-]\s*\d+)/i;
const TO_HIT_LOOSE = /([+-]\s*\d+)\s+to hit\b/i;
// "Hit: 7 (2d4 + 2) piercing damage", "Hit: 1 piercing damage", "Hit: 9 (1d10 + 4) Bludgeoning damage plus 3 (1d6) Fire".
const HIT = /\bHit:\s*(\d+)\s*(?:\(([^)]*)\))?\s*([A-Za-z]+)?/i;

/** The attacks among a companion's actions: those whose text has an attack roll. Others (Multiattack) aren't buttons. */
export function companionAttacks(actions: readonly CompanionFeature[]): CompanionAttack[] {
  const out: CompanionAttack[] = [];
  for (const a of actions) {
    const text = a.text ?? "";
    const m = TO_HIT.exec(text) ?? TO_HIT_LOOSE.exec(text);
    if (!m) continue;
    const toHit = Number(m[1]!.replace(/\s+/g, ""));
    if (!Number.isFinite(toHit)) continue;
    const hit = HIT.exec(text);
    let damage: DiceExpr | null = null;
    let damageText: string | null = null;
    if (hit) {
      const kind = hit[3] && !/^damage$/i.test(hit[3]) ? ` ${hit[3].toLowerCase()}` : "";
      damage = damageDice(hit[2] ?? hit[1]!);
      damageText = `${hit[1]}${hit[2] ? ` (${hit[2].trim()})` : ""}${kind}`;
    }
    out.push({ name: a.name.replace(/\.\s*$/, "").trim() || "Attack", toHit, damage, damageText });
  }
  return out;
}

export const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);
