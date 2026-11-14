import { serverFetch, type ServerEvent } from "../../api/http";
import { liveEvents } from "../../api/liveEvents";
import { loadSession, type ServerSession } from "../../api/session";
import type { HealthBand } from "../../api/overrides/parties";
import type { D20Input, StatBlock } from "../shared/types";

/**
 * The player's side of a live combat (plan 4.4, 0.3.0 part F), as party/docs/combat-api.md describes it: the server's
 * PLAYER projection of a party's combat (`view_for`) and the few writes a player may make for their own PC
 * (initiative, death saves, concentration saves, and resolving the DM's "Take 7 slashing?" prompt). The server builds
 * the projection, so hidden creatures, their turns and anything about them never reach this module: there is nothing
 * here to filter (plan 3.5, D5). `fromWire` turns the projection into the view model the Combat sub-tab draws.
 *
 * Every call goes through {@link combatPlayerDeps}, so tests answer them from a fake server (../../test/combatServer.ts).
 */

export type { D20Input, StatBlock } from "../shared/types";

/** Whose turn players see (D5): while hidden creatures act, it stays on the last visible turn. */
export type TurnPointer =
  | { kind: "combatant"; id: string; round: number }
  | { kind: "enemy_turn"; round: number }
  | { kind: "waiting"; round: number }
  | { kind: "not_started" }
  | { kind: "ended" };

// ---- the wire (combat-api.md "The projection") -------------------------------------------------------------------

/**
 * A death-save state, the engine's `Life`: `{state: "alive" | "stable" | "dead"}` or
 * `{state: "dying", successes, failures}`. Earlier drafts sent a bare string or `{dying: {…}}`; both are still read.
 */
export type WireLife =
  | { state: "alive" | "stable" | "dead" }
  | { state: "dying"; successes: number; failures: number }
  | "alive" | "stable" | "dead"
  | { dying: { successes: number; failures: number } };

export interface WireCombatant {
  id: string;
  kind: "creature" | "character";
  name: string;
  mine?: boolean;
  /** PCs only: creatures' initiative is never sent. */
  initiative?: number | null;
  band?: HealthBand | null;
  life?: WireLife;
  /** Condition names; objects with turns left are taken too. */
  conditions?: (string | { name: string; remaining?: number | null; text?: string | null })[];
  exhaustion?: number;
  concentration?: string | null;
  /** Revealed creatures (D20 b). */
  hp?: { current: number; max: number };
  ac?: number;
  statBlock?: StatBlock | null;
  /** The engine's `player_view_of` (snake_case) for my own PC and for revealed creatures. */
  view?: { owner_id?: string | null; hp?: { current: number; max: number; temp?: number }; ac?: number | null } & Record<string, unknown>;
  /** An earlier draft's owner field. */
  ownerId?: string | null;
}

export interface WirePrompt {
  id: string;
  kind: "damage" | "heal";
  amount: number;
  parts?: { amount: number; damage_type?: string | null }[];
  criticalHit?: boolean;
  at?: number;
}

export interface WireMine {
  id: string;
  /** My PC (`character`) or a creature it controls (`creature`). */
  kind?: "character" | "creature";
  /** C1: not in this fight. */
  absent?: boolean;
  /** 0.3.3 (role-views.md): this PC is in the combat's initiative; false = the fight goes on without it. */
  inInitiative?: boolean;
  /** My PC's companions the server offers to summon (from the character), by `key`. */
  offers?: { key: string; name: string; hp?: number | null; ac?: number | null; initiative?: number | null }[];
  /** My latest roll results, newest first (D21). */
  rolls?: { id: string; name: string; total: number; natural?: number | null; source?: string; breakdown?: string }[];
  /** The PC's character (asked of the server; matched by party and name until it sends it). */
  characterId?: string | null;
  hp?: { current: number; max: number; temp: number };
  life?: WireLife;
  concentration?: string | null;
  concentrationChecks?: { dc: number }[];
  prompts?: WirePrompt[];
}

/**
 * 0.3.2: a save one of my PCs is asked for against a pending cast (party-combat `visibility::player_casts`). `spell`
 * and `from` (the caster's id) are left out while the caster is hidden ("from an unseen creature").
 */
export interface WireCastRow {
  cast: string;
  target: string;
  ray?: number | null;
  ability: string;
  dc?: number | null;
  resolved: boolean;
  spell?: string;
  from?: string;
}

/**
 * One of the player's own open casts (party/docs/role-views.md `myCasts`): the attack rows are theirs to roll
 * (`resolve_cast_row`), the DM's creatures' save rows wait for the DM (outcome null), and once every row has an
 * outcome the amounts are theirs to roll and apply (`apply_cast`, in this order). Rows on hidden combatants are left out.
 */
export interface MyCast {
  cast: string;
  caster: string;
  spell: string;
  slot: number | null;
  delivery: "auto" | "attack" | "save";
  rows: { target: string; ray?: number | null; outcome: null | "lands" | "hit" | "crit" | "miss" | "saved" | "failed" }[];
  amounts: { kind: "damage" | "heal" | "temp_hp"; dice: string; damage_type?: string | null; choice?: string[]; add_mod?: boolean }[];
  needs_apply: boolean;
}

export interface WireProjection {
  id: string;
  partyId: string;
  encounter?: { id: string; publicName?: string | null } | null;
  status: "setup" | "active" | "ended";
  pseq: number;
  turn: TurnPointer;
  showEnemyTurn?: boolean;
  order: WireCombatant[];
  me: WireMine[];
  casts?: WireCastRow[];
  /** 0.3.3 (role-views.md): the player's own open casts (their PC's and its creatures'). */
  myCasts?: MyCast[];
}

// ---- the view model -------------------------------------------------------------------------------------------------

export interface CombatCondition {
  name: string;
  /** Turns left, when it has a turn count. */
  remaining?: number | null;
  /** The rules text, when the server sends it. */
  text?: string | null;
}

export type Life = "alive" | "dying" | "stable" | "dead";

/** A combatant players can see: a PC, or a creature the DM revealed. */
export interface PlayerCombatant {
  id: string;
  kind: "creature" | "character";
  /** The PC's name, or the creature's public name. */
  name: string;
  /** One of the signed-in account's own PCs. */
  mine: boolean;
  /** Own PCs: the character's id in the player's folder, when known. */
  characterId?: string | null;
  initiative: number | null;
  /** Unhurt / Bloodied / Down (D6, combat only). */
  band?: HealthBand | null;
  conditions: CombatCondition[];
  life?: Life;
  /** Own PCs only. */
  deathSaves?: { successes: number; failures: number } | null;
  concentration?: string | null;
  /** My own controlled creatures (theirs lives in the combat); never a monster's. */
  hp?: { current: number; max: number; temp?: number } | null;
  ac?: number | null;
  /** A controlled creature's owner (a combatant id): it acts right after it unless it has its own initiative. */
  ownerId?: string | null;
  /** In the visible turn order (my PC isn't before its initiative is in). */
  inOrder: boolean;
  /** My own: the latest roll, as words. */
  lastRoll?: string | null;
  /** My PC: false when the server says it isn't in this fight's initiative (no live combat section for it). */
  inInitiative?: boolean;
  /** My PC: the companions the server offers to summon (`summon {owner, companion: key}`). */
  offers?: { key: string; name: string; hp?: number | null; ac?: number | null }[] | null;
}

/** Something the server asks of the signed-in player for their own PC. */
export type CombatPrompt =
  /** The DM hit the PC (D7 a): the player applies it to their own character, or not. */
  | { id: string; kind: "damage"; combatant: string; amount: number; damageType?: string | null; critical?: boolean }
  | { id: string; kind: "heal"; combatant: string; amount: number }
  /** The PC took damage while concentrating: a save is waiting. */
  | { id: string; kind: "concentration"; combatant: string; dc: number }
  /** 0.3.2: a spell's (or a breath weapon's) save: the player rolls at the table and types it, or the app rolls. */
  | { id: string; kind: "cast_save"; combatant: string; cast: string; ray: number | null; spell: string | null; ability: string; dc: number | null; from: string | null };

/** A line of the revealed-events log, built on this device from what the player saw change (D32: revealed only). */
export interface CombatLogEntry {
  round: number;
  text: string;
  /** ms */
  at: number;
}

/** A live combat as the Combat sub-tab draws it. */
export interface PlayerCombat {
  id: string;
  partyId: string;
  partyName?: string;
  /** The encounter's public name. */
  title?: string | null;
  /** This player's own sequence: moves on with every change they can see. */
  seq: number;
  status: "setup" | "active" | "ended";
  round: number;
  pointer: TurnPointer;
  /** The turn order, visible combatants only. */
  combatants: PlayerCombatant[];
  prompts: CombatPrompt[];
  log: CombatLogEntry[];
  /** The player's own open casts. */
  myCasts: MyCast[];
}

/** What `fromWire` knows besides the projection: the party's own characters of this account. */
export interface WireContext {
  partyName?: string;
  /** The account's characters in this party. */
  myCharacters?: string[];
}

const life = (l: WireLife | undefined): { life?: Life; deathSaves?: { successes: number; failures: number } | null } => {
  if (!l) return {};
  if (typeof l === "string") return { life: l };
  if ("dying" in l) return { life: "dying", deathSaves: { ...l.dying } };
  if (l.state === "dying") return { life: "dying", deathSaves: { successes: l.successes, failures: l.failures } };
  return { life: l.state };
};

const conditions = (c: WireCombatant): CombatCondition[] => [
  ...(c.conditions ?? []).map((x) => (typeof x === "string" ? { name: x } : { name: x.name, remaining: x.remaining ?? null, text: x.text ?? null })),
  ...(c.exhaustion && c.exhaustion > 0 ? [{ name: `exhaustion ${c.exhaustion}` }] : []),
];

const roundOf = (t: TurnPointer) => ("round" in t ? t.round : 0);

/** The projection as the Combat sub-tab draws it. */
export function fromWire(w: WireProjection, ctx: WireContext = {}): PlayerCombat {
  const mineById = new Map((w.me ?? []).map((m) => [m.id, m]));
  // My PCs: in the order (mine, or listed in `me`), and in `me` only before their initiative is in.
  const minePcs = new Set([
    ...(w.order ?? []).filter((o) => o.kind === "character" && (o.mine || mineById.has(o.id))).map((o) => o.id),
    ...(w.me ?? []).filter((m) => (m.kind ?? "character") === "character").map((m) => m.id),
  ]);
  // Which character each own PC is: the server's, else the party's only one of this account's characters.
  const onlyCharacter = (ctx.myCharacters ?? []).length === 1 && minePcs.size === 1 ? ctx.myCharacters![0]! : null;
  const row = (o: WireCombatant, inOrder: boolean): PlayerCombatant => {
    const m = mineById.get(o.id);
    const mine = Boolean(o.mine || m);
    const roll = m?.rolls?.[0];
    return {
      id: o.id, kind: o.kind, name: o.name, mine, inOrder,
      ...(mine && o.kind === "character" ? { characterId: m?.characterId ?? onlyCharacter } : {}),
      initiative: o.initiative ?? null,
      band: o.band ?? null,
      conditions: conditions(o),
      ...life(m?.life ?? o.life),
      concentration: m?.concentration ?? o.concentration ?? null,
      ownerId: o.view?.owner_id ?? o.ownerId ?? null,
      // A monster is its name and band only (user, 2026-11-14); my own creatures keep their HP, which I play.
      ...(o.kind === "creature" && mine ? { hp: m?.hp ?? o.view?.hp ?? o.hp ?? null, ac: o.view?.ac ?? o.ac ?? null } : {}),
      ...(m?.inInitiative === false ? { inInitiative: false } : {}),
      ...(m?.offers ? { offers: m.offers.map((k) => ({ key: k.key, name: k.name, hp: k.hp ?? null, ac: k.ac ?? null })) } : {}),
      ...(roll ? { lastRoll: `${roll.name} ${roll.total}${roll.breakdown ? ` (${roll.breakdown})` : ""}` } : {}),
    };
  };
  const inOrder = new Set((w.order ?? []).map((o) => o.id));
  const combatants = [
    ...(w.order ?? []).map((o) => row(o, true)),
    // Mine that aren't in the order yet (no initiative): the PC before it rolls, or an absent one.
    ...(w.me ?? []).filter((m) => !inOrder.has(m.id)).map((m) => row({
      id: m.id, kind: m.kind ?? "character", name: "", mine: true, initiative: null, conditions: [], life: m.life, concentration: m.concentration,
    }, false)),
  ];
  const prompts: CombatPrompt[] = (w.me ?? []).flatMap((m) => [
    ...(m.prompts ?? []).map((p): CombatPrompt => {
      if (p.kind === "heal") return { id: p.id, kind: "heal", combatant: m.id, amount: p.amount };
      const types = [...new Set((p.parts ?? []).map((x) => x.damage_type).filter((t): t is string => Boolean(t)))];
      return { id: p.id, kind: "damage", combatant: m.id, amount: p.amount, damageType: types.length ? types.join(" and ") : null, critical: Boolean(p.criticalHit) };
    }),
    ...(m.concentrationChecks ?? []).slice(0, 1).map((c): CombatPrompt => ({ id: `concentration-${m.id}`, kind: "concentration", combatant: m.id, dc: c.dc })),
  ]);
  // Saves against pending casts, typed from the table first (0.3.2); a hidden caster stays unnamed.
  const nameOf = (id: string | undefined) => (id ? (w.order ?? []).find((o) => o.id === id)?.name ?? null : null);
  for (const r of w.casts ?? []) {
    if (r.resolved) continue;
    prompts.push({
      id: `cast-${r.cast}-${r.target}-${r.ray ?? 0}`, kind: "cast_save", combatant: r.target, cast: r.cast, ray: r.ray ?? null,
      spell: r.spell ?? null, ability: r.ability.toLowerCase(), dc: r.dc ?? null, from: nameOf(r.from),
    });
  }
  return {
    id: w.id, partyId: w.partyId, partyName: ctx.partyName, title: w.encounter?.publicName ?? null,
    seq: w.pseq ?? 0, status: w.status, round: roundOf(w.turn), pointer: w.turn ?? { kind: "not_started" },
    combatants, prompts, log: [], myCasts: w.myCasts ?? [],
  };
}

// ---- calls ----------------------------------------------------------------------------------------------------------

export interface CombatPlayerDeps {
  fetch: (session: Pick<ServerSession, "baseUrl" | "token">, method: string, path: string, body?: unknown) => Promise<unknown>;
  /** The account's event stream: events, and opens (`again` after a drop). Answers a stop. */
  events: (onEvent: (e: ServerEvent) => void, onOpen: (again: boolean) => void) => () => void;
  /** One id per user action (combat-api.md "Writes"). */
  requestId: () => string;
}

const DEFAULT_DEPS: CombatPlayerDeps = {
  fetch: (session, method, path, body) => serverFetch(session, method, path, body),
  events: (onEvent, onOpen) => {
    const session = loadSession();
    return session ? liveEvents(session, onEvent, { onOpen }) : () => {};
  },
  requestId: () => {
    try { return crypto.randomUUID(); } catch { return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`; }
  },
};

export const combatPlayerDeps: CombatPlayerDeps = { ...DEFAULT_DEPS };

/** Tests: start over. */
export function resetCombatPlayer(): void {
  Object.assign(combatPlayerDeps, DEFAULT_DEPS);
}

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject({ code: "unauthorized", message: "Not signed in." });
  return combatPlayerDeps.fetch(session, method, path, body) as Promise<T>;
}

const seg = (id: string) => encodeURIComponent(id);
const notFound = (e: unknown) => (e as { code?: string } | null)?.code === "not_found";

/** The party's live combat as the signed-in player sees it, or null when there is none (or it's not theirs to see). */
export async function liveCombat(partyId: string, ctx: WireContext = {}): Promise<PlayerCombat | null> {
  try {
    const found = await call<{ combat: string | null }>("GET", `/api/parties/${seg(partyId)}/combat`);
    if (!found.combat) return null;
    const view = await call<WireProjection>("GET", `/api/combats/${seg(found.combat)}`);
    return fromWire(view, ctx);
  } catch (e) {
    if (notFound(e)) return null;
    throw e;
  }
}

/** A player write; it answers the new projection. `id` must be the player's own PC; the server checks. */
/**
 * A player write; it answers the new projection. `id` must be the player's own PC; the server checks. The same
 * `requestId` is sent again on a retry, so the server does it at most once (combat-api.md "Writes").
 */
export const playerAction = (combat: string, action: string, args: Record<string, unknown>, ctx?: WireContext, requestId = combatPlayerDeps.requestId()) =>
  call<WireProjection>("POST", `/api/combats/${seg(combat)}/${action}`, { requestId, args })
    .then((w) => fromWire(w, ctx));
const act = (combat: string, action: string, args: Record<string, unknown>, ctx?: WireContext) => playerAction(combat, action, args, ctx);

export const setInitiative = (combat: string, combatant: string, roll: D20Input, ctx?: WireContext) => act(combat, "set_initiative", { id: combatant, roll }, ctx);
export const deathSave = (combat: string, combatant: string, roll: D20Input, ctx?: WireContext) => act(combat, "death_save", { id: combatant, roll }, ctx);
export const resolveConcentration = (combat: string, combatant: string, roll: D20Input, ctx?: WireContext) =>
  act(combat, "resolve_concentration", { id: combatant, roll }, ctx);
/** The damage or healing prompt was applied to the player's own character (by them), with its HP after, or dismissed. */
export const resolvePrompt = (combat: string, promptId: string, applied: boolean, hp?: { current: number; max: number; temp: number }, ctx?: WireContext) =>
  act(combat, "resolve_prompt", { promptId, applied, ...(hp ? { hp } : {}) }, ctx);

/**
 * 0.3.3: the player casts a spell from their own sheet in the live combat (party/docs/role-views.md). The server
 * compiles its effect from the spell's mechanics and the caster's sheet numbers; what it applies (Bless on allies,
 * a save the DM's creatures make) reaches the DM, never the player's slots: the player spends the slot on their
 * own sheet.
 */
export interface PlayerCast {
  /** My PC's combatant id. */
  caster: string;
  /** The spell element's id on the sheet. */
  spellKey: string;
  name: string;
  level: number;
  /** The slot level spent (none for a cantrip, a ritual or a free cast). */
  slot?: number;
  targets: string[];
  /** How it's cast: action, bonus or reaction (it spends that). */
  economy?: "action" | "bonus" | "reaction";
  /** The caster's numbers from the sheet, for a save DC or a spell attack. */
  saveDc?: number;
  attackBonus?: number;
  /** The spellcasting ability's modifier, and the cantrip damage tier by character level (1–4). */
  mod?: number;
  cantripTier?: number;
  concentration?: boolean;
}

/**
 * The `cast_spell` args a player sends (party/docs/role-views.md): the server compiles the definition from `spellKey`
 * with the sheet's numbers (`dc`, `attack`, `mod`, `cantripTier`); the slot level is the one the app spends on the
 * sheet. The rest of {@link PlayerCast} is what this device uses (the economy).
 */
export const castArgs = (c: PlayerCast): Record<string, unknown> => ({
  caster: c.caster, spellKey: c.spellKey, targets: c.targets, slot: c.slot ?? null,
  ...(c.saveDc != null ? { dc: c.saveDc } : {}), ...(c.attackBonus != null ? { attack: c.attackBonus } : {}),
  ...(c.mod != null ? { mod: c.mod } : {}), ...(c.cantripTier != null ? { cantripTier: c.cantripTier } : {}),
});

const COMBAT_EVENT = "party://combat";
const PARTY_EVENT = "party://party";
const RESYNC_EVENT = "party://resync";

export type CombatEvent =
  /** A party's combat changed as this player sees it, or started or ended: fetch it again. */
  | { type: "combat"; party: string; seq: number }
  /** A party changed some other way (members joined or left): its combat may have too. */
  | { type: "party"; party: string }
  /** Events may have been missed: fetch everything again. */
  | { type: "resync" };

/** Combat events, as they come; a reopened stream counts as a resync. Answers a stop. */
export function watchCombats(listener: (e: CombatEvent) => void): () => void {
  return combatPlayerDeps.events((e) => {
    const p = (e.payload ?? {}) as Record<string, unknown>;
    if (e.event === COMBAT_EVENT && typeof p.party === "string") listener({ type: "combat", party: p.party, seq: Number(p.pseq ?? 0) });
    // A combat starting or ending (kind "combat") carries the party's own sequence, not the combat's: always refetch.
    else if (e.event === PARTY_EVENT && typeof p.party === "string") {
      listener(String(p.kind ?? "") === "combat" ? { type: "combat", party: p.party, seq: 0 } : { type: "party", party: p.party });
    } else if (e.event === RESYNC_EVENT) listener({ type: "resync" });
  }, (again) => { if (again) listener({ type: "resync" }); });
}

/** Whose turn it is, as a combatant id, or null (waiting, an enemy turn, not started, ended). */
export function actingId(c: Pick<PlayerCombat, "pointer">): string | null {
  return c.pointer.kind === "combatant" ? c.pointer.id : null;
}

/**
 * The signed-in player's combatant that is on deck (acts right after whoever is acting now), or null. Worked out from
 * the visible order only (a hidden creature in between can make the banner come one turn early, and nothing about it
 * leaks). Nothing is on deck while it's already one of mine, or on an enemy turn (whose place isn't shown).
 */
export function onDeck(c: Pick<PlayerCombat, "pointer" | "combatants" | "status">): PlayerCombatant | null {
  const acting = actingId(c);
  if (!acting || c.status !== "active") return null;
  const order = c.combatants.filter((x) => x.inOrder && x.life !== "dead");
  const at = order.findIndex((x) => x.id === acting);
  if (at < 0 || order[at].mine) return null;
  for (let i = 1; i < order.length; i += 1) {
    const next = order[(at + i) % order.length];
    if (next.id === acting) return null;
    return next.mine ? next : null;
  }
  return null;
}

/** Which character a session's open sheet is, for finding its PC. */
export interface OpenCharacter { id: string; name?: string | null }

/**
 * The signed-in player's PC in this combat: the one for the open character when given (by its id, else by its name
 * while the server doesn't say which character a PC is), else the first.
 */
export function myCombatant(c: Pick<PlayerCombat, "combatants">, open?: OpenCharacter | string | null): PlayerCombatant | null {
  // The player's PCs only; their summons and pets are theirs too, but no sheet is open for them.
  const mine = c.combatants.filter((x) => x.mine && x.kind === "character");
  if (!open) return mine[0] ?? null;
  const { id, name } = typeof open === "string" ? { id: open, name: null } : open;
  return mine.find((x) => x.characterId === id) ?? (name ? mine.find((x) => !x.characterId && x.name === name) : undefined) ?? null;
}

const titleCase = (s: string) => s.replace(/(^|\s)\S/g, (m) => m.toUpperCase());
const BAND_WORD: Record<HealthBand, string> = { unhurt: "unhurt", bloodied: "bloodied", down: "down" };

/**
 * What changed between two projections the player saw, as log lines (D32 a: players see revealed events only, and the
 * projection holds nothing else). Turn moves aren't logged; the header says whose turn it is.
 */
export function diffEvents(before: PlayerCombat | null, after: PlayerCombat, at: number): CombatLogEntry[] {
  const round = after.round;
  const line = (text: string): CombatLogEntry => ({ round, text, at });
  if (!before) return [line(after.status === "setup" ? "The DM started a combat: roll initiative." : "The combat is on.")];
  const out: CombatLogEntry[] = [];
  if (before.status === "setup" && after.status === "active") out.push(line("The fight begins."));
  if (after.round > before.round && before.round > 0) out.push(line(`Round ${after.round}.`));
  const old = new Map(before.combatants.map((c) => [c.id, c]));
  const now = new Map(after.combatants.map((c) => [c.id, c]));
  for (const c of after.combatants) {
    const was = old.get(c.id);
    if (!was) { out.push(line(c.kind === "creature" ? `${c.name} appears.` : `${c.name} joins the fight.`)); continue; }
    // A monster: only its name and whether it's bloodied or down (user, 2026-11-14).
    const monster = c.kind === "creature" && !c.mine;
    if (c.band && was.band && c.band !== was.band && !(monster && c.band === "unhurt")) out.push(line(`${c.name} is ${BAND_WORD[c.band]}.`));
    if (monster) continue;
    if (c.life !== was.life && c.life && c.life !== "alive") out.push(line(`${c.name} is ${c.life}.`));
    const had = new Set(was.conditions.map((x) => x.name));
    const has = new Set(c.conditions.map((x) => x.name));
    for (const n of has) if (!had.has(n)) out.push(line(`${c.name}: ${titleCase(n)}.`));
    for (const n of had) if (!has.has(n)) out.push(line(`${c.name}: no longer ${titleCase(n)}.`));
  }
  for (const c of before.combatants) if (!now.has(c.id)) out.push(line(`${c.name} is gone.`));
  return out;
}
