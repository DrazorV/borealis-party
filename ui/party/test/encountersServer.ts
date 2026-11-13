import type { Creature, Effect, Encounter, MonsterBlock, MonsterSummary, Rule } from "../api/overrides/encounters";

/**
 * An in-memory encounters server for tests, answering party/docs/encounters-api.md and the bestiary search the way
 * the server does where the app depends on it: the DM gets everything, a player gets the projection of a shown
 * encounter only (hidden creatures absent, no DM fields), everything else is 404 `not_found`; times are seconds.
 * Difficulty is the SRD 5.2.1 budget for the levels in `levels`.
 */

const BUDGET: Record<number, [number, number, number]> = {
  1: [50, 75, 100], 2: [100, 150, 200], 3: [150, 225, 400], 4: [250, 375, 500], 5: [500, 750, 1100],
};

export interface FakeMonster extends MonsterSummary { block: MonsterBlock; mapsTo?: string }

/** SRD monsters (5.2.1) for the picker. */
export const MONSTERS: FakeMonster[] = [
  monster("ID_SRD521_MONSTER_GOBLIN_WARRIOR", "Goblin Warrior", "1/4", 50, "fey", "small", 15, 10, "3d6"),
  monster("ID_SRD521_MONSTER_OWLBEAR", "Owlbear", "3", 700, "monstrosity", "large", 13, 59, "7d10+21"),
  monster("ID_SRD521_MONSTER_BUGBEAR_WARRIOR", "Bugbear Warrior", "1", 200, "fey", "medium", 14, 33, "6d8+6"),
];

function monster(id: string, name: string, cr: string, xp: number, type: string, size: string, ac: number, hp: number, formula: string): FakeMonster {
  return {
    id, name, cr, xp, type, size: [size], ac, hp, source: "SRD 5.2.1 Monsters", ruleset: "dnd2024", legendary: false,
    block: {
      ruleset: "dnd2024", size: [size], creature_type: type, alignment: "neutral", ac: [{ value: ac }], hp: { average: hp, formula },
      speed: { walk: 30 }, abilities: { str: 14, dex: 12, con: 12, int: 8, wis: 10, cha: 8 }, cr, xp,
      actions: [{ name: "Bite", text: "Melee Attack Roll: +5." }],
    },
  };
}

interface Row extends Omit<Encounter, "creatures" | "effects" | "rules" | "evaluation" | "ruleset" | "visibleToPlayers"> { dmId: string }

export interface FakeEncounters {
  me: string;
  dmId: string;
  partyId: string;
  members: string[];
  levels: number[];
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
    me: dmId, dmId, partyId, members: ["pl1"], levels: [1, 1, 1, 1], encounters: [], creatures: new Map(), effects: new Map(), rules: new Map(),
    calls: [], noBestiary: false, fetch: async () => null,
  };
  const visible = (e: Row) => e.playerVisible && (e.status === "ready" || e.status === "live");
  const evaluation = (e: Row) => {
    const list = s.creatures.get(e.id) ?? [];
    const total = list.reduce((n, c) => n + (c.xp ?? 0), 0);
    const t = s.levels.reduce<[number, number, number]>((acc, l) => { const b = BUDGET[l] ?? [0, 0, 0]; return [acc[0] + b[0], acc[1] + b[1], acc[2] + b[2]]; }, [0, 0, 0]);
    const band = total === 0 ? "trivial" : total <= t[0] ? "low" : total <= t[1] ? "moderate" : total <= t[2] ? "high" : "extreme";
    const warnings = list.some((c) => c.modified) ? [{ kind: "modified", text: "Changed stat blocks are still rated by their CR." }] : [];
    return {
      method: "srd521", ruleset: e.rulesetSet ?? e.partyRuleset, party: { levels: s.levels, from: "members" }, totalXp: total, band,
      thresholds: { low: t[0], moderate: t[1], high: t[2] }, remaining: { low: t[0] - total, moderate: t[1] - total, high: t[2] - total },
      creatures: list.length, unrated: 0, warnings,
      ...((e.rulesetSet ?? e.partyRuleset) === "dnd2014" ? { note: "2014 encounters are rated with the SRD 5.2.1 XP budget." } : {}),
    };
  };
  const dmView = (e: Row): Encounter => {
    const { dmId: _dm, ...rest } = e;
    void _dm;
    return {
      ...rest, ruleset: e.rulesetSet ?? e.partyRuleset, visibleToPlayers: visible(e), creatures: s.creatures.get(e.id) ?? [],
      effects: s.effects.get(e.id) ?? [], rules: s.rules.get(e.id) ?? [], evaluation: evaluation(e),
    } as Encounter;
  };
  const playerView = (e: Row) => ({
    id: e.id, partyId: e.partyId, publicName: e.publicName, status: e.status,
    creatures: (s.creatures.get(e.id) ?? []).filter((c) => !c.hidden).map((c) => ({
      id: c.id, publicName: c.publicName, revealLevel: c.revealLevel, size: c.size, type: c.type, conditions: [],
      ...(c.revealLevel !== "name" ? { band: "unhurt" } : {}),
      ...(c.revealLevel === "full" ? { hp: { current: c.hp?.average ?? 1, max: c.hp?.average ?? 1 }, statBlock: { ...c.statBlock, xp: undefined } } : {}),
    })),
  });
  const mine = (id: string) => s.encounters.find((e) => e.id === id && e.dmId === s.me);
  s.fetch = async (_session, method, path, body) => {
    s.calls.push({ method, path, body });
    const b = (body ?? {}) as Record<string, unknown>;
    let m: RegExpExecArray | null;
    if (path === "/api/bestiary/facets") {
      if (s.noBestiary) return Promise.reject({ code: "not_found", message: "The server does not offer that." });
      return { total: MONSTERS.length, types: [{ value: "fey", count: 2 }, { value: "monstrosity", count: 1 }], sizes: [], crs: [], rulesets: [], sources: [] };
    }
    if (path === "/api/bestiary/search") {
      if (s.noBestiary) return Promise.reject({ code: "not_found", message: "The server does not offer that." });
      const text = String(b.text ?? "").toLowerCase();
      const types = (b.types as string[] | undefined) ?? [];
      const items = MONSTERS.filter((x) => (!text || x.name.toLowerCase().includes(text)) && (!types.length || types.includes(x.type)))
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
            const ev = evaluation(e);
            return { id: e.id, name: e.name, publicName: e.publicName, status: e.status, playerVisible: e.playerVisible, visibleToPlayers: visible(e),
              creatureCount: list.length, hiddenCount: list.filter((c) => c.hidden).length, xp: ev.totalXp, band: ev.band, updatedAt: e.updatedAt };
          }) };
        }
        return { role: "player", encounters: s.encounters.filter(visible).map((e) => ({ id: e.id, publicName: e.publicName, status: e.status })) };
      }
      if (!dm) return notFound();
      const row: Row = {
        id: uid("e"), partyId: s.partyId, dmId: s.dmId, name: String(b.name), publicName: String(b.publicName ?? b.name), rulesetSet: null,
        partyRuleset: "dnd2024", status: "draft", playerVisible: false, hpMode: "average", dmNotes: "", createdAt: 1, updatedAt: 1,
      };
      s.encounters.unshift(row);
      return dmView(row);
    }
    if ((m = /^\/api\/encounters\/([^/]+)(?:\/(creatures|effects|rules|evaluate)(?:\/([^/]+)(?:\/(refresh|swap))?)?)?$/.exec(path))) {
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
        Object.assign(e, b.ruleset !== undefined ? { rulesetSet: b.ruleset } : {}, ...["name", "publicName", "status", "playerVisible", "hpMode", "dmNotes"]
          .filter((k) => b[k] !== undefined).map((k) => ({ [k]: b[k] })));
        if (e.status === "draft") e.playerVisible = false;
        return dmView(e);
      }
      if (!part && method === "DELETE") { s.encounters = s.encounters.filter((x) => x !== e); return { deleted: true }; }
      if (part === "evaluate") return evaluation(e);
      const list = s.creatures.get(id) ?? [];
      s.creatures.set(id, list);
      if (part === "creatures" && !sub) {
        const mon = MONSTERS.find((x) => x.id === b.elementId);
        if (!mon) return Promise.reject({ code: "invalid_argument", message: "That monster isn't in your content." });
        for (let i = 0; i < Number(b.count ?? 1); i++) {
          list.push({
            id: uid("c"), sort: list.length, dmLabel: Number(b.count ?? 1) > 1 ? `${mon.name} ${i + 1}` : mon.name, publicName: cap(mon.type),
            hidden: b.hidden !== false, revealLevel: "name", revealedAt: null, elementId: mon.id, packId: "monsters", packRev: "r1", name: mon.name,
            source: mon.source, ruleset: mon.ruleset, mapsTo: mon.mapsTo ?? null, cr: mon.cr, xp: mon.xp, type: cap(mon.type), size: mon.size,
            hp: mon.block.hp ?? null, hpMode: "encounter", hpMax: null, ac: mon.ac, printed: { hp: mon.block.hp ?? null, ac: mon.ac,
              saves: { str: 2, dex: 1, con: 1, int: -1, wis: 0, cha: -1 } }, overrides: {}, modified: false, statBlock: mon.block,
          });
        }
        return dmView(e);
      }
      if (part === "creatures" && sub) {
        const c = list.find((x) => x.id === decodeURIComponent(sub));
        if (!c) return notFound();
        if (action === "refresh") {
          if (b.choices) return { changed: true, conflicts: [], applied: true, encounter: dmView(e) };
          return { changed: true, applied: false, encounter: dmView(e), conflicts: [{ field: "hp", was: { average: 50 }, source: { average: 59 }, value: { average: 80 } }] };
        }
        if (method === "DELETE") { s.creatures.set(id, list.filter((x) => x !== c)); return dmView(e); }
        for (const k of ["dmLabel", "publicName", "hidden", "revealLevel", "hpMode", "hpMax"] as const) {
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
