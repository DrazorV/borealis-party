/**
 * A stand-in for the combat server, for tests and for the tracker before the server is deployed: the commands of
 * `party_combat::apply` that the tracker sends, with the same rules in short (party/docs/combat-engine.md), an undo
 * journal of 50 and the export format. It is not the judge in production; the server's engine is.
 */
import { parseNotation, rollExpr, seededDie, rollD20, type DieRoller } from "../../../src/lib/dice";
import { ABILITY_NAMES, INCAPACITATING, type D20Input, type D20Result, type DiceInput, type DiceResult } from "./common";
import { abilityStatus, byId, concentrationDc, conditionsOf, effectiveInitiative, isAlive, poolOf, poolResetOwner, takesTurn, turnOrder } from "./derive";
import { runSpell, SPELL_COMMANDS, type SpellHost } from "./mockSpells";
import type { Combatant, CombatState, Command, DamageClause, DamageResult, DmEvent, Effect, EffectSpec, StatBlockSnapshot } from "./types";

/** The mock's own record of a combat: the state, its log and stat blocks (the mock server turns it into the DM view). */
export interface MockView {
  id: string;
  partyId: string;
  encounterId: string | null;
  seq: number;
  state: CombatState;
  log: DmEvent[];
  statblocks: Record<string, StatBlockSnapshot>;
  undo: { label: string; player: boolean } | null;
}

export class CombatRefusal extends Error {
  constructor(message: string, readonly code = "invalid") {
    super(message);
  }
}

const refuse = (message: string, code?: string): never => { throw new CombatRefusal(message, code); };
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

export const EXPORT_FORMAT = "borealis-party-combat";
export const EXPORT_SCHEMA = 1;
const UNDO_STEPS = 50;

export interface MockCombat {
  view: MockView;
  journal: { label: string; state: CombatState; player: boolean }[];
  die: DieRoller;
  /** Player events, for tests: who was asked to take damage, whose turn it is. */
  playerEvents: Record<string, unknown>[];
}

export function newCombat(id: string, partyId: string, state: CombatState, statblocks: Record<string, StatBlockSnapshot> = {}, seed = 1): MockCombat {
  return { view: { id, partyId, encounterId: null, seq: 0, state, log: [], statblocks, undo: null }, journal: [], die: seededDie(seed), playerEvents: [] };
}

export function emptyState(edition: CombatState["edition"] = "2024"): CombatState {
  return {
    edition, settings: { show_enemy_turn: false, auto_recharge: true }, status: "setup", round: 0, combatants: [], effects: [],
    legendary_pools: {}, rules: [], group_limits: {}, next_effect: 1, next_tiebreak: 0,
  };
}

function d20(roll: D20Input, die: DieRoller): D20Result {
  if (roll.kind === "outcome") throw new CombatRefusal("This roll needs its number, not just pass or fail.");
  if (roll.kind === "typed") {
    const natural = roll.natural ?? null;
    return {
      source: "typed", mode: "normal", dice: natural == null ? [] : [natural], natural, modifier: null, total: roll.total,
      is_nat20: natural === 20 || (natural == null && roll.total === 20), is_nat1: natural === 1 || (natural == null && roll.total === 1),
    };
  }
  const r = rollD20(roll.modifier ?? 0, roll.mode ?? "normal", die);
  return { source: "app", mode: r.mode, dice: r.dice, natural: r.natural, modifier: r.modifier, total: r.total, is_nat20: r.isNat20, is_nat1: r.isNat1 };
}

function dice(input: DiceInput, die: DieRoller): DiceResult {
  if (input.kind === "typed") return { source: "typed", notation: String(input.total), total: input.total, breakdown: String(input.total) };
  const r = rollExpr(parseNotation(input.notation), die);
  return { source: "app", notation: r.notation, total: r.total, breakdown: r.breakdown };
}

const clauseApplies = (list: DamageClause[] | undefined, type: string, magical: boolean, qualified: boolean): boolean =>
  (list ?? []).some((c) => c.types.includes(type as never) && (!c.text || (c.text.toLowerCase().includes("nonmagical") ? !magical : qualified)));

class Run {
  events: DmEvent[] = [];
  constructor(readonly s: CombatState, readonly m: MockCombat) {}

  ev(subject: string | undefined, event: string, text: string, extra: Record<string, unknown> = {}) {
    this.events.push({ round: this.s.round, ...(subject ? { subject } : {}), event, text, ...extra });
  }
  c(id: string): Combatant {
    return byId(this.s, id) ?? refuse(`No combatant ${id}.`, "not_found");
  }
  name(id: string | undefined) {
    return (id && byId(this.s, id)?.label) || "someone";
  }
  player(event: Record<string, unknown>) {
    this.m.playerEvents.push(event);
  }

  /** 0.3.2's commands (./mockSpells.ts). */
  spellHost(): SpellHost {
    return {
      s: this.s, statblocks: this.m.view.statblocks,
      ev: (a, b, c, d) => this.ev(a, b, c, d), c: (id) => this.c(id),
      d20: (roll) => d20(roll, this.m.die), dice: (input) => dice(input, this.m.die),
      damage: (cmd) => this.damage(cmd), addEffect: (spec) => this.addEffect(spec),
      startConcentration: (c, name) => this.startConcentration(c, name), refuse,
    };
  }

  run(cmd: Command) {
    const s = this.s;
    if (SPELL_COMMANDS.has(cmd.command)) { runSpell(this.spellHost(), cmd as Parameters<typeof runSpell>[1]); return; }
    switch (cmd.command) {
      case "add_combatant": {
        if (byId(s, cmd.combatant.id)) refuse(`Combatant ${cmd.combatant.id} already exists.`);
        const c: Combatant = { economy: { action: false, bonus_action: false, reaction: false }, life: { state: "alive" }, ...clone(cmd.combatant), tiebreak: s.next_tiebreak ?? 0 };
        s.next_tiebreak = (s.next_tiebreak ?? 0) + 1;
        s.combatants.push(c);
        this.ev(c.id, "added", `${c.label} joins`);
        if (cmd.roll_initiative) this.rollOne(c);
        return;
      }
      case "remove_combatant": {
        const c = this.c(cmd.id);
        this.wentDown(c, "removed");
        for (const e of [...s.effects]) if (e.target === c.id || e.source === c.id) this.endEffect(e.id, "removed", false);
        c.removed = true;
        this.ev(c.id, "removed", `${c.label} leaves the fight`);
        if (s.current === c.id && s.status === "active") this.nextTurn();
        return;
      }
      case "rename": { const c = this.c(cmd.id); const old = c.label; c.label = cmd.label; this.ev(c.id, "edited", `${old} is now ${cmd.label}`); return; }
      case "set_public_name": { this.c(cmd.id).public_name = cmd.name; this.ev(cmd.id, "edited", `Public name set`); return; }
      case "reveal": { const c = this.c(cmd.id); c.visible = true; c.ever_revealed = true; this.ev(c.id, "revealed", `${c.label} is revealed`); this.player({ event: "appeared", id: c.id }); return; }
      case "hide": {
        const c = this.c(cmd.id);
        if (c.kind === "character") refuse("Characters are always visible.");
        const was = Boolean(c.ever_revealed);
        c.visible = false;
        this.ev(c.id, "hidden", `${c.label} is hidden${was ? " (players saw it)" : ""}`, { was_seen: was });
        if (was) this.player({ event: "disappeared", id: c.id });
        return;
      }
      case "set_absent": { const c = this.c(cmd.id); c.absent = cmd.absent; this.ev(c.id, "edited", `${c.label} ${cmd.absent ? "is not in this fight" : "joins the fight"}`); return; }
      case "set_notes": { this.c(cmd.id).notes = cmd.notes; return; }
      case "set_settings": { s.settings = cmd.settings; return; }
      case "roll_initiative": {
        const list = cmd.ids?.length ? cmd.ids.map((id) => this.c(id)) : s.combatants.filter((c) => c.kind === "creature" && !c.removed);
        for (const c of list) if (!(cmd.only_missing && c.initiative != null) && !c.linked_to && !(c.owner_id && (c.initiative_mode ?? "after_owner") === "after_owner")) this.rollOne(c);
        this.ev(undefined, "order_changed", "Initiative rolled");
        return;
      }
      case "set_initiative": { const c = this.c(cmd.id); const r = d20(cmd.roll.kind === "app" ? { ...cmd.roll, modifier: cmd.roll.modifier ?? c.initiative_bonus ?? 0 } : cmd.roll, this.m.die); c.initiative = r.total; this.ev(c.id, "initiative", `${c.label} initiative ${r.total}${r.source === "app" ? ` (${r.dice.join(", ")})` : ""}`, { roll: r }); return; }
      case "clear_initiative": { this.c(cmd.id).initiative = null; return; }
      case "swap_tie": { const a = this.c(cmd.a); const b = this.c(cmd.b); [a.tiebreak, b.tiebreak] = [b.tiebreak, a.tiebreak]; this.ev(a.id, "order_changed", `${a.label} and ${b.label} swap`); return; }
      case "link": { const c = this.c(cmd.id); if (cmd.to) { this.c(cmd.to); c.linked_to = cmd.to; } else delete c.linked_to; this.ev(c.id, "order_changed", cmd.to ? `${c.label} acts after ${this.name(cmd.to)}` : `${c.label} acts on its own`); return; }
      case "start": {
        if (s.status !== "setup") refuse("The combat has already started.");
        const first = turnOrder(s).map((id) => byId(s, id)!).find(takesTurn) ?? refuse("Nobody has an initiative yet.");
        s.status = "active";
        s.round = 1;
        this.ev(undefined, "started", "Combat starts");
        this.roundStart();
        this.turnStart(first);
        return;
      }
      case "next_turn": { if (s.status !== "active") refuse("The combat isn't running."); this.nextTurn(); return; }
      case "previous_turn": {
        const order = turnOrder(s);
        const at = order.indexOf(s.current ?? "");
        if (at < 0) refuse("Nobody has an initiative yet.");
        for (let step = 1; step <= order.length; step++) {
          const id = order[(at + order.length - step) % order.length]!;
          if (takesTurn(this.c(id))) {
            if (step > at) { if (s.round <= 1) refuse("This is the first turn."); s.round--; }
            s.current = id;
            this.ev(id, "turn_started", `Back to ${this.name(id)}'s turn`);
            return;
          }
        }
        return;
      }
      case "end": { s.status = "ended"; this.ev(undefined, "ended", "Combat ends"); this.player({ event: "ended" }); return; }
      case "set_economy": {
        const c = this.c(cmd.id);
        const e = c.economy ?? { action: false, bonus_action: false, reaction: false };
        c.economy = { action: cmd.action ?? e.action, bonus_action: cmd.bonus_action ?? e.bonus_action, reaction: cmd.reaction ?? e.reaction };
        this.ev(c.id, "economy", `${c.label}: economy changed`);
        return;
      }
      case "damage": this.damage(cmd); return;
      case "multi_damage": {
        // The engine's MultiDamage (D13): one roll, each target's save, half (or none) on a success.
        if (!cmd.targets.length) refuse("Pick at least one target.");
        const roll = dice(cmd.amount, this.m.die);
        if (roll.total < 0) refuse("Damage can't be negative.");
        for (const t of cmd.targets) {
          const c = this.c(t.id);
          if (cmd.save && !t.save) refuse("Each target needs its save: typed, rolled, saved or failed.");
          if (cmd.save && t.save?.kind === "app" && c.hp_authority === "player") refuse("A player's save is theirs to roll: type it, or set saved or failed.");
        }
        this.ev(undefined, "edited", `Damage to ${cmd.targets.length} targets: ${roll.breakdown}`);
        for (const t of cmd.targets) {
          const c = this.c(t.id);
          let saved = false;
          if (cmd.save && t.save) {
            if (t.save.kind === "outcome") saved = t.save.outcome === "saved";
            else if (t.save.kind === "typed") saved = t.save.total >= cmd.save.dc;
            else saved = d20({ kind: "app", modifier: c.stats?.saves?.[ABILITY_NAMES.indexOf(cmd.save.ability)] ?? 0, mode: "normal" }, this.m.die).total >= cmd.save.dc;
          }
          const dealt = cmd.save && saved ? (cmd.save.half ? Math.floor(roll.total / 2) : 0) : roll.total;
          if (dealt === 0) { this.ev(c.id, "edited", `${c.label} takes no damage`); continue; }
          this.damage({ command: "damage", target: c.id, parts: [{ amount: { kind: "typed", total: dealt }, ...(cmd.damage_type ? { damage_type: cmd.damage_type } : {}) }],
            ...(cmd.magical ? { magical: true } : {}), ...(cmd.apply_qualified ? { apply_qualified: true } : {}) });
        }
        return;
      }
      case "heal": {
        const c = this.c(cmd.target);
        if (c.life?.state === "dead") refuse("Dead: set HP to revive.", "dead");
        const r = dice(cmd.amount, this.m.die);
        if (r.total < 0) refuse("Healing can't be negative.");
        if (c.hp_authority === "player") {
          this.player({ event: "heal_prompt", id: c.id, owner: c.owner, amount: r.total });
          this.ev(c.id, "healed", `${c.label}: asked to heal ${r.total}`, { amount: r.total, prompted: true });
          return;
        }
        const before = c.hp.current;
        c.hp.current = Math.min(c.hp.max, before + r.total);
        if (before === 0 && c.hp.current > 0) this.revive(c);
        this.ev(c.id, "healed", `${c.label} heals ${c.hp.current - before}`, { amount: c.hp.current - before, prompted: false });
        return;
      }
      case "set_hp": {
        const c = this.c(cmd.id);
        if (cmd.max != null) c.hp.max = Math.max(1, cmd.max);
        if (cmd.temp != null) c.hp.temp = Math.max(0, cmd.temp);
        const before = c.hp.current;
        c.hp.current = Math.max(0, Math.min(c.hp.max, cmd.current));
        if (c.hp.current > 0 && before === 0) this.revive(c);
        if (c.hp.current === 0 && before > 0) this.dropToZero(c);
        this.ev(c.id, "hp_set", `${c.label} HP set to ${c.hp.current}/${c.hp.max}`);
        return;
      }
      case "grant_temp_hp": { const c = this.c(cmd.id); c.hp.temp = Math.max(c.hp.temp ?? 0, cmd.amount); this.ev(c.id, "temp_hp", `${c.label} has ${c.hp.temp} temp HP`); return; }
      case "death_save": {
        const c = this.c(cmd.id);
        const life = c.life;
        if (life?.state !== "dying") return refuse("Not making death saves.", "not_dying");
        let { successes, failures } = life;
        if (cmd.roll.kind === "outcome") {
          const o = cmd.roll.outcome;
          if (o === "nat20") { c.hp.current = 1; this.revive(c); this.ev(c.id, "death_save", `${c.label} death save: natural 20, back up`, { outcome: o }); return; }
          if (o === "nat1") failures += 2; else if (o === "pass") successes++; else failures++;
          const word = o === "pass" ? "success" : o === "fail" ? "failure" : "natural 1";
          if (failures >= 3) { c.life = { state: "dead" }; this.ev(c.id, "died", `${c.label} dies`); return; }
          if (successes >= 3) { c.life = { state: "stable" }; this.ev(c.id, "stabilized", `${c.label} is stable`); return; }
          c.life = { state: "dying", successes, failures };
          this.ev(c.id, "death_save", `${c.label} death save: ${word}`, { outcome: o });
          return;
        }
        const r = d20(cmd.roll, this.m.die);
        if (r.is_nat20) { c.hp.current = 1; this.revive(c); this.ev(c.id, "death_save", `${c.label} rolls a natural 20 and is back up`, { roll: r }); return; }
        if (r.is_nat1) failures += 2; else if (r.total >= 10) successes++; else failures++;
        if (failures >= 3) { c.life = { state: "dead" }; this.ev(c.id, "died", `${c.label} dies`); return; }
        if (successes >= 3) { c.life = { state: "stable" }; this.ev(c.id, "stabilized", `${c.label} is stable`); return; }
        c.life = { state: "dying", successes, failures };
        this.ev(c.id, "death_save", `${c.label} death save ${r.total}: ${successes} successes, ${failures} failures`, { roll: r });
        return;
      }
      case "stabilize": { const c = this.c(cmd.id); if (c.life?.state === "dying") c.life = { state: "stable" }; this.ev(c.id, "stabilized", `${c.label} is stable`); return; }
      case "set_death_saves": { const c = this.c(cmd.id); c.life = { state: "dying", successes: cmd.successes, failures: cmd.failures }; return; }
      case "add_condition": {
        const k = cmd.effect.conditions?.[0];
        const c = this.c(cmd.effect.target);
        if (k && c.defenses?.condition_immunities?.includes(k)) { this.ev(c.id, "condition_immune", `${c.label} is immune to ${k}`); return; }
        this.addEffect({ ...cmd.effect, name: cmd.effect.name || (k ?? "Condition") });
        return;
      }
      case "remove_condition": {
        for (const e of [...this.s.effects].filter((x) => x.target === cmd.target && x.conditions?.includes(cmd.condition))) {
          const rest = (e.conditions ?? []).filter((k) => k !== cmd.condition);
          if (rest.length === 0 && !(e.modifiers?.length)) this.endEffect(e.id, "condition removed", false);
          else e.conditions = rest;
        }
        this.ev(cmd.target, "edited", `${this.name(cmd.target)} is no longer ${cmd.condition}`);
        return;
      }
      case "set_exhaustion": { const c = this.c(cmd.id); c.exhaustion = Math.max(0, Math.min(6, cmd.level)); if (c.exhaustion === 6) { c.life = { state: "dead" }; this.wentDown(c, "exhaustion"); } this.ev(c.id, "exhaustion", `${c.label} exhaustion ${c.exhaustion}`, { level: c.exhaustion }); return; }
      case "set_concentration": this.startConcentration(this.c(cmd.id), cmd.name); return;
      case "end_concentration": this.endConcentration(this.c(cmd.id), "ended"); return;
      case "resolve_concentration": {
        const c = this.c(cmd.id);
        const check = c.pending_concentration?.[0] ?? refuse("No concentration check is waiting.", "no_pending_check");
        const r = cmd.roll.kind === "outcome" ? null : d20(cmd.roll.kind === "app" ? { ...cmd.roll, modifier: cmd.roll.modifier ?? 0 } : cmd.roll, this.m.die);
        c.pending_concentration = c.pending_concentration!.slice(1);
        const kept = r ? r.total >= check.dc : cmd.roll.kind === "outcome" && (cmd.roll.outcome === "pass" || cmd.roll.outcome === "nat20");
        this.ev(c.id, "concentration_save", r ? `${c.label} concentration save ${r.total} vs DC ${check.dc}: ${kept ? "kept" : "lost"}` : `${c.label} concentration save (DC ${check.dc}): ${kept ? "kept" : "lost"}`, { ...(r ? { roll: r } : {}), kept });
        if (!kept) this.endConcentration(c, "failed the save");
        return;
      }
      case "add_effect": this.addEffect(cmd.effect); return;
      case "remove_effect": { if (!s.effects.some((e) => e.id === cmd.effect)) refuse("No such effect."); this.endEffect(cmd.effect, "removed", !cmd.skip_follow_up); return; }
      case "use_charge": {
        const e = s.effects.find((x) => x.id === cmd.effect) ?? refuse("No such effect.");
        if (e.charges == null) refuse("That effect has no charges.");
        if (e.charges === 0) refuse("No charges left.");
        e.charges!--;
        this.ev(e.target, "charge_used", `${e.name}: ${e.charges} left`, { effect: e.id, remaining: e.charges });
        if (e.charges === 0) this.endEffect(e.id, "charges spent", true);
        return;
      }
      case "resolve_effect_save": {
        const e = s.effects.find((x) => x.id === cmd.effect) ?? refuse("No such effect.");
        const save = e.repeat_save ?? refuse("The effect has no repeated save.", "no_save");
        const r = cmd.roll.kind === "outcome" ? null : d20(cmd.roll, this.m.die);
        const success = r ? r.total >= save.dc : cmd.roll.kind === "outcome" && (cmd.roll.outcome === "pass" || cmd.roll.outcome === "nat20");
        this.ev(e.target, "effect_save", `${this.name(e.target)} ${save.ability.toUpperCase()} save ${r ? `${r.total} vs DC ${save.dc}` : `(DC ${save.dc})`} against ${e.name}: ${success ? "ends" : "stays"}`, { ...(r ? { roll: r } : {}), success });
        if (success) this.endEffect(e.id, "saved", true);
        return;
      }
      case "use_ability": {
        const c = this.c(cmd.id);
        const a = c.abilities?.find((x) => x.key === cmd.ability) ?? refuse(`No ability ${cmd.ability}.`, "unknown_ability");
        const why = abilityStatus(s, c, a, cmd.slot);
        if (why) refuse(`Unavailable: ${why}`, "unavailable");
        const e = c.economy ?? (c.economy = { action: false, bonus_action: false, reaction: false });
        if (a.kind === "action") e.action = true;
        if (a.kind === "bonus_action") e.bonus_action = true;
        if (a.kind === "reaction") e.reaction = true;
        if (a.recharge) a.recharge.ready = false;
        if (a.uses && c.resources?.[a.uses]) c.resources[a.uses]!.pool.remaining--;
        if (cmd.slot !== undefined) c.spell_slots![String(cmd.slot)]!.remaining--;
        if (a.group_limit && s.group_limits[a.group_limit]) s.group_limits[a.group_limit]!.used++;
        if (a.kind === "legendary") poolOf(s, c.id)![1].remaining -= a.legendary_cost ?? 1;
        this.ev(c.id, "ability_used", `${c.label} uses ${a.name}${cmd.slot !== undefined ? ` (level ${cmd.slot} slot)` : ""}`, { ability: a.key });
        return;
      }
      case "roll_recharge": {
        const c = this.c(cmd.id);
        const a = c.abilities?.find((x) => x.key === cmd.ability) ?? refuse(`No ability ${cmd.ability}.`);
        if (!a.recharge) refuse("That ability has no recharge.");
        const r = dice(cmd.roll, this.m.die);
        if (r.total < 1 || r.total > 6) refuse("A recharge d6 is 1 to 6.");
        this.finishRecharge(c, a.key, r);
        return;
      }
      case "set_recharge": { const a = this.c(cmd.id).abilities?.find((x) => x.key === cmd.ability); if (a?.recharge) a.recharge.ready = cmd.ready; this.ev(cmd.id, "recharge_set", `${a?.name ?? cmd.ability} ${cmd.ready ? "ready" : "spent"}`); return; }
      case "set_legendary_pool": {
        // As the engine: the members leave their other pools (a DM link is opt-in), and an emptied pool goes.
        for (const [key, p] of Object.entries(s.legendary_pools)) {
          if (key === cmd.pool) continue;
          p.members = p.members.filter((m) => !cmd.value.members.includes(m));
          if (p.members.length === 0) delete s.legendary_pools[key];
        }
        s.legendary_pools[cmd.pool] = { ...clone(cmd.value), remaining: Math.min(cmd.value.remaining, cmd.value.max) };
        return;
      }
      case "adjust_legendary": {
        const p = s.legendary_pools[cmd.pool] ?? refuse("No such pool.");
        p.remaining = Math.max(0, Math.min(p.max, p.remaining + cmd.delta));
        this.ev(undefined, "resources", `Legendary actions (${cmd.pool}): ${p.remaining}/${p.max}`);
        return;
      }
      case "adjust_legendary_resistance": {
        const c = this.c(cmd.id);
        const p = c.legendary_resistance ?? refuse("No Legendary Resistance.");
        p.remaining = Math.max(0, Math.min(p.max, p.remaining + cmd.delta));
        this.ev(c.id, "resources", `${c.label} ${cmd.delta < 0 ? "uses" : "restores"} Legendary Resistance: ${p.remaining}/${p.max}`);
        return;
      }
      case "adjust_slot": {
        const c = this.c(cmd.id);
        const p = c.spell_slots?.[String(cmd.level)] ?? refuse(`No level ${cmd.level} slots.`);
        p.remaining = Math.max(0, Math.min(p.max, p.remaining + cmd.delta));
        this.ev(c.id, "resources", `${c.label} level ${cmd.level} slots: ${p.remaining}/${p.max}`);
        return;
      }
      case "set_slots": { const c = this.c(cmd.id); c.spell_slots = { ...(c.spell_slots ?? {}), [String(cmd.level)]: cmd.pool }; return; }
      case "adjust_resource": {
        const c = this.c(cmd.id);
        const r = c.resources?.[cmd.key] ?? refuse("No such resource.");
        r.pool.remaining = Math.max(0, Math.min(r.pool.max, r.pool.remaining + cmd.delta));
        this.ev(c.id, "resources", `${c.label} ${r.label}: ${r.pool.remaining}/${r.pool.max}`);
        return;
      }
      case "set_rule": { const i = s.rules.findIndex((r) => r.id === cmd.rule.id); if (i >= 0) s.rules[i] = clone(cmd.rule); else s.rules.push(clone(cmd.rule)); return; }
      case "set_rule_active": { const r = s.rules.find((x) => x.id === cmd.id) ?? refuse("No such rule."); r.active = cmd.active; this.ev(r.linked, "rule", `${r.name} ${cmd.active ? "is active" : "ends"}`, { rule: r.id, active: cmd.active }); return; }
      case "remove_rule": { s.rules = s.rules.filter((r) => r.id !== cmd.id); return; }
      case "summon": {
        const owner = this.c(cmd.owner);
        let base: Combatant;
        if ("combatant" in cmd) base = clone(cmd.combatant);
        else if ("copyOf" in cmd) {
          const from = this.c(cmd.copyOf);
          const n = s.combatants.filter((x) => x.label.replace(/\s*\d+$/, "") === from.label.replace(/\s*\d+$/, "")).length + 1;
          base = { ...clone(from), label: `${from.label.replace(/\s*\d+$/, "")} ${n}`, hp: { current: from.hp.max, max: from.hp.max, temp: 0 }, concentration: undefined, pending_concentration: [] };
          base.public_name = base.label;
        } else return refuse("The server builds companions and bestiary creatures.");
        if (base.kind === "character") refuse("A player character can't be summoned.");
        if (!("combatant" in cmd)) base.id = `summon-${(s.next_tiebreak ?? 0) + 1}-${s.combatants.length}`;
        if (byId(s, base.id)) refuse(`Combatant ${base.id} already exists.`);
        const controller = owner.kind === "character" ? { kind: "player" as const, pc: owner.id } : (owner.controller ?? { kind: "dm" as const });
        const c: Combatant = {
          ...base, owner_id: owner.id, controller, hp_authority: "engine", initiative: null, linked_to: undefined,
          visible: controller.kind === "player" ? true : (cmd.visible ?? owner.visible), initiative_mode: cmd.initiative ? "own" : "after_owner",
          economy: { action: false, bonus_action: false, reaction: false }, life: { state: "alive" },
        };
        c.ever_revealed = c.visible;
        c.tiebreak = s.next_tiebreak ?? 0;
        s.next_tiebreak = c.tiebreak + 1;
        s.combatants.push(c);
        if (cmd.initiative) c.initiative = d20(cmd.initiative.kind === "app" ? { ...cmd.initiative, modifier: cmd.initiative.modifier ?? c.initiative_bonus ?? 0 } : cmd.initiative, this.m.die).total;
        this.ev(c.id, "summoned", `${owner.label} summons ${c.label}`, { owner: owner.id });
        return;
      }
      case "dismiss": {
        const c = this.c(cmd.id);
        if (!c.owner_id) refuse("Only a controlled creature can be dismissed.");
        this.wentDown(c, "dismissed");
        for (const e of [...s.effects]) if (e.target === c.id) this.endEffect(e.id, "dismissed", false);
        c.removed = true;
        this.ev(c.id, "dismissed", `${c.label} is dismissed`);
        if (s.current === c.id && s.status === "active") this.nextTurn();
        return;
      }
      case "set_initiative_mode": {
        const c = this.c(cmd.id);
        if (!c.owner_id) refuse("Only a controlled creature follows an owner.");
        c.initiative_mode = cmd.mode;
        if (cmd.mode === "own" && c.initiative == null) {
          const r = d20(cmd.roll ?? { kind: "app", modifier: c.initiative_bonus ?? 0 }, this.m.die);
          c.initiative = r.total;
        }
        this.ev(c.id, "order_changed", `${c.label} acts ${cmd.mode === "own" ? `on its own initiative (${c.initiative})` : "right after its owner"}`);
        return;
      }
      case "set_group_limit": { if (cmd.per_round === 0) delete s.group_limits[cmd.group]; else s.group_limits[cmd.group] = { per_round: cmd.per_round, used: s.group_limits[cmd.group]?.used ?? 0 }; return; }
    }
  }

  rollOne(c: Combatant) {
    const r = d20({ kind: "app", modifier: c.initiative_bonus ?? 0, mode: c.initiative_advantage ? "advantage" : "normal" }, this.m.die);
    c.initiative = r.total;
    this.ev(c.id, "initiative", `${c.label} rolls initiative ${r.total}`, { roll: r });
  }

  nextTurn() {
    const s = this.s;
    const order = turnOrder(s);
    const start = order.indexOf(s.current ?? "");
    for (let step = 1; step <= order.length; step++) {
      const at = start < 0 ? step - 1 : start + step;
      const c = byId(s, order[at % order.length]!)!;
      if (takesTurn(c)) {
        if (s.current) this.tick(s.current, "end");
        if (start >= 0 && at >= order.length) { s.round++; this.roundStart(); }
        this.turnStart(c);
        return;
      }
    }
    refuse("Nobody has an initiative yet.");
  }

  roundStart() {
    for (const g of Object.values(this.s.group_limits)) g.used = 0;
    for (const c of this.s.combatants) for (const r of Object.values(c.resources ?? {})) if (r.reset === "round_start") r.pool.remaining = r.pool.max;
    this.ev(undefined, "round_started", `Round ${this.s.round}`);
  }

  turnStart(c: Combatant) {
    const s = this.s;
    s.current = c.id;
    if (c.visible) s.last_visible = { id: c.id, round: s.round };
    this.ev(c.id, "turn_started", `${c.label}'s turn`);
    const surprised = s.edition === "2014" && s.round === 1 && Boolean(c.surprised);
    c.economy = { action: surprised, bonus_action: surprised, reaction: surprised };
    for (const r of Object.values(c.resources ?? {})) if (r.reset === "turn_start") r.pool.remaining = r.pool.max;
    for (const [key, pool] of Object.entries(s.legendary_pools)) {
      if (poolResetOwner(s, pool)?.id === c.id) { pool.remaining = pool.max; this.ev(c.id, "legendary_reset", `Legendary actions (${key}) reset to ${pool.max}`, { pool: key }); }
    }
    for (const a of c.abilities ?? []) {
      if (a.recharge && !a.recharge.ready) {
        if (s.settings?.auto_recharge !== false && c.kind === "creature") this.finishRecharge(c, a.key, dice({ kind: "app", notation: "1d6" }, this.m.die));
        else this.ev(c.id, "recharge_due", `${c.label}: roll recharge for ${a.name}`, { ability: a.key });
      }
    }
    this.tick(c.id, "start");
    if (c.kind === "character") this.player({ event: "your_turn", id: c.id, owner: c.owner });
  }

  tick(id: string, at: "start" | "end") {
    const expired: number[] = [];
    for (const e of this.s.effects) {
      const d = e.duration;
      if (d && d.kind !== "manual" && d.at === at && (d.kind === "source_turns" ? e.source === id : e.target === id)) {
        d.remaining = Math.max(0, d.remaining - 1);
        if (d.remaining === 0) expired.push(e.id);
      }
      if (e.repeat_save?.at === at && e.target === id) this.ev(id, "save_due", `${this.name(id)}: ${e.repeat_save.ability.toUpperCase()} save DC ${e.repeat_save.dc} against ${e.name}`, { effect: e.id });
    }
    for (const e of expired) this.endEffect(e, "duration expired", true);
  }

  finishRecharge(c: Combatant, key: string, roll: DiceResult) {
    const a = c.abilities!.find((x) => x.key === key)!;
    const ready = roll.total >= a.recharge!.min;
    a.recharge!.ready = ready;
    this.ev(c.id, "recharge", `${c.label}: recharge ${a.name} d6 = ${roll.total} · ${ready ? "ready" : "not ready"}`, { ability: key, roll, ready });
  }

  damage(cmd: Extract<Command, { command: "damage" }>) {
    const c = this.c(cmd.target);
    if (!cmd.parts.length) refuse("No damage to apply.");
    const result: DamageResult = { parts: [], total: 0 };
    for (const part of cmd.parts) {
      const roll = dice(part.amount, this.m.die);
      if (roll.total < 0) refuse("Damage can't be negative.");
      let amount = roll.total;
      const t = part.damage_type;
      const d = c.defenses ?? {};
      const immune = Boolean(t) && clauseApplies(d.immunities, t!, Boolean(cmd.magical), Boolean(cmd.apply_qualified));
      const resisted = !immune && Boolean(t) && clauseApplies(d.resistances, t!, Boolean(cmd.magical), Boolean(cmd.apply_qualified));
      const vulnerable = !immune && Boolean(t) && clauseApplies(d.vulnerabilities, t!, Boolean(cmd.magical), Boolean(cmd.apply_qualified));
      if (immune) amount = 0;
      if (resisted) amount = Math.floor(amount / 2);
      if (vulnerable) amount *= 2;
      result.parts.push({ ...(t ? { damage_type: t } : {}), roll, amount, immune, resisted, vulnerable });
      result.total += amount;
    }
    const detail = result.parts.map((p) => `${p.amount}${p.damage_type ? ` ${p.damage_type}` : ""}${p.immune ? " (immune)" : p.resisted ? " (resisted)" : p.vulnerable ? " (vulnerable)" : ""}`).join(" + ");
    if (c.hp_authority === "player") {
      this.player({ event: "damage_prompt", id: c.id, owner: c.owner, amount: result.total, critical_hit: Boolean(cmd.critical_hit) });
      this.ev(c.id, "damage", `${c.label}: asked to take ${result.total} (${detail})`, { result, prompted: true });
      return;
    }
    this.ev(c.id, "damage", `${c.label} takes ${result.total} (${detail})`, { result, prompted: false });
    this.loseHp(c, result.total, Boolean(cmd.critical_hit));
  }

  loseHp(c: Combatant, amount: number, crit: boolean) {
    if (amount <= 0) return;
    const temp = c.hp.temp ?? 0;
    const absorbed = Math.min(amount, temp);
    c.hp.temp = temp - absorbed;
    const rest = amount - absorbed;
    const before = c.hp.current;
    c.hp.current = Math.max(0, before - rest);
    const overflow = rest - before;
    if (before > 0 && c.hp.current === 0) {
      if ((c.zero_hp ?? "dies") === "dies" || overflow >= c.hp.max) { c.life = { state: "dead" }; this.ev(c.id, "died", `${c.label} dies`); }
      else { c.life = { state: "dying", successes: 0, failures: 0 }; this.ev(c.id, "dying", `${c.label} drops to 0 HP and is dying`); this.addEffect({ name: "Dying", target: c.id, conditions: ["unconscious"], detail: "At 0 HP: unconscious until it regains HP." }, "dying"); }
      this.wentDown(c, "dropped to 0 HP");
      return;
    }
    if (before === 0 && rest > 0) {
      if (c.life?.state === "dying" || c.life?.state === "stable") {
        const prev = c.life.state === "dying" ? c.life : { successes: 0, failures: 0 };
        const failures = prev.failures + (crit ? 2 : 1);
        c.life = failures >= 3 ? { state: "dead" } : { state: "dying", successes: prev.successes, failures };
        this.ev(c.id, failures >= 3 ? "died" : "dying", failures >= 3 ? `${c.label} dies (damage at 0 HP)` : `${c.label}: damage at 0 HP, ${failures} death-save failures`);
      }
      return;
    }
    if (c.concentration) {
      const dc = concentrationDc(this.s, amount);
      c.pending_concentration = [...(c.pending_concentration ?? []), { dc, damage: amount }];
      this.ev(c.id, "concentration_check", `${c.label}: concentration save DC ${dc}`, { dc });
    }
  }

  dropToZero(c: Combatant) {
    if ((c.zero_hp ?? "dies") === "dies") { c.life = { state: "dead" }; this.ev(c.id, "died", `${c.label} dies`); }
    else { c.life = { state: "dying", successes: 0, failures: 0 }; this.addEffect({ name: "Dying", target: c.id, conditions: ["unconscious"] }, "dying"); }
    this.wentDown(c, "dropped to 0 HP");
  }

  revive(c: Combatant) {
    if (isAlive(c)) return;
    c.life = { state: "alive" };
    for (const e of [...this.s.effects]) if (e.target === c.id && e.origin === "dying") this.endEffect(e.id, "regained HP", false);
    this.ev(c.id, "revived", `${c.label} is back up`);
  }

  wentDown(c: Combatant, reason: string) {
    if (c.concentration) this.endConcentration(c, reason);
    for (const pet of this.s.combatants.filter((x) => !x.removed && x.owner_id === c.id)) {
      this.ev(pet.id, "owner_defeated", `${c.label} is down: keep ${pet.label} after it, dismiss it, or give it its own initiative`, { owner: c.id });
    }
    for (const f of this.s.combatants.filter((x) => x.linked_to === c.id)) {
      delete f.linked_to;
      this.ev(f.id, "unlinked", `${f.label} now acts on its own initiative (${c.label} is down)`, { from: c.id });
    }
    for (const e of [...this.s.effects].filter((x) => x.ends_on_defeat_of?.includes(c.id))) this.endEffect(e.id, `${c.label} is down`, true);
    for (const r of this.s.rules.filter((x) => x.active && x.linked === c.id)) {
      r.active = false;
      this.ev(c.id, "rule", `${r.name} ends: ${c.label} is down`, { rule: r.id, active: false });
    }
  }

  startConcentration(c: Combatant, name: string) {
    if (c.concentration?.name === name) return;
    if (c.concentration) this.endConcentration(c, "concentrates on something else");
    c.concentration = { name };
    this.ev(c.id, "concentration_started", `${c.label} concentrates on ${name}`);
  }

  /** The cascade: every effect its concentration holds ends, and their follow-ups are added. */
  endConcentration(c: Combatant, reason: string) {
    const k = c.concentration;
    if (!k) return;
    delete c.concentration;
    c.pending_concentration = [];
    for (const e of this.s.effects.filter((x) => x.concentration && x.source === c.id)) this.endEffect(e.id, `concentration ended (${reason})`, true);
    this.ev(c.id, "concentration_ended", `${c.label} stops concentrating on ${k.name} (${reason})`, { name: k.name });
  }

  addEffect(spec: EffectSpec, origin: Effect["origin"] = "command"): number {
    const s = this.s;
    const t = this.c(spec.target);
    if (spec.source) this.c(spec.source);
    if (spec.duration?.kind === "source_turns" && !spec.source) refuse("A duration in the source's turns needs a source.");
    if (spec.concentration) {
      const src = this.c(spec.source ?? refuse("A concentration effect needs a source."));
      if (!isAlive(src)) refuse("A creature at 0 HP can't concentrate.");
      this.startConcentration(src, spec.name);
    }
    const conditions = (spec.conditions ?? []).filter((k) => !t.defenses?.condition_immunities?.includes(k));
    const id = s.next_effect ?? 1;
    s.next_effect = id + 1;
    s.effects.push({ ...clone(spec), conditions, id, origin });
    this.ev(t.id, "effect_added", `${spec.name} on ${t.label}`, { effect: id });
    if (conditions.some((k) => INCAPACITATING.includes(k)) && t.concentration) this.endConcentration(t, "incapacitated");
    return id;
  }

  endEffect(id: number, reason: string, followUp: boolean) {
    const s = this.s;
    const k = s.effects.findIndex((e) => e.id === id);
    if (k < 0) return;
    const [e] = s.effects.splice(k, 1) as [Effect];
    this.ev(e.target, "effect_ended", `${e.name} on ${this.name(e.target)} ends (${reason})`, { effect: id, reason });
    if (e.concentration && e.source) {
      const src = byId(s, e.source);
      const still = s.effects.some((o) => o.concentration && o.source === e.source && o.name === e.name);
      if (!still && src?.concentration?.name === e.name) {
        delete src.concentration;
        src.pending_concentration = [];
        this.ev(src.id, "concentration_ended", `${src.label} stops concentrating on ${e.name}`, { name: e.name });
      }
    }
    // 0.3.2: an anchor effect (a creature added for a summon spell) takes its target with it.
    if ((e as Effect & { dismiss_on_end?: boolean }).dismiss_on_end) {
      const t = byId(s, e.target);
      if (t && !t.removed) { t.removed = true; this.ev(t.id, "dismissed", `${t.label} is gone (${e.name} ended)`); }
    }
    if (followUp && e.follow_up && byId(s, e.follow_up.target) && !byId(s, e.follow_up.target)!.removed) this.addEffect(e.follow_up);
  }
}

/** Applies one DM command: the new view, or a {@link CombatRefusal} with nothing changed. */
export function execute(m: MockCombat, cmd: Command, seq?: number, actor: "dm" | "player" = "dm"): MockView {
  if (seq !== undefined && seq !== m.view.seq) refuse("The combat changed; showing the latest.", "conflict");
  if (m.view.state.status === "ended" && cmd.command !== "end") refuse("The combat has ended.");
  const before = clone(m.view.state);
  const next = clone(m.view.state);
  const run = new Run(next, m);
  run.run(cmd);
  m.journal.push({ label: commandLabel(cmd), state: before, player: actor === "player" });
  if (m.journal.length > UNDO_STEPS) m.journal.shift();
  const last = m.journal[m.journal.length - 1];
  m.view = { ...m.view, seq: m.view.seq + 1, state: next, log: [...m.view.log, ...run.events], undo: last ? { label: last.label, player: last.player } : null };
  return m.view;
}

/** Undo (C8): back to the exact state before the last write. A player's write needs `force`. */
export function undo(m: MockCombat, seq?: number, force = false): MockView {
  if (seq !== undefined && seq !== m.view.seq) refuse("The combat changed; showing the latest.", "conflict");
  const last = m.journal[m.journal.length - 1] ?? refuse("Nothing to undo.", "nothing_to_undo");
  if (last.player && !force) refuse("The last change was a player's; undo it anyway?", "undo_would_revert_player");
  m.journal.pop();
  const prev = m.journal[m.journal.length - 1];
  m.view = {
    ...m.view, seq: m.view.seq + 1, state: last.state,
    log: [...m.view.log, { round: last.state.round, event: "undone", text: `Undone: ${last.label}`, label: last.label }],
    undo: prev ? { label: prev.label, player: prev.player } : null,
  };
  return m.view;
}

export function exportCombat(m: MockCombat): string {
  return JSON.stringify({ format: EXPORT_FORMAT, schema_version: EXPORT_SCHEMA, state: m.view.state, log: m.view.log, statblocks: m.view.statblocks }, null, 2);
}

/** Import (D45): always a new combat, with a fresh journal and seq 0. */
export function importCombat(text: string, id: string, partyId: string): MockCombat {
  if (text.length > 8 * 1024 * 1024) refuse("Can't import: the file is over 8 MiB.", "import");
  let data: { format?: string; schema_version?: number; state?: CombatState; log?: DmEvent[]; statblocks?: Record<string, StatBlockSnapshot> };
  try { data = JSON.parse(text) as typeof data; } catch { return refuse("Can't import: not a combat export.", "import"); }
  if (data.format !== EXPORT_FORMAT) refuse("Can't import: not a combat export.", "import");
  if ((data.schema_version ?? 0) > EXPORT_SCHEMA) refuse("Can't import: made by a newer version.", "import");
  const state = data.state ?? refuse("Can't import: no combat in it.", "import");
  const ids = new Set(state.combatants.map((c) => c.id));
  if (state.current && !ids.has(state.current)) refuse("Can't import: its turn points at nobody.", "import");
  const m = newCombat(id, partyId, state, data.statblocks ?? {});
  m.view.log = data.log ?? [];
  return m;
}

/**
 * What a DM command does to `state`, for showing it at once while the server confirms (optimistic writes). Null when
 * it can't be told here: it rolls dice (only the server rolls, D21), it would be refused, or it's a server action.
 * The server's answer always replaces it.
 */
export function predict(state: CombatState, cmd: Command): CombatState | null {
  const m: MockCombat = { view: { id: "", partyId: "", encounterId: null, seq: 0, state, log: [], statblocks: {}, undo: null }, journal: [], die: () => { throw new CombatRefusal("rolls"); }, playerEvents: [] };
  try {
    return execute(m, cmd).state;
  } catch {
    return null;
  }
}

export function commandLabel(cmd: Command): string {
  return cmd.command.replace(/_/g, " ");
}

/** Who acts next, for the tracker's "up next" line. */
export function upNext(state: CombatState): Combatant | undefined {
  const order = turnOrder(state);
  const at = order.indexOf(state.current ?? "");
  for (let step = 1; step <= order.length; step++) {
    const c = byId(state, order[(at + step) % order.length]!);
    if (c && takesTurn(c)) return c;
  }
  return undefined;
}

export { effectiveInitiative, conditionsOf };
