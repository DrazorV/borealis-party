/**
 * The live preview of a spell's numbers in the cast sheet ("9d6 at L4"): the engine's Formula grammar evaluated for a
 * slot. Only for showing; the server compiles the CastSpec and rolls (D21). Variables: slot, slot_above, mod,
 * cantrip_tier. Unknown text is returned as it is.
 */
import type { DiceTerm, Formula, SpellMech } from "./types";

export interface FormulaVars { slot?: number; slot_above?: number; mod?: number; cantrip_tier?: number }

const VAR = /\b(slot_above|slot|mod|cantrip_tier)\b/g;

function arithmetic(expr: string): number | null {
  if (!/^[\d+\-*\s]+$/.test(expr)) return null;
  // Products first, then a left-to-right sum.
  const terms = expr.replace(/\s+/g, "").split(/(?=[+-])/);
  let total = 0;
  for (const t of terms) {
    if (!t) continue;
    const sign = t.startsWith("-") ? -1 : 1;
    const body = t.replace(/^[+-]/, "");
    if (!body) return null;
    const product = body.split("*").reduce<number | null>((n, f) => (n === null || !/^\d+$/.test(f) ? null : n * Number(f)), 1);
    if (product === null) return null;
    total += sign * product;
  }
  return total;
}

/** "8d6+(slot_above)d6" at slot 4 of a 3rd-level spell → "9d6"; "1+slot_above" → "2". */
export function evalFormula(f: Formula, vars: FormulaVars): string {
  if (typeof f === "number") return String(f);
  let s = f.replace(VAR, (_m, v: keyof FormulaVars) => String(vars[v] ?? 0)).replace(/\s+/g, "");
  // Innermost parentheses of plain arithmetic become numbers: "(1)d6" → "1d6".
  for (let i = 0; i < 20; i++) {
    const next = s.replace(/\(([^()]*)\)/g, (m, inner: string) => {
      const n = arithmetic(inner);
      return n === null ? m : String(n);
    });
    if (next === s) break;
    s = next;
  }
  if (/[()]/.test(s)) return f;
  const dice = new Map<number, number>();
  let flat = 0;
  for (const raw of s.split(/(?=[+-])/)) {
    if (!raw) continue;
    const sign = raw.startsWith("-") ? -1 : 1;
    const t = raw.replace(/^[+-]/, "");
    const d = /^(\d+(?:\*\d+)*)?d(\d+)$/.exec(t);
    if (d) {
      const count = d[1] ? arithmetic(d[1])! : 1;
      dice.set(Number(d[2]), (dice.get(Number(d[2])) ?? 0) + sign * count);
      continue;
    }
    const n = arithmetic(t);
    if (n === null) return f;
    flat += sign * n;
  }
  const parts = [...dice].filter(([, n]) => n !== 0).sort(([a], [b]) => b - a).map(([size, n]) => `${n}d${size}`);
  if (flat !== 0 || parts.length === 0) parts.push(String(flat));
  return parts.join("+").replace(/\+-/g, "-");
}

/** A whole-number formula (a target count): null when it isn't one. */
export function evalCount(f: Formula, vars: FormulaVars): number | null {
  const v = evalFormula(f, vars);
  return /^-?\d+$/.test(v) ? Number(v) : null;
}

export const varsFor = (mech: Pick<SpellMech, "level">, slot: number | undefined, cantripTier = 1, mod = 0): Required<FormulaVars> => ({
  slot: slot ?? mech.level, slot_above: Math.max(0, (slot ?? mech.level) - mech.level), mod, cantrip_tier: cantripTier,
});

export const termText = (t: DiceTerm): string => `${t.sign === "minus" ? "−" : "+"}${t.count}d${t.sides}`;

/** The cast sheet's one-line preview: "9d6 fire at L4 · DEX save half · up to 2 targets". */
export function preview(mech: SpellMech, slot: number | undefined, cantripTier = 1, mod = 0): string {
  const vars = varsFor(mech, slot, cantripTier, mod);
  const bits: string[] = [];
  for (const p of [...(mech.on_land ?? []), ...(mech.always ?? [])]) {
    if (p.kind === "damage") bits.push(`${evalFormula(p.dice, vars)}${p.damage_type ? ` ${p.damage_type}` : ""}`);
    if (p.kind === "heal") bits.push(`heal ${evalFormula(p.dice, vars)}${p.add_mod ? (mod ? `+${mod}` : " + mod") : ""}`);
    if (p.kind === "temp_hp") bits.push(`${evalFormula(p.dice, vars)} temp HP`);
    if (p.kind === "condition") bits.push(p.condition);
    if (p.kind === "ac_base") bits.push(`AC ${p.base}${p.add_dex ? " + DEX" : ""}`);
    if (p.kind === "ac_bonus") bits.push(`AC +${p.n}`);
    if (p.kind === "ac_min") bits.push(`AC at least ${p.n}`);
    if (p.kind === "roll_dice") bits.push(`${termText(p.term)} on ${p.on}`);
  }
  const at = mech.level > 0 && slot !== undefined ? ` at L${slot}` : "";
  const head = bits.length ? `${bits.join(", ")}${at}` : at.trim();
  const d = mech.delivery;
  const how = d.kind === "save" ? `${d.ability.toUpperCase()} save${d.success === "half" ? ", half on a success" : ""}` : d.kind === "attack" ? "spell attack" : "";
  const t = mech.targeting;
  const max = t.kind === "creatures" ? evalCount(t.max, vars) : null;
  const who = t.kind === "self" ? "self" : t.kind === "area" ? `${t.size_ft}-ft ${t.shape}` : max != null ? `up to ${max} target${max === 1 ? "" : "s"}` : "";
  return [head, how, who].filter(Boolean).join(" · ");
}
