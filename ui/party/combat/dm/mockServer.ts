/**
 * The combat server's DM routes (party/docs/combat-api.md) answered in memory from ./mockEngine.ts: for tests, and
 * for trying the tracker against a server that doesn't have them yet (`localStorage["party.combatMock"] = "1"`, then
 * open `#/combat/c1`).
 */
import type { CombatDeps } from "../../api/overrides/combat";
import { character, sampleCombat } from "./fixtures";
import { seededDie } from "../../../src/lib/dice";
import { CombatRefusal, execute, exportCombat, importCombat, undo, type MockCombat } from "./mockEngine";
import { MOCK_BEASTS, newSpellStore, spellRoute, type MockSpellStore } from "./mockSpellRoutes";
import { redactState } from "./privacy";
import type { Combatant, Command, CompanionOffer, DmCombat } from "./types";

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
  /** Another DM tab's write: done on the server, announced on the stream. */
  otherWrite: (combat: string, command: Command) => void;
  /** 0.3.2: spells, effect definitions, precasts and the bestiary (./mockSpellRoutes.ts). */
  spells: MockSpellStore;
  /** The stream: a ping, a drop, a reopen (events may have been missed). */
  stream: { beat: () => void; drop: () => void; reopen: () => void };
}

const fail = (code: string, message: string) => ({ code, message });

/** The sample PCs' sheet companions (the server reads them from the character files). */
function offersFor(m: MockCombat): CompanionOffer[] {
  const pc = m.view.state.combatants.find((c) => c.id === "pc-aria");
  if (!pc || m.view.state.combatants.some((c) => c.owner_id === pc.id && c.label === "Wolf")) return [];
  return [{ pc: pc.id, key: "wolf", name: "Wolf", hp: 11, ac: 13, initiative: 2 }];
}

/** 0.3.5 (A2): the sample party's member who joined after the fight began (the server lists party members not in it). */
export const LATE_MEMBER = { characterId: "c_late", name: "Ilse", owner: "pl3" };

function joinableOf(m: MockCombat): NonNullable<DmCombat["joinable"]> {
  const fighting = (id: string) => m.view.state.combatants.some((c) => c.id === `pc-${id}` && !c.removed && !c.absent && c.initiative != null);
  return fighting(LATE_MEMBER.characterId) ? [] : [{ characterId: LATE_MEMBER.characterId, name: LATE_MEMBER.name }];
}

/**
 * The damage and healing the DM sent players (D7 a), from the mock's player events, with how each stands (0.3.3,
 * party/docs/role-views.md): `delivered` until the player answers, then `accepted` or `declined`.
 */
function promptsOf(m: MockCombat): NonNullable<DmCombat["prompts"]> {
  return m.playerEvents.flatMap((e, i) => {
    if (e.event !== "damage_prompt" && e.event !== "heal_prompt") return [];
    const status = (e.status as string | undefined) ?? "delivered";
    return [{ id: `prompt-${i}`, combatantId: String(e.id), kind: e.event === "heal_prompt" ? "heal" as const : "damage" as const, amount: Number(e.amount),
      criticalHit: Boolean(e.critical_hit), at: i + 1, status }];
  });
}

/** The player answers a prompt the DM sent (the rig and tests): accepted or declined. */
export function answerPrompt(m: MockCombat, promptId: string, accepted: boolean): void {
  const e = m.playerEvents[Number(promptId.replace(/^prompt-/, ""))];
  if (!e) return;
  e.status = accepted ? "accepted" : "declined";
  // The server's resolve_prompt is a write: the DM's seq moves on.
  m.view.seq += 1;
}

/** The DM view of a mock combat, as the server sends it: no player's HP or resources (./privacy.ts). */
export function dmView(m: MockCombat): DmCombat {
  const v = m.view;
  const creatures: DmCombat["creatures"] = {};
  for (const [id, b] of Object.entries(v.statblocks)) creatures[id] = { encounterCreatureId: null, statBlock: b.data, name: b.name };
  return {
    id: v.id, partyId: v.partyId, encounter: v.encounterId ? { id: v.encounterId, name: "Sample fight", publicName: "A fight" } : null,
    seq: v.seq, status: v.state.status, state: redactState(v.state), creatures, characters: {}, prompts: promptsOf(m), offers: offersFor(m),
    joinable: joinableOf(m), undo: [...m.journal].reverse().map((j) => j.label),
  };
}

/**
 * Several tabs of one browser sharing the mock (the rig's two-tab test): the combats live in localStorage, and a
 * write in one tab is announced to the others over a BroadcastChannel, as the server's event stream would.
 */
export interface MockSharing {
  load: () => string | null;
  save: (text: string) => void;
  channel: { postMessage: (m: unknown) => void; listen: (fn: (data: unknown) => void) => void };
}

export function browserSharing(): MockSharing | undefined {
  try {
    if (typeof BroadcastChannel === "undefined") return undefined;
    const channel = new BroadcastChannel("party.combatMock");
    return {
      load: () => localStorage.getItem("party.combatMock.state"),
      save: (t) => localStorage.setItem("party.combatMock.state", t),
      channel: { postMessage: (m) => channel.postMessage(m), listen: (fn) => { channel.onmessage = (e) => fn(e.data); } },
    };
  } catch { return undefined; }
}

export function mockCombatServer(seed?: MockCombat[], sharing?: MockSharing): MockServer {
  const combats = new Map<string, MockCombat>((seed ?? [sampleCombat()]).map((m) => [m.view.id, m]));
  const pull = () => {
    const text = sharing?.load();
    if (!text) return;
    try {
      for (const [id, saved] of Object.entries(JSON.parse(text) as Record<string, Pick<MockCombat, "view" | "journal">>)) {
        combats.set(id, { view: saved.view, journal: saved.journal, die: seededDie(7 + saved.view.seq), playerEvents: [] });
      }
    } catch { /* a broken save: start from the seed */ }
  };
  const push = (combat: string) => {
    if (!sharing) return;
    sharing.save(JSON.stringify(Object.fromEntries([...combats].map(([id, m]) => [id, { view: m.view, journal: m.journal }]))));
    sharing.channel.postMessage({ combat });
  };
  const listeners = new Set<(e: { event: string; payload: unknown }) => void>();
  const opens = new Set<(again: boolean) => void>();
  const statuses = new Set<{ onBeat?: () => void; onDrop?: () => void }>();
  const done = new Map<string, true>();
  let made = 0;
  const server: MockServer = {
    combats,
    spells: newSpellStore(),
    offline: false,
    requests: [],
    notify: (combat) => {
      const m = combats.get(combat);
      for (const l of [...listeners]) l({ event: "party://combat", payload: { party: m?.view.partyId, combat, seq: m?.view.seq ?? 0 } });
      for (const st of [...statuses]) st.onBeat?.();
    },
    playerWrite: (combat, command) => {
      execute(combats.get(combat)!, command, undefined, "player");
      server.notify(combat);
    },
    otherWrite: (combat, command) => {
      execute(combats.get(combat)!, command);
      server.notify(combat);
    },
    stream: {
      beat: () => { for (const s of [...statuses]) s.onBeat?.(); },
      drop: () => { for (const s of [...statuses]) s.onDrop?.(); },
      reopen: () => { for (const o of [...opens]) o(true); },
    },
    deps: {
      fetch: async (_session, method, path, body) => {
        server.requests.push(`${method} ${path}`);
        pull();
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
          const spellAnswer = spellRoute(server.spells, method, path, body);
          if (spellAnswer !== undefined) return spellAnswer;
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
          else if (action === "summon" && typeof args.elementId === "string") {
            // 0.3.2: a creature of the DM's bestiary under a caster's control (the server builds it from the element).
            const beast = MOCK_BEASTS.find((x) => x.id === args.elementId);
            if (!beast) throw fail("refused", "That creature isn't in your content.");
            const n = m.view.state.combatants.filter((c) => c.label.startsWith(beast.name)).length + 1;
            const id = `summon-${++made}`;
            const { elementId: _e, ...rest } = args;
            void _e;
            execute(m, { command: "summon", ...rest, combatant: {
              id, kind: "creature", label: `${beast.name} ${n}`, public_name: beast.name, visible: true, hp: { current: beast.hp ?? 10, max: beast.hp ?? 10, temp: 0 },
              ac: beast.ac, initiative_bonus: 1, speed: { walk: 40 }, abilities: [{ key: "bite", name: "Bite", kind: "action", text: "A bite." }],
            } } as Command);
            m.view.statblocks[id] = { name: beast.name, data: { ac: [{ value: beast.ac ?? 10 }], hp: { average: beast.hp ?? 10 } } };
          } else if (action === "add_member") {
            // A2: the late member's PC joins with its initiative (typed, or the server rolls with the card's bonus).
            if (args.characterId !== LATE_MEMBER.characterId || !joinableOf(m).length) throw fail("invalid_argument", "That character isn't in this party.");
            const id = `pc-${LATE_MEMBER.characterId}`;
            if (!m.view.state.combatants.some((c) => c.id === id)) {
              execute(m, { command: "add_combatant", combatant: character(id, LATE_MEMBER.owner, LATE_MEMBER.name, 24, 15, 1, null) });
            }
            const init = args.initiative as { kind: "typed"; total: number } | { kind: "app" };
            execute(m, { command: "set_initiative", id, roll: init.kind === "typed" ? init : { kind: "app", modifier: 1, mode: "normal" } });
          } else if (action === "add_creature") {
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
          } else if (action === "summon" && typeof args.companion === "string") {
            const offer = offersFor(m).find((o) => o.key === args.companion);
            if (!offer) throw fail("refused", "No such companion on that character.");
            const { companion: _key, ...rest } = args;
            void _key;
            execute(m, { command: "summon", ...rest, combatant: {
              id: `summon-${++made}`, kind: "creature", label: offer.name, public_name: offer.name, visible: true,
              hp: { current: offer.hp, max: offer.hp, temp: 0 }, ac: offer.ac, initiative_bonus: offer.initiative,
            } } as Command);
          } else {
            const before = new Set(m.view.state.combatants.map((c) => c.id));
            // The server mints a summon's id, whatever the client sent.
            if (action === "summon" && args.combatant) args.combatant = { ...(args.combatant as Combatant), id: `summon-${++made}` };
            const tag = action === "prev_turn" ? "previous_turn" : action === "remove_creature" ? "remove_combatant"
              : action === "condition_add" ? "add_condition" : action === "condition_remove" ? "remove_condition" : action;
            execute(m, { command: tag, ...args } as Command);
            // A copied summon fights with its source's stat block.
            if (action === "summon" && typeof args.copyOf === "string" && m.view.statblocks[args.copyOf]) {
              for (const c of m.view.state.combatants) if (!before.has(c.id)) m.view.statblocks[c.id] = m.view.statblocks[args.copyOf]!;
            }
          }
          if (b.requestId) done.set(key, true);
          push(m.view.id);
          server.notify(m.view.id);
          return dmView(m);
        } catch (e) {
          if (e instanceof CombatRefusal) throw fail(e.code === "invalid" || e.code === "unavailable" ? "refused" : e.code, e.message);
          throw e;
        }
      },
      events: (onEvent, onOpen, status) => {
        listeners.add(onEvent);
        opens.add(onOpen);
        const st = status ?? {};
        statuses.add(st);
        onOpen(false);
        return () => { listeners.delete(onEvent); opens.delete(onOpen); statuses.delete(st); };
      },
    },
  };
  if (sharing) {
    pull();
    sharing.channel.listen((data) => {
      const combat = (data as { combat?: string } | null)?.combat;
      if (!combat) return;
      pull();
      server.notify(combat);
    });
  }
  return server;
}

/** The mock is switched on for this browser. */
export function mockWanted(): boolean {
  try { return localStorage.getItem("party.combatMock") === "1"; } catch { return false; }
}
