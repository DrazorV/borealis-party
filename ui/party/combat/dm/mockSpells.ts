/**
 * The mock engine's side of 0.3.2 (casts, statblock actions, standard actions, zones), for tests and the rig before
 * the server has them. It is deliberately small: it spends what a cast spends, opens a pending cast with one row per
 * target, takes typed rolls first, and applies one damage roll with halves for savers. The engine (party-combat) is
 * the judge; this only lets the tracker's flow run. Every command is also kept in `MockCombat.spellCommands`.
 */
import type { D20Input, D20Result, DiceInput, DiceResult, DamageType } from "./common";
import { abilityStatus, byId, isAlive, poolOf } from "./derive";
import { evalCount, evalFormula, varsFor } from "./formula";
import { kindOfCast, mechOf, spellOf, type CastRow, type PendingCast, type Primitive, type SpellCommand, type SpellEffectSpec, type SpellMech } from "./spellTypes";
import { featureFor } from "./statRef";
import type { Ability, Combatant, CombatState, Command, Effect, Modifier, StatBlockSnapshot } from "./types";

/** What the mock engine's Run gives this module. */
export interface SpellHost {
  s: CombatState;
  statblocks: Record<string, StatBlockSnapshot>;
  ev(subject: string | undefined, event: string, text: string, extra?: Record<string, unknown>): void;
  c(id: string): Combatant;
  d20(roll: D20Input): D20Result;
  dice(input: DiceInput): DiceResult;
  damage(cmd: Extract<Command, { command: "damage" }>): void;
  addEffect(spec: SpellEffectSpec): number;
  startConcentration(c: Combatant, name: string): void;
  refuse(message: string, code?: string): never;
}

export const SPELL_COMMANDS = new Set(["cast_spell", "resolve_cast_row", "apply_cast", "cancel_cast", "apply_zone", "perform_statblock", "standard_action"]);

type State = CombatState & { casts?: PendingCast[]; next_cast?: number };

function spend(h: SpellHost, c: Combatant, kind: Ability["kind"]) {
  const e = c.economy ?? (c.economy = { action: false, bonus_action: false, reaction: false });
  if (kind === "action") e.action = true;
  if (kind === "bonus_action") e.bonus_action = true;
  if (kind === "reaction") e.reaction = true;
  void h;
}

function newCast(h: SpellHost, cast: Omit<PendingCast, "id" | "status">): PendingCast {
  const s = h.s as State;
  const n = s.next_cast ?? 1;
  s.next_cast = n + 1;
  const made: PendingCast = { ...cast, id: `cast-${n}`, status: "open" };
  s.casts = [...(s.casts ?? []).filter((x) => x.status === "open"), made];
  return made;
}

function modifiersOf(ps: Primitive[]): Modifier[] {
  const out: Modifier[] = [];
  for (const p of ps) {
    if (p.kind === "ac_bonus") out.push({ kind: "ac", value: p.value });
    if (p.kind === "ac_base") out.push({ kind: "ac_base", base: p.base, add_dex: p.add_dex } as unknown as Modifier);
    if (p.kind === "ac_min") out.push({ kind: "ac_min", value: p.value } as unknown as Modifier);
    if (p.kind === "speed" && p.op === "add") out.push({ kind: "speed_add", value: p.value ?? 0 });
    if (p.kind === "speed" && p.op === "mult") out.push({ kind: "speed_multiply", value: p.value ?? 2 });
    if (p.kind === "speed" && p.op === "zero") out.push({ kind: "speed_zero" });
  }
  return out;
}

/** The lasting part of a spell on one target: its modifiers and conditions, held by the caster's concentration. */
function lasting(h: SpellHost, caster: Combatant, mech: SpellMech, target: string, ps: Primitive[], dc: number | undefined) {
  const modifiers = modifiersOf(ps);
  const conds = ps.filter((p): p is Extract<Primitive, { kind: "condition" }> => p.kind === "condition");
  const zone = ps.find((p): p is Extract<Primitive, { kind: "zone" }> => p.kind === "zone");
  const notes = ps.filter((p) => p.kind === "note" || p.kind === "roll_dice" || p.kind === "roll_mode");
  if (!modifiers.length && !conds.length && !zone && !notes.length && !mech.concentration) return;
  const repeat = conds.find((x) => x.repeat_save)?.repeat_save;
  h.addEffect({
    name: mech.name, source: caster.id, target,
    duration: mech.duration_turns ? { kind: "source_turns", remaining: mech.duration_turns, at: "start" } : { kind: "manual" },
    ...(mech.concentration ? { concentration: true } : {}),
    ...(conds.length ? { conditions: conds.map((x) => x.condition) as Effect["conditions"] } : {}),
    ...(modifiers.length ? { modifiers } : {}),
    ...(repeat && dc != null ? { repeat_save: { ability: repeat.ability, dc, at: repeat.at } } : {}),
    ...(mech.ends_on?.length ? { ends_on: mech.ends_on } : {}),
    ...(zone && dc != null ? { zone: { ability: zone.ability, dc, ...(zone.damage ? { damage: { dice: evalFormula(zone.damage.dice, varsFor(mech, undefined)), type: zone.damage.type } } : {}), half: zone.half } } : {}),
    spell: { key: mech.key, name: mech.name },
    detail: notes.map((p) => (p.kind === "note" ? p.text ?? p.key : p.kind === "roll_dice" ? `${p.sign === -1 ? "−" : "+"}${p.term} to ${p.on}s` : `${p.mode === "adv" ? "advantage" : "disadvantage"} on ${p.on}`)).join("; ") || undefined,
  });
}

function damagePart(mech: SpellMech, slot: number | undefined, choice?: DamageType): { dice: string; type?: DamageType } | undefined {
  const p = mech.on_land?.find((x): x is Extract<Primitive, { kind: "damage" }> => x.kind === "damage");
  if (!p) return undefined;
  return { dice: evalFormula(p.dice, varsFor(mech, slot)), type: choice ?? p.type };
}

function cast(h: SpellHost, cmd: Extract<SpellCommand, { command: "cast_spell" }>) {
  const c = h.c(cmd.caster);
  const a = cmd.ability ? c.abilities?.find((x) => x.key === cmd.ability) ?? h.refuse(`No ability ${cmd.ability}.`) : undefined;
  const mech = (a ? mechOf(a) : undefined) ?? cmd.adhoc;
  const name = mech?.name ?? a?.name ?? "a spell";
  const level = mech?.level ?? (a ? spellOf(a)?.level : undefined) ?? 0;
  const kind = mech ? kindOfCast(mech.cast) : a?.kind ?? "action";
  if (!cmd.precast) {
    const why = a ? abilityStatus(h.s, c, { ...a, kind }) : !isAlive(c) ? "Down" : null;
    if (why) h.refuse(`Unavailable: ${why}`, "unavailable");
  }
  if (cmd.slot !== undefined) {
    if (cmd.slot < level) h.refuse(`${name} needs a level ${level} slot or higher.`);
    const p = c.spell_slots?.[String(cmd.slot)];
    if (!p || p.remaining <= 0) h.refuse(`No level ${cmd.slot} slot left.`);
    p.remaining--;
  } else if (a?.uses && c.resources?.[a.uses]) c.resources[a.uses]!.pool.remaining--;
  if (!cmd.precast) spend(h, c, kind);
  if (mech?.targeting.kind === "creatures") {
    const max = evalCount(mech.targeting.max, varsFor(mech, cmd.slot));
    if (max != null && cmd.targets.length > max) h.refuse(`${name} takes up to ${max} target${max === 1 ? "" : "s"}.`);
  }
  for (const t of cmd.targets) h.c(t);
  if (mech?.concentration) h.startConcentration(c, name);
  h.ev(c.id, "cast", `${c.label} casts ${name}${cmd.slot !== undefined ? ` (level ${cmd.slot})` : ""}${cmd.precast ? " before the fight" : ""}`, { spell: mech?.key ?? name });
  if (!mech || mech.tier === "ruling" || mech.tier === "uncurated") return;
  const summon = mech.always?.find((p) => p.kind === "summon_info") ?? mech.on_land?.find((p) => p.kind === "summon_info");
  if (summon) h.ev(c.id, "summon_options", `${name}: add the creatures with Add creature`, { spell: mech.key });
  const targets = mech.targeting.kind === "self" ? [c.id] : cmd.targets;
  const d = mech.delivery;
  if (d.kind === "auto") {
    for (const t of targets) {
      for (const p of mech.on_land ?? []) {
        if (p.kind === "heal") h.c(t).hp.current = Math.min(h.c(t).hp.max, h.c(t).hp.current + h.dice(cmd.damage ?? { kind: "app", notation: evalFormula(p.dice, varsFor(mech, cmd.slot)) }).total);
        if (p.kind === "damage") h.damage({ command: "damage", target: t, parts: [{ amount: cmd.damage ?? { kind: "app", notation: evalFormula(p.dice, varsFor(mech, cmd.slot)) }, ...(p.type ? { damage_type: p.type } : {}) }], magical: true });
      }
      lasting(h, c, mech, t, [...(mech.always ?? []), ...(mech.on_land ?? [])], mech.dc);
    }
    return;
  }
  const rows: CastRow[] = targets.map((t) => (d.kind === "save" ? { target: t, kind: "save", ability: d.ability } : { target: t, kind: "attack" }));
  const made = newCast(h, {
    caster: c.id, ability: a?.key, name, slot: cmd.slot, dc: mech.dc, attack: mech.attack, delivery: d, rows,
    ...(damagePart(mech, cmd.slot, cmd.choices?.damage_type) ? { damage: { ...damagePart(mech, cmd.slot, cmd.choices?.damage_type)!, half_on_save: d.kind === "save" && d.success === "half" } } : {}),
  });
  for (const r of cmd.rolls ?? []) resolveRow(h, { command: "resolve_cast_row", cast: made.id, target: r.target, roll: r.roll });
  if (mech.always?.length) for (const t of targets) lasting(h, c, { ...mech, on_land: [] }, t, mech.always, mech.dc);
}

function findCast(h: SpellHost, id: string): PendingCast {
  return (h.s as State).casts?.find((x) => x.id === id && x.status === "open") ?? h.refuse("That cast is no longer waiting.", "not_found");
}

function resolveRow(h: SpellHost, cmd: Extract<SpellCommand, { command: "resolve_cast_row" }>) {
  const k = findCast(h, cmd.cast);
  const row = k.rows.find((r) => r.target === cmd.target) ?? h.refuse("Not a target of that cast.");
  const t = h.c(row.target);
  if (cmd.use_lr) {
    const lr = t.legendary_resistance;
    if (!lr || lr.remaining <= 0) h.refuse("No Legendary Resistance left.");
    lr.remaining--;
    row.outcome = "saved";
    row.lr_used = true;
    h.ev(t.id, "legendary_resistance", `${t.label} uses Legendary Resistance against ${k.name}`);
    return;
  }
  if (cmd.outcome) { row.outcome = cmd.outcome; h.ev(t.id, "cast_row", `${t.label}: ${cmd.outcome} against ${k.name}`); return; }
  if (!cmd.roll) h.refuse("A roll or an outcome is needed.");
  const r = h.d20(cmd.roll);
  row.roll = r;
  if (row.kind === "save") row.outcome = k.dc == null || r.total >= k.dc ? "saved" : "failed";
  else row.outcome = r.is_nat20 ? "crit" : r.is_nat1 ? "miss" : t.ac != null && r.total >= t.ac ? "hit" : "miss";
  h.ev(t.id, "cast_row", `${t.label}: ${row.kind === "save" ? `${row.ability?.toUpperCase()} save` : "attack"} ${r.total} · ${row.outcome}`, { roll: r });
}

function apply(h: SpellHost, cmd: Extract<SpellCommand, { command: "apply_cast" }>) {
  const k = findCast(h, cmd.cast);
  const waiting = k.rows.filter((r) => !r.outcome).length;
  if (waiting) h.refuse(`Waiting on ${waiting} roll${waiting === 1 ? "" : "s"}.`);
  const caster = byId(h.s, k.caster);
  const a = caster?.abilities?.find((x) => x.key === k.ability);
  const mech = a ? mechOf(a) : undefined;
  if (k.damage) {
    const anyCrit = k.rows.some((r) => r.outcome === "crit");
    const input = cmd.damage ?? { kind: "app" as const, notation: k.damage.dice, ...(anyCrit ? { critical: true } : {}) };
    const roll = h.dice(input);
    for (const r of k.rows) {
      const full = r.outcome === "failed" || r.outcome === "hit" || r.outcome === "crit";
      const amount = full ? roll.total : r.outcome === "saved" && k.damage.half_on_save ? Math.floor(roll.total / 2) : 0;
      if (amount > 0) h.damage({ command: "damage", target: r.target, parts: [{ amount: { kind: "typed", total: amount }, ...(k.damage.type ? { damage_type: k.damage.type } : {}) }], magical: Boolean(mech) });
    }
  }
  if (mech && caster) {
    for (const r of k.rows) {
      const landed = r.outcome === "failed" || r.outcome === "hit" || r.outcome === "crit";
      const ps = (landed ? mech.on_land : mech.on_resist)?.filter((p) => p.kind !== "damage") ?? [];
      if (ps.length) lasting(h, caster, mech, r.target, ps, k.dc);
    }
  }
  k.status = "applied";
  h.ev(k.caster, "cast_applied", `${k.name} applied`);
}

function perform(h: SpellHost, cmd: Extract<SpellCommand, { command: "perform_statblock" }> & { as_reaction?: boolean }) {
  const c = h.c(cmd.id);
  const a = c.abilities?.find((x) => x.key === cmd.ability) ?? h.refuse(`No ability ${cmd.ability}.`);
  const kind = cmd.as_reaction ? "reaction" : a.kind;
  const why = abilityStatus(h.s, c, { ...a, kind });
  if (why) h.refuse(`Unavailable: ${why}`, "unavailable");
  spend(h, c, kind);
  if (a.recharge) a.recharge.ready = false;
  if (a.uses && c.resources?.[a.uses]) c.resources[a.uses]!.pool.remaining--;
  if (a.kind === "legendary" && !cmd.as_reaction) poolOf(h.s, c.id)![1].remaining -= a.legendary_cost ?? 1;
  const f = featureFor(h.statblocks[c.id]?.data, a);
  for (const t of cmd.targets) h.c(t);
  h.ev(c.id, "ability_used", `${c.label} uses ${a.name}`, { ability: a.key });
  if (f?.attack) {
    const parts = (f.attack.damage ?? []).map((d) => d.dice ?? String(d.average));
    newCast(h, {
      caster: c.id, ability: a.key, name: a.name, attack: f.attack.to_hit, delivery: { kind: "attack" },
      rows: cmd.targets.map((t) => ({ target: t, kind: "attack" })),
      ...(parts.length ? { damage: { dice: parts.join("+"), type: f.attack.damage?.[0]?.type, half_on_save: false } } : {}),
    });
  } else if (f?.save) {
    const m = /\((\d+d\d+(?:\s*[+-]\s*\d+)?)\)\s*([a-z]+)?\s*damage/i.exec(f.text);
    newCast(h, {
      caster: c.id, ability: a.key, name: a.name, dc: f.save.dc, delivery: { kind: "save", ability: f.save.ability, success: f.save.half_on_success ? "half" : "none" },
      rows: cmd.targets.map((t) => ({ target: t, kind: "save", ability: f.save!.ability })),
      ...(m ? { damage: { dice: m[1]!.replace(/\s+/g, ""), type: m[2]?.toLowerCase() as DamageType, half_on_save: Boolean(f.save.half_on_success) } } : {}),
    });
  }
}

function standard(h: SpellHost, cmd: Extract<SpellCommand, { command: "standard_action" }>) {
  const c = h.c(cmd.id);
  const e = c.economy ?? { action: false, bonus_action: false, reaction: false };
  if (e.action) h.refuse("Unavailable: Action spent", "unavailable");
  if (!isAlive(c)) h.refuse("Unavailable: Down", "unavailable");
  spend(h, c, "action");
  const word = { dodge: "Dodge", dash: "Dash", disengage: "Disengage", help: "Help" }[cmd.action];
  if (cmd.action === "dodge" || cmd.action === "disengage") {
    h.addEffect({ name: word, source: c.id, target: c.id, duration: { kind: "source_turns", remaining: 1, at: "start" },
      detail: cmd.action === "dodge" ? "Attacks against it have disadvantage; advantage on DEX saves." : "Its movement doesn't provoke opportunity attacks." });
  }
  if (cmd.action === "help" && cmd.target) {
    h.addEffect({ name: "Helped", source: c.id, target: cmd.target, duration: { kind: "source_turns", remaining: 1, at: "start" }, detail: `Advantage on its next check or attack (from ${c.label}).` });
  }
  h.ev(c.id, "standard_action", `${c.label} takes the ${word} action${cmd.target ? ` (${byId(h.s, cmd.target)?.label ?? ""})` : ""}`);
}

function zone(h: SpellHost, cmd: Extract<SpellCommand, { command: "apply_zone" }>) {
  const e = h.s.effects.find((x) => x.id === cmd.effect) as (Effect & SpellEffectSpec) | undefined;
  if (!e?.zone) h.refuse("That effect has no area.");
  for (const t of cmd.targets) h.c(t);
  const z = e.zone;
  newCast(h, {
    caster: e.source ?? e.target, name: e.name, dc: z.dc, delivery: { kind: "save", ability: z.ability, success: z.half ? "half" : "none" },
    rows: cmd.targets.map((t) => ({ target: t, kind: "save", ability: z.ability })),
    ...(z.damage ? { damage: { dice: z.damage.dice, type: z.damage.type, half_on_save: Boolean(z.half) } } : {}),
  });
}

export function runSpell(h: SpellHost, cmd: SpellCommand) {
  switch (cmd.command) {
    case "cast_spell": return cast(h, cmd);
    case "resolve_cast_row": return resolveRow(h, cmd);
    case "apply_cast": return apply(h, cmd);
    case "cancel_cast": {
      const k = findCast(h, cmd.cast);
      k.status = "cancelled";
      if (cmd.refund && k.slot !== undefined) {
        const p = byId(h.s, k.caster)?.spell_slots?.[String(k.slot)];
        if (p) p.remaining = Math.min(p.max, p.remaining + 1);
      }
      h.ev(k.caster, "cast_cancelled", `${k.name} cancelled`);
      return;
    }
    case "apply_zone": return zone(h, cmd);
    case "perform_statblock": return perform(h, cmd);
    case "standard_action": return standard(h, cmd);
  }
}
