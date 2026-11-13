import { serverFetch } from "../http";
import { loadSession, type ServerSession } from "../session";
import { watchParties, type Ruleset } from "./parties";
import { override } from "./registry";

/**
 * Encounters (release 0.3.0): the server's `/api/parties/{party}/encounters` and `/api/encounters/*` routes as
 * party/docs/encounters-api.md describes them, and the monster picker's `/api/bestiary/*` search (part A's
 * party/docs/bestiary-api.md).
 *
 * The DM gets everything; a player gets the server's projection of a player-visible encounter, which never holds a
 * hidden creature or anything DM-only. Every call goes through {@link encountersDeps}, so tests answer them from a
 * fake server (../../test/encountersServer.ts). Times from the server are seconds since the epoch.
 */

export type Status = "draft" | "ready" | "live" | "ended";
export type RevealLevel = "name" | "band" | "full";
export type Band = "trivial" | "low" | "moderate" | "high" | "extreme";
export type HpMode = "average" | "rolled";
export type Ability = "str" | "dex" | "con" | "int" | "wis" | "cha";
export const ABILITIES: Ability[] = ["str", "dex", "con", "int", "wis", "cha"];

/** A stat block (upstream's `MonsterData`, snake_case as the server sends it). Only the parts the app shows. */
export interface MonsterBlock {
  ruleset: string;
  size: string[];
  creature_type: string;
  tags?: string[];
  alignment?: string;
  ac?: { value: number; source?: string; condition?: string }[];
  ac_note?: string;
  hp?: { average: number; formula?: string };
  hp_note?: string;
  speed?: Record<string, number | boolean | undefined>;
  abilities?: Record<Ability, number>;
  saves?: Partial<Record<Ability, number>>;
  skills?: Record<string, number>;
  damage_vulnerabilities?: { text?: string; types?: string[] }[];
  damage_resistances?: { text?: string; types?: string[] }[];
  damage_immunities?: { text?: string; types?: string[] }[];
  condition_immunities?: string[];
  senses?: Record<string, unknown>;
  languages?: string[];
  cr?: string;
  xp?: number;
  traits?: Feature[];
  actions?: Feature[];
  bonus_actions?: Feature[];
  reactions?: Feature[];
  legendary?: { uses?: number; intro?: string; actions?: Feature[] } & Record<string, unknown>;
}

export interface Feature {
  name: string;
  text: string;
  cost?: number;
  usage?: unknown;
}

export interface HpValue { average: number; formula?: string }
export interface AcValue { value: number; source?: string }
export interface Changed<T> { value: T; base: T | null }

/** What an encounter changes on one creature's block (D41). `base` is the source value, recorded by the server. */
export interface Overrides {
  hp?: Changed<HpValue>;
  ac?: Changed<AcValue>;
  saves?: Partial<Record<Ability, Changed<number>>>;
  traits?: { name: string; text: string }[];
  notes?: string;
}

/** What the app sends to change overrides: absent = leave, null = drop. */
export interface OverridesForm {
  hp?: HpValue | null;
  ac?: AcValue | null;
  saves?: Partial<Record<Ability, number | null>>;
  traits?: { name: string; text: string }[];
  notes?: string | null;
}

/** A creature as the DM sees it. */
export interface Creature {
  id: string;
  sort: number;
  dmLabel: string;
  publicName: string;
  hidden: boolean;
  revealLevel: RevealLevel;
  revealedAt: number | null;
  elementId: string;
  packId: string;
  packRev: string;
  name: string;
  source: string;
  ruleset: string;
  mapsTo: string | null;
  cr: string | null;
  xp: number | null;
  type: string;
  size: string[];
  hp: HpValue | null;
  hpMode: "encounter" | "fixed";
  hpMax: number | null;
  ac: number | null;
  printed: { hp: HpValue | null; ac: number | null; saves: Record<Ability, number> };
  overrides: Overrides;
  modified: boolean;
  statBlock: MonsterBlock;
}

export interface Modifiers { ac?: number; speedAdd?: number; speedMult?: number; speedZero?: boolean }

/** A starting effect (C19), applied when combat starts. */
export interface Effect {
  id: string;
  creatureId: string;
  sort: number;
  name: string;
  detail: string;
  sourceCreatureId: string | null;
  durationTurns: number | null;
  charges: number | null;
  concentration: boolean;
  slotLevel: number | null;
  modifiers: Modifiers;
}

export interface RuleFlag { creatureId: string; ability: string; whileActive: "on" | "off" }

/** An encounter rule (C20, D42): a banner and on/off flags tied to a creature. */
export interface Rule {
  id: string;
  sort: number;
  name: string;
  text: string;
  creatureId: string | null;
  endsWhen: "creature_down" | "manual";
  active: boolean;
  flags: RuleFlag[];
}

export interface Warning { kind: string; text: string; creatureIds?: string[]; creatures?: { creatureId: string; ruleset: string; mapsTo: string | null }[]; heuristic?: boolean }

export interface Evaluation {
  method: "srd521";
  ruleset: Ruleset;
  note?: string;
  party: { levels: number[]; from: "members" | "given" };
  totalXp: number;
  band: Band | null;
  thresholds: { low: number; moderate: number; high: number };
  remaining: { low: number; moderate: number; high: number };
  creatures: number;
  unrated: number;
  warnings: Warning[];
}

/** An encounter as its DM sees it. */
export interface Encounter {
  id: string;
  partyId: string;
  name: string;
  publicName: string;
  ruleset: Ruleset;
  rulesetSet: Ruleset | null;
  partyRuleset: Ruleset;
  status: Status;
  playerVisible: boolean;
  visibleToPlayers: boolean;
  hpMode: HpMode;
  dmNotes: string;
  createdAt: number;
  updatedAt: number;
  creatures: Creature[];
  effects: Effect[];
  rules: Rule[];
  evaluation: Evaluation;
}

/** One row of the DM's list. */
export interface EncounterSummary {
  id: string;
  name: string;
  publicName: string;
  status: Status;
  playerVisible: boolean;
  visibleToPlayers: boolean;
  creatureCount: number;
  hiddenCount: number;
  xp: number;
  band: Band | null;
  updatedAt: number;
}

/** What a player sees of a revealed creature. */
export interface SeenCreature {
  id: string;
  publicName: string;
  revealLevel: RevealLevel;
  size: string[];
  type: string;
  conditions: string[];
  band?: "unhurt" | "bloodied" | "down";
  hp?: { current: number; max: number };
  statBlock?: MonsterBlock;
}

/** A player-visible encounter as a player sees it. */
export interface SeenEncounter {
  id: string;
  partyId: string;
  publicName: string;
  status: Status;
  creatures: SeenCreature[];
}

export type EncounterList =
  | { role: "dm"; encounters: EncounterSummary[] }
  | { role: "player"; encounters: { id: string; publicName: string; status: Status }[] };

/** A refresh: the fields both the DM and the source changed (D41). */
export interface Conflict { field: string; was: unknown; source: unknown; value: unknown }
export interface RefreshOutcome { changed: boolean; conflicts: Conflict[]; applied: boolean; encounter: Encounter }

// The monster picker (part A's bestiary API).

export interface MonsterQuery {
  /** Part A lists Aurora companions too (D17 b); encounters take typed Monster elements only for now. */
  kinds?: ("monster" | "companion")[];
  text?: string;
  crMin?: string;
  crMax?: string;
  types?: string[];
  sizes?: string[];
  ruleset?: Ruleset;
  legendary?: boolean;
  sources?: string[];
  sort?: "name" | "cr";
  limit?: number;
  offset?: number;
}

export interface MonsterSummary {
  id: string;
  name: string;
  cr: string | null;
  xp: number | null;
  type: string;
  size: string[];
  ac: number | null;
  hp: number | null;
  source: string;
  ruleset: string;
  legendary: boolean;
}

export interface MonsterPage { total: number; offset: number; limit: number; items: MonsterSummary[] }
export interface Facet { value: string; count: number; name?: string }
export interface Facets { total: number; types: Facet[]; sizes: Facet[]; crs: Facet[]; rulesets: Facet[]; sources: Facet[] }

export interface EncountersDeps {
  fetch: (session: Pick<ServerSession, "baseUrl" | "token">, method: string, path: string, body?: unknown) => Promise<unknown>;
}

const DEFAULT_DEPS: EncountersDeps = {
  // Late-bound: ../http.ts imports this module before its own exports are set.
  fetch: (session, method, path, body) => serverFetch(session, method, path, body),
};

export const encountersDeps: EncountersDeps = { ...DEFAULT_DEPS };

/** Tests: start over. */
export function resetEncounters(): void {
  Object.assign(encountersDeps, DEFAULT_DEPS);
}

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject({ code: "unauthorized", message: "Not signed in." });
  return encountersDeps.fetch(session, method, path, body) as Promise<T>;
}

export const isNotFound = (e: unknown) => (e as { code?: string } | null)?.code === "not_found";
const seg = (id: string) => encodeURIComponent(id);
const ms = (seconds: number) => seconds * 1000;

const encounter = (e: Encounter): Encounter => ({
  ...e,
  createdAt: ms(e.createdAt),
  updatedAt: ms(e.updatedAt),
  creatures: e.creatures.map((c) => ({ ...c, revealedAt: c.revealedAt == null ? null : ms(c.revealedAt) })),
});

/** The limits the server keeps (party/docs/encounters-api.md). */
export const LIMITS = {
  name: 80, publicName: 60, label: 80, notes: 4000, addAtOnce: 20, creatures: 60, effects: 40, rules: 20,
  effectName: 80, detail: 2000, ruleText: 2000, flags: 20, traits: 12, traitName: 80,
} as const;

/** A party's encounters: all of them for its DM, the player-visible ones for a player. Null when it isn't yours. */
export async function listEncounters(partyId: string): Promise<EncounterList | null> {
  try {
    const out = await call<EncounterList>("GET", `/api/parties/${seg(partyId)}/encounters`);
    if (out.role === "dm") return { role: "dm", encounters: out.encounters.map((e) => ({ ...e, updatedAt: ms(e.updatedAt) })) };
    return out;
  } catch (e) {
    if (isNotFound(e)) return null;
    throw e;
  }
}

/** The DM's view; for a player, use {@link seeEncounter}. Null when it is gone or not the caller's to see. */
export async function loadEncounter(id: string): Promise<Encounter | null> {
  try {
    return encounter(await call<Encounter>("GET", `/api/encounters/${seg(id)}`));
  } catch (e) {
    if (isNotFound(e)) return null;
    throw e;
  }
}

/** A player's projection of a player-visible encounter, or null (a draft, gone, not shown). */
export async function seeEncounter(id: string): Promise<SeenEncounter | null> {
  try {
    return await call<SeenEncounter>("GET", `/api/encounters/${seg(id)}`);
  } catch (e) {
    if (isNotFound(e)) return null;
    throw e;
  }
}

export interface EncounterFields {
  name?: string;
  publicName?: string;
  ruleset?: Ruleset | null;
  status?: "draft" | "ready" | "ended";
  playerVisible?: boolean;
  hpMode?: HpMode;
  dmNotes?: string;
}

export const createEncounter = (partyId: string, fields: EncounterFields) =>
  call<Encounter>("POST", `/api/parties/${seg(partyId)}/encounters`, fields).then(encounter);
export const updateEncounter = (id: string, fields: EncounterFields) =>
  call<Encounter>("PATCH", `/api/encounters/${seg(id)}`, fields).then(encounter);
export const deleteEncounter = (id: string) => call<unknown>("DELETE", `/api/encounters/${seg(id)}`);
export const evaluateEncounter = (id: string, levels?: number[]) =>
  call<Evaluation>("POST", `/api/encounters/${seg(id)}/evaluate`, levels ? { levels } : {});

export interface AddFields { elementId: string; count?: number; dmLabel?: string; publicName?: string; hidden?: boolean }
export interface CreatureFields {
  dmLabel?: string;
  publicName?: string;
  hidden?: boolean;
  revealLevel?: RevealLevel;
  hpMode?: "encounter" | "fixed";
  hpMax?: number | null;
  sort?: number;
  overrides?: OverridesForm;
}

const creatures = (id: string) => `/api/encounters/${seg(id)}/creatures`;
export const addCreatures = (id: string, fields: AddFields) => call<Encounter>("POST", creatures(id), fields).then(encounter);
export const updateCreature = (id: string, cid: string, fields: CreatureFields) =>
  call<Encounter>("PATCH", `${creatures(id)}/${seg(cid)}`, fields).then(encounter);
export const removeCreature = (id: string, cid: string) => call<Encounter>("DELETE", `${creatures(id)}/${seg(cid)}`).then(encounter);
export const refreshCreature = (id: string, cid: string, choices?: Record<string, "keep" | "source">) =>
  call<RefreshOutcome>("POST", `${creatures(id)}/${seg(cid)}/refresh`, choices ? { choices } : {})
    .then((r) => ({ ...r, encounter: encounter(r.encounter) }));
export const swapCreature = (id: string, cid: string, elementId?: string) =>
  call<Encounter>("POST", `${creatures(id)}/${seg(cid)}/swap`, elementId ? { elementId } : {}).then(encounter);

export type EffectFields = Partial<Omit<Effect, "id" | "sort">>;
export const addEffect = (id: string, fields: EffectFields) => call<Encounter>("POST", `/api/encounters/${seg(id)}/effects`, fields).then(encounter);
export const updateEffect = (id: string, eid: string, fields: EffectFields) =>
  call<Encounter>("PATCH", `/api/encounters/${seg(id)}/effects/${seg(eid)}`, fields).then(encounter);
export const removeEffect = (id: string, eid: string) => call<Encounter>("DELETE", `/api/encounters/${seg(id)}/effects/${seg(eid)}`).then(encounter);

export type RuleFields = Partial<Omit<Rule, "id" | "sort">>;
export const addRule = (id: string, fields: RuleFields) => call<Encounter>("POST", `/api/encounters/${seg(id)}/rules`, fields).then(encounter);
export const updateRule = (id: string, rid: string, fields: RuleFields) =>
  call<Encounter>("PATCH", `/api/encounters/${seg(id)}/rules/${seg(rid)}`, fields).then(encounter);
export const removeRule = (id: string, rid: string) => call<Encounter>("DELETE", `/api/encounters/${seg(id)}/rules/${seg(rid)}`).then(encounter);

export const searchMonsters = (query: MonsterQuery) => call<MonsterPage>("POST", "/api/bestiary/search", query);
export const monsterFacets = () => call<Facets>("GET", "/api/bestiary/facets");

/** A party's encounters changed (`kind: "encounters"` on its party events), or events may have been missed. */
export function watchEncounters(partyId: string, listener: (encounterId: string | null) => void): () => void {
  return watchParties((e) => {
    if (e.type === "resync" || e.type === "mine") listener(null);
    else if (e.party === partyId && e.kind === "encounters") listener(e.id);
  });
}

// Words for the app.

export const BAND_WORD: Record<Band, string> = { trivial: "Trivial", low: "Low", moderate: "Moderate", high: "High", extreme: "Beyond High" };
export const BAND_TONE: Record<Band, "default" | "success" | "info" | "warning" | "danger"> = {
  trivial: "default", low: "success", moderate: "info", high: "warning", extreme: "danger",
};
export const STATUS_WORD: Record<Status, string> = { draft: "Draft", ready: "Ready", live: "Live", ended: "Ended" };
export const REVEAL_WORD: Record<RevealLevel, string> = { name: "Name", band: "Name + health", full: "Full stat block" };
export const ABILITY_WORD: Record<Ability, string> = { str: "STR", dex: "DEX", con: "CON", int: "INT", wis: "WIS", cha: "CHA" };
export const EDITION_WORD: Record<string, string> = { dnd2024: "2024", dnd2014: "2014" };

/** The challenge ratings in order, for the picker's chips. */
export const CR_STEPS = ["0", "1/8", "1/4", "1/2", ...Array.from({ length: 30 }, (_, i) => String(i + 1))];

/** "1,800 XP". */
export const xpText = (xp: number) => `${xp.toLocaleString("en-US")} XP`;

/** A signed modifier: +3, -1. */
export const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);

/** The ability modifier of a score. */
export const modifier = (score: number) => Math.floor((score - 10) / 2);

/** True for `NdM`, `NdM+K`, `NdM-K` (the server's check). */
export function validFormula(f: string): boolean {
  return /^([1-9]\d?)d([1-9]\d?|100)([+-]\d{1,4})?$/.test(f.replace(/\s/g, ""));
}

// Encounters change no upstream command: the module registers so the override list stays complete (registry.test.ts).
override("encounters", () => {});
