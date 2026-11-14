import type { CompanionBlock, Creature, Effect, Encounter, MonsterBlock, MonsterSummary, Rule } from "../api/overrides/encounters";

/**
 * An in-memory encounters server for tests, answering party/docs/encounters-api.md and the bestiary search the way
 * the server does where the app depends on it: the DM gets everything, a player gets the projection of a shown
 * encounter only (hidden creatures absent, revealed ones with their stat block, no DM fields), everything else is
 * 404 `not_found`; times are seconds. No XP or difficulty anywhere (D16).
 */

export interface FakeMonster extends MonsterSummary { block: MonsterBlock | CompanionBlock; mapsTo?: string }

/** SRD monsters (5.2.1) and a made-up Aurora companion, for the picker. */
export const MONSTERS: FakeMonster[] = [
  monster("ID_SRD521_MONSTER_GOBLIN_WARRIOR", "Goblin Warrior", "1/4", "fey", "small", 15, 10, "3d6"),
  monster("ID_SRD521_MONSTER_OWLBEAR", "Owlbear", "3", "monstrosity", "large", 13, 59, "7d10+21"),
  monster("ID_SRD521_MONSTER_BUGBEAR_WARRIOR", "Bugbear Warrior", "1", "fey", "medium", 14, 33, "6d8+6"),
  {
    id: "ID_TEST_COMPANION_OWL", name: "Owl", kind: "companion", cr: "0", type: "beast", size: ["tiny"], ac: 11, hp: 1,
    source: "Test Companions", ruleset: "any", legendary: false,
    block: {
      ac: 11, acText: "11", hp: 1, hpText: "1 (1d4-1)", hpFormula: "1d4-1", speed: "5 ft., fly 60 ft.",
      abilities: { str: 3, dex: 13, con: 8, int: 2, wis: 12, cha: null }, saves: {}, skills: null, senses: null, languages: null,
      challenge: "0", size: "Tiny", type: "Beast", alignment: "unaligned", defenses: [], traits: [{ name: "Flyby", text: "No opportunity attacks." }],
      actions: [{ name: "Talons", text: "1 slashing damage." }], reactions: [],
    },
  },
];

function monster(id: string, name: string, cr: string, type: string, size: string, ac: number, hp: number, formula: string): FakeMonster {
  return {
    id, name, kind: "monster", cr, type, size: [size], ac, hp, source: "SRD 5.2.1 Monsters", ruleset: "dnd2024", legendary: false,
    block: {
      ruleset: "dnd2024", size: [size], creature_type: type, alignment: "neutral", ac: [{ value: ac }], hp: { average: hp, formula },
      speed: { walk: 30 }, abilities: { str: 14, dex: 12, con: 12, int: 8, wis: 10, cha: 8 }, cr,
      actions: [{ name: "Bite", text: "Melee Attack Roll: +5." }],
    },
  };
}

interface Row extends Omit<Encounter, "creatures" | "effects" | "rules" | "warnings" | "visibleToPlayers"> { dmId: string }

export interface FakeEncounters {
  me: string;
  dmId: string;
  partyId: string;
  members: string[];
  encounters: Row[];
  creatures: Map<string, Creature[]>;
  effects: Map<string, Effect[]>;
  rules: Map<string, Rule[]>;
  calls: { method: string; path: string; body?: unknown }[];
  /** No bestiary routes (part A not on this server). */
  noBestiary: boolean;
  fetch: (session: unknown, method: string, path: string, body?: unknown) => Promise<unknown>;
}

const notFound = () => Promise.reject({ code: "not_found", message: "There is no such encounter here." });
let next = 1;
const uid = (p: string) => `${p}-${next++}`;
const cap = (s: string) => s[0]!.toUpperCase() + s.slice(1);

export function fakeEncountersServer(partyId = "p1", dmId = "dm1"): FakeEncounters {
  const s: FakeEncounters = {
    me: dmId, dmId, partyId, members: ["pl1"], encounters: [], creatures: new Map(), effects: new Map(), rules: new Map(),
    calls: [], noBestiary: false, fetch: async () => null,
  };
  const visible = (e: Row) => e.playerVisible && (e.status === "ready" || e.status === "live");
  const warnings = (e: Row) => {
    const other = (s.creatures.get(e.id) ?? []).filter((c) => (c.ruleset === "dnd2014" || c.ruleset === "dnd2024") && c.ruleset !== e.ruleset);
    return other.length ? [{ kind: "edition", text: "Some stat blocks are from the other edition.", creatureIds: other.map((c) => c.id),
      creatures: other.map((c) => ({ creatureId: c.id, ruleset: c.ruleset, mapsTo: c.mapsTo })) }] : [];
  };
  const dmView = (e: Row): Encounter => {
    const { dmId: _dm, ...rest } = e;
    void _dm;
    return { ...rest, visibleToPlayers: visible(e), creatures: s.creatures.get(e.id) ?? [], effects: s.effects.get(e.id) ?? [],
      rules: s.rules.get(e.id) ?? [], warnings: warnings(e) };
  };
  const playerView = (e: Row) => {
    const list = s.creatures.get(e.id) ?? [];
    // Like the server (0.3.3): a revealed creature is its public name and band only.
    return {
      id: e.id, partyId: e.partyId, publicName: e.publicName, status: e.status,
      creatures: list.filter((c) => !c.hidden).map((c) => ({ id: c.id, publicName: c.publicName, band: "unhurt" as const })),
    };
  };
  const mine = (id: string) => s.encounters.find((e) => e.id === id && e.dmId === s.me);
  s.fetch = async (_session, method, path, body) => {
    s.calls.push({ method, path, body });
    const b = (body ?? {}) as Record<string, unknown>;
    let m: RegExpExecArray | null;
    if (path === "/api/bestiary/facets") {
      if (s.noBestiary) return Promise.reject({ code: "not_found", message: "The server does not offer that." });
      return { total: MONSTERS.length, types: [{ value: "fey", count: 2 }, { value: "monstrosity", count: 1 }, { value: "beast", count: 1 }],
        sizes: [], crs: [], rulesets: [], sources: [], kinds: [{ value: "monster", count: 3 }, { value: "companion", count: 1 }] };
    }
    if (path === "/api/bestiary/search") {
      if (s.noBestiary) return Promise.reject({ code: "not_found", message: "The server does not offer that." });
      const text = String(b.text ?? "").toLowerCase();
      const types = (b.types as string[] | undefined) ?? [];
      const kinds = (b.kinds as string[] | undefined) ?? [];
      const items = MONSTERS.filter((x) => (!text || x.name.toLowerCase().includes(text)) && (!types.length || types.includes(x.type ?? ""))
        && (!kinds.length || kinds.includes(x.kind ?? "monster")))
        .map((x) => { const summary: Partial<FakeMonster> = { ...x }; delete summary.block; delete summary.mapsTo; return summary; });
      return { total: items.length, offset: 0, limit: 30, items };
    }
    if ((m = /^\/api\/parties\/([^/]+)\/encounters$/.exec(path))) {
      if (decodeURIComponent(m[1]!) !== s.partyId) return notFound();
      const dm = s.me === s.dmId;
      if (!dm && !s.members.includes(s.me)) return notFound();
      if (method === "GET") {
        if (dm) {
          return { role: "dm", encounters: s.encounters.map((e) => {
            const list = s.creatures.get(e.id) ?? [];
            return { id: e.id, name: e.name, publicName: e.publicName, status: e.status, playerVisible: e.playerVisible, visibleToPlayers: visible(e),
              creatureCount: list.length, hiddenCount: list.filter((c) => c.hidden).length, updatedAt: e.updatedAt };
          }) };
        }
        return { role: "player", encounters: s.encounters.filter(visible).map((e) => ({ id: e.id, publicName: e.publicName, status: e.status })) };
      }
      if (!dm) return notFound();
      const row: Row = {
        id: uid("e"), partyId: s.partyId, dmId: s.dmId, name: String(b.name), publicName: String(b.publicName ?? b.name), ruleset: "dnd2024",
        status: "draft", playerVisible: false, hpMode: "average", dmNotes: "", createdAt: 1, updatedAt: 1,
      };
      s.encounters.unshift(row);
      return dmView(row);
    }
    if ((m = /^\/api\/encounters\/([^/]+)(?:\/(creatures|effects|rules)(?:\/([^/]+)(?:\/(refresh|swap))?)?)?$/.exec(path))) {
      const id = decodeURIComponent(m[1]!);
      const [, , part, sub, action] = m;
      const row = s.encounters.find((e) => e.id === id);
      if (!row) return notFound();
      if (!part && method === "GET") {
        if (row.dmId === s.me) return dmView(row);
        if (visible(row) && s.members.includes(s.me)) return playerView(row);
        return notFound();
      }
      const e = mine(id);
      if (!e) return notFound();
      if (!part && method === "PATCH") {
        Object.assign(e, ...["name", "publicName", "status", "playerVisible", "hpMode", "dmNotes"].filter((k) => b[k] !== undefined).map((k) => ({ [k]: b[k] })));
        if (e.status === "draft") e.playerVisible = false;
        return dmView(e);
      }
      if (!part && method === "DELETE") { s.encounters = s.encounters.filter((x) => x !== e); return { deleted: true }; }
      const list = s.creatures.get(id) ?? [];
      s.creatures.set(id, list);
      if (part === "creatures" && !sub) {
        const mon = MONSTERS.find((x) => x.id === b.elementId);
        if (!mon) return Promise.reject({ code: "invalid_argument", message: "That creature isn't in your content." });
        const owner = typeof b.ownerId === "string" ? b.ownerId : null;
        const at = owner ? list.findIndex((c) => c.id === owner) + 1 + list.filter((c) => c.ownerId === owner).length : list.length;
        const made: Creature[] = [];
        for (let i = 0; i < Number(b.count ?? 1); i++) {
          const hp = "hpFormula" in mon.block ? (mon.block.hp != null ? { average: mon.block.hp, formula: mon.block.hpFormula ?? undefined } : null) : mon.block.hp ?? null;
          made.push({
            id: uid("c"), sort: 0, kind: mon.kind ?? "monster", dmLabel: Number(b.count ?? 1) > 1 ? `${mon.name} ${i + 1}` : mon.name,
            publicName: cap(mon.type ?? "creature"), hidden: b.hidden !== false, revealedAt: null, ownerId: owner,
            initMode: (b.initMode as Creature["initMode"]) ?? "after_owner", elementId: mon.id, packId: "monsters", packRev: "r1", name: mon.name,
            source: mon.source, ruleset: mon.ruleset, mapsTo: mon.mapsTo ?? null, cr: mon.cr, type: cap(mon.type ?? "creature"), size: mon.size,
            hp, hpMode: "encounter", hpMax: null, ac: mon.ac,
            printed: { hp, ac: mon.ac != null ? { value: mon.ac } : null, saves: { str: 2, dex: 1, con: 1, int: -1, wis: 0, cha: mon.kind === "companion" ? null : -1 } },
            overrides: {}, modified: false, statBlock: mon.block,
          });
        }
        list.splice(at, 0, ...made);
        list.forEach((c, i) => { c.sort = i; });
        return dmView(e);
      }
      if (part === "creatures" && sub) {
        const c = list.find((x) => x.id === decodeURIComponent(sub));
        if (!c) return notFound();
        if (action === "refresh") {
          if (b.choices) return { changed: true, conflicts: [], applied: true, encounter: dmView(e) };
          return { changed: true, applied: false, encounter: dmView(e), conflicts: [{ field: "hp", was: { average: 50 }, source: { average: 59 }, value: { average: 80 } }] };
        }
        if (method === "DELETE") { s.creatures.set(id, list.filter((x) => x !== c && x.ownerId !== c.id)); return dmView(e); }
        for (const k of ["dmLabel", "publicName", "hidden", "hpMode", "hpMax", "ownerId", "initMode"] as const) {
          if (b[k] !== undefined) (c as unknown as Record<string, unknown>)[k] = b[k];
        }
        const o = b.overrides as Record<string, unknown> | undefined;
        if (o) {
          if (o.hp) { c.overrides.hp = { value: o.hp as { average: number }, base: c.printed.hp }; c.hp = o.hp as { average: number }; }
          if (o.hp === null) { delete c.overrides.hp; c.hp = c.printed.hp; }
          if (o.notes !== undefined) c.overrides.notes = (o.notes as string | null) ?? undefined;
          c.modified = Boolean(c.overrides.hp);
        }
        return dmView(e);
      }
      if (part === "effects") {
        const effects = s.effects.get(id) ?? [];
        s.effects.set(id, effects);
        if (!sub) {
          effects.push({ id: uid("fx"), sort: effects.length, creatureId: String(b.creatureId), name: String(b.name), detail: String(b.detail ?? ""), sourceCreatureId: null,
            durationTurns: (b.durationTurns as number) ?? null, charges: null, concentration: Boolean(b.concentration), slotLevel: (b.slotLevel as number) ?? null, modifiers: {} });
          return dmView(e);
        }
        if (method === "DELETE") { s.effects.set(id, effects.filter((x) => x.id !== sub)); return dmView(e); }
        return dmView(e);
      }
      if (part === "rules") {
        const rules = s.rules.get(id) ?? [];
        s.rules.set(id, rules);
        if (!sub) {
          rules.push({ id: uid("r"), sort: rules.length, name: String(b.name), text: String(b.text ?? ""), creatureId: (b.creatureId as string) ?? null,
            endsWhen: (b.endsWhen as Rule["endsWhen"]) ?? "creature_down", active: true, flags: (b.flags as Rule["flags"]) ?? [] });
          return dmView(e);
        }
        const r = rules.find((x) => x.id === sub);
        if (!r) return notFound();
        if (method === "DELETE") { s.rules.set(id, rules.filter((x) => x !== r)); return dmView(e); }
        Object.assign(r, b);
        return dmView(e);
      }
    }
    return Promise.reject({ code: "not_found", message: "The server does not offer that." });
  };
  return s;
}
