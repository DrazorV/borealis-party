/**
 * What the tracker shows that the state doesn't hold: the turn order, effective AC and speed, the conditions on a
 * combatant and whether an ability can be used. A port of `party_combat::derive`, read-only; the server stays the
 * judge (a refused write says why), this only lets the screen say so before the DM taps.
 */
import { INCAPACITATING, STOPS_MOVEMENT, type Condition } from "./common";
import type { Ability, Combatant, CombatState, Effect, LegendaryPool, Speeds } from "./types";

export const byId = (state: CombatState, id: string | undefined): Combatant | undefined =>
  id === undefined ? undefined : state.combatants.find((c) => c.id === id);

export const isDead = (c: Combatant): boolean => c.life?.state === "dead";
export const isAlive = (c: Combatant): boolean => (c.life?.state ?? "alive") === "alive";
export const isDefeated = (c: Combatant): boolean => Boolean(c.removed) || isDead(c);
export const isCharacter = (c: Combatant): boolean => c.kind === "character";

/** The follower's C3 link holds: its leader is in the fight, alive and has an initiative. */
export function linkActive(state: CombatState, c: Combatant): boolean {
  const leader = byId(state, c.linked_to);
  return Boolean(leader && !leader.removed && !leader.absent && isAlive(leader) && effectiveInitiative(state, leader) != null);
}

/**
 * Whom a combatant acts right after (`party_combat::derive::leader`): its C3 link while active; otherwise its owner,
 * for a controlled creature acting after its owner whose owner is in the fight (a defeated owner still leads until
 * the DM decides).
 */
export function leader(state: CombatState, c: Combatant, depth = 0): Combatant | undefined {
  if (depth > state.combatants.length) return undefined;
  const l = byId(state, c.linked_to);
  if (l && !l.removed && !l.absent && isAlive(l) && effectiveInitiative(state, l, depth + 1) != null) return l;
  if ((c.initiative_mode ?? "after_owner") === "after_owner") {
    const o = byId(state, c.owner_id);
    if (o && !o.absent && effectiveInitiative(state, o, depth + 1) != null) return o;
  }
  return undefined;
}

/** The initiative it acts on: its leader's while it follows one. */
export function effectiveInitiative(state: CombatState, c: Combatant, depth = 0): number | null {
  if (depth > state.combatants.length) return c.initiative ?? null;
  const l = leader(state, c, depth);
  return l ? effectiveInitiative(state, l, depth + 1) : (c.initiative ?? null);
}

function anchorOrder(state: CombatState): string[] {
  const roots = state.combatants
    .filter((c) => c.initiative != null && !leader(state, c))
    .sort((a, b) => (b.initiative! - a.initiative!) || ((b.initiative_bonus ?? 0) - (a.initiative_bonus ?? 0)) || ((a.tiebreak ?? 0) - (b.tiebreak ?? 0)));
  const out: string[] = [];
  const push = (c: Combatant, depth: number) => {
    if (depth > state.combatants.length || out.includes(c.id)) return;
    out.push(c.id);
    state.combatants
      .filter((f) => f.id !== c.id && leader(state, f)?.id === c.id)
      .sort((a, b) => (a.tiebreak ?? 0) - (b.tiebreak ?? 0))
      .forEach((f) => push(f, depth + 1));
  };
  roots.forEach((r) => push(r, 0));
  return out;
}

/** The turn order: initiative, then the modifier, then the tie-break, with followers nested after leaders (C1, C3). */
export function turnOrder(state: CombatState): string[] {
  return anchorOrder(state).filter((id) => {
    const c = byId(state, id);
    return c && !c.removed && !c.absent;
  });
}

/** How deep a combatant sits under the leaders it follows (0 for one acting on its own), for the roster's indent. */
export function followDepth(state: CombatState, c: Combatant): number {
  let depth = 0;
  for (let l = leader(state, c); l && depth <= state.combatants.length; l = leader(state, l)) depth++;
  return depth;
}

/** A summon, pet, familiar or companion. */
export const isControlled = (c: Combatant): boolean => Boolean(c.owner_id);

/**
 * Controlled creatures whose owner is down and that still act after it: the DM chooses for each (keep / dismiss /
 * own initiative). Nothing changes on its own.
 */
export function ownerDefeated(state: CombatState): Combatant[] {
  return state.combatants.filter((c) => {
    if (!c.owner_id || c.removed || (c.initiative_mode ?? "after_owner") !== "after_owner") return false;
    const o = byId(state, c.owner_id);
    return Boolean(o && (o.removed || !isAlive(o)));
  });
}

/** Takes turns: not removed, absent or dead. */
export const takesTurn = (c: Combatant): boolean => !c.removed && !c.absent && !isDead(c);

/**
 * The roster as the tracker lists it: the turn order first, then everyone without an initiative yet (PCs whose
 * player hasn't entered one, creatures not rolled) and absent PCs, so nobody is lost from the screen.
 */
export function rosterOrder(state: CombatState): Combatant[] {
  const ordered = turnOrder(state).map((id) => byId(state, id)!).filter(Boolean);
  const rest = state.combatants.filter((c) => !c.removed && !ordered.includes(c));
  return [...ordered, ...rest];
}

export const effectsOn = (state: CombatState, id: string): Effect[] => state.effects.filter((e) => e.target === id);

/** The conditions effects give it. */
export function conditionsOf(state: CombatState, id: string): Condition[] {
  const set = new Set<Condition>();
  for (const e of effectsOn(state, id)) for (const k of e.conditions ?? []) set.add(k);
  return [...set].sort();
}

export const incapacitated = (state: CombatState, id: string): boolean =>
  conditionsOf(state, id).some((k) => INCAPACITATING.includes(k));

export interface AcBreakdown {
  /** Null when the stat block prints none (a thin Aurora companion, D17 b). */
  base: number | null;
  effects: number;
  cover: number;
  /** Null with an unknown base: effects can't make a number out of nothing. */
  total: number | null;
}

/** AC from the stat block, effects and the best cover (C13). */
export function effectiveAc(state: CombatState, c: Combatant): AcBreakdown {
  let effects = 0;
  let cover = 0;
  for (const e of effectsOn(state, c.id)) {
    for (const m of e.modifiers ?? []) {
      if (m.kind === "ac") effects += m.value;
      if (m.kind === "cover") cover = Math.max(cover, m.value === "half" ? 2 : 5);
    }
  }
  const base = c.ac ?? null;
  return { base, effects, cover, total: base === null ? null : base + effects + cover };
}

/** Speeds with effect modifiers, conditions and exhaustion applied (C13). */
export function effectiveSpeed(state: CombatState, c: Combatant): Speeds {
  const mods = effectsOn(state, c.id).flatMap((e) => e.modifiers ?? []);
  const exhaustion = c.exhaustion ?? 0;
  const zero = mods.some((m) => m.kind === "speed_zero")
    || conditionsOf(state, c.id).some((k) => STOPS_MOVEMENT.includes(k))
    || (state.edition === "2014" && exhaustion >= 5);
  const add = mods.reduce((n, m) => n + (m.kind === "speed_add" ? m.value : 0), 0);
  const adjust = (base: number): number => {
    if (zero) return 0;
    let v = base + add;
    for (const m of mods) {
      if (m.kind === "speed_multiply") v *= m.value;
      if (m.kind === "speed_halve") v = Math.trunc(v / 2);
    }
    if (state.edition === "2014" && exhaustion >= 2) v = Math.trunc(v / 2);
    if (state.edition === "2024") v -= 5 * exhaustion;
    return Math.max(0, v);
  };
  const s = c.speed ?? { walk: 30 };
  const out: Speeds = { walk: adjust(s.walk ?? 0) };
  for (const k of ["fly", "swim", "climb", "burrow"] as const) if (s[k] != null) out[k] = adjust(s[k]);
  if (s.hover) out.hover = true;
  return out;
}

const SPEED_WORD: Record<string, string> = { walk: "", fly: "Fly ", swim: "Swim ", climb: "Climb ", burrow: "Burrow " };

/** "30 ft, Fly 60 ft (hover)". */
export function speedText(s: Speeds): string {
  const parts = (["walk", "fly", "swim", "climb", "burrow"] as const)
    .filter((k) => k === "walk" || s[k] != null)
    .map((k) => `${SPEED_WORD[k]}${s[k] ?? 0} ft${k === "fly" && s.hover ? " (hover)" : ""}`);
  return parts.join(", ");
}

export const sameSpeeds = (a: Speeds, b: Speeds): boolean => speedText(a) === speedText(b);

/** The legendary pool it belongs to (C16: several creatures may share one). */
export function poolOf(state: CombatState, id: string): [string, LegendaryPool] | null {
  for (const [key, pool] of Object.entries(state.legendary_pools ?? {})) if (pool.members.includes(id)) return [key, pool];
  return null;
}

/** The member whose turn start refills the pool: the first not defeated. */
export function poolResetOwner(state: CombatState, pool: LegendaryPool): Combatant | undefined {
  for (const m of pool.members) {
    const c = byId(state, m);
    if (c && !isDefeated(c)) return c;
  }
  return undefined;
}

/** The encounter rule that switches this ability off now (D42). */
export function ruleBlocking(state: CombatState, c: Combatant, ability: string): string | null {
  for (const rule of state.rules ?? []) {
    for (const flag of (rule.flags ?? []).filter((f) => f.ability === ability)) {
      const hits = flag.target.kind === "combatant" ? flag.target.id === c.id : c.group === flag.target.id;
      if (hits && rule.active !== flag.while_active) return rule.name;
    }
  }
  return null;
}

/** Why an ability can't be used now, in the engine's words (`Unavailable`), or null when it can (C14). */
export function abilityStatus(state: CombatState, c: Combatant, ability: Ability, slot?: number): string | null {
  if (isDefeated(c)) return "Defeated";
  if (ability.kind === "passive") return "Passive";
  if (!isAlive(c)) return "Down";
  if (ability.kind !== "free" && incapacitated(state, c.id)) return "Incapacitated";
  const rule = ruleBlocking(state, c, ability.key);
  if (rule) return `Switched off by ${rule}`;
  if (ability.requires_effect) {
    const name = ability.requires_effect;
    const on = effectsOn(state, c.id).some((e) => e.name === name) || c.concentration?.name === name;
    if (!on) return `Requires ${name} active`;
  }
  if (ability.recharge && !ability.recharge.ready) return "Needs recharge";
  if (ability.uses) {
    const r = c.resources?.[ability.uses];
    if (!r) return `No uses left: ${ability.uses}`;
    if (r.pool.remaining <= 0) return `No uses left: ${r.label}`;
  }
  if (slot !== undefined) {
    const p = c.spell_slots?.[String(slot)];
    if (!p || p.remaining <= 0) return `No level ${slot} slot left`;
  }
  if (ability.group_limit) {
    const g = state.group_limits?.[ability.group_limit];
    if (g && g.used >= g.per_round) return `Group limit used this round: ${ability.group_limit}`;
  }
  const economy = c.economy ?? { action: false, bonus_action: false, reaction: false };
  if (ability.kind === "action" && economy.action) return "Action spent";
  if (ability.kind === "bonus_action" && economy.bonus_action) return "Bonus action spent";
  if (ability.kind === "reaction" && economy.reaction) return "Reaction spent";
  if (ability.kind === "legendary") {
    const found = poolOf(state, c.id);
    if (!found) return "No legendary actions";
    if (state.current === c.id) return "Not on its own turn";
    if (found[1].remaining < (ability.legendary_cost ?? 1)) return "Not enough legendary actions";
  }
  return null;
}

/** The concentration save DC for this much damage (C11): max(10, half), capped at 30 under 2024. */
export function concentrationDc(state: CombatState, damage: number): number {
  const dc = Math.max(10, Math.floor(damage / 2));
  return state.edition === "2024" ? Math.min(30, dc) : dc;
}

/** How long an effect has left, in words. */
export function durationText(state: CombatState, e: Effect): string {
  const d = e.duration ?? { kind: "manual" };
  if (d.kind === "manual") return "Until removed";
  const whose = d.kind === "source_turns" ? byId(state, e.source) : byId(state, e.target);
  const who = whose ? `${whose.label}'s` : d.kind === "source_turns" ? "source's" : "target's";
  const n = d.remaining;
  if (n > 10 && n % 10 === 0) return `${n / 10} min`;
  return `${n} ${who} turn${n === 1 ? "" : "s"} (${d.at})`;
}
