import { cryptoDie, parseNotation, rollExpr, type DieRoller } from "../../../src/lib/dice";
import { withEnabledSources } from "../contentRules";
import { serverFetch } from "../http";
import { loadSession, type ServerSession } from "../session";
import { watchParties, type Ruleset } from "./parties";
import { override } from "./registry";

/**
 * Encounters (release 0.3.0): the server's `/api/parties/{party}/encounters` and `/api/encounters/*` routes as
 * party/docs/encounters-api.md describes them, and the creature picker's `/api/bestiary/*` search (part A's
 * party/docs/bestiary-api.md).
 *
 * The DM gets everything; a player gets the server's projection of a player-visible encounter: only revealed
 * creatures, each with its whole stat block (the user's D20 b), never anything DM-only. There is no XP or difficulty
 * anywhere (the user's D16). Every call goes through {@link encountersDeps}, so tests answer them from a fake server
 * (../../test/encountersServer.ts). Times from the server are seconds since the epoch.
 */

export type Status = "draft" | "ready" | "live" | "ended";
export type HpMode = "average" | "rolled";
export type Kind = "monster" | "companion";
export type InitMode = "after_owner" | "own";
export type Ability = "str" | "dex" | "con" | "int" | "wis" | "cha";
export const ABILITIES: Ability[] = ["str", "dex", "con", "int", "wis", "cha"];

/** A monster's stat block (upstream's `MonsterData`, snake_case as the server sends it, without XP). */
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
  languages?: string[];
  cr?: string;
  traits?: Feature[];
  actions?: Feature[];
  bonus_actions?: Feature[];
  reactions?: Feature[];
  legendary?: { actions?: Feature[] } & Record<string, unknown>;
}

/** An Aurora companion's printed block (the bestiary's `CompanionDetail`): what is not printed is null. */
export interface CompanionBlock {
  ac: number | null;
  acText: string | null;
  hp: number | null;
  hpText: string | null;
  hpFormula: string | null;
  speed: string | null;
  abilities: Record<Ability, number | null>;
  saves: Partial<Record<Ability, number>>;
  skills: string | null;
  senses: string | null;
  languages: string | null;
  challenge: string | null;
  size: string | null;
  type: string | null;
  alignment: string | null;
  defenses: { name: string; text: string }[];
  traits: { name: string; text: string }[];
  actions: { name: string; text: string }[];
  reactions: { name: string; text: string }[];
}

export type StatBlock = MonsterBlock | CompanionBlock;

export interface Feature {
  name: string;
  text: string;
  cost?: number;
}

export interface HpValue { average: number; formula?: string }
export interface AcValue { value: number; source?: string }
export interface Changed<T> { value: T; base: T | null }

/** How often a creature casts a spell it was given (0.3.2, party-encounter `spellbook::Frequency`). */
export type SpellFrequency =
  | { kind: "atWill" }
  | { kind: "perDay"; uses: number; each: boolean }
  | { kind: "slots"; level: number; slots: number };

/** A real spell from the DM's content on a creature (0.3.2): the installed element's id and name, and how often. */
export interface OverrideSpell {
  id: string;
  name: string;
  frequency: SpellFrequency;
}

/** The override dialog's Spells section (party-encounter `SpellsOverride`). */
export interface SpellsOverride {
  spells?: OverrideSpell[];
  /** Slots by spell level ("1" → 4), replacing the printed ones for the levels given. */
  slots?: Record<string, number>;
  /** Spellcasting ability, short ("int"). */
  ability?: string;
  saveDc?: number;
  attackBonus?: number;
}

/** What an encounter changes on one creature's block (D41). `base` is the source value, recorded by the server. */
export interface Overrides {
  hp?: Changed<HpValue>;
  ac?: Changed<AcValue>;
  saves?: Partial<Record<Ability, Changed<number>>>;
  traits?: { name: string; text: string }[];
  notes?: string;
  /** 0.3.2: spells, slots per level, and the casting numbers. */
  spells?: SpellsOverride;
}

/** What the app sends to change overrides: absent = leave, null = drop. */
export interface OverridesForm {
  hp?: HpValue | null;
  ac?: AcValue | null;
  saves?: Partial<Record<Ability, number | null>>;
  traits?: { name: string; text: string }[];
  notes?: string | null;
  spells?: SpellsOverride | null;
}

/** A creature as the DM sees it. */
export interface Creature {
  id: string;
  sort: number;
  kind: Kind;
  dmLabel: string;
  publicName: string;
  hidden: boolean;
  revealedAt: number | null;
  /** The creature it is controlled by (a pet, a summon), or null. */
  ownerId: string | null;
  initMode: InitMode;
  elementId: string;
  packId: string;
  packRev: string;
  name: string;
  source: string;
  ruleset: string;
  mapsTo: string | null;
  /** As printed; only a stat (nothing rates encounters, D16). */
  cr: string | null;
  type: string;
  size: string[];
  hp: HpValue | null;
  hpMode: "encounter" | "fixed";
  hpMax: number | null;
  ac: number | null;
  printed: { hp: HpValue | null; ac: AcValue | null; saves: Record<Ability, number | null> };
  overrides: Overrides;
  modified: boolean;
  statBlock: StatBlock;
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

/** Creatures or characters of the other edition (D23 b). */
export interface Warning {
  kind: "edition" | "characters_edition" | string;
  text: string;
  creatureIds?: string[];
  creatures?: { creatureId: string; ruleset: string; mapsTo: string | null }[];
  count?: number;
}

/** An encounter as its DM sees it. */
export interface Encounter {
  id: string;
  partyId: string;
  name: string;
  publicName: string;
  /** The party's edition, which every encounter of it uses (D23 b). */
  ruleset: Ruleset;
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
  warnings: Warning[];
  /** 0.3.2: spells cast before the fight (DM only). */
  casts?: Precast[];
  /** The server's note after a precast write ("two concentration spells: only the last one lasts"). */
  castWarning?: string;
}

/** A spell cast before the fight (replaces "Starting effect"); its slot is spent when combat starts. */
export interface Precast {
  id: string;
  casterId: string;
  sort: number;
  /** The spell's element id. */
  spellKey: string;
  name: string;
  slotLevel: number | null;
  /** Creature ids of the encounter, or "self". */
  targets: string[];
  choices: { damage_type?: string };
}

export interface PrecastFields {
  casterId: string;
  spellKey: string;
  slotLevel: number | null;
  targets: string[];
  choices?: { damage_type?: string };
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
  updatedAt: number;
}

/**
 * What a player sees of a revealed creature (0.3.3, the user: "only the name and if bloodied"): its public name and
 * health band (null when its HP isn't known). No stat block (this narrows D20 b).
 */
export interface SeenCreature {
  id: string;
  publicName: string;
  band: "unhurt" | "bloodied" | "down" | null;
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

// The creature picker (part A's bestiary API).

export interface MonsterQuery {
  /** Monsters, Aurora companions (D17 b), or both when empty. */
  kinds?: Kind[];
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
  kind?: Kind;
  cr: string | null;
  type: string | null;
  size: string[];
  ac: number | null;
  hp: number | null;
  source: string;
  ruleset: string;
  legendary: boolean;
}

export interface MonsterPage { total: number; offset: number; limit: number; items: MonsterSummary[] }
export interface Facet { value: string; count: number; name?: string }
export interface Facets { total: number; types: Facet[]; sizes: Facet[]; crs: Facet[]; rulesets: Facet[]; sources: Facet[]; kinds?: Facet[] }

export interface EncountersDeps {
  fetch: (session: Pick<ServerSession, "baseUrl" | "token">, method: string, path: string, body?: unknown) => Promise<unknown>;
  /** The die for "Roll" buttons (D21: every roll can also be typed). */
  die: DieRoller;
}

const DEFAULT_DEPS: EncountersDeps = {
  // Late-bound: ../http.ts imports this module before its own exports are set.
  fetch: (session, method, path, body) => serverFetch(session, method, path, body),
  die: cryptoDie,
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

export interface AddFields { elementId: string; count?: number; dmLabel?: string; publicName?: string; hidden?: boolean; ownerId?: string; initMode?: InitMode }
export interface CreatureFields {
  dmLabel?: string;
  publicName?: string;
  hidden?: boolean;
  hpMode?: "encounter" | "fixed";
  hpMax?: number | null;
  sort?: number;
  ownerId?: string | null;
  initMode?: InitMode;
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

/** Precast spells (0.3.2): each write answers the encounter (with `casts` and, maybe, `castWarning`). */
export const addPrecast = (id: string, fields: PrecastFields) => call<Encounter>("POST", `/api/encounters/${seg(id)}/casts`, fields).then(encounter);
export const updatePrecast = (id: string, cid: string, fields: Partial<PrecastFields>) =>
  call<Encounter>("PATCH", `/api/encounters/${seg(id)}/casts/${seg(cid)}`, fields).then(encounter);
export const removePrecast = (id: string, cid: string) => call<Encounter>("DELETE", `/api/encounters/${seg(id)}/casts/${seg(cid)}`).then(encounter);

export type RuleFields = Partial<Omit<Rule, "id" | "sort">>;
export const addRule = (id: string, fields: RuleFields) => call<Encounter>("POST", `/api/encounters/${seg(id)}/rules`, fields).then(encounter);
export const updateRule = (id: string, rid: string, fields: RuleFields) =>
  call<Encounter>("PATCH", `/api/encounters/${seg(id)}/rules/${seg(rid)}`, fields).then(encounter);
export const removeRule = (id: string, rid: string) => call<Encounter>("DELETE", `/api/encounters/${seg(id)}/rules/${seg(rid)}`).then(encounter);

/** The creature picker: creatures of the account's enabled sources only (../contentRules.ts). */
export const searchMonsters = async (query: MonsterQuery) =>
  call<MonsterPage>("POST", "/api/bestiary/search", await withEnabledSources((m, p, b) => call(m, p, b), query));
export const monsterFacets = () => call<Facets>("GET", "/api/bestiary/facets");

/** A party's encounters changed (`kind: "encounters"` on its party events), or events may have been missed. */
export function watchEncounters(partyId: string, listener: (encounterId: string | null) => void): () => void {
  return watchParties((e) => {
    if (e.type === "resync" || e.type === "mine") listener(null);
    else if (e.party === partyId && e.kind === "encounters") listener(e.id);
    // The party's edition (or name) changed: every encounter of it follows the party's edition (D23 b).
    else if (e.party === partyId && e.kind === "party") listener(null);
  });
}

/** Roll hit dice in the app (D21: the DM may type a total rolled with real dice instead). Null for a bad formula. */
export function rollFormula(formula: string): number | null {
  try {
    return Math.max(1, rollExpr(parseNotation(formula), encountersDeps.die).total);
  } catch {
    return null;
  }
}

// Words for the app.

export const STATUS_WORD: Record<Status, string> = { draft: "Draft", ready: "Ready", live: "Live", ended: "Ended" };
export const ABILITY_WORD: Record<Ability, string> = { str: "STR", dex: "DEX", con: "CON", int: "INT", wis: "WIS", cha: "CHA" };
export const EDITION_WORD: Record<string, string> = { dnd2024: "2024", dnd2014: "2014" };
export const INIT_WORD: Record<InitMode, string> = { after_owner: "Acts right after its owner", own: "Its own initiative" };

/** A signed modifier: +3, -1. */
export const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);

/** The ability modifier of a score. */
export const modifier = (score: number) => Math.floor((score - 10) / 2);

/** True for `NdM`, `NdM+K`, `NdM-K` (the server's check). */
export function validFormula(f: string): boolean {
  return /^([1-9]\d?)d([1-9]\d?|100)([+-]\d{1,4})?$/.test(f.replace(/\s/g, ""));
}

/** True for a companion's block (the bestiary's `CompanionDetail`). */
export const isCompanionBlock = (b: StatBlock): b is CompanionBlock => !("creature_type" in b);

/** Encounter creatures in builder order: each one that isn't controlled, followed by the ones it controls. */
export function nested(list: Creature[]): { creature: Creature; pets: Creature[] }[] {
  const top = list.filter((c) => !c.ownerId || !list.some((o) => o.id === c.ownerId));
  return top.map((creature) => ({ creature, pets: list.filter((c) => c.ownerId === creature.id) }));
}

// Encounters change no upstream command: the module registers so the override list stays complete (registry.test.ts).
override("encounters", () => {});
