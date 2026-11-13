import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App as UpstreamApp } from "../../../src/App";
import { AppProviders } from "../../../src/AppProviders";
import { MockApi } from "../../../src/api/mock";
import { combatPlayerDeps, resetCombatPlayer, type PlayerCombat } from "./api";
import { saveSession } from "../../api/session";
import { combatOf, creature, fakeCombatServer, pc, type FakeCombatServer } from "../../test/combatServer";
import { memoryStorage } from "../../test/memoryStorage";

/**
 * The player's Combat sub-tab (plan 4.4, 0.3.0 part F) on upstream's real Play page, with the party's Tabs standing in
 * for upstream's as the party build does (vite.config.ts REPLACED). The fake server answers the PLAYER projection.
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
const { LiveChip, YourTurnBanner } = await import("./LiveChip");
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
function liveWith(extra: Partial<PlayerCombat> = {}): PlayerCombat {
  return combatOf("p1", [
    creature("cr1", "Goblin", { initiative: 16, conditions: [{ name: "prone" }], band: "bloodied",
      statBlock: { size: ["small"], creature_type: "humanoid", ac: [{ value: 15, source: "leather armor, shield" }], hp: { average: 7, formula: "2d6" },
        abilities: { str: 8, dex: 14, con: 10, int: 10, wis: 8, cha: 8 }, actions: [{ name: "Scimitar", text: "Melee Weapon Attack: +4 to hit." }] } }),
    pc("me1", "Seraphine", hero, true, { initiative: 14 }),
    pc("ally1", "Bram", "c-bram", false, { initiative: 9, band: "bloodied", conditions: [{ name: "poisoned", remaining: 2 }] }),
  ], { round: 2, pointer: { kind: "combatant", id: "cr1", round: 2 }, ...extra });
}

function renderPlay(route = "/play") {
  try { sessionStorage.setItem("lk.currentCharacter", hero); } catch { /* ignore */ }
  return render(
    <AppProviders api={api}>
      <MemoryRouter initialEntries={[route]}>
        <LiveChip />
        <YourTurnBanner />
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

const tab = (name: string) => screen.queryByRole("tab", { name });

beforeEach(async () => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "pl1", username: "alice", email: null, admin: false, mustChangePassword: false } });
  api = new MockApi({ aiChunkDelayMs: 1, rng: () => 0.5 });
  api.presetSettings({ showImportNotice: false });
  hero = await seeded("Seraphine");
  server = fakeCombatServer([{ id: "p1", name: "The Company", myCharacters: [hero] }]);
  combatPlayerDeps.fetch = server.fetch;
  combatPlayerDeps.events = server.listen;
  buzz.mockReset();
  locks.taken = 0;
  locks.released = 0;
  store.deviceDeps.vibrate = buzz;
  store.deviceDeps.staleMs = 60_000;
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
    await user.click(screen.getByRole("tab", { name: "Combat" }));
    const pane = await screen.findByTestId("combat-pane");
    expect(within(pane).getByText("Round 2")).toBeTruthy();
    expect(within(pane).getByRole("status", { name: "" }).textContent).toContain("Goblin");
    const order = within(pane).getByRole("list", { name: "Turn order" });
    const rows = within(order).getAllByRole("listitem").filter((li) => li.classList.contains("combat-row"));
    expect(rows.map((r) => r.querySelector(".combat-row-name")!.textContent)).toEqual(["Goblin is acting now", "Seraphine (you)", "Bram"]);
    expect(rows[0]!.getAttribute("aria-current")).toBe("true");
    // Allies' health bands and the visible conditions.
    expect(within(rows[2]!).getByLabelText("Health: Bloodied")).toBeTruthy();
    expect(within(rows[2]!).getByText(/Poisoned/)).toBeTruthy();
    expect(within(rows[0]!).getByText("Prone")).toBeTruthy();
    // Another sub-tab, then back.
    await user.click(screen.getByRole("tab", { name: "Abilities" }));
    expect(screen.queryByTestId("combat-pane")).toBeNull();
    await user.click(screen.getByRole("tab", { name: "Combat" }));
    expect(await screen.findByTestId("combat-pane")).toBeTruthy();
    // The DM ends it: the sub-tab goes, and so do the reload hold and the wake lock.
    act(() => server.change("p1", (c) => ({ ...c, status: "ended", pointer: { kind: "ended" } })));
    await waitFor(() => expect(tab("Combat")).toBeNull());
    expect(screen.queryByTestId("combat-pane")).toBeNull();
    expect(holds).toEqual([]);
    expect(locks.released).toBe(1);
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
    act(() => server.change("p1", (c) => ({ ...c, pointer: { kind: "combatant", id: "me1", round: 2 } })));
    expect(await screen.findByRole("button", { name: /Your turn · Seraphine/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^Your turn$/ })).toBeTruthy();
    expect(buzz).toHaveBeenCalledTimes(1);
    expect(buzz).toHaveBeenCalledWith(store.YOUR_TURN_BUZZ);
    // The same turn again (a resync) doesn't buzz again.
    await act(async () => { server.reopen(); await store.refreshCombats(); });
    expect(buzz).toHaveBeenCalledTimes(1);
    // The banner leads to the sub-tab, which says it in its header; the banner goes there.
    await userEvent.setup().click(screen.getByRole("button", { name: /Your turn · Seraphine/ }));
    const pane = await screen.findByTestId("combat-pane");
    expect(within(pane).getByText("Your turn")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Your turn · Seraphine/ })).toBeNull();
  });

  it("keeps the turn on the last visible combatant while hidden creatures act (D5)", async () => {
    // The server's projection already stays on the last visible turn; the view shows what it says.
    server.combats.set("p1", liveWith({ pointer: { kind: "combatant", id: "ally1", round: 2 } }));
    await start();
    renderPlay();
    await userEvent.setup().click(await screen.findByRole("tab", { name: "Combat" }));
    const pane = await screen.findByTestId("combat-pane");
    expect(within(pane).getAllByRole("status")[0]!.textContent).toContain("Bram");
    expect(pane.textContent).not.toMatch(/hidden/i);
  });
});

describe("the damage prompt (D7 a)", () => {
  async function prompted() {
    server.combats.set("p1", liveWith({ prompts: [{ id: "pr1", kind: "damage", combatant: "me1", amount: 7, damageType: "slashing" }] }));
    await start();
    renderPlay();
    await userEvent.setup().click(await screen.findByRole("tab", { name: "Combat" }));
    return screen.findByRole("alertdialog", { name: "Take 7 slashing damage?" });
  }
  const hp = async () => (await api.getSnapshot({ characterId: hero })).play.hitPoints.current;

  it("Apply takes it on the player's own character, then tells the server", async () => {
    const before = await hp();
    const prompt = await prompted();
    await userEvent.setup().click(within(prompt).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: /Take 7/ })).toBeNull());
    expect(await hp()).toBe(before - 7);
    expect(server.calls.at(-1)).toEqual({ method: "POST", path: "/api/combats/c-p1/answer_prompt", body: { prompt: "pr1", answer: "applied" } });
    const pane = screen.getByTestId("combat-pane");
    expect(within(pane).getByRole("status", { name: new RegExp(`Hit points: ${before - 7} of`) })).toBeTruthy();
  });

  it("Not now leaves the character as it is", async () => {
    const before = await hp();
    const prompt = await prompted();
    await userEvent.setup().click(within(prompt).getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: /Take 7/ })).toBeNull());
    expect(await hp()).toBe(before);
    expect(server.calls.at(-1)?.body).toEqual({ prompt: "pr1", answer: "declined" });
  });
});

describe("rolls: in the app or typed from the table (D21, D43)", () => {
  it("initiative: typed", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({ status: "setup", round: 0, pointer: { kind: "not_started" },
      combatants: liveWith().combatants.map((c) => (c.id === "me1" ? { ...c, initiative: null } : c)) }));
    await start();
    renderPlay();
    await user.click(await screen.findByRole("tab", { name: "Combat" }));
    const prompt = await screen.findByRole("alertdialog", { name: "Roll initiative" });
    await user.click(within(prompt).getByRole("button", { name: "Type my table roll" }));
    const field = within(prompt).getByLabelText("Initiative total");
    expect(within(prompt).getByRole("button", { name: "Send" }).hasAttribute("disabled")).toBe(true);
    await user.type(field, "17");
    await user.click(within(prompt).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: "Roll initiative" })).toBeNull());
    expect(server.calls.at(-1)).toEqual({ method: "POST", path: "/api/combats/c-p1/set_initiative", body: { id: "me1", roll: { kind: "typed", total: 17 } } });
    expect(screen.getByLabelText("Initiative 17")).toBeTruthy();
  });

  it("initiative: in the app, with the character's modifier", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({ combatants: liveWith().combatants.map((c) => (c.id === "me1" ? { ...c, initiative: null } : c)) }));
    await start();
    renderPlay();
    await user.click(await screen.findByRole("tab", { name: "Combat" }));
    const init = (await api.getSnapshot({ characterId: hero })).stats.initiative;
    const prompt = await screen.findByRole("alertdialog", { name: "Roll initiative" });
    await user.click(within(prompt).getByRole("button", { name: "Roll in app" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: "Roll initiative" })).toBeNull());
    expect(server.calls.at(-1)?.body).toEqual({ id: "me1", roll: { kind: "roll", modifier: init, mode: "normal" } });
  });

  it("death saves: the pips, a typed save and an in-app one, then stable", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({ combatants: liveWith().combatants.map((c) => (c.id === "me1"
      ? { ...c, life: "dying", deathSaves: { successes: 1, failures: 2 }, conditions: [{ name: "unconscious" }] } : c)) }));
    await start();
    renderPlay();
    await user.click(await screen.findByRole("tab", { name: "Combat" }));
    const saves = await screen.findByRole("group", { name: "Death saves" });
    expect(within(saves).getByRole("img", { name: "Successes: 1 of 3" })).toBeTruthy();
    expect(within(saves).getByRole("img", { name: "Failures: 2 of 3" })).toBeTruthy();
    await user.click(within(saves).getByRole("button", { name: "Type my table roll" }));
    await user.type(within(saves).getByLabelText("Death save total"), "12");
    await user.click(within(saves).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(within(screen.getByRole("group", { name: "Death saves" })).getByRole("img", { name: "Successes: 2 of 3" })).toBeTruthy());
    expect(server.calls.at(-1)?.body).toEqual({ id: "me1", roll: { kind: "typed", total: 12 } });
    await user.click(within(screen.getByRole("group", { name: "Death saves" })).getByRole("button", { name: "Roll in app" }));
    await waitFor(() => expect(screen.getByText("Stable")).toBeTruthy());
    expect(server.calls.at(-1)?.body).toEqual({ id: "me1", roll: { kind: "roll", modifier: 0, mode: "normal" } });
    expect(within(screen.getByRole("group", { name: "Death saves" })).queryByRole("button", { name: "Roll in app" })).toBeNull();
  });

  it("a concentration save asked by the server", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith({ prompts: [{ id: "pr2", kind: "concentration", combatant: "me1", dc: 12 }] }));
    await start();
    renderPlay();
    await user.click(await screen.findByRole("tab", { name: "Combat" }));
    const prompt = await screen.findByRole("alertdialog", { name: "Concentration save, DC 12" });
    await user.click(within(prompt).getByRole("button", { name: "Type my table roll" }));
    await user.type(within(prompt).getByLabelText("Concentration save total"), "15");
    await user.click(within(prompt).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: /Concentration save/ })).toBeNull());
    expect(server.calls.at(-1)).toEqual({ method: "POST", path: "/api/combats/c-p1/resolve_concentration", body: { id: "me1", roll: { kind: "typed", total: 15 } } });
  });
});

describe("what else the sub-tab shows", () => {
  it("a revealed creature's stat block opens as a sheet (D20 b)", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    await user.click(await screen.findByRole("tab", { name: "Combat" }));
    await user.click(await screen.findByRole("button", { name: "Goblin stat block" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Armor Class")).toBeTruthy();
    expect(within(dialog).getByText("15 (leather armor, shield)")).toBeTruthy();
    expect(within(dialog).getByText("Scimitar.")).toBeTruthy();
  });

  it("the revealed-events log, newest first and short", async () => {
    const user = userEvent.setup();
    const log = Array.from({ length: 12 }, (_, i) => ({ round: 1 + Math.floor(i / 4), text: `Event ${i + 1}`, at: 1_700_000_000_000 + i * 1000 }));
    server.combats.set("p1", liveWith({ log }));
    await start();
    renderPlay();
    await user.click(await screen.findByRole("tab", { name: "Combat" }));
    const section = await screen.findByRole("region", { name: "What happened" });
    const items = within(section).getAllByRole("listitem").map((li) => li.textContent);
    expect(items).toHaveLength(8);
    expect(items[0]).toContain("Event 12");
    expect(items.at(-1)).toContain("Event 5");
  });

  it("offline: keeps the last state with an Offline chip, and catches up when the stream is back (D44)", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    await user.click(await screen.findByRole("tab", { name: "Combat" }));
    await screen.findByTestId("combat-pane");
    server.down = true;
    act(() => server.change("p1", (c) => ({ ...c, round: 3, pointer: { kind: "combatant", id: "ally1", round: 3 } })));
    expect(await screen.findByText("Offline")).toBeTruthy();
    const pane = screen.getByTestId("combat-pane");
    expect(within(pane).getByText("Round 2")).toBeTruthy();
    expect(tab("Combat")).toBeTruthy();
    expect(holds).toEqual(["combat"]);
    server.down = false;
    await act(async () => { server.reopen(); });
    await waitFor(() => expect(within(screen.getByTestId("combat-pane")).getByText("Round 3")).toBeTruthy());
    expect(screen.queryByText("Offline")).toBeNull();
  });

  it("offline for over a minute: the chip says the state may be old", async () => {
    const user = userEvent.setup();
    server.combats.set("p1", liveWith());
    await start();
    renderPlay();
    await user.click(await screen.findByRole("tab", { name: "Combat" }));
    await screen.findByTestId("combat-pane");
    store.deviceDeps.staleMs = 300;
    server.down = true;
    act(() => server.change("p1", (c) => c));
    expect(await screen.findByText("Offline")).toBeTruthy();
    expect(await screen.findByText("Offline · may be old", undefined, { timeout: 3000 })).toBeTruthy();
  });

  it("only the parties with one of the account's characters are asked", async () => {
    server.parties.push({ id: "p2", name: "Someone else's", myCharacters: [] });
    server.combats.set("p1", null);
    await start();
    expect(server.calls.map((c) => c.path)).toEqual(["/api/parties", "/api/parties/p1/combat"]);
  });
});
