import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { combatDeps, resetCombatDeps } from "../../api/overrides/combat";
import { saveSession } from "../../api/session";
import { memoryStorage } from "../../test/memoryStorage";
import { CombatClient, OFFLINE_AFTER_MS, type ClientDeps } from "./client";
import { mockCombatServer, type MockServer } from "./mockServer";
import { loadCombat, sendCommand, undoCombat, watchCombats } from "../../api/overrides/combat";

/** The DM's write queue and offline state (D44): about a minute of queued writes, then "offline". */

let server: MockServer;
let now = 0;
let timers: { at: number; fn: () => void }[] = [];
const reports: string[] = [];

function deps(): ClientDeps {
  return {
    now: () => now,
    setTimeout: (fn, ms) => { const t = { at: now + ms, fn }; timers.push(t); return t; },
    clearTimeout: (h) => { timers = timers.filter((t) => t !== h); },
    load: loadCombat,
    send: (id, command, op) => sendCommand(id, command, op),
    undo: undoCombat,
    watch: watchCombats,
    report: (m) => { reports.push(m); },
  };
}

/** Moves the clock on and runs the timers due. */
async function advance(ms: number) {
  const until = now + ms;
  for (;;) {
    const due = timers.filter((t) => t.at <= until).sort((a, b) => a.at - b.at)[0];
    if (!due) break;
    timers = timers.filter((t) => t !== due);
    now = due.at;
    due.fn();
    await settle();
  }
  now = until;
}

const settle = () => new Promise((r) => setTimeout(r, 0)).then(() => new Promise((r) => setTimeout(r, 0)));

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "dm1", username: "dana", email: null, admin: false, mustChangePassword: false } });
  server = mockCombatServer();
  Object.assign(combatDeps, server.deps);
  now = 1_000_000;
  timers = [];
  reports.length = 0;
});
afterEach(() => { resetCombatDeps(); vi.unstubAllGlobals(); });

async function opened() {
  const c = new CombatClient("c1", deps());
  c.open();
  await settle();
  return c;
}

describe("CombatClient", () => {
  it("loads the combat and sends writes in order, each with its own requestId", async () => {
    const c = await opened();
    expect(c.snapshot.view?.seq).toBe(0);
    c.send({ command: "set_initiative", id: "mage", roll: { kind: "typed", total: 14 } });
    c.send({ command: "set_initiative", id: "knight", roll: { kind: "typed", total: 16 } });
    await settle();
    expect(c.snapshot.view?.seq).toBe(2);
    expect(c.snapshot.queue).toHaveLength(0);
    expect(c.snapshot.connection).toBe("live");
  });

  it("keeps the last state and queues writes without signal, then sends them when it's back (D44)", async () => {
    const c = await opened();
    server.offline = true;
    c.send({ command: "damage", target: "mage", parts: [{ amount: { kind: "typed", total: 5 } }] });
    await settle();
    expect(c.snapshot.connection).toBe("reconnecting");
    expect(c.snapshot.queue).toHaveLength(1);
    expect(c.snapshot.view?.state.combatants.find((x) => x.id === "mage")?.hp.current).toBe(40);
    // Still taking writes while reconnecting.
    expect(c.send({ command: "damage", target: "mage", parts: [{ amount: { kind: "typed", total: 1 } }] })).toBe(true);
    server.offline = false;
    await advance(5000);
    expect(c.snapshot.connection).toBe("live");
    expect(c.snapshot.queue).toHaveLength(0);
    expect(c.snapshot.view?.state.combatants.find((x) => x.id === "mage")?.hp.current).toBe(34);
  });

  it("after a minute without an answer it is offline: no new writes, and the DM chooses what to do with the waiting ones", async () => {
    const c = await opened();
    server.offline = true;
    c.send({ command: "damage", target: "mage", parts: [{ amount: { kind: "typed", total: 5 } }] });
    await settle();
    await advance(OFFLINE_AFTER_MS + 1000);
    expect(c.snapshot.connection).toBe("offline");
    expect(c.accepting).toBe(false);
    expect(c.send({ command: "next_turn" })).toBe(false);
    expect(reports.at(-1)).toMatch(/Offline/);
    // The server answers again: the waiting write is held for the DM's choice, not sent on its own.
    server.offline = false;
    await advance(11_000);
    expect(c.snapshot.connection).toBe("live");
    expect(c.snapshot.held).toHaveLength(1);
    expect(server.combats.get("c1")!.view.seq).toBe(0);
    c.resolveHeld(true);
    await settle();
    expect(server.combats.get("c1")!.view.seq).toBe(1);
    expect(c.snapshot.held).toHaveLength(0);
  });

  it("drops the held writes when the DM says so", async () => {
    const c = await opened();
    server.offline = true;
    c.send({ command: "damage", target: "mage", parts: [{ amount: { kind: "typed", total: 5 } }] });
    await settle();
    await advance(OFFLINE_AFTER_MS + 1000);
    server.offline = false;
    await advance(11_000);
    c.resolveHeld(false);
    await settle();
    expect(server.combats.get("c1")!.view.seq).toBe(0);
    expect(c.accepting).toBe(true);
  });

  it("a player's write in between doesn't block the DM's queued write (no expectSeq: an intent applies to the newest state)", async () => {
    const c = await opened();
    server.playerWrite("c1", { command: "set_initiative", id: "pc-bram", roll: { kind: "typed", total: 13 } });
    c.send({ command: "set_initiative", id: "mage", roll: { kind: "typed", total: 14 } });
    await settle();
    const view = server.combats.get("c1")!.view;
    expect(view.seq).toBe(2);
    expect(c.snapshot.view?.seq).toBe(2);
    expect(view.state.combatants.find((x) => x.id === "pc-bram")?.initiative).toBe(13);
    expect(view.state.combatants.find((x) => x.id === "mage")?.initiative).toBe(14);
  });

  it("a retry after no answer reuses the requestId, so the server does the write once (D44)", async () => {
    const c = await opened();
    const seen: string[] = [];
    const real = combatDeps.fetch;
    let drop = true;
    combatDeps.fetch = async (session, method, path, body) => {
      if (method === "POST") seen.push(String((body as { requestId: string }).requestId));
      const out = await real(session, method, path, body);
      // The write landed, but the answer is lost on the way back.
      if (method === "POST" && drop) { drop = false; throw { code: "network", message: "The connection dropped." }; }
      return out;
    };
    c.send({ command: "damage", target: "mage", parts: [{ amount: { kind: "typed", total: 5 } }] });
    await settle();
    expect(c.snapshot.connection).toBe("reconnecting");
    await advance(2000);
    expect(seen).toHaveLength(2);
    expect(seen[0]).toBe(seen[1]);
    expect(server.combats.get("c1")!.view.state.combatants.find((x) => x.id === "mage")?.hp.current).toBe(35);
    expect(c.snapshot.connection).toBe("live");
  });

  it("a refused write is dropped with the server's reason", async () => {
    const c = await opened();
    c.send({ command: "next_turn" });
    await settle();
    expect(reports.at(-1)).toMatch(/isn't running/);
    expect(c.snapshot.queue).toHaveLength(0);
  });

  it("follows changes made elsewhere through the event stream", async () => {
    const c = await opened();
    server.playerWrite("c1", { command: "set_initiative", id: "pc-bram", roll: { kind: "typed", total: 13 } });
    await settle();
    expect(c.snapshot.view?.seq).toBe(1);
  });

  it("undo asks before reverting a player's write (C8)", async () => {
    const c = await opened();
    server.playerWrite("c1", { command: "set_initiative", id: "pc-bram", roll: { kind: "typed", total: 13 } });
    await settle();
    expect(await c.undo()).toBe("needs_force");
    expect(await c.undo(true)).toBe("done");
    expect(c.snapshot.view?.state.combatants.find((x) => x.id === "pc-bram")?.initiative).toBeNull();
  });
});
