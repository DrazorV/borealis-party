import type { ServerEvent } from "../api/http";
import type { CombatPrompt, D20Input, PlayerCombat, PlayerCombatant } from "../combat/player/api";

/**
 * An in-memory server for the player's side of a combat, for tests. It answers what api/overrides/combatPlayer.ts
 * calls with the PLAYER projection only, the way the server does (plan 3.5, D5): hidden creatures are never in it.
 * - `GET /api/parties` lists the parties the account has a character in;
 * - `GET /api/parties/{p}/combat` answers `{combat}` (null when none);
 * - `POST /api/combats/{c}/{action}` applies a player write and answers the projection;
 * - `down` makes every request fail as a network error (no signal);
 * - `emit` sends an event on the account's stream (`listen` is what `combatPlayerDeps.events` calls).
 */
export interface FakeCombatServer {
  parties: { id: string; name: string; myCharacters: string[] }[];
  /** Each party's live combat as players see it. */
  combats: Map<string, PlayerCombat | null>;
  calls: { method: string; path: string; body?: unknown }[];
  down: boolean;
  fetch: (session: unknown, method: string, path: string, body?: unknown) => Promise<unknown>;
  listen: (onEvent: (e: ServerEvent) => void, onOpen: (again: boolean) => void) => () => void;
  emit: (event: string, payload: unknown) => void;
  /** Change a party's combat and tell the players, as the server does after a DM's command. */
  change: (party: string, edit: (c: PlayerCombat) => PlayerCombat | null) => void;
  reopen: () => void;
}

const fail = (code: string, message: string) => Object.assign(new Error(message), { code, message });
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

export function pc(id: string, name: string, characterId: string, mine: boolean, extra: Partial<PlayerCombatant> = {}): PlayerCombatant {
  return { id, kind: "character", name, mine, characterId, ownerName: mine ? "alice" : "bob", initiative: 12, band: mine ? null : "unhurt", conditions: [], life: "alive", ...extra };
}

export function creature(id: string, name: string, extra: Partial<PlayerCombatant> = {}): PlayerCombatant {
  return { id, kind: "creature", name, mine: false, initiative: 15, conditions: [], ...extra };
}

export function combatOf(partyId: string, combatants: PlayerCombatant[], extra: Partial<PlayerCombat> = {}): PlayerCombat {
  return {
    id: `c-${partyId}`, partyId, seq: 1, status: "active", round: 1,
    pointer: { kind: "combatant", id: combatants[0]!.id, round: 1 }, combatants, prompts: [], log: [], ...extra,
  };
}

/** What an in-app roll comes to here: 10 plus the modifier. */
const rolled = (r: D20Input) => (r.kind === "typed" ? r.total : 10 + r.modifier);

export function fakeCombatServer(parties: FakeCombatServer["parties"]): FakeCombatServer {
  const listeners = new Set<{ onEvent: (e: ServerEvent) => void; onOpen: (again: boolean) => void }>();
  const server: FakeCombatServer = {
    parties,
    combats: new Map(),
    calls: [],
    down: false,
    async fetch(_session, method, path, body) {
      server.calls.push({ method, path, body });
      if (server.down) throw fail("network", "The server can't be reached. Check your connection.");
      if (method === "GET" && path === "/api/parties") {
        return { dm: false, parties: server.parties.map((p) => ({ ...p, role: "player", dmName: "dana", memberCount: 2, ruleset: "dnd2024", createdAt: 0, updatedAt: 0 })) };
      }
      let m = /^\/api\/parties\/([^/]+)\/combat$/.exec(path);
      if (method === "GET" && m) {
        const party = decodeURIComponent(m[1]!);
        if (!server.parties.some((p) => p.id === party)) throw fail("not_found", "Not found.");
        const c = server.combats.get(party) ?? null;
        return { combat: c ? clone({ ...c, log: c.log.map((l) => ({ ...l, at: l.at / 1000 })) }) : null };
      }
      m = /^\/api\/combats\/([^/]+)\/([a-z_]+)$/.exec(path);
      if (method === "POST" && m) {
        const id = decodeURIComponent(m[1]!);
        const entry = [...server.combats.entries()].find(([, c]) => c?.id === id);
        if (!entry || !entry[1]) throw fail("not_found", "Not found.");
        const [party, c] = entry;
        const args = body as { id?: string; roll?: D20Input; prompt?: string; answer?: string };
        const me = c.combatants.find((x) => x.id === args.id);
        if (args.id !== undefined && !me?.mine) throw fail("not_found", "Not found.");
        switch (m[2]) {
          case "set_initiative":
            me!.initiative = rolled(args.roll!);
            break;
          case "death_save": {
            const total = rolled(args.roll!);
            const saves = me!.deathSaves ?? { successes: 0, failures: 0 };
            if (total >= 10) saves.successes += 1;
            else saves.failures += 1;
            me!.deathSaves = saves;
            if (saves.successes >= 3) me!.life = "stable";
            if (saves.failures >= 3) me!.life = "dead";
            break;
          }
          case "resolve_concentration":
            c.prompts = c.prompts.filter((p) => !(p.kind === "concentration" && p.combatant === me!.id));
            break;
          case "answer_prompt":
            c.prompts = c.prompts.filter((p: CombatPrompt) => p.id !== args.prompt);
            break;
          default:
            throw fail("not_found", "Not found.");
        }
        c.seq += 1;
        server.combats.set(party, c);
        return clone({ ...c, log: c.log.map((l) => ({ ...l, at: l.at / 1000 })) });
      }
      throw fail("not_found", "Not found.");
    },
    listen(onEvent, onOpen) {
      const l = { onEvent, onOpen };
      listeners.add(l);
      return () => { listeners.delete(l); };
    },
    emit(event, payload) {
      for (const l of [...listeners]) l.onEvent({ event, payload });
    },
    change(party, edit) {
      const before = server.combats.get(party);
      const next = before ? edit(clone(before)) : null;
      if (next) next.seq = (before?.seq ?? 0) + 1;
      server.combats.set(party, next);
      server.emit("party://combat", { party, seq: next?.seq ?? (before?.seq ?? 0) + 1 });
    },
    reopen() {
      for (const l of [...listeners]) l.onOpen(true);
    },
  };
  return server;
}
