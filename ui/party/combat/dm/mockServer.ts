/**
 * The combat server's DM routes (party/docs/combat-api.md) answered in memory from ./mockEngine.ts: for tests, and
 * for trying the tracker against a server that doesn't have them yet (`localStorage["party.combatMock"] = "1"`, then
 * open `#/combat/c1`).
 */
import type { CombatDeps } from "../../api/overrides/combat";
import { sampleCombat } from "./fixtures";
import { CombatRefusal, execute, exportCombat, importCombat, undo, type MockCombat } from "./mockEngine";
import type { Combatant, Command, DmCombat } from "./types";

export interface MockServer {
  combats: Map<string, MockCombat>;
  /** Simulates no signal: every request fails as the network would. */
  offline: boolean;
  /** The requests seen, `METHOD path`. */
  requests: string[];
  deps: CombatDeps;
  /** Sends a combat change on the event stream. */
  notify: (combat: string) => void;
  /** A player's write (their initiative): moves `seq` on, as the server would. */
  playerWrite: (combat: string, command: Command) => void;
}

const fail = (code: string, message: string) => ({ code, message });

/** The DM view of a mock combat, as the server sends it. */
export function dmView(m: MockCombat): DmCombat {
  const v = m.view;
  const creatures: DmCombat["creatures"] = {};
  for (const [id, b] of Object.entries(v.statblocks)) creatures[id] = { encounterCreatureId: null, statBlock: b.data, name: b.name };
  return {
    id: v.id, partyId: v.partyId, encounter: v.encounterId ? { id: v.encounterId, name: "Sample fight", publicName: "A fight" } : null,
    seq: v.seq, status: v.state.status, state: v.state, creatures, characters: {}, prompts: [],
    undo: [...m.journal].reverse().map((j) => j.label),
  };
}

export function mockCombatServer(seed?: MockCombat[]): MockServer {
  const combats = new Map<string, MockCombat>((seed ?? [sampleCombat()]).map((m) => [m.view.id, m]));
  const listeners = new Set<(e: { event: string; payload: unknown }) => void>();
  const done = new Map<string, true>();
  let made = 0;
  const server: MockServer = {
    combats,
    offline: false,
    requests: [],
    notify: (combat) => {
      const m = combats.get(combat);
      for (const l of [...listeners]) l({ event: "party://combat", payload: { party: m?.view.partyId, combat, seq: m?.view.seq ?? 0 } });
    },
    playerWrite: (combat, command) => {
      execute(combats.get(combat)!, command, undefined, "player");
      server.notify(combat);
    },
    deps: {
      fetch: async (_session, method, path, body) => {
        server.requests.push(`${method} ${path}`);
        await Promise.resolve();
        if (server.offline) throw fail("network", "The server can't be reached. Check your connection.");
        const b = (body ?? {}) as { requestId?: string; expectSeq?: number; args?: Record<string, unknown>; file?: unknown };
        const parts = path.split("?")[0]!.split("/").map(decodeURIComponent);
        try {
          if (method === "POST" && parts[2] === "encounters" && parts[4] === "start") {
            const m = sampleCombat(`c-${parts[3]}`);
            m.view.encounterId = parts[3]!;
            combats.set(m.view.id, m);
            return dmView(m);
          }
          if (method === "GET" && parts[2] === "parties" && parts[4] === "combat") {
            const live = [...combats.values()].find((m) => m.view.partyId === parts[3] && m.view.state.status !== "ended");
            return { combat: live?.view.id ?? null };
          }
          if (method === "POST" && parts[2] === "parties" && parts[4] === "combats" && parts[5] === "import") {
            const m = importCombat(JSON.stringify(b.file), `imported-${++made}`, parts[3]!);
            combats.set(m.view.id, m);
            return dmView(m);
          }
          if (parts[2] !== "combats") throw fail("not_found", "The server does not offer that.");
          const m = combats.get(parts[3] ?? "");
          if (!m) throw fail("not_found", "There is no such combat here.");
          const action = parts[4];
          if (method === "GET" && !action) return dmView(m);
          if (method === "GET" && action === "log") {
            return { entries: m.view.log.map((event, i) => ({ seq: i + 1, at: 0, author: "dm", event })).reverse() };
          }
          if (method !== "POST" || !action) throw fail("not_found", "There is no such combat here.");
          if (action === "export") return JSON.parse(exportCombat(m));
          const key = `${m.view.id}:${b.requestId}`;
          if (b.requestId && done.has(key)) return { ...dmView(m), replayed: true };
          if (b.expectSeq !== undefined && b.expectSeq !== m.view.seq) {
            throw { code: "stale", message: "The combat changed. Refresh and try again.", seq: m.view.seq };
          }
          const args = b.args ?? {};
          if (action === "undo") undo(m, undefined, Boolean(args.force));
          else if (action === "add_creature") {
            const from = m.view.state.combatants.find((c) => c.id === args.copyOf);
            if (!from) throw fail("refused", "No such creature.");
            const count = Number(args.count ?? 1);
            for (let i = 0; i < count; i++) {
              const id = `${from.id}-copy-${++made}`;
              const n = m.view.state.combatants.filter((c) => c.label.replace(/\s*\d+$/, "") === from.label.replace(/\s*\d+$/, "")).length + 1;
              const c: Combatant = {
                ...structuredClone(from), id, label: `${from.label.replace(/\s*\d+$/, "")} ${n}`, initiative: null, linked_to: undefined, removed: false,
                hp: { current: from.hp.max, max: from.hp.max, temp: 0 }, life: { state: "alive" }, concentration: undefined, pending_concentration: [],
                visible: args.hidden === undefined ? from.visible : !args.hidden,
              };
              c.public_name = c.label;
              execute(m, { command: "add_combatant", combatant: c, roll_initiative: args.rollInitiative !== false });
              if (m.view.statblocks[from.id]) m.view.statblocks[id] = m.view.statblocks[from.id]!;
            }
          } else {
            const tag = action === "prev_turn" ? "previous_turn" : action === "remove_creature" ? "remove_combatant"
              : action === "condition_add" ? "add_condition" : action === "condition_remove" ? "remove_condition" : action;
            execute(m, { command: tag, ...args } as Command);
          }
          if (b.requestId) done.set(key, true);
          server.notify(m.view.id);
          return dmView(m);
        } catch (e) {
          if (e instanceof CombatRefusal) throw fail(e.code === "invalid" || e.code === "unavailable" ? "refused" : e.code, e.message);
          throw e;
        }
      },
      events: (onEvent, onOpen) => {
        listeners.add(onEvent);
        onOpen(false);
        return () => { listeners.delete(onEvent); };
      },
    },
  };
  return server;
}

/** The mock is switched on for this browser. */
export function mockWanted(): boolean {
  try { return localStorage.getItem("party.combatMock") === "1"; } catch { return false; }
}
