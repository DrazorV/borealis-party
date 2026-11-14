/**
 * The mock engine's side of 0.3.2 (casts, statblock actions, standard actions, zones, binding to concentration), for
 * tests and the rig. It follows the engine's contract (./types.ts) in short: a cast spends what it costs, opens a
 * pending cast with one row per target, takes typed rolls first, and applies one roll per amount with halves for
 * savers. The engine (party-combat) is the judge; this only lets the tracker's flow run.
 */
import type { Condition, D20Input, D20Result, DiceInput, DiceResult } from "./common";
import { abilityStatus, byId, effectiveAc, isAlive, poolOf } from "./derive";
import { evalCount, varsFor } from "./formula";
import { castAmounts, castVars, halfOnSave, kindOfCast, landed } from "./spellTypes";
import { featureFor } from "./statRef";
import { statblockSpec } from "./turn";
import type {
  Ability, AmountRoll, CastRow, CastSpec, Combatant, CombatState, Command, EffectSpec, Modifier, PendingCast, Primitive,
  RowInput, SpellDuration, SpellMech, StatBlockSnapshot,
} from "./types";

/** What the mock engine's Run gives this module. */
export interface SpellHost {
  s: CombatState;
  statblocks: Record<string, StatBlockSnapshot>;
  ev(subject: string | undefined, event: string, text: string, extra?: Record<string, unknown>): void;
  c(id: string): Combatant;
  d20(roll: D20Input): D20Result;
  dice(input: DiceInput): DiceResult;
  damage(cmd: Extract<Command, { command: "damage" }>): void;
  addEffect(spec: EffectSpec): number;
  startConcentration(c: Combatant, name: string): void;
  refuse(message: string, code?: string): never;
}

export const SPELL_COMMANDS = new Set(["cast_spell", "resolve_cast_row", "apply_cast", "cancel_cast", "apply_zone", "perform_statblock", "standard_action", "bind_to_concentration"]);
type SpellCommand = Extract<Command, { command: "cast_spell" | "resolve_cast_row" | "apply_cast" | "cancel_cast" | "apply_zone" | "perform_statblock" | "standard_action" | "bind_to_concentration" }>;

function spend(c: Combatant, kind: Ability["kind"]) {
  const e = c.economy ?? (c.economy = { action: false, bonus_action: false, reaction: false });
  if (kind === "action") e.action = true;
  if (kind === "bonus_action") e.bonus_action = true;
  if (kind === "reaction") e.reaction = true;
}

function newCast(h: SpellHost, cast: Omit<PendingCast, "id">): PendingCast {
  const n = h.s.next_cast ?? 1;
  h.s.next_cast = n + 1;
  const made: PendingCast = { ...cast, id: `cast-${n}` };
  h.s.casts = [...(h.s.casts ?? []), made];
  return made;
}

const closeCast = (h: SpellHost, id: string) => { h.s.casts = (h.s.casts ?? []).filter((k) => k.id !== id); };

function modifiersOf(ps: Primitive[]): Modifier[] {
  const out: Modifier[] = [];
  for (const p of ps) {
    if (p.kind === "ac_bonus") out.push({ kind: "ac", value: p.n });
    if (p.kind === "ac_base") out.push({ kind: "ac_base", value: { base: p.base, add_dex: Boolean(p.add_dex) } });
    if (p.kind === "ac_min") out.push({ kind: "ac_min", value: p.n });
    if (p.kind === "roll_dice") out.push({ kind: "roll_dice", value: { on: p.on, term: p.term, ...(p.ability ? { ability: p.ability } : {}) } });
    if (p.kind === "speed" && p.change.op === "add") out.push({ kind: "speed_add", value: p.change.ft });
    if (p.kind === "speed" && p.change.op === "mult") out.push({ kind: "speed_multiply", value: p.change.n });
    if (p.kind === "speed" && p.change.op === "zero") out.push({ kind: "speed_zero" });
  }
  return out;
}

function durationOf(d: SpellDuration | undefined): EffectSpec["duration"] {
  if (!d) return { kind: "manual" };
  if (d.kind === "source_turns") return { kind: "source_turns", remaining: d.n, at: d.at };
  if (d.kind === "target_turns") return { kind: "target_turns", remaining: d.n, at: d.at };
  if (d.kind === "minutes") return { kind: "source_turns", remaining: d.n * 10, at: "start" };
  return { kind: "manual" };
}

/** The lasting part of a spell on one target: its modifiers and conditions, held by the caster's concentration. */
function lasting(h: SpellHost, caster: Combatant, spec: CastSpec, target: string, ps: Primitive[], slot: number | undefined) {
  const mech = spec.mech;
  const modifiers = modifiersOf(ps);
  const conds = ps.filter((p): p is Extract<Primitive, { kind: "condition" }> => p.kind === "condition");
  const zone = ps.find((p): p is Extract<Primitive, { kind: "zone" }> => p.kind === "zone");
  const notes = ps.filter((p): p is Extract<Primitive, { kind: "note" }> => p.kind === "note");
  if (!modifiers.length && !conds.length && !zone && !notes.length && !mech.concentration) return;
  const repeat = conds.find((x) => x.repeat_save)?.repeat_save;
  h.addEffect({
    name: mech.name, source: caster.id, target, duration: durationOf(mech.duration),
    ...(mech.concentration ? { concentration: true } : {}),
    ...(conds.length ? { conditions: conds.map((x) => x.condition) as Condition[] } : {}),
    ...(modifiers.length ? { modifiers } : {}),
    ...(repeat && spec.dc != null ? { repeat_save: { ability: repeat.ability, dc: spec.dc, at: repeat.at } } : {}),
    spell: { key: mech.key, level: mech.level, ...(slot !== undefined ? { slot } : {}), ...(spec.dc != null ? { dc: spec.dc } : {}),
      ...(zone ? { zone: zone.zone, vars: varsFor(mech, slot, spec.cantrip_tier ?? 1, spec.mod ?? 0) } : {}) },
    ...(notes.length ? { detail: notes.map((n) => n.text).join("; ") } : {}),
  });
}

function rowsFor(spec: CastSpec, targets: string[]): CastRow[] {
  const d = spec.mech.delivery;
  return targets.map((t) => (d.kind === "auto" ? { target: t, outcome: "lands" as const } : { target: t }));
}

/** A statblock action's definition, from its own mech or the block's printed attack or save (as the UI sends it). */
const statSpec = (h: SpellHost, c: Combatant, a: Ability): CastSpec | null => a.mech ?? statblockSpec(h.s.edition, a, featureFor(h.statblocks[c.id]?.data, a));

function cast(h: SpellHost, cmd: Extract<Command, { command: "cast_spell" }>) {
  const c = h.c(cmd.caster);
  const a = cmd.ability ? c.abilities?.find((x) => x.key === cmd.ability) ?? h.refuse(`No ability ${cmd.ability}.`) : undefined;
  const spec = cmd.spec ?? a?.mech;
  const mech: SpellMech | undefined = spec?.mech;
  const name = mech?.name ?? a?.name ?? "a spell";
  const level = mech?.level ?? a?.spell?.level ?? 0;
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
  if (!cmd.precast) spend(c, kind);
  const targets = cmd.targets ?? [];
  if (mech?.targeting.kind === "creatures") {
    const max = evalCount(mech.targeting.max, varsFor(mech, cmd.slot));
    if (max != null && targets.length > max) h.refuse(`${name} takes up to ${max} target${max === 1 ? "" : "s"}.`);
  }
  for (const t of targets) h.c(t);
  if (mech?.concentration) h.startConcentration(c, name);
  h.ev(c.id, "cast_started", `${c.label} casts ${name}${cmd.slot !== undefined ? ` (level ${cmd.slot})` : ""}${cmd.precast ? " before the fight" : ""}`, { spell: mech?.key ?? name });
  if (!spec || !mech || mech.tier === "ruling" || mech.tier === "uncurated") return;
  const summon = [...(mech.always ?? []), ...(mech.on_land ?? [])].find((p) => p.kind === "summon");
  if (summon && summon.kind === "summon") h.ev(c.id, "summon_options", `${name}: add the creatures with Add creature`, { caster: c.id, spell: mech.key, slot: cmd.slot, info: summon.info, concentration: Boolean(mech.concentration) });
  const on = mech.targeting.kind === "self" ? [c.id] : targets;
  const vars = varsFor(mech, cmd.slot, spec.cantrip_tier ?? 1, spec.mod ?? 0);
  const k = newCast(h, { caster: c.id, ...(a ? { ability: a.key } : {}), spec, ...(cmd.slot !== undefined ? { slot: cmd.slot } : {}), vars, ...(cmd.choices ? { choices: cmd.choices } : {}), rows: rowsFor(spec, on) });
  for (const r of cmd.rows ?? []) resolveRow(h, { cast: k.id, ...r });
  for (const t of on) if (mech.always?.length) lasting(h, c, spec, t, mech.always, cmd.slot);
  // Nothing left to roll: it applies now (the fast path).
  if (castAmounts(k).length === 0 && k.rows.every((r) => r.outcome)) apply(h, { command: "apply_cast", cast: k.id }, true);
  else if (cmd.amounts?.length && k.rows.every((r) => r.outcome)) apply(h, { command: "apply_cast", cast: k.id, amounts: cmd.amounts });
}

function findCast(h: SpellHost, id: string): PendingCast {
  return h.s.casts?.find((x) => x.id === id) ?? h.refuse("That cast is no longer waiting.", "not_found");
}

function resolveRow(h: SpellHost, cmd: { cast: string } & RowInput) {
  const k = findCast(h, cmd.cast);
  const row = k.rows.find((r) => r.target === cmd.target && (cmd.ray == null || r.ray === cmd.ray)) ?? h.refuse("Not a target of that cast.");
  const t = h.c(row.target);
  if (cmd.use_lr) {
    const lr = t.legendary_resistance;
    if (!lr || lr.remaining <= 0) h.refuse("No Legendary Resistance left.");
    lr.remaining--;
    row.outcome = "saved";
    row.used_lr = true;
    h.ev(t.id, "cast_row", `${t.label} uses Legendary Resistance against ${k.spec.mech.name}`, { cast: k.id, target: t.id, outcome: "saved" });
    return;
  }
  const roll = cmd.roll;
  if (roll.kind === "outcome") { row.outcome = roll.outcome; h.ev(t.id, "cast_row", `${t.label}: ${roll.outcome} against ${k.spec.mech.name}`, { cast: k.id, target: t.id, outcome: roll.outcome }); return; }
  const d = k.spec.mech.delivery;
  const mod = d.kind === "save" ? (t.stats?.saves?.[["str", "dex", "con", "int", "wis", "cha"].indexOf(d.ability)] ?? 0) : k.spec.attack ?? 0;
  const r = h.d20(roll.kind === "typed" ? roll : { kind: "app", modifier: mod });
  row.roll = r;
  if (d.kind === "save") row.outcome = k.spec.dc == null || r.total >= k.spec.dc ? "saved" : "failed";
  else {
    const ac = effectiveAc(h.s, t).total;
    row.outcome = r.is_nat20 ? "crit" : r.is_nat1 ? "miss" : ac != null && r.total >= ac ? "hit" : "miss";
  }
  h.ev(t.id, "cast_row", `${t.label}: ${d.kind === "save" ? `${d.ability.toUpperCase()} save` : "attack"} ${r.total} · ${row.outcome}`, { cast: k.id, target: t.id, outcome: row.outcome, roll: r });
}

function apply(h: SpellHost, cmd: Extract<Command, { command: "apply_cast" }>, quiet = false) {
  const k = findCast(h, cmd.cast);
  const waiting = k.rows.filter((r) => !r.outcome).length;
  if (waiting && !cmd.force) h.refuse(`Waiting on ${waiting} roll${waiting === 1 ? "" : "s"}.`);
  const caster = byId(h.s, k.caster);
  const amounts = castAmounts(k);
  const rolled = amounts.map((am, i) => {
    const input: AmountRoll | undefined = cmd.amounts?.[i];
    const crit = k.rows.some((r) => r.outcome === "crit");
    return { am, total: h.dice(input?.kind === "typed" ? input : { kind: "app", notation: am.dice, ...(crit ? { critical: true } : {}) }).total };
  });
  for (const r of k.rows) {
    const full = landed(r.outcome);
    for (const { am, total } of rolled) {
      const amount = full ? total : r.outcome === "saved" && halfOnSave(k) ? Math.floor(total / 2) : 0;
      if (amount <= 0) continue;
      if (am.kind === "damage") h.damage({ command: "damage", target: r.target, parts: [{ amount: { kind: "typed", total: amount }, ...(am.type ? { damage_type: am.type } : {}) }], magical: k.spec.origin.kind !== "statblock" });
      if (am.kind === "heal") { const t = h.c(r.target); t.hp.current = Math.min(t.hp.max, t.hp.current + amount); h.ev(t.id, "healed", `${t.label} regains ${amount} HP`); }
      if (am.kind === "temp_hp") { const t = h.c(r.target); t.hp.temp = Math.max(t.hp.temp ?? 0, amount); }
    }
    if (caster) {
      const ps = (full ? k.spec.mech.on_land : k.spec.mech.on_resist)?.filter((p) => p.kind !== "damage" && p.kind !== "heal" && p.kind !== "temp_hp") ?? [];
      if (ps.length) lasting(h, caster, k.spec, r.target, ps, k.slot);
    }
  }
  closeCast(h, k.id);
  if (!quiet || rolled.length) h.ev(k.caster, "cast_applied", `${k.spec.mech.name} applied`, { cast: k.id, amounts: rolled.map((r) => r.total) });
  void castVars;
}

function perform(h: SpellHost, cmd: Extract<Command, { command: "perform_statblock" }> & { as_reaction?: boolean }) {
  const c = h.c(cmd.id);
  const a = c.abilities?.find((x) => x.key === cmd.ability) ?? h.refuse(`No ability ${cmd.ability}.`);
  const kind = cmd.as_reaction ? "reaction" : a.kind;
  const why = abilityStatus(h.s, c, { ...a, kind });
  if (why) h.refuse(`Unavailable: ${why}`, "unavailable");
  spend(c, kind);
  if (a.recharge) a.recharge.ready = false;
  if (a.uses && c.resources?.[a.uses]) c.resources[a.uses]!.pool.remaining--;
  if (a.kind === "legendary" && !cmd.as_reaction) poolOf(h.s, c.id)![1].remaining -= a.legendary_cost ?? 1;
  const targets = cmd.targets ?? [];
  for (const t of targets) h.c(t);
  h.ev(c.id, "ability_used", `${c.label} uses ${a.name}`, { ability: a.key });
  const spec = cmd.spec ?? statSpec(h, c, a);
  if (!spec) return;
  newCast(h, { caster: c.id, ability: a.key, spec, vars: varsFor(spec.mech, undefined), rows: rowsFor(spec, targets), statblock: true });
}

function standard(h: SpellHost, cmd: Extract<Command, { command: "standard_action" }>) {
  const c = h.c(cmd.id);
  const e = c.economy ?? { action: false, bonus_action: false, reaction: false };
  if (cmd.bonus ? e.bonus_action : e.action) h.refuse(`Unavailable: ${cmd.bonus ? "Bonus action" : "Action"} spent`, "unavailable");
  if (!isAlive(c)) h.refuse("Unavailable: Down", "unavailable");
  spend(c, cmd.bonus ? "bonus_action" : "action");
  const word = { dodge: "Dodge", dash: "Dash", disengage: "Disengage", help: "Help" }[cmd.action];
  if (cmd.action === "dodge" || cmd.action === "disengage") {
    h.addEffect({ name: word, source: c.id, target: c.id, duration: { kind: "source_turns", remaining: 1, at: "start" },
      detail: cmd.action === "dodge" ? "Attacks against it have disadvantage; advantage on DEX saves." : "Its movement doesn't provoke opportunity attacks." });
  }
  if (cmd.action === "help" && cmd.target) {
    h.addEffect({ name: "Helped", source: c.id, target: cmd.target, duration: { kind: "source_turns", remaining: 1, at: "start" }, detail: `Advantage on its next check or attack (from ${c.label}).` });
  }
  h.ev(c.id, "standard_action", `${c.label} takes the ${word} action`, { action: cmd.action });
}

function zone(h: SpellHost, cmd: Extract<Command, { command: "apply_zone" }>) {
  const e = h.s.effects.find((x) => x.id === cmd.effect);
  const z = e?.spell?.zone ?? h.refuse("That effect has no area.");
  for (const t of cmd.targets) h.c(t);
  const save = z.save;
  const spec: CastSpec = { v: 1, origin: { kind: "statblock" }, ...(e!.spell!.dc != null ? { dc: e!.spell!.dc } : {}),
    mech: { v: 1, key: e!.spell!.key, edition: h.s.edition, name: e!.name, level: e!.spell!.level, cast: "action", tier: "auto", targeting: { kind: "area", shape: "emanation", size_ft: 0 },
      delivery: save ? { kind: "save", ability: save.ability, success: save.success } : { kind: "auto" },
      on_land: z.damage ? [{ kind: "damage", dice: z.damage, damage_type: z.damage_type }] : [] } };
  const vars = e!.spell!.vars ?? varsFor(spec.mech, e!.spell!.slot);
  newCast(h, { caster: e!.source ?? e!.target, spec, vars, rows: rowsFor(spec, cmd.targets), zone_of: e!.id });
}

/** bind_to_concentration: the creature goes when its owner's concentration ends (an anchor effect, dismiss_on_end). */
function bind(h: SpellHost, cmd: Extract<Command, { command: "bind_to_concentration" }>) {
  const c = h.c(cmd.id);
  const owner = byId(h.s, c.owner_id) ?? h.refuse("It has no owner.");
  if (!owner.concentration) h.refuse(`${owner.label} isn't concentrating.`);
  h.addEffect({ name: owner.concentration.name, source: owner.id, target: c.id, concentration: true, dismiss_on_end: true,
    detail: `Bound to ${owner.concentration.name}: it goes when the concentration ends.` });
}

export function runSpell(h: SpellHost, cmd: SpellCommand) {
  switch (cmd.command) {
    case "cast_spell": return cast(h, cmd);
    case "resolve_cast_row": return resolveRow(h, cmd);
    case "apply_cast": return apply(h, cmd);
    case "cancel_cast": {
      const k = findCast(h, cmd.cast);
      closeCast(h, k.id);
      if (cmd.refund && k.slot !== undefined) {
        const p = byId(h.s, k.caster)?.spell_slots?.[String(k.slot)];
        if (p) p.remaining = Math.min(p.max, p.remaining + 1);
      }
      h.ev(k.caster, "cast_cancelled", `${k.spec.mech.name} cancelled`, { cast: k.id, refunded: Boolean(cmd.refund) });
      return;
    }
    case "apply_zone": return zone(h, cmd);
    case "perform_statblock": return perform(h, cmd);
    case "standard_action": return standard(h, cmd);
    case "bind_to_concentration": return bind(h, cmd);
  }
}
