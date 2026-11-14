import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App as UpstreamApp } from "../../../src/App";
import { AppProviders } from "../../../src/AppProviders";
import { MockApi } from "../../../src/api/mock";
import { saveSession } from "../../api/session";
import { creature, fakeCombatServer, mine, myCreature, pc, projection, type FakeCombatServer } from "../../test/combatServer";
import { memoryStorage } from "../../test/memoryStorage";
import { combatPlayerDeps, diffEvents, fromWire, resetCombatPlayer, type WireMine, type WireProjection } from "./api";

/**
 * The player's Combat sub-tab (plan 4.4, 0.3.0 part F) on upstream's real Play page, with the party's Tabs standing in
 * for upstream's as the party build does (vite.config.ts REPLACED). The fake server answers the PLAYER projection of
 * party/docs/combat-api.md.
 */

// The party build points PlayPage's "../../components" at playTabs.tsx; vitest runs upstream's config, so do it here.
vi.mock("../../../src/components", async (importOriginal) => {
  const original = await importOriginal<Record<string, unknown>>();
  const party = await import("./playTabsCore");
  return { ...original, Tabs: party.Tabs, TabPanel: party.TabPanel };
});

const holds: string[] = [];
vi.mock("../../api/connectivity", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../api/connectivity")>();
  return {
    ...original,
    holdReload: (why?: string) => {
      holds.push(why ?? "hold");
      return () => { holds.splice(holds.indexOf(why ?? "hold"), 1); };
    },
  };
});

const store = await import("./store");
const { LiveChip } = await import("./LiveChip");
const { leaveCombatTab } = await import("./tabState");

let server: FakeCombatServer;
let api: MockApi;
let hero: string;
let stop: () => void = () => {};
const buzz = vi.fn();
const locks = { taken: 0, released: 0 };

async function seeded(name: string) {
  const list = await api.listCharacters();
  return list.find((c) => c.name.startsWith(name))!.id;
}

/** A combat with the player's PC, an ally and a revealed creature. Hidden creatures are not in a player projection. */
function liveWith(extra: Partial<WireProjection> = {}, me: Partial<WireMine> = {}, initiative: number | null = 14): WireProjection {
  return projection("p1", [
    creature("cr1", "Goblin", { conditions: ["prone"], band: "bloodied", hp: { current: 3, max: 7 },
      statBlock: { size: ["small"], creature_type: "humanoid", ac: [{ value: 15, source: "leather armor, shield" }], hp: { average: 7, formula: "2d6" },
        abilities: { str: 8, dex: 14, con: 10, int: 10, wis: 8, cha: 8 }, actions: [{ name: "Scimitar", text: "Melee Weapon Attack: +4 to hit." }] } }),
    pc("me1", "Seraphine", true, { initiative }),
    pc("ally1", "Bram", false, { initiative: 9, band: "bloodied", conditions: [{ name: "poisoned", remaining: 2 }] }),
  ], [mine("me1", hero, me)], { turn: { kind: "combatant", id: "cr1", round: 2 }, ...extra });
}

function renderPlay(route = "/play") {
  try { sessionStorage.setItem("lk.currentCharacter", hero); } catch { /* ignore */ }
  return render(
    <AppProviders api={api}>
      <MemoryRouter initialEntries={[route]}>
        <LiveChip />
        <UpstreamApp />
      </MemoryRouter>
    </AppProviders>,
  );
}

async function start() {
  await act(async () => {
    stop = store.startCombatWatch();
    await store.combatWatchReady();
  });
}

async function openCombatTab() {
  await userEvent.setup().click(await screen.findByRole("tab", { name: "Combat" }));
  return screen.findByTestId("combat-pane");
}

const tab = (name: string) => screen.queryByRole("tab", { name });
const logLines = () => within(screen.getByRole("region", { name: "What happened" })).getAllByRole("listitem").map((li) => li.textContent);

beforeEach(async () => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "pl1", username: "alice", email: null, admin: false, mustChangePassword: false } });
  api = new MockApi({ aiChunkDelayMs: 1, rng: () => 0.5 });
  api.presetSettings({ showImportNotice: false });
  hero = await seeded("Seraphine");
  server = fakeCombatServer([{ id: "p1", name: "The Company", myCharacters: [hero] }]);
  combatPlayerDeps.fetch = server.fetch;
  combatPlayerDeps.events = server.listen;
  let ids = 0;
  combatPlayerDeps.requestId = () => `req-${++ids}`;
  buzz.mockReset();
  locks.taken = 0;
  locks.released = 0;
  store.deviceDeps.vibrate = buzz;
  store.deviceDeps.staleMs = 60_000;
  store.deviceDeps.reconnectingMs = 0;
  store.writeDeps.sleep = async () => {};
  store.deviceDeps.wakeLock = async () => { locks.taken += 1; return { release: async () => { locks.released += 1; } }; };
});

afterEach(() => {
  stop();
  store.resetCombatStore();
  leaveCombatTab();
  resetCombatPlayer();
  vi.unstubAllGlobals();
});

describe("the Play tab's Combat sub-tab", () => {
  it("is not there without a live combat, and there is no Live chip", async () => {
    server.combats.set("p1", null);
    await start();
    renderPlay();
    expect(await screen.findByRole("tab", { name: "Abilities" })).toBeTruthy();
    expect(tab("Combat")).toBeNull();
    expect(screen.queryByRole("button", { name: /Live/ })).toBeNull();
    expect(holds).toEqual([]);
  });

  it("appears while the party has a live combat, and goes when it ends", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    expect(await screen.findByRole("tab", { name: "Combat" })).toBeTruthy();
    expect(holds).toEqual(["combat"]);
    await waitFor(() => expect(locks.taken).toBe(1));
    const pane = await openCombatTab();
    expect(within(pane).getByText("Round 2")).toBeTruthy();
    expect(within(pane).getByText("Road ambush")).toBeTruthy();
    expect(pane.querySelector(".combat-now")!.textContent).toContain("Goblin");
    const order = within(pane).getByRole("list", { name: "Turn order" });
    const rows = within(order).getAllByRole("listitem").filter((li) => li.classList.contains("combat-row"));
    expect(rows.map((r) => r.querySelector(".combat-row-name")!.textContent)).toEqual(["Goblin is acting now", "Seraphine (you)", "Bram"]);
    expect(rows[0]!.getAttribute("aria-current")).toBe("true");
    // Allies' health bands and the visible conditions; PCs' initiative (creatures' isn't sent).
    expect(within(rows[2]!).getByLabelText("Health: Bloodied")).toBeTruthy();
    expect(within(rows[2]!).getByText(/Poisoned/)).toBeTruthy();
    expect(within(rows[2]!).getByLabelText("Initiative 9")).toBeTruthy();
    expect(within(rows[0]!).getByText("Prone")).toBeTruthy();
    // The player's own HP comes from their own character (D7 a), not the combat.
    const own = (await api.getSnapshot({ characterId: hero })).play.hitPoints;
    expect(within(pane).getByRole("status", { name: new RegExp(`^Hit points: ${own.current} of ${own.max}`) })).toBeTruthy();
    // Another sub-tab, then back.
    await user.click(screen.getByRole("tab", { name: "Abilities" }));
    expect(screen.queryByTestId("combat-pane")).toBeNull();
    await user.click(screen.getByRole("tab", { name: "Combat" }));
    expect(await screen.findByTestId("combat-pane")).toBeTruthy();
    // The DM ends it: the sub-tab goes, and so do the reload hold and the wake lock.
    act(() => server.change("p1", (c) => ({ ...c, status: "ended", turn: { kind: "ended" } })));
    await waitFor(() => expect(tab("Combat")).toBeNull());
    expect(screen.queryByTestId("combat-pane")).toBeNull();
    expect(holds).toEqual([]);
    expect(locks.released).toBe(1);
    // The next combat opens on the Play page's own sub-tab until the Combat tab is chosen.
    server.combats.set("p1", liveWith({ id: "c-p1-2" }));
    act(() => server.emit("party://party", { party: "p1", pseq: 5, kind: "combat", id: "c-p1-2" }));
    expect(await screen.findByRole("tab", { name: "Combat" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Combat" }).getAttribute("aria-selected")).toBe("false");
  });

  it("a party event about a combat starting shows the sub-tab without a reload", async () => {
    server.combats.set("p1", null);
    await start();
    renderPlay();
    await screen.findByRole("tab", { name: "Abilities" });
    expect(tab("Combat")).toBeNull();
    server.combats.set("p1", liveWith({ status: "setup", turn: { kind: "not_started" } }, {}, null));
    act(() => server.emit("party://party", { party: "p1", pseq: 3, kind: "combat", id: "c-p1" }));
    expect(await screen.findByRole("tab", { name: "Combat" })).toBeTruthy();
    // A combat event no newer than what's shown asks nothing.
    const asked = server.calls.length;
    act(() => server.emit("party://combat", { party: "p1", combat: "c-p1", pseq: 1 }));
    await act(async () => { await Promise.resolve(); });
    expect(server.calls.length).toBe(asked);
  });

  it("the Live chip opens it from another page", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith());
    await start();
    renderPlay("/equipment");
    await user.click(await screen.findByRole("button", { name: /Live/ }));
    expect(await screen.findByTestId("combat-pane")).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Combat" }).getAttribute("aria-selected")).toBe("true");
  });

  it("says Your turn, buzzes once and shows the banner on other pages", async () => {
    server.combats.set("p1", liveWith());
    await start();
    renderPlay("/equipment");
    await screen.findByRole("button", { name: /Live/ });
    expect(buzz).not.toHaveBeenCalled();
    act(() => server.change("p1", (c) => ({ ...c, turn: { kind: "combatant", id: "me1", round: 2 } })));
    expect(await screen.findByRole("button", { name: /Your turn · Seraphine/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Live$/ }), "the banner takes the chip's place").toBeNull();
    expect(buzz).toHaveBeenCalledTimes(1);
    expect(buzz).toHaveBeenCalledWith(store.YOUR_TURN_BUZZ);
    // The same turn again (a resync) doesn't buzz again.
    await act(async () => { server.reopen(); await store.refreshCombats(); });
    expect(buzz).toHaveBeenCalledTimes(1);
    // The banner leads to the sub-tab, which says it in its header; the banner goes there.
    await userEvent.setup().click(screen.getByRole("button", { name: /Your turn · Seraphine/ }));
    const pane = await screen.findByTestId("combat-pane");
    expect(pane.querySelector(".combat-now.yours")!.textContent).toContain("Your turn");
    expect(screen.queryByRole("button", { name: /Your turn · Seraphine/ })).toBeNull();
    expect(screen.getByRole("button", { name: /^Your turn$/ }), "a chip again on the Combat sub-tab").toBeTruthy();
  });

  it("keeps the turn on the last visible combatant while hidden creatures act (D5)", async () => {
    // The server's projection stays on the last visible turn; the view shows what it says and counts nothing.
    server.combats.set("p1", liveWith({ turn: { kind: "combatant", id: "ally1", round: 2 } }));
    await start();
    renderPlay();
    const pane = await openCombatTab();
    expect(pane.querySelector(".combat-now")!.textContent).toContain("Bram");
    expect(pane.textContent).not.toMatch(/hidden/i);
    // With the DM's opt-in "Enemy turn" (D5), it says so and nothing more.
    act(() => server.change("p1", (c) => ({ ...c, showEnemyTurn: true, turn: { kind: "enemy_turn", round: 2 } })));
    await waitFor(() => expect(pane.querySelector(".combat-now")!.textContent).toContain("Enemy turn"));
  });
});

describe("the damage prompt (D7 a)", () => {
  async function prompted() {
    server.combats.set("p1", liveWith({}, { prompts: [{ id: "pr1", kind: "damage", amount: 7, parts: [{ amount: 7, damage_type: "slashing" }], criticalHit: false, at: 1 }] }));
    await start();
    renderPlay();
    await openCombatTab();
    return screen.findByRole("alertdialog", { name: "Take 7 slashing damage?" });
  }
  const hp = async () => (await api.getSnapshot({ characterId: hero })).play.hitPoints.current;

  it("Apply takes it on the player's own character, then tells the server with the sheet's HP", async () => {
    const before = await hp();
    const prompt = await prompted();
    await userEvent.setup().click(within(prompt).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: /Take 7/ })).toBeNull());
    expect(await hp()).toBe(before - 7);
    expect(server.calls.at(-1)).toMatchObject({ method: "POST", path: "/api/combats/c-p1/resolve_prompt",
      body: { requestId: expect.stringMatching(/^req-/), args: { promptId: "pr1", applied: true, hp: { current: before - 7 } } } });
    const pane = screen.getByTestId("combat-pane");
    expect(within(pane).getByRole("status", { name: new RegExp(`Hit points: ${before - 7} of`) })).toBeTruthy();
  });

  it("Not now leaves the character as it is", async () => {
    const before = await hp();
    const prompt = await prompted();
    await userEvent.setup().click(within(prompt).getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: /Take 7/ })).toBeNull());
    expect(await hp()).toBe(before);
    expect(server.calls.at(-1)?.body).toEqual({ requestId: expect.stringMatching(/^req-/), args: { promptId: "pr1", applied: false } });
  });
});

describe("rolls: in the app or typed from the table (D21, D43)", () => {
  it("initiative: typed", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({ status: "setup", turn: { kind: "not_started" } }, {}, null));
    await start();
    renderPlay();
    const pane = await openCombatTab();
    expect(within(pane).getByText("Setup")).toBeTruthy();
    const prompt = await screen.findByRole("alertdialog", { name: "Roll initiative" });
    await user.click(within(prompt).getByRole("button", { name: "Type my table roll" }));
    const field = within(prompt).getByLabelText("Initiative total");
    expect(within(prompt).getByRole("button", { name: "Send" }).hasAttribute("disabled")).toBe(true);
    await user.type(field, "17");
    await user.click(within(prompt).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: "Roll initiative" })).toBeNull());
    expect(server.calls.at(-1)).toEqual({ method: "POST", path: "/api/combats/c-p1/set_initiative", body: { requestId: expect.stringMatching(/^req-/), args: { id: "me1", roll: { kind: "typed", total: 17 } } } });
    expect(screen.getByLabelText("Initiative 17")).toBeTruthy();
  });

  it("initiative: in the app, with the character's modifier", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({}, {}, null));
    await start();
    renderPlay();
    await openCombatTab();
    const init = (await api.getSnapshot({ characterId: hero })).stats.initiative;
    const prompt = await screen.findByRole("alertdialog", { name: "Roll initiative" });
    await user.click(within(prompt).getByRole("button", { name: "Roll in app" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: "Roll initiative" })).toBeNull());
    expect(server.calls.at(-1)?.body).toEqual({ requestId: expect.stringMatching(/^req-/), args: { id: "me1", roll: { kind: "app", modifier: init, mode: "normal" } } });
  });

  it("death saves: the pips, a typed save and an in-app one, then stable", async () => {
    const user = userEvent.setup();
    const dying = liveWith({}, { hp: { current: 0, max: 24, temp: 0 }, life: { state: "dying", successes: 1, failures: 2 } });
    dying.order[1]!.conditions = ["unconscious"];
    server.combats.set("p1", dying);
    await start();
    renderPlay();
    await openCombatTab();
    const saves = await screen.findByRole("group", { name: "Death saves" });
    expect(within(saves).getByRole("img", { name: "Successes: 1 of 3" })).toBeTruthy();
    expect(within(saves).getByRole("img", { name: "Failures: 2 of 3" })).toBeTruthy();
    await user.click(within(saves).getByRole("button", { name: "Type my table roll" }));
    await user.type(within(saves).getByLabelText("Death save total"), "12");
    await user.click(within(saves).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(within(screen.getByRole("group", { name: "Death saves" })).getByRole("img", { name: "Successes: 2 of 3" })).toBeTruthy());
    expect(server.calls.at(-1)?.body).toEqual({ requestId: expect.stringMatching(/^req-/), args: { id: "me1", roll: { kind: "typed", total: 12 } } });
    await user.click(within(screen.getByRole("group", { name: "Death saves" })).getByRole("button", { name: "Roll in app" }));
    await waitFor(() => expect(screen.getByText("Stable")).toBeTruthy());
    expect(server.calls.at(-1)?.body).toEqual({ requestId: expect.stringMatching(/^req-/), args: { id: "me1", roll: { kind: "app", modifier: 0, mode: "normal" } } });
    expect(within(screen.getByRole("group", { name: "Death saves" })).queryByRole("button", { name: "Roll in app" })).toBeNull();
  });

  it("a refused save is taken back to what the server says (no stale state)", async () => {
    const user = userEvent.setup();
    const dying = liveWith({}, { life: { state: "dying", successes: 0, failures: 0 } });
    server.combats.set("p1", dying);
    await start();
    renderPlay();
    await openCombatTab();
    // The server already took the PC out of dying (the DM healed it): its 409 reaches the player as it is.
    server.combats.get("p1")!.me[0]!.life = { state: "alive" };
    await user.click(within(await screen.findByRole("group", { name: "Death saves" })).getByRole("button", { name: "Roll in app" }));
    // The refusal (409) refetches the projection: the PC isn't dying, so the saves go.
    await waitFor(() => expect(screen.queryByRole("group", { name: "Death saves" })).toBeNull());
    expect(server.calls.at(-1)?.path).toBe("/api/combats/c-p1");
  });

  it("a concentration save asked by the server", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({}, { concentration: "Bless", concentrationChecks: [{ dc: 12 }] }));
    await start();
    renderPlay();
    await openCombatTab();
    const prompt = await screen.findByRole("alertdialog", { name: "Concentration save, DC 12" });
    await user.click(within(prompt).getByRole("button", { name: "Type my table roll" }));
    await user.type(within(prompt).getByLabelText("Concentration save total"), "15");
    await user.click(within(prompt).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: /Concentration save/ })).toBeNull());
    expect(server.calls.at(-1)).toEqual({ method: "POST", path: "/api/combats/c-p1/resolve_concentration", body: { requestId: expect.stringMatching(/^req-/), args: { id: "me1", roll: { kind: "typed", total: 15 } } } });
  });

  it("a spell's save waits for the player: the table roll is typed first, and a hidden caster stays unnamed (0.3.2)", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({}, { castRows: [{ cast: "cast-1", name: "Fireball", kind: "save", ability: "dex", dc: 14, from: null }] }));
    await start();
    renderPlay();
    await openCombatTab();
    const prompt = await screen.findByRole("alertdialog", { name: "DEX save DC 14 against Fireball" });
    expect(within(prompt).getByText("From an unseen creature")).toBeTruthy();
    const box = within(prompt).getByLabelText("DEX save DC 14 against Fireball: my table roll");
    await user.type(box, "17{Enter}");
    await waitFor(() => expect(server.calls.some((c) => c.path === "/api/combats/c-p1/resolve_cast_row")).toBe(true));
    expect(server.calls.find((c) => c.path === "/api/combats/c-p1/resolve_cast_row")).toEqual({ method: "POST", path: "/api/combats/c-p1/resolve_cast_row", body: { requestId: expect.stringMatching(/^req-/), args: { cast: "cast-1", target: "me1", roll: { kind: "typed", total: 17 } } } });
  });
});

describe("what else the sub-tab shows", () => {
  it("a revealed creature's stat block opens as a sheet, with its HP and AC (D20 b)", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    await openCombatTab();
    await user.click(await screen.findByRole("button", { name: "Goblin stat block" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("HP 3 / 7")).toBeTruthy();
    expect(within(dialog).getByText("Armor Class")).toBeTruthy();
    expect(within(dialog).getByText("15 (leather armor, shield)")).toBeTruthy();
    expect(within(dialog).getByText("Scimitar.")).toBeTruthy();
  });

  it("the revealed-events log: built from what the player saw change, newest first", async () => {
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    await openCombatTab();
    expect(logLines()).toEqual(["R2 The combat is on."]);
    // The DM reveals a creature, the ally drops, the goblin stands up, a new round.
    act(() => server.change("p1", (c) => ({
      ...c, turn: { kind: "combatant", id: "cr1", round: 3 },
      order: [...c.order.map((o) => (o.id === "ally1" ? { ...o, band: "down" as const } : o.id === "cr1" ? { ...o, conditions: [] } : o)),
        creature("cr2", "Wolf")],
    })));
    await waitFor(() => expect(logLines()[0]).toBe("R3 Wolf appears."));
    expect(logLines()).toEqual(["R3 Wolf appears.", "R3 Bram is down.", "R3 Goblin: no longer Prone.", "R3 Round 3.", "R2 The combat is on."]);
    // Hidden again: it leaves the order, and the log says so (what players saw stays, plan 4.4 "Hide after reveal").
    act(() => server.change("p1", (c) => ({ ...c, order: c.order.filter((o) => o.id !== "cr2") })));
    await waitFor(() => expect(logLines()[0]).toBe("R3 Wolf is gone."));
    expect(screen.queryByRole("button", { name: "Wolf stat block" })).toBeNull();
  });

  it("the log shows the newest few lines only", async () => {
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    await openCombatTab();
    for (let i = 0; i < 6; i++) {
      act(() => server.change("p1", (c) => ({ ...c, order: [...c.order, creature(`w${i}`, `Wolf ${i}`)] })));
      await waitFor(() => expect(logLines()[0]).toBe(`R2 Wolf ${i} appears.`));
    }
    act(() => server.change("p1", (c) => ({ ...c, order: c.order.filter((o) => !o.id.startsWith("w")) })));
    await waitFor(() => expect(logLines()).toHaveLength(8));
    expect(logLines()[0]).toBe("R2 Wolf 5 is gone.");
  });

  it("fromWire reads the projection, and diffEvents logs only what changed", () => {
    const a = fromWire(liveWith(), { myCharacters: [hero] });
    expect(diffEvents(a, a, 0)).toEqual([]);
    // The PC's character: the server's, else the party's only one of this account's characters.
    const w = liveWith();
    delete w.me[0]!.characterId;
    expect(fromWire(w, { myCharacters: [hero] }).combatants[1]).toMatchObject({ mine: true, characterId: hero });
    expect(fromWire(w, { myCharacters: [hero, "other"] }).combatants[1]?.characterId).toBeNull();
    // Life, conditions and exhaustion; creatures' HP and AC (D20 b); a prompt with two damage types.
    const d = liveWith({}, { life: { dying: { successes: 2, failures: 1 } }, /* an earlier draft's shape, still read */
      prompts: [{ id: "p", kind: "damage", amount: 9, parts: [{ amount: 5, damage_type: "fire" }, { amount: 4, damage_type: "cold" }], criticalHit: true }] });
    d.order[2]!.exhaustion = 2;
    const v = fromWire(d);
    expect(v.combatants[1]).toMatchObject({ life: "dying", deathSaves: { successes: 2, failures: 1 } });
    expect(v.combatants[2]!.conditions.map((c) => c.name)).toEqual(["poisoned", "exhaustion 2"]);
    expect(v.combatants[0]).toMatchObject({ hp: { current: 3, max: 7 }, ac: 15, initiative: null });
    expect(v.prompts).toEqual([{ id: "p", kind: "damage", combatant: "me1", amount: 9, damageType: "fire and cold", critical: true }]);
    expect(v.round).toBe(2);
    expect(v.title).toBe("Road ambush");
  });

  it("offline: keeps the last state with an Offline chip, and catches up when the stream is back (D44)", async () => {
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    const pane = await openCombatTab();
    server.down = true;
    act(() => server.change("p1", (c) => ({ ...c, turn: { kind: "combatant", id: "ally1", round: 3 } })));
    expect(await screen.findByText("Reconnecting…")).toBeTruthy();
    expect(within(pane).getByText("Round 2")).toBeTruthy();
    expect(tab("Combat")).toBeTruthy();
    expect(holds).toEqual(["combat"]);
    server.down = false;
    await act(async () => { server.reopen(); });
    await waitFor(() => expect(within(screen.getByTestId("combat-pane")).getByText("Round 3")).toBeTruthy());
    expect(screen.queryByText("Reconnecting…")).toBeNull();
  });

  it("a blip shows nothing; Reconnecting… only after a few seconds without an answer", async () => {
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    await openCombatTab();
    store.deviceDeps.reconnectingMs = 300;
    server.down = true;
    act(() => server.change("p1", (c) => c));
    await act(async () => { await new Promise((r) => setTimeout(r, 50)); });
    expect(screen.queryByText("Reconnecting…")).toBeNull();
    expect(await screen.findByText("Reconnecting…", undefined, { timeout: 3000 })).toBeTruthy();
  });

  it("offline for over a minute: the chip says the state may be old", async () => {
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    await openCombatTab();
    store.deviceDeps.staleMs = 300;
    server.down = true;
    act(() => server.change("p1", (c) => c));
    expect(await screen.findByText("Reconnecting…")).toBeTruthy();
    expect(await screen.findByText("Offline · may be old", undefined, { timeout: 3000 })).toBeTruthy();
  });

  it("only the parties with one of the account's characters are asked", async () => {
    server.parties.push({ id: "p2", name: "Someone else's", myCharacters: [] });
    server.combats.set("p1", null);
    await start();
    expect(server.calls.map((c) => c.path)).toEqual(["/api/parties", "/api/parties/p1/combat"]);
  });
});

describe("no stale states: order, gaps, waking, and the player's own writes", () => {
  it("a gap in pseq fetches the whole projection once and replaces what's shown", async () => {
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    const pane = await openCombatTab();
    const asked = server.calls.length;
    // Three changes the phone never heard about, then one event: one fetch, the newest state.
    const c = server.combats.get("p1")!;
    server.combats.set("p1", { ...c, pseq: c.pseq + 3, turn: { kind: "combatant", id: "ally1", round: 4 } });
    act(() => server.emit("party://combat", { party: "p1", combat: c.id, pseq: c.pseq + 3 }));
    await waitFor(() => expect(within(pane).getByText("Round 4")).toBeTruthy());
    expect(server.calls.slice(asked).map((x) => x.path)).toEqual(["/api/parties/p1/combat", "/api/combats/c-p1"]);
    // An older event after it changes nothing.
    act(() => server.emit("party://combat", { party: "p1", combat: c.id, pseq: c.pseq + 1 }));
    await act(async () => { await Promise.resolve(); });
    expect(server.calls.length).toBe(asked + 2);
  });

  it("an answer older than what's shown never takes it back", () => {
    const newer = fromWire({ ...liveWith(), pseq: 9 });
    const older = fromWire({ ...liveWith({ turn: { kind: "combatant", id: "me1", round: 2 } }), pseq: 4 });
    store.adopt(newer);
    store.adopt(older);
    expect(store.combatStore().combats[0]!.pointer).toEqual(newer.pointer);
  });

  it("waking up (the app back from the background) asks again and shows the newest", async () => {
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    const pane = await openCombatTab();
    // Missed while asleep: no event reached the phone.
    const c = server.combats.get("p1")!;
    server.combats.set("p1", { ...c, pseq: c.pseq + 1, turn: { kind: "combatant", id: "ally1", round: 5 } });
    act(() => { document.dispatchEvent(new Event("visibilitychange")); });
    await waitFor(() => expect(within(pane).getByText("Round 5")).toBeTruthy());
  });

  it("a typed initiative shows at once, and a 409 takes it back", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({}, {}, null));
    await start();
    renderPlay();
    await openCombatTab();
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => { release = r; });
    const real = server.fetch;
    combatPlayerDeps.fetch = async (session, method, path, body) => {
      if (method === "POST") await gate;
      return real(session, method, path, body);
    };
    server.refuseNext = { code: "conflict", message: "The combat isn't running." };
    const prompt = await screen.findByRole("alertdialog", { name: "Roll initiative" });
    await user.click(within(prompt).getByRole("button", { name: "Type my table roll" }));
    await user.type(within(prompt).getByLabelText("Initiative total"), "19");
    await user.click(within(prompt).getByRole("button", { name: "Send" }));
    // Optimistic: shown before the server answers.
    expect(await screen.findByLabelText("Initiative 19")).toBeTruthy();
    await act(async () => { release(); });
    // Refused: back to no initiative, with the server's words.
    await waitFor(() => expect(screen.queryByLabelText("Initiative 19")).toBeNull());
    expect(await screen.findByText("The combat isn't running.")).toBeTruthy();
    expect(screen.getByRole("alertdialog", { name: "Roll initiative" })).toBeTruthy();
  });

  it("a write through a lost signal is retried with the same requestId and done once", async () => {
    const user = userEvent.setup();
    let n = 0;
    combatPlayerDeps.requestId = () => `req-${++n}`;
    server.combats.set("p1", liveWith({}, {}, null));
    await start();
    renderPlay();
    await openCombatTab();
    server.down = true;
    store.writeDeps.sleep = async () => { server.down = false; };
    const prompt = await screen.findByRole("alertdialog", { name: "Roll initiative" });
    await user.click(within(prompt).getByRole("button", { name: "Type my table roll" }));
    await user.type(within(prompt).getByLabelText("Initiative total"), "11");
    await user.click(within(prompt).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: "Roll initiative" })).toBeNull());
    const writes = server.calls.filter((c) => c.method === "POST").map((c) => (c.body as { requestId: string }).requestId);
    expect(writes).toEqual(["req-1", "req-1"]);
    expect(server.done.size).toBe(1);
    expect(screen.getByLabelText("Initiative 11")).toBeTruthy();
  });
});

describe("the DM's own live combat", () => {
  it("the Live chip leads to the DM tracker's route", async () => {
    const seen: string[] = [];
    function Where() { seen.push(useLocation().pathname); return null; }
    server.combats.set("p1", null);
    let changed: () => void = () => {};
    let list = [{ id: "dm-c1", partyName: "My table" }];
    store.registerDmLiveCombats({ list: async () => list, watch: (f) => { changed = f; return () => {}; }, path: (id) => `/combat/${id}` });
    await start();
    render(
      <AppProviders api={api}>
        <MemoryRouter initialEntries={["/characters"]}>
          <Where />
          <LiveChip />
          <UpstreamApp />
        </MemoryRouter>
      </AppProviders>,
    );
    await userEvent.setup().click(await screen.findByRole("button", { name: /^Live$/ }));
    expect(seen).toContain("/combat/dm-c1");
    // The tracker says it ended: the chip goes.
    list = [];
    await act(async () => { changed(); await Promise.resolve(); });
    await waitFor(() => expect(screen.queryByRole("button", { name: /^Live$/ })).toBeNull());
  });
});

describe("summons and pets (controlled creatures)", () => {
  /** My PC with a wolf following it, and another player's hawk following theirs. */
  function withPets(extra: Partial<WireProjection> = {}): WireProjection {
    const w = liveWith(extra);
    const wolf = myCreature("wolf1", "Wolf", "me1", 11);
    const hawk = creature("hawk1", "Hawk", { mine: false, band: "unhurt", view: { owner_id: "ally1" } });
    w.order.splice(2, 0, wolf.row);
    w.order.push(hawk);
    w.me.push(wolf.mine);
    return w;
  }

  it("follow their owner in the turn order; other players' are there but not mine to play", async () => {
    server.combats.set("p1", withPets());
    await start();
    renderPlay();
    const pane = await openCombatTab();
    const rows = within(within(pane).getByRole("list", { name: "Turn order" })).getAllByRole("listitem").filter((li) => li.classList.contains("combat-row"));
    expect(rows.map((r) => r.querySelector(".combat-row-name")!.textContent)).toEqual(["Goblin is acting now", "Seraphine (you)", "Wolf (yours)", "Bram", "Hawk"]);
    expect(rows[2]!.classList.contains("follows")).toBe(true);
    expect(within(rows[2]!).getByText("with Seraphine")).toBeTruthy();
    expect(within(rows[4]!).getByText("with Bram")).toBeTruthy();
    // Only my wolf gets controls.
    const mineList = within(pane).getByRole("region", { name: "Your creatures" });
    expect(within(mineList).getByRole("button", { name: "Damage Wolf" })).toBeTruthy();
    expect(within(pane).queryByRole("button", { name: "Damage Hawk" })).toBeNull();
    expect(within(mineList).getByRole("status", { name: "Wolf hit points: 11 of 11" })).toBeTruthy();
  });

  it("their turn is the player's turn too", async () => {
    server.combats.set("p1", withPets());
    await start();
    renderPlay("/equipment");
    await screen.findByRole("button", { name: /Live/ });
    act(() => server.change("p1", (c) => ({ ...c, turn: { kind: "combatant", id: "wolf1", round: 2 } })));
    expect(await screen.findByRole("button", { name: /Your turn · Wolf/ })).toBeTruthy();
    expect(buzz).toHaveBeenCalledTimes(1);
    await userEvent.setup().click(screen.getByRole("button", { name: /Your turn · Wolf/ }));
    const pane = await screen.findByTestId("combat-pane");
    expect(pane.querySelector(".combat-now.yours")!.textContent).toContain("Your turn: Wolf");
  });

  it("the player hurts, heals and gives temp HP to their own creature, straight to the combat (no Apply prompt)", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", withPets());
    await start();
    renderPlay();
    const pane = await openCombatTab();
    const card = within(within(pane).getByRole("region", { name: "Your creatures" }).querySelector(".combat-creature") as HTMLElement);
    await user.type(card.getByLabelText("Wolf amount"), "4");
    await user.click(card.getByRole("button", { name: "Damage Wolf" }));
    await waitFor(() => expect(card.getByRole("status", { name: "Wolf hit points: 7 of 11" })).toBeTruthy());
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/damage",
      body: { args: { target: "wolf1", parts: [{ amount: { kind: "typed", total: 4 }, damage_type: null }], magical: false, apply_qualified: false, critical_hit: false } } });
    expect(screen.queryByRole("alertdialog", { name: /Take/ })).toBeNull();
    await user.type(card.getByLabelText("Wolf amount"), "2");
    await user.click(card.getByRole("button", { name: "Heal Wolf" }));
    await waitFor(() => expect(card.getByRole("status", { name: "Wolf hit points: 9 of 11" })).toBeTruthy());
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/heal", body: { args: { target: "wolf1", amount: { kind: "typed", total: 2 } } } });
    await user.type(card.getByLabelText("Wolf amount"), "3");
    await user.click(card.getByRole("button", { name: "Temporary hit points for Wolf" }));
    await waitFor(() => expect(card.getByRole("status", { name: "Wolf hit points: 9 of 11, 3 temporary" })).toBeTruthy());
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/grant_temp_hp", body: { args: { id: "wolf1", amount: 3 } } });
  });

  it("conditions on, conditions off, a roll, and dismiss after an app dialog", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", withPets());
    await start();
    renderPlay();
    const pane = await openCombatTab();
    const region = within(pane).getByRole("region", { name: "Your creatures" });
    await user.selectOptions(within(region).getByRole("combobox", { name: "Add a condition to Wolf" }), "prone");
    const chips = () => within(region).queryByRole("list", { name: "Wolf conditions" });
    await waitFor(() => expect(chips()?.textContent).toContain("Prone"));
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/add_condition", body: { args: { effect: { name: "Prone", target: "wolf1", conditions: ["prone"] } } } });
    await user.click(within(region).getByRole("button", { name: "Remove prone" }));
    await waitFor(() => expect(chips()).toBeNull());
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/remove_condition", body: { args: { target: "wolf1", condition: "prone" } } });
    await user.click(within(region).getByRole("button", { name: "Type my table roll" }));
    await user.type(within(region).getByLabelText("Wolf d20 total"), "13");
    await user.click(within(region).getByRole("button", { name: "Send" }));
    expect(await within(region).findByText("Last roll: d20 13 (13)")).toBeTruthy();
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/roll", body: { args: { id: "wolf1", label: "d20", roll: { d20: { kind: "typed", total: 13 } } } } });
    await user.click(within(region).getByRole("button", { name: "Dismiss Wolf" }));
    const ask = await screen.findByRole("dialog", { name: /Dismiss Wolf\?/ });
    await user.click(within(ask).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(within(pane).queryByText("Wolf (yours)")).toBeNull());
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/dismiss", body: { args: { id: "wolf1" } } });
  });

  it("Summon: a quick stat block, linked to the player's PC, right after it", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    const pane = await openCombatTab();
    await user.click(within(pane).getByRole("button", { name: "Summon…" }));
    const dialog = await screen.findByRole("dialog", { name: "Summon" });
    expect(within(dialog).getByRole("button", { name: "Summon" }).hasAttribute("disabled")).toBe(true);
    await user.type(within(dialog).getByLabelText("Name"), "Spirit Bear");
    await user.type(within(dialog).getByLabelText("Hit points"), "15");
    await user.type(within(dialog).getByLabelText("Armor class"), "13");
    await user.click(within(dialog).getByRole("button", { name: "Summon" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Summon" })).toBeNull());
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/summon", body: { args: {
      owner: "me1", combatant: { kind: "creature", label: "Spirit Bear", public_name: "Spirit Bear", hp: { current: 15, max: 15, temp: 0 }, ac: 13 } } } });
    const names = within(within(pane).getByRole("list", { name: "Turn order" })).getAllByRole("listitem")
      .filter((li) => li.classList.contains("combat-row")).map((r) => r.querySelector(".combat-row-name")!.textContent);
    expect(names.indexOf("Spirit Bear (yours)")).toBe(names.indexOf("Seraphine (you)") + 1);
  });

  it("Summon with its own initiative, typed from the table", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    const pane = await openCombatTab();
    await user.click(within(pane).getByRole("button", { name: "Summon…" }));
    const dialog = await screen.findByRole("dialog", { name: "Summon" });
    await user.type(within(dialog).getByLabelText("Name"), "Imp");
    await user.type(within(dialog).getByLabelText("Hit points"), "10");
    await user.click(within(dialog).getByRole("checkbox", { name: /Its own initiative/ }));
    await user.click(within(dialog).getByRole("button", { name: "Summon, type its roll" }));
    await user.type(within(dialog).getByLabelText("Its initiative total"), "18");
    await user.click(within(dialog).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Summon" })).toBeNull());
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/summon", body: { args: { owner: "me1", initiative: { kind: "typed", total: 18 } } } });
  });
});

describe("the character's companions at the start of a fight", () => {
  it("are offered as optional summons, and come in after the PC", async () => {
    const user = userEvent.setup();
    api = new MockApi({ aiChunkDelayMs: 1, rng: () => 0.5, sampleCompanions: true });
    api.presetSettings({ showImportNotice: false });
    hero = await seeded("Brannoc");
    const snap = await api.getSnapshot({ characterId: hero });
    const grip = snap.companions[0]!;
    server = fakeCombatServer([{ id: "p1", name: "The Company", myCharacters: [hero] }]);
    combatPlayerDeps.fetch = server.fetch;
    combatPlayerDeps.events = server.listen;
    const w = liveWith({ status: "setup", turn: { kind: "not_started" } });
    w.order[1] = { ...w.order[1]!, name: snap.name };
    server.combats.set("p1", w);
    await start();
    renderPlay();
    const pane = await openCombatTab();
    const offer = within(pane).getByRole("group", { name: "Bring your companions" });
    const names = snap.companions.map((k) => k.name);
    expect(offer.querySelector(".combat-prompt-q")!.textContent).toBe(`Bring ${names.slice(0, -1).join(", ")} and ${names.at(-1)} into the fight?`);
    await user.click(within(offer).getByRole("button", { name: `Bring ${grip.name}` }));
    // Brought: it's no longer offered; the others still are (each is optional).
    await waitFor(() => expect(within(offer).queryByRole("button", { name: `Bring ${grip.name}` })).toBeNull());
    expect(within(offer).getByRole("button", { name: `Bring ${names[1]}` })).toBeTruthy();
    expect(server.calls.at(-1)).toMatchObject({ path: "/api/combats/c-p1/summon", body: { args: {
      owner: "me1", combatant: { label: grip.name, hp: { current: grip.hp, max: grip.hp }, ac: grip.armorClass } } } });
    const rowNames = [...pane.querySelectorAll(".combat-row-name")].map((e) => e.textContent);
    expect(rowNames).toContain(`${grip.name} (yours)`);
  });
});

describe("before the PC's initiative is in", () => {
  it("the PC is in `me` only, and the sub-tab still shows with the initiative prompt", async () => {
    const w = liveWith({ status: "setup", turn: { kind: "not_started" } });
    w.order = w.order.filter((o) => o.id !== "me1");
    server.combats.set("p1", w);
    await start();
    renderPlay();
    const pane = await openCombatTab();
    expect(within(pane).getByRole("alertdialog", { name: "Roll initiative" })).toBeTruthy();
    expect(within(within(pane).getByRole("list", { name: "Turn order" })).queryByText(/\(you\)/)).toBeNull();
  });
});

describe("the server sends no characterId", () => {
  it("before initiative, the PC in `me` only is the party's one character of this account", async () => {
    const w = liveWith({ status: "setup", turn: { kind: "not_started" } });
    w.order = w.order.filter((o) => o.id !== "me1");
    delete w.me[0]!.characterId;
    server.combats.set("p1", w);
    await start();
    renderPlay();
    const pane = await openCombatTab();
    expect(within(pane).getByRole("alertdialog", { name: "Roll initiative" })).toBeTruthy();
  });
});

describe("the server's companion offers", () => {
  it("are offered at the start and summoned by their key", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({ status: "setup", turn: { kind: "not_started" } }, { offers: [{ key: "companion-1", name: "Owl", hp: 1, ac: 11 }] }));
    await start();
    renderPlay();
    const pane = await openCombatTab();
    const offer = within(pane).getByRole("group", { name: "Bring your companions" });
    await user.click(within(offer).getByRole("button", { name: "Bring Owl" }));
    await waitFor(() => expect(server.calls.at(-1)?.path).toBe("/api/combats/c-p1/summon"));
    expect(server.calls.at(-1)?.body).toMatchObject({ args: { owner: "me1", companion: "companion-1" } });
  });
});

