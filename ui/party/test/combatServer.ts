import type { ServerEvent } from "../api/http";
import type { D20Input, WireCombatant, WireMine, WireProjection } from "../combat/player/api";

/**
 * An in-memory combat server for the player's side, for tests. It answers what combat/player/api.ts calls the way
 * party/docs/combat-api.md describes, with the PLAYER projection only (plan 3.5, D5): hidden creatures are never in it.
 * - `GET /api/parties` lists the parties the account has a character in;
 * - `GET /api/parties/{p}/combat` answers `{combat: id | null}`; `GET /api/combats/{c}` the projection;
 * - `POST /api/combats/{c}/{action}` with `{requestId, args}` applies a player write and answers the projection; an
 *   action or PC that isn't the player's is the one 404;
 * - `down` makes every request fail as a network error (no signal);
 * - `emit` sends an event on the account's stream (`listen` is what `combatPlayerDeps.events` calls).
 */
export interface FakeCombatServer {
  parties: { id: string; name: string; myCharacters: string[] }[];
  /** Each party's live combat as this player sees it. */
  combats: Map<string, WireProjection | null>;
  calls: { method: string; path: string; body?: unknown }[];
  down: boolean;
  /** The next write is refused with this, as the server's 409 would. */
  refuseNext: { code: string; message: string } | null;
  /** Writes done, by requestId: a repeated id is done once (combat-api.md). */
  done: Set<string>;
  fetch: (session: unknown, method: string, path: string, body?: unknown) => Promise<unknown>;
  listen: (onEvent: (e: ServerEvent) => void, onOpen: (again: boolean) => void) => () => void;
  emit: (event: string, payload: unknown) => void;
  /** Change a party's combat and tell the player, as the server does after a write that changes their projection. */
  change: (party: string, edit: (c: WireProjection) => WireProjection | null) => void;
  reopen: () => void;
}

const NOT_FOUND = "There is no such combat here.";
const fail = (code: string, message: string) => Object.assign(new Error(message), { code, message });
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

export function pc(id: string, name: string, mine: boolean, extra: Partial<WireCombatant> = {}): WireCombatant {
  return { id, kind: "character", name, mine, initiative: 12, band: "unhurt", life: { state: "alive" }, conditions: [], exhaustion: 0, concentration: null, ...extra };
}

export function creature(id: string, name: string, extra: Partial<WireCombatant> = {}): WireCombatant {
  return { id, kind: "creature", name, band: "unhurt", life: { state: "alive" }, conditions: [], exhaustion: 0, hp: { current: 7, max: 7 }, ac: 15, ...extra };
}

export function mine(id: string, characterId: string | null, extra: Partial<WireMine> = {}): WireMine {
  return { id, ...(characterId ? { characterId } : {}), hp: { current: 20, max: 24, temp: 0 }, life: { state: "alive" }, concentration: null, concentrationChecks: [], prompts: [], ...extra };
}

export function projection(partyId: string, order: WireCombatant[], me: WireMine[], extra: Partial<WireProjection> = {}): WireProjection {
  return {
    id: `c-${partyId}`, partyId, encounter: { id: "e1", publicName: "Road ambush" }, status: "active", pseq: 1,
    turn: { kind: "combatant", id: order[0]!.id, round: 1 }, showEnemyTurn: false, order, me, ...extra,
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
    refuseNext: null,
    done: new Set(),
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
        const c = server.combats.get(party);
        return { combat: c && c.status !== "ended" ? c.id : null };
      }
      const find = (id: string) => [...server.combats.entries()].find(([, c]) => c?.id === id);
      m = /^\/api\/combats\/([^/]+)$/.exec(path);
      if (method === "GET" && m) {
        const entry = find(decodeURIComponent(m[1]!));
        if (!entry?.[1]) throw fail("not_found", NOT_FOUND);
        return clone(entry[1]);
      }
      m = /^\/api\/combats\/([^/]+)\/([a-z_]+)$/.exec(path);
      if (method === "POST" && m) {
        const entry = find(decodeURIComponent(m[1]!));
        if (!entry?.[1]) throw fail("not_found", NOT_FOUND);
        const [party, c] = entry;
        const { requestId, args } = (body ?? {}) as { requestId?: string; args?: { id?: string; roll?: D20Input; promptId?: string; applied?: boolean } };
        if (!requestId || !args) throw fail("not_found", NOT_FOUND);
        if (server.refuseNext) { const no = server.refuseNext; server.refuseNext = null; throw fail(no.code, no.message); }
        if (server.done.has(requestId)) return { ...clone(c), replayed: true };
        server.done.add(requestId);
        const own = c.me.find((x) => x.id === args.id);
        const row = c.order.find((x) => x.id === args.id);
        switch (m[2]) {
          case "set_initiative":
            if (!own || !row) throw fail("not_found", NOT_FOUND);
            row.initiative = rolled(args.roll!);
            break;
          case "death_save": {
            if (!own || !row) throw fail("not_found", NOT_FOUND);
            const l = own.life;
            if (typeof l !== "object" || !("state" in l) || l.state !== "dying") throw fail("conflict", "Not making death saves.");
            if (rolled(args.roll!) >= 10) l.successes += 1;
            else l.failures += 1;
            if (l.successes >= 3) own.life = { state: "stable" };
            if (l.failures >= 3) own.life = { state: "dead" };
            row.life = own.life;
            break;
          }
          case "resolve_concentration":
            if (!own) throw fail("not_found", NOT_FOUND);
            if (!own.concentrationChecks?.length) throw fail("conflict", "No concentration check is waiting.");
            own.concentrationChecks = own.concentrationChecks.slice(1);
            break;
          case "resolve_prompt": {
            const holder = c.me.find((x) => (x.prompts ?? []).some((p) => p.id === args.promptId));
            if (!holder) throw fail("not_found", NOT_FOUND);
            holder.prompts = holder.prompts!.filter((p) => p.id !== args.promptId);
            break;
          }
          default:
            throw fail("not_found", NOT_FOUND);
        }
        c.pseq += 1;
        server.combats.set(party, c);
        return clone(c);
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
      if (next) next.pseq = (before?.pseq ?? 0) + 1;
      server.combats.set(party, next);
      if (next && before) server.emit("party://combat", { party, combat: next.id, pseq: next.pseq });
      // A combat that starts or ends is a party event.
      else server.emit("party://party", { party, pseq: 9, kind: "combat", id: before?.id ?? next?.id ?? null });
    },
    reopen() {
      for (const l of [...listeners]) l.onOpen(true);
    },
  };
  return server;
}
