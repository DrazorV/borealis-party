/**
 * The mock server's 0.3.2 routes (./spellApi.ts): an SRD-only spell index with one copy per spell per edition, the
 * account's effect definitions, a tag draft, encounter precasts and a small SRD bestiary for Add creature. In memory.
 */
import type { MonsterSummary } from "../../api/bestiary";
import type { MechDef, MechDraft, Precast, SpellEntry } from "./spellApi";

const S = (key: string, name: string, level: number, concentration: boolean, coverage: SpellEntry["coverage"], ruleset = "2024", text = "", alsoIn?: string[]): SpellEntry =>
  ({ key, elementId: `ID_SRD_SPELL_${key.toUpperCase()}`, name, level, ruleset, concentration, coverage, source: ruleset === "2014" ? "SRD 5.1" : "SRD 5.2.1", text, ...(alsoIn ? { alsoIn } : {}) });

export const MOCK_SPELLS: SpellEntry[] = [
  S("fire_bolt", "Fire Bolt", 0, false, "auto", "2024", "Ranged spell attack. On a hit, the target takes 1d10 fire damage."),
  S("sacred_flame", "Sacred Flame", 0, false, "auto"),
  S("mage_armor", "Mage Armor", 1, false, "auto", "2024", "The target's base AC becomes 13 + its Dexterity modifier."),
  S("shield", "Shield", 1, false, "auto"),
  S("bless", "Bless", 1, true, "auto"),
  S("cure_wounds", "Cure Wounds", 1, false, "auto"),
  S("hold_person", "Hold Person", 2, true, "auto"),
  S("misty_step", "Misty Step", 2, false, "ruling"),
  S("fireball", "Fireball", 3, false, "auto", "2024", "Each creature in a 20-foot-radius sphere makes a Dexterity saving throw, taking 8d6 fire damage on a failed save or half as much on a successful one.", ["a 5etools copy (hidden)"]),
  S("fireball", "Fireball", 3, false, "auto", "2014", "A 20-foot-radius sphere: Dexterity saving throw, 8d6 fire damage on a failed save, half on a success."),
  S("counterspell", "Counterspell", 3, false, "ruling"),
  S("haste", "Haste", 3, true, "partial"),
  S("conjure_animals", "Conjure Animals", 3, true, "partial", "2014"),
  S("spirit_guardians", "Spirit Guardians", 3, true, "partial"),
  S("greater_invisibility", "Greater Invisibility", 4, true, "auto"),
  S("cone_of_cold", "Cone of Cold", 5, false, "suggested", "2024", "Each creature in a 60-foot cone makes a Constitution saving throw, taking 8d8 cold damage on a failed save or half as much on a successful one."),
  S("arcane_word", "Arcane Word", 1, false, "unlinked", "2024", "A made-up homebrew spell: one creature within 60 feet makes a Wisdom save or takes 2d6 psychic damage and is frightened until the end of its next turn."),
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

export interface MockSpellStore {
  mechanics: Map<string, MechDef>;
  precasts: Map<string, Precast[]>;
  made: number;
}

export const newSpellStore = (): MockSpellStore => ({ mechanics: new Map(), precasts: new Map(), made: 0 });

const fail = (code: string, message: string) => ({ code, message });

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
  if (parts[2] === "encounters" && parts[4] === "casts") {
    const enc = parts[3]!;
    const list = store.precasts.get(enc) ?? [];
    if (method === "GET") return { items: list };
    if (method === "POST") {
      const { requestId: _r, ...fields } = b;
      void _r;
      const made = { ...(fields as Omit<Precast, "id">), id: `pc-${++store.made}` };
      store.precasts.set(enc, [...list, made]);
      return made;
    }
    const i = list.findIndex((x) => x.id === parts[5]);
    if (i < 0) throw fail("not_found", "No such precast spell.");
    if (method === "PATCH") { list[i] = { ...list[i]!, ...(b as Partial<Precast>) }; return list[i]; }
    if (method === "DELETE") { list.splice(i, 1); return {}; }
  }
  if (parts[2] !== "spells") return undefined;
  if (parts[3] === "search" && method === "POST") {
    const text = String(b.text ?? "").toLowerCase();
    const items = MOCK_SPELLS.filter((s) => (!text || s.name.toLowerCase().includes(text)) && (b.ruleset == null || s.ruleset === b.ruleset) && (b.level == null || s.level === b.level))
      .map((s) => (store.mechanics.has(`${s.key}:${s.ruleset}`) && (s.coverage === "unlinked" || s.coverage === "ruling" || s.coverage === "suggested") ? { ...s, coverage: "partial" as const } : s));
    return { items: items.slice(0, Number(b.limit ?? 50)) };
  }
  if (parts[3] === "mechanics") {
    if (parts[4] === "export" && method === "GET") return { format: "borealis-party-spell-effects", version: 1, items: [...store.mechanics.values()] };
    if (parts[4] === "import" && method === "POST") {
      const file = b.file as { format?: string; items?: MechDef[] } | undefined;
      if (file?.format !== "borealis-party-spell-effects" || !Array.isArray(file.items)) throw fail("invalid", "That isn't an exported effects file.");
      for (const d of file.items) store.mechanics.set(`${d.key}:${d.ruleset}`, { ...d, origin: "import" });
      return { imported: file.items.length };
    }
    if (!parts[4] && method === "GET") return { items: [...store.mechanics.values()] };
    const key = `${parts[4]}:${q.get("ruleset") ?? "2024"}`;
    if (method === "PUT") {
      const def = { key: parts[4]!, ruleset: q.get("ruleset") ?? "2024", def: b.def as MechDef["def"], rev: (store.mechanics.get(key)?.rev ?? 0) + 1, origin: "builder" as const };
      store.mechanics.set(key, def);
      return def;
    }
    if (method === "DELETE") { store.mechanics.delete(key); return {}; }
  }
  if (parts[4] === "draft" && method === "GET") {
    const s = MOCK_SPELLS.find((x) => x.key === parts[3]);
    if (!s || s.coverage !== "suggested") return null;
    const draft: MechDraft = {
      preset: "damage_save", text: s.text, checks: ["delivery", "on_land", "targeting"],
      def: { name: s.name, level: s.level, concentration: s.concentration, cast: "action", targeting: { kind: "area", shape: "cone", size_ft: 60 },
        delivery: { kind: "save", ability: "con", success: "half" }, on_land: [{ kind: "damage", dice: "8d8+(slot_above)d8", type: "cold" }] },
    };
    return draft;
  }
  return undefined;
}
