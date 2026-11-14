/**
 * The mock server's 0.3.2 routes, shaped like feat/spell-server's (./spellApi.ts): an SRD-only spell list with one
 * copy per spell per edition, a spell's details with a tag draft, the account's effect definitions
 * (`/api/spell-mechanics`) and a small SRD bestiary for Add creature. Precasts are the encounter's (not mocked here).
 */
import type { MonsterSummary } from "../../api/bestiary";
import type { MechRow, SpellDetail, SpellEntry } from "./spellApi";
import type { SpellMech } from "./types";

interface MockSpell extends SpellEntry { text: string; draft?: SpellMech; checks?: string[] }

const S = (key: string, name: string, level: number, concentration: boolean, coverage: string, ruleset = "dnd2024", text = "", alsoIn: string[] = []): MockSpell => ({
  key: `ID_SRD_SPELL_${key.toUpperCase()}${ruleset === "dnd2014" ? "_2014" : ""}`, name, level, ruleset, concentration, coverage,
  source: ruleset === "dnd2014" ? "SRD 5.1" : "SRD 5.2.1", alsoIn: alsoIn.map((source, i) => ({ key: `COPY_${key}_${i}`, source, packId: "copy" })), text,
});

export const MOCK_SPELLS: MockSpell[] = [
  S("fire_bolt", "Fire Bolt", 0, false, "auto", "dnd2024", "Ranged spell attack. On a hit, the target takes 1d10 fire damage."),
  S("sacred_flame", "Sacred Flame", 0, false, "auto"),
  S("mage_armor", "Mage Armor", 1, false, "auto", "dnd2024", "The target's base AC becomes 13 + its Dexterity modifier."),
  S("shield", "Shield", 1, false, "auto"),
  S("bless", "Bless", 1, true, "auto"),
  S("cure_wounds", "Cure Wounds", 1, false, "auto"),
  S("hold_person", "Hold Person", 2, true, "auto"),
  S("misty_step", "Misty Step", 2, false, "ruling"),
  S("fireball", "Fireball", 3, false, "auto", "dnd2024", "Each creature in a 20-foot-radius sphere makes a Dexterity saving throw, taking 8d6 fire damage on a failed save or half as much on a successful one.", ["a 5etools copy"]),
  S("fireball", "Fireball", 3, false, "auto", "dnd2014", "A 20-foot-radius sphere: Dexterity saving throw, 8d6 fire damage on a failed save, half on a success."),
  S("counterspell", "Counterspell", 3, false, "ruling"),
  S("haste", "Haste", 3, true, "partial"),
  S("fly", "Fly", 3, true, "auto"),
  S("conjure_animals", "Conjure Animals", 3, true, "partial", "dnd2014"),
  S("spirit_guardians", "Spirit Guardians", 3, true, "partial"),
  S("greater_invisibility", "Greater Invisibility", 4, true, "auto"),
  { ...S("cone_of_cold", "Cone of Cold", 5, false, "ruling", "dnd2024", "Each creature in a 60-foot cone makes a Constitution saving throw, taking 8d8 cold damage on a failed save or half as much on a successful one."),
    draft: { v: 1, key: "ID_SRD_SPELL_CONE_OF_COLD", edition: "2024", name: "Cone of Cold", level: 5, cast: "action", tier: "partial", targeting: { kind: "area", shape: "cone", size_ft: 0 },
      delivery: { kind: "save", ability: "con", success: "none" }, on_land: [{ kind: "damage", dice: "8d8+(slot_above)d8", damage_type: "cold" }] },
    checks: ["area_size", "half_on_save"] },
];

const M = (id: string, name: string, cr: string, crValue: number, hp: number, ac: number): MonsterSummary => ({
  id, name, kind: "monster", cr, crValue, type: "beast", tags: [], size: ["L"], ac, hp, hpFormula: null, source: "SRD 5.1", packId: "srd", packRev: "1",
  ruleset: "dnd2014", legendary: false, mythic: false, lair: false, environments: [], page: null,
});

export const MOCK_BEASTS: MonsterSummary[] = [
  M("ID_SRD_MONSTER_GIANT_ELK", "Giant Elk", "2", 2, 42, 14),
  M("ID_SRD_MONSTER_GIANT_BOAR", "Giant Boar", "2", 2, 42, 12),
  M("ID_SRD_MONSTER_BROWN_BEAR", "Brown Bear", "1", 1, 34, 11),
  M("ID_SRD_MONSTER_DIRE_WOLF", "Dire Wolf", "1", 1, 37, 14),
  M("ID_SRD_MONSTER_BLACK_BEAR", "Black Bear", "1/2", 0.5, 19, 11),
  M("ID_SRD_MONSTER_WOLF", "Wolf", "1/4", 0.25, 11, 13),
  M("ID_SRD_MONSTER_PANTHER", "Panther", "1/4", 0.25, 13, 12),
];

const crNumber = (s: string | undefined): number | null => {
  if (!s) return null;
  if (s.includes("/")) { const [a, b] = s.split("/").map(Number); return a! / b!; }
  return Number(s);
};

export interface MockSpellStore { rows: MechRow[] }

export const newSpellStore = (): MockSpellStore => ({ rows: [] });

const fail = (code: string, message: string) => ({ code, message });
const entry = ({ text: _t, draft: _d, checks: _c, ...e }: MockSpell): SpellEntry => { void _t; void _d; void _c; return e; };

/** Answers a 0.3.2 route, or returns `undefined` when the path isn't one. */
export function spellRoute(store: MockSpellStore, method: string, path: string, body: unknown): unknown {
  const [bare, query = ""] = path.split("?");
  const parts = bare!.split("/").map(decodeURIComponent);
  const q = new URLSearchParams(query);
  const b = (body ?? {}) as Record<string, unknown>;
  if (parts[2] === "bestiary" && parts[3] === "search" && method === "POST") {
    const text = String(b.text ?? "").toLowerCase();
    const max = crNumber(b.crMax as string | undefined);
    const types = (b.types as string[] | undefined) ?? [];
    const items = MOCK_BEASTS.filter((m) => (!text || m.name.toLowerCase().includes(text)) && (max == null || (m.crValue ?? 0) <= max) && (!types.length || types.includes(m.type ?? "")));
    return { total: items.length, offset: 0, limit: 50, items };
  }
  if (parts[2] === "spell-mechanics") {
    if (parts[3] === "export" && method === "GET") return { format: "borealis-party-spell-rules", version: 1, rows: store.rows };
    if (parts[3] === "import" && method === "POST") {
      const file = b as { format?: string; rows?: MechRow[] };
      if (!Array.isArray(file.rows)) throw fail("invalid", "That isn't a spell rules file.");
      for (const r of file.rows) store.rows = [...store.rows.filter((x) => !(x.spellKey === r.spellKey && x.ruleset === r.ruleset && x.kind === r.kind)), { ...r, origin: "import" }];
      return { rows: file.rows.length };
    }
    if (!parts[3] && method === "GET") return { rows: store.rows };
    if (!parts[3] && method === "PUT") {
      const r = b as unknown as MechRow;
      const old = store.rows.find((x) => x.spellKey === r.spellKey && x.ruleset === r.ruleset && x.kind === r.kind);
      const row: MechRow = { ...r, rev: (old?.rev ?? 0) + 1, schemaV: 1 };
      store.rows = [...store.rows.filter((x) => x !== old), row];
      return { row };
    }
    if (!parts[3] && method === "DELETE") {
      const r = b as unknown as MechRow;
      store.rows = store.rows.filter((x) => !(x.spellKey === r.spellKey && x.ruleset === r.ruleset && x.kind === r.kind));
      return {};
    }
  }
  if (parts[2] !== "spells") return undefined;
  if (!parts[3] && method === "GET") {
    const text = (q.get("text") ?? "").toLowerCase();
    const ruleset = q.get("ruleset");
    const items = MOCK_SPELLS.filter((s) => (!text || s.name.toLowerCase().includes(text)) && (!ruleset || s.ruleset === ruleset))
      .map((s) => (store.rows.some((r) => r.spellKey === s.key && r.kind === "def") ? { ...entry(s), coverage: "defined" } : entry(s)));
    return { total: items.length, offset: 0, items: items.slice(0, Number(q.get("limit") ?? 50)) };
  }
  if (parts[3] && !parts[4] && method === "GET") {
    const s = MOCK_SPELLS.find((x) => x.key === parts[3]);
    if (!s) throw fail("not_found", "No such spell.");
    const defined = store.rows.some((r) => r.spellKey === s.key && r.kind === "def");
    const detail: SpellDetail = {
      spell: entry(s), element: { id: s.key, name: s.name, source: s.source, description: s.text }, rows: store.rows.filter((r) => r.spellKey === s.key),
      mechanics: null, suggested: s.draft && !defined ? { mech: s.draft, checks: s.checks ?? [] } : null, sourceHash: "mockhash",
    };
    return detail;
  }
  return undefined;
}
