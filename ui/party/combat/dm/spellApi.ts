/**
 * The server routes the 0.3.2 spell UI calls (PROVISIONAL until S3's routes land; shared note
 * feat-spell-ui-contract-asks.md): the deduplicated spell index, the account's effect definitions, drafts from the
 * user's own spell tags, encounter precasts and Add creature from the bestiary. Every call goes through
 * `combatDeps.fetch`, so the mock server answers them in tests and in the rig.
 */
import { combatAction, combatDeps, newRequestId } from "../../api/overrides/combat";
import type { MonsterPage, MonsterQuery } from "../../api/bestiary";
import { loadSession } from "../../api/session";
import type { Coverage, SpellMech } from "./spellTypes";

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject({ code: "unauthorized", message: "Not signed in." });
  return combatDeps.fetch(session, method, path, body) as Promise<T>;
}
const seg = encodeURIComponent;

/** One spell of the account's content: ONE copy per spell per edition (the user's rule), the others "also in". */
export interface SpellEntry {
  key: string;
  elementId: string;
  name: string;
  level: number;
  ruleset: string;
  school?: string;
  concentration: boolean;
  coverage: Coverage;
  source?: string;
  alsoIn?: string[];
  /** The element's own rules text (the user's content), for the builder's number chips. */
  text?: string;
}

export const searchSpells = (q: { text?: string; ruleset?: string; level?: number; limit?: number }) =>
  call<{ items: SpellEntry[] }>("POST", "/api/spells/search", q).then((r) => r?.items ?? []);

/** An effect the DM built, saved to their account. */
export interface MechDef { key: string; ruleset: string; def: SpellMech; rev?: number; origin?: "builder" | "suggested" | "import" }

export const listMechanics = () => call<{ items: MechDef[] }>("GET", "/api/spells/mechanics").then((r) => r?.items ?? []);
export const saveMechanic = (key: string, ruleset: string, def: SpellMech) =>
  call<MechDef>("PUT", `/api/spells/mechanics/${seg(key)}?ruleset=${seg(ruleset)}`, { def });
export const deleteMechanic = (key: string, ruleset: string) =>
  call<unknown>("DELETE", `/api/spells/mechanics/${seg(key)}?ruleset=${seg(ruleset)}`);
export const exportMechanics = () => call<unknown>("GET", "/api/spells/mechanics/export");
export const importMechanics = (file: unknown) => call<{ imported: number }>("POST", "/api/spells/mechanics/import", { file });

/** A draft from the user's own spell data's tags ("Suggested"): fields to check, never applied unreviewed. */
export interface MechDraft {
  preset: Preset;
  def: Partial<SpellMech>;
  /** Fields that came from tags and need a look. */
  checks: string[];
  text?: string;
}
export type Preset = "buff" | "damage_save" | "spell_attack" | "heal" | "condition" | "summon" | "ruling" | "note";

export const draftMechanic = (key: string, ruleset: string) =>
  call<MechDraft | null>("GET", `/api/spells/${seg(key)}/draft?ruleset=${seg(ruleset)}`).catch(() => null);

/** A spell cast before the fight (replaces "Starting effect"; it spends its slot at the start). */
export interface Precast {
  id: string;
  casterCreatureId: string;
  spellKey: string;
  spellName: string;
  level: number;
  concentration: boolean;
  slotLevel: number | null;
  targets: string[];
}
export type PrecastFields = Omit<Precast, "id">;

export const listPrecasts = (encounter: string) =>
  call<{ items: Precast[] }>("GET", `/api/encounters/${seg(encounter)}/casts`).then((r) => r?.items ?? []);
export const addPrecast = (encounter: string, f: PrecastFields) =>
  call<Precast>("POST", `/api/encounters/${seg(encounter)}/casts`, { requestId: newRequestId(), ...f });
export const updatePrecast = (encounter: string, id: string, f: Partial<PrecastFields>) =>
  call<Precast>("PATCH", `/api/encounters/${seg(encounter)}/casts/${seg(id)}`, f);
export const deletePrecast = (encounter: string, id: string) =>
  call<unknown>("DELETE", `/api/encounters/${seg(encounter)}/casts/${seg(id)}`);

/** The DM's bestiary, for Add creature (a summon's beasts, filtered by type and CR). */
export const searchCreatures = (q: MonsterQuery) => call<MonsterPage>("POST", "/api/bestiary/search", q);

/** Add creature from the bestiary: controlled by a caster, optionally removed when its concentration ends. */
export const addCreatureFromBestiary = (combat: string, args: {
  elementId: string; count: number; owner?: string; bindToConcentration?: boolean; hidden: boolean; rollInitiative: boolean; initiativeMode?: "after_owner" | "own";
}) => combatAction(combat, "add_creature", args);
