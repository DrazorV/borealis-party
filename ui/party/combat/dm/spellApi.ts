/**
 * The server routes the 0.3.2 spell UI calls (feat/spell-server `spells.rs`, `combat.rs`): the deduplicated spell
 * list (one copy per spell per edition), a spell's details with its tag draft ("Suggested"), the account's effect
 * definitions (`/api/spell-mechanics`, export and import), and Add creature from the bestiary through `summon`.
 * Precast spells live with the encounter (../../api/overrides/encounters.ts). Every call goes through
 * `combatDeps.fetch`, so the mock server answers them in tests and in the rig.
 */
import { combatAction, combatDeps } from "../../api/overrides/combat";
import type { MonsterPage, MonsterQuery } from "../../api/bestiary";
import { withEnabledSources, type Fetch } from "../../api/contentRules";
import { loadSession } from "../../api/session";
import type { DmCombat, SpellMech } from "./types";

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject({ code: "unauthorized", message: "Not signed in." });
  return combatDeps.fetch(session, method, path, body) as Promise<T>;
}
const seg = encodeURIComponent;

/** An installed copy of a spell, hidden from new picks when another copy is the chosen one. */
export interface SpellCopy { key: string; source: string; packId: string; packKind?: string }

/** One spell of the account's content: ONE copy per spell per edition (the user's rule), the others "also in". */
export interface SpellEntry {
  /** The chosen copy's element id. */
  key: string;
  name: string;
  /** `dnd2014`, `dnd2024` or `any`. */
  ruleset: string;
  level: number | null;
  school?: string | null;
  concentration: boolean;
  ritual?: boolean;
  source: string;
  packId?: string;
  chosenBy?: string;
  alsoIn: SpellCopy[];
  /** The other copies' books, once each, the enabled ones only (the "also in" note). */
  alsoInSources?: string[];
  /** `defined`, `linked`, or the curated tier (`auto`, `partial`, `utility`, `ruling`, `uncurated`). */
  coverage: string;
}

const fetchVia: Fetch = (method, path, body) => call(method, path, body);

/**
 * The account's spells for a pick. `forRuleset` (a creature's ruleset): only what a new pick of a character of that
 * ruleset may take. Always with the account's content rules: its enabled sources (contentRules.ts) and one copy per spell.
 */
export const searchSpells = async (q: { text?: string; ruleset?: string; forRuleset?: string; level?: number; limit?: number }) => {
  const body = await withEnabledSources(fetchVia, { ...q, limit: q.limit ?? 50 });
  return call<{ total: number; items: SpellEntry[] }>("POST", "/api/spells/search", body).then((r) => r?.items ?? []);
};

/** A spell's other copies' books, once each and only enabled ones (older servers: from `alsoIn`). */
export const alsoInBooks = (s: Pick<SpellEntry, "alsoIn" | "alsoInSources" | "source">): string[] =>
  s.alsoInSources ?? [...new Set(s.alsoIn.map((c) => c.source))].filter((n) => n !== s.source);

/** What the builder needs to confirm in a tag draft (the server's `Check`s). */
export type DraftCheck = string;

/** A spell's details: the chosen copy, its element (the user's own text), its mechanics and a tag draft. */
export interface SpellDetail {
  spell: SpellEntry;
  element: { id: string; name: string; source: string; description: string };
  rows: MechRow[];
  mechanics: unknown;
  /** A draft from the user's own spell data's tags, until the DM confirms one. */
  suggested: { mech: SpellMech; checks: DraftCheck[] } | null;
  sourceHash?: string;
}

export const spellDetail = (key: string) => call<SpellDetail>("GET", `/api/spells/${seg(key)}`);

/** One row of the account's spell mechanics: a definition (`def`), a link to another spell, or a chosen copy. */
export interface MechRow {
  kind: "def" | "link" | "alias";
  spellKey: string;
  ruleset: string;
  aliasName?: string;
  def?: SpellMech | { target: string } | null;
  schemaV?: number;
  rev?: number;
  origin?: "dm" | "suggested" | "import";
  sourceHash?: string;
  updatedAt?: number;
}

export const listMechanics = () => call<{ rows: MechRow[] }>("GET", "/api/spell-mechanics").then((r) => r?.rows ?? []);
export const saveMechanic = (key: string, ruleset: string, def: SpellMech, origin: "dm" | "suggested" = "dm", sourceHash?: string) =>
  call<{ row: MechRow }>("PUT", "/api/spell-mechanics", { kind: "def", spellKey: key, ruleset, def, origin, ...(sourceHash ? { sourceHash } : {}) }).then((r) => r?.row);
export const deleteMechanic = (key: string, ruleset: string) =>
  call<unknown>("DELETE", "/api/spell-mechanics", { kind: "def", spellKey: key, ruleset });
export const exportMechanics = () => call<unknown>("GET", "/api/spell-mechanics/export");
export const importMechanics = (file: unknown, mode: "merge" | "replace" = "merge") =>
  call<{ imported?: number; rows?: number }>("POST", `/api/spell-mechanics/import?mode=${mode}`, file);

/** The DM's bestiary, for Add creature (a summon's beasts, filtered by type and CR). */
export const searchCreatures = async (q: MonsterQuery) => call<MonsterPage>("POST", "/api/bestiary/search", await withEnabledSources(fetchVia, q));

/**
 * Add creature from the bestiary (0.3.2 summons): the server's `summon {owner, elementId}`, once per creature, then
 * `bind_to_concentration` when it should go with the owner's concentration, and its own initiative when asked.
 * Answers the last view and the new creatures' ids.
 */
export async function addCreatureFromBestiary(combat: string, before: string[], args: {
  elementId: string; count: number; owner: string; bindToConcentration: boolean; hidden: boolean; initiativeMode: "after_owner" | "own";
}): Promise<{ view: DmCombat; added: string[] }> {
  let view = undefined as DmCombat | undefined;
  const known = new Set(before);
  const added: string[] = [];
  for (let i = 0; i < args.count; i++) {
    const next: DmCombat = await combatAction(combat, "summon", { owner: args.owner, elementId: args.elementId, visible: !args.hidden });
    view = next;
    for (const c of next.state.combatants) if (!known.has(c.id)) { known.add(c.id); added.push(c.id); }
  }
  for (const id of added) {
    if (args.bindToConcentration) view = await combatAction(combat, "bind_to_concentration", { id });
    if (args.initiativeMode === "own") view = await combatAction(combat, "set_initiative_mode", { id, mode: "own" });
  }
  if (!view) throw { code: "invalid", message: "Nothing was added." };
  return { view, added };
}
