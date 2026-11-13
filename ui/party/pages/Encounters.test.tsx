import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { sequenceDie } from "../../src/lib/dice";
import type { ServerEvent } from "../api/http";
import { encountersDeps, nested, resetEncounters, rollFormula, validFormula, type Creature } from "../api/overrides/encounters";
import { partiesDeps, resetParties } from "../api/overrides/parties";
import { saveSession } from "../api/session";
import { fakeEncountersServer, type FakeEncounters } from "../test/encountersServer";
import { memoryStorage } from "../test/memoryStorage";

const state = vi.hoisted(() => ({ confirm: "confirm" as string, toasts: [] as { text: string; tone?: string }[] }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => state.confirm }));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (text: string, tone?: string) => state.toasts.push({ text, tone }) }) }));

const { EncountersPage, EncounterTeaser } = await import("./EncountersPage");
const { EncounterBuilder } = await import("./EncounterBuilder");

let server: FakeEncounters;
let listeners: ((e: ServerEvent) => void)[] = [];

function signIn(id: string) {
  server.me = id;
  saveSession({ baseUrl: "http://server", token: "t", user: { id, username: id, email: null, admin: false, mustChangePassword: false } });
}

function page(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/parties/:party/encounters" element={<EncountersPage />} />
        <Route path="/parties/:party/encounters/:encounter" element={<EncounterBuilder />} />
        <Route path="/parties" element={<EncounterTeaser partyId="p1" dm={server.me === "dm1"} />} />
      </Routes>
    </MemoryRouter>,
  );
}

const dialog = () => screen.getByRole("dialog");

/** The DM makes an encounter from the list page; answers its id. */
async function makeEncounter(name = "Goblin ambush") {
  page("/parties/p1/encounters");
  fireEvent.click(await screen.findByRole("button", { name: /New encounter/ }));
  fireEvent.change(within(dialog()).getByLabelText("Encounter name"), { target: { value: name } });
  fireEvent.click(within(dialog()).getByRole("button", { name: "Create" }));
  await screen.findByRole("heading", { name });
  return server.encounters[0]!.id;
}

/** Pick creatures in the open picker and add them. */
async function pickAndAdd(title: string, names: [string, number][]) {
  const picker = await screen.findByRole("dialog", { name: title });
  for (const [name, count] of names) {
    const pick = await within(picker).findByRole("button", { name: `Pick ${name}` });
    for (let i = 0; i < count; i++) fireEvent.click(pick);
  }
  const total = names.reduce((n, [, c]) => n + c, 0);
  fireEvent.click(within(picker).getByRole("button", { name: total === 1 ? "Add 1 creature" : `Add ${total} creatures` }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: title })).toBeNull());
}

async function addFromPicker(names: [string, number][]) {
  fireEvent.click(within(screen.getByRole("region", { name: "Creatures" })).getByRole("button", { name: /Add creatures/ }));
  await pickAndAdd("Add creatures", names);
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  server = fakeEncountersServer();
  encountersDeps.fetch = server.fetch;
  listeners = [];
  partiesDeps.events = (onEvent) => { listeners.push(onEvent); return () => { listeners = listeners.filter((l) => l !== onEvent); }; };
  partiesDeps.fetch = async (_s, _m, path) => {
    if (path === "/api/parties/p1") return { id: "p1", name: "Tuesday Party", ruleset: "dnd2024", role: "dm", dmName: "dana", memberCount: 1, myCharacters: [], createdAt: 1, updatedAt: 1, members: [] };
    return Promise.reject({ code: "not_found" });
  };
  state.toasts = [];
  state.confirm = "confirm";
  signIn("dm1");
});

afterEach(() => {
  resetEncounters();
  resetParties();
  vi.unstubAllGlobals();
});

describe("encounters list (DM)", () => {
  it("creates a draft and opens the builder", async () => {
    await makeEncounter("Goblin ambush");
    expect(server.calls.some((c) => c.method === "POST" && c.path === "/api/parties/p1/encounters")).toBe(true);
    expect(screen.getByRole("radio", { name: "Draft" }).getAttribute("aria-checked")).toBe("true");
    expect(screen.getByRole("switch", { name: "Shown to players" }).getAttribute("aria-checked")).toBe("false");
    expect(screen.getByText("Rules: the party's edition (2024).")).toBeTruthy();
  });

  it("lists encounters with their creatures and status, and no XP or difficulty", async () => {
    await makeEncounter("Goblin ambush");
    await addFromPicker([["Goblin Warrior", 4]]);
    cleanup();
    page("/parties/p1/encounters");
    const row = await screen.findByRole("link", { name: "Open Goblin ambush" });
    expect(row.textContent).toContain("4 creatures (4 hidden)");
    expect(within(row).getByText("Draft")).toBeTruthy();
    expect(row.textContent).not.toMatch(/XP|Low|Moderate|High/);
  });
});

describe("encounter builder (DM)", () => {
  it("adds creatures from the picker, hidden, and counts them in the bar", async () => {
    await makeEncounter();
    await addFromPicker([["Goblin Warrior", 4], ["Owlbear", 1]]);
    const add = server.calls.filter((c) => c.path.endsWith("/creatures") && c.method === "POST");
    expect(add.map((c) => c.body)).toEqual([{ elementId: "ID_SRD521_MONSTER_GOBLIN_WARRIOR", count: 4 }, { elementId: "ID_SRD521_MONSTER_OWLBEAR", count: 1 }]);
    const bar = screen.getByRole("status", { name: "Encounter summary" });
    expect(bar.textContent).toContain("5 creatures");
    expect(bar.textContent).toContain("none revealed");
    expect(bar.textContent).not.toContain("XP");
    expect(screen.getAllByText("Hidden from players")).toHaveLength(5);
    expect(state.toasts.at(-1)?.text).toBe("5 creatures added, hidden from players.");
  });

  it("filters the picker with chips, monsters and companions, and shows no XP", async () => {
    await makeEncounter();
    fireEvent.click(within(screen.getByRole("region", { name: "Creatures" })).getByRole("button", { name: /Add creatures/ }));
    const picker = await screen.findByRole("dialog", { name: "Add creatures" });
    await within(picker).findByRole("button", { name: "Pick Owlbear" });
    expect(picker.textContent).not.toContain("XP");
    fireEvent.click(within(picker).getByRole("button", { name: "Monstrosity" }));
    await waitFor(() => expect(within(picker).queryByRole("button", { name: "Pick Goblin Warrior" })).toBeNull());
    expect(server.calls.filter((c) => c.path === "/api/bestiary/search").at(-1)!.body).toMatchObject({ types: ["monstrosity"], ruleset: "dnd2024" });
    fireEvent.click(within(picker).getByRole("button", { name: "Monstrosity" }));
    fireEvent.click(within(picker).getByRole("button", { name: "2024 rules" }));
    fireEvent.click(within(picker).getByRole("button", { name: "Companions" }));
    const owl = await within(picker).findByRole("button", { name: "Pick Owl" });
    expect(owl.closest("li")!.textContent).toContain("Companion");
    expect(server.calls.filter((c) => c.path === "/api/bestiary/search").at(-1)!.body).toMatchObject({ kinds: ["companion"] });
    fireEvent.change(within(picker).getByLabelText("Search creatures"), { target: { value: "owl" } });
    await waitFor(() => expect((server.calls.filter((c) => c.path === "/api/bestiary/search").at(-1)!.body as { text?: string }).text).toBe("owl"));
  });

  it("says so when the server has no creature search", async () => {
    server.noBestiary = true;
    await makeEncounter();
    fireEvent.click(within(screen.getByRole("region", { name: "Creatures" })).getByRole("button", { name: /Add creatures/ }));
    expect(await within(await screen.findByRole("dialog", { name: "Add creatures" })).findByText("This server can't search creatures yet.")).toBeTruthy();
  });

  it("reveals a creature: players then see it with its stat block", async () => {
    await makeEncounter();
    await addFromPicker([["Owlbear", 1]]);
    const row = screen.getByRole("listitem", { name: "Owlbear" });
    expect(within(row).queryByRole("radiogroup")).toBeNull();
    fireEvent.click(within(row).getByRole("switch", { name: "Owlbear: shown to players" }));
    await waitFor(() => expect(within(row).getByText('Players see "Monstrosity" and its stat block')).toBeTruthy());
    expect(server.calls.filter((c) => c.method === "PATCH").map((c) => c.body)).toEqual([{ hidden: false }]);
    expect(screen.getByRole("status", { name: "Encounter summary" }).textContent).toContain("1 revealed");
  });

  it("gives a villain a pet, nested under it, acting right after it", async () => {
    await makeEncounter();
    await addFromPicker([["Goblin Warrior", 1]]);
    fireEvent.click(screen.getByRole("button", { name: "More for Goblin Warrior" }));
    fireEvent.click(within(await screen.findByRole("dialog", { name: "Goblin Warrior" })).getByRole("button", { name: "Add a pet or summon" }));
    await pickAndAdd("Pets and summons for Goblin Warrior", [["Owl", 1]]);
    const add = server.calls.filter((c) => c.path.endsWith("/creatures") && c.method === "POST").at(-1)!;
    expect(add.body).toEqual({ elementId: "ID_TEST_COMPANION_OWL", count: 1, ownerId: server.creatures.get(server.encounters[0]!.id)![0]!.id });
    const owl = screen.getByRole("listitem", { name: "Owl" });
    expect(owl.className).toContain("is-pet");
    expect(owl.textContent).toContain("Acts right after Goblin Warrior");
    expect(owl.textContent).toContain("Companion");
    const rows = screen.getAllByRole("listitem").filter((li) => li.classList.contains("enc-creature")).map((li) => li.getAttribute("aria-label"));
    expect(rows).toEqual(["Goblin Warrior", "Owl"]);
    // Its own initiative instead.
    fireEvent.click(within(owl).getByRole("button", { name: "Edit" }));
    const d = await screen.findByRole("dialog", { name: "Owl" });
    fireEvent.click(within(d).getByRole("radio", { name: "Its own initiative" }));
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.getByRole("listitem", { name: "Owl" }).textContent).toContain("Its own initiative"));
    expect(server.calls.filter((c) => c.method === "PATCH").at(-1)!.body).toMatchObject({ initMode: "own" });
  });

  it("starting HP: roll the hit dice in the app or type a real roll (D21)", async () => {
    encountersDeps.die = sequenceDie([6, 6, 6, 6, 6, 6, 6]);
    await makeEncounter();
    await addFromPicker([["Owlbear", 1]]);
    fireEvent.click(within(screen.getByRole("listitem", { name: "Owlbear" })).getByRole("button", { name: "Edit" }));
    const d = await screen.findByRole("dialog", { name: "Owlbear" });
    fireEvent.click(within(d).getByRole("radio", { name: "A number I type" }));
    fireEvent.click(within(d).getByRole("button", { name: "Roll 7d10+21" }));
    expect((within(d).getByLabelText("Hit points it starts with") as HTMLInputElement).value).toBe("63");
    expect(within(d).getByText("Rolled 7d10+21: 63")).toBeTruthy();
    // A typed total wins.
    fireEvent.change(within(d).getByLabelText("Hit points it starts with"), { target: { value: "70" } });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Owlbear" })).toBeNull());
    expect(server.calls.filter((c) => c.method === "PATCH").at(-1)!.body).toMatchObject({ hpMode: "fixed", hpMax: 70 });
  });

  it("shows the encounter to players only after the app's own confirm", async () => {
    await makeEncounter();
    state.confirm = "cancel";
    fireEvent.click(screen.getByRole("switch", { name: "Shown to players" }));
    await waitFor(() => expect(server.calls.some((c) => c.method === "PATCH")).toBe(false));
    state.confirm = "confirm";
    fireEvent.click(screen.getByRole("switch", { name: "Shown to players" }));
    await waitFor(() => expect(screen.getByRole("switch", { name: "Shown to players" }).getAttribute("aria-checked")).toBe("true"));
    expect(server.calls.filter((c) => c.method === "PATCH").at(-1)!.body).toEqual({ status: "ready", playerVisible: true });
  });

  it("changes a creature's numbers and keeps the printed ones beside them", async () => {
    await makeEncounter();
    await addFromPicker([["Owlbear", 1]]);
    fireEvent.click(within(screen.getByRole("listitem", { name: "Owlbear" })).getByRole("button", { name: "Edit" }));
    const d = await screen.findByRole("dialog", { name: "Owlbear" });
    expect(within(d).getByText("Hit points (printed 59)")).toBeTruthy();
    fireEvent.change(within(d).getByLabelText("Hit dice"), { target: { value: "lots" } });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(await within(d).findByText("Hit dice look like 4d10+8.")).toBeTruthy();
    fireEvent.change(within(d).getByLabelText("Hit dice"), { target: { value: "" } });
    fireEvent.change(within(d).getByLabelText("Hit points"), { target: { value: "80" } });
    fireEvent.change(within(d).getByLabelText("Your note"), { target: { value: "Tougher one" } });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Owlbear" })).toBeNull());
    const body = server.calls.filter((c) => c.method === "PATCH").at(-1)!.body as { overrides: Record<string, unknown> };
    expect(body.overrides.hp).toEqual({ average: 80 });
    expect(body.overrides.notes).toBe("Tougher one");
    expect(screen.getByText("Changed")).toBeTruthy();
  });

  it("asks which value wins when a refresh finds a conflict", async () => {
    await makeEncounter();
    await addFromPicker([["Owlbear", 1]]);
    fireEvent.click(screen.getByRole("button", { name: "More for Owlbear" }));
    fireEvent.click(within(await screen.findByRole("dialog", { name: "Owlbear" })).getByRole("button", { name: "Refresh stat block" }));
    const d = await screen.findByRole("dialog", { name: "Refresh Owlbear" });
    const refresh = within(d).getByRole("button", { name: "Refresh" }) as HTMLButtonElement;
    expect(refresh.disabled).toBe(true);
    fireEvent.click(within(d).getByRole("radio", { name: /Keep mine/ }));
    fireEvent.click(refresh);
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Refresh Owlbear" })).toBeNull());
    expect(server.calls.at(-1)!.body).toEqual({ choices: { hp: "keep" } });
  });

  it("adds a starting effect and an encounter rule, shown as a banner", async () => {
    await makeEncounter();
    await addFromPicker([["Goblin Warrior", 2]]);
    fireEvent.click(within(screen.getByRole("region", { name: "Starting effects" })).getByRole("button", { name: /Add/ }));
    let d = await screen.findByRole("dialog", { name: "Starting effect" });
    fireEvent.change(within(d).getByLabelText("Effect"), { target: { value: "Mage Armor" } });
    fireEvent.change(within(d).getByLabelText("Spell slot spent"), { target: { value: "1" } });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    await screen.findByText("Mage Armor");
    expect(server.calls.at(-1)!.body).toMatchObject({ name: "Mage Armor", slotLevel: 1, creatureId: expect.any(String) });
    fireEvent.click(within(screen.getByRole("region", { name: "Encounter rules" })).getByRole("button", { name: /Add/ }));
    d = await screen.findByRole("dialog", { name: "Encounter rule" });
    fireEvent.change(within(d).getByLabelText("Rule"), { target: { value: "While the leader stands" } });
    fireEvent.change(within(d).getByLabelText("What it does"), { target: { value: "The others get a third attack." } });
    fireEvent.click(within(d).getByRole("button", { name: "Switch an ability" }));
    fireEvent.change(within(d).getByLabelText("Switch 1: ability"), { target: { value: "Third Attack" } });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    const banner = await screen.findByRole("note", { name: "While the leader stands" });
    expect(banner.textContent).toContain("The others get a third attack.");
    expect(banner.textContent).toContain("Ends when Goblin Warrior 1 drops.");
    expect(server.calls.at(-1)!.body).toMatchObject({ endsWhen: "creature_down", flags: [{ ability: "Third Attack", whileActive: "on" }] });
  });

  it("refetches when another device of the DM's changes it", async () => {
    const id = await makeEncounter("First");
    server.encounters[0]!.name = "Renamed elsewhere";
    act(() => { for (const l of listeners) l({ event: "party://party", payload: { party: "p1", pseq: 9, kind: "encounters", id } } as ServerEvent); });
    expect(await screen.findByRole("heading", { name: "Renamed elsewhere" })).toBeTruthy();
  });

  it("a player opening the builder's address gets nothing of a draft", async () => {
    const id = await makeEncounter("Secret");
    cleanup();
    signIn("pl1");
    page(`/parties/p1/encounters/${id}`);
    expect(await screen.findByText("This encounter isn't there any more.")).toBeTruthy();
    expect(screen.queryByText("Secret")).toBeNull();
  });
});

describe("players", () => {
  it("see 'Next:' only once the DM shows an encounter, and only revealed creatures, with their stat blocks", async () => {
    await makeEncounter("DM's own name");
    await addFromPicker([["Goblin Warrior", 1], ["Owlbear", 1]]);
    const owlbear = server.creatures.get(server.encounters[0]!.id)!.find((c) => c.name === "Owlbear")!;
    owlbear.hidden = false;
    owlbear.publicName = "Big beast";
    cleanup();
    signIn("pl1");
    const first = page("/parties");
    await waitFor(() => expect(server.calls.some((c) => c.path === "/api/parties/p1/encounters" && c.method === "GET")).toBe(true));
    expect(screen.queryByText(/Next:/)).toBeNull();
    first.unmount();
    Object.assign(server.encounters[0]!, { status: "ready", playerVisible: true, publicName: "Goblin ambush" });
    page("/parties");
    const next = await screen.findByRole("link", { name: "Next: Goblin ambush" });
    expect(next.textContent).not.toContain("DM's own name");
    page("/parties/p1/encounters");
    const card = await screen.findByLabelText("Goblin ambush");
    await within(card).findByText("Big beast");
    expect(card.textContent).not.toContain("Goblin Warrior");
    fireEvent.click(within(card).getByRole("button", { name: "Stat block of Big beast" }));
    const sheet = await screen.findByRole("article", { name: "Big beast" });
    expect(sheet.textContent).toContain("Armor Class");
    expect(sheet.textContent).not.toContain("XP");
  });

  it("the DM's party card links to the encounters", () => {
    page("/parties");
    expect(screen.getByRole("link", { name: "Encounters" }).getAttribute("href")).toBe("/parties/p1/encounters");
  });
});

describe("helpers", () => {
  it("checks hit dice like the server", () => {
    for (const ok of ["4d10+8", "1d6", "26d20+234", "3d6-1", "2d100"]) expect(validFormula(ok)).toBe(true);
    for (const bad of ["d6", "0d6", "4d0", "lots", "4d10+", "100d6"]) expect(validFormula(bad)).toBe(false);
  });

  it("rolls a formula with the app's die, at least 1", () => {
    encountersDeps.die = sequenceDie([1]);
    expect(rollFormula("1d4-3")).toBe(1);
    expect(rollFormula("lots")).toBeNull();
  });

  it("nests controlled creatures under their owner", () => {
    const c = (id: string, ownerId: string | null = null) => ({ id, ownerId }) as Creature;
    expect(nested([c("a"), c("p", "a"), c("b"), c("q", "a")]).map((g) => [g.creature.id, g.pets.map((p) => p.id)]))
      .toEqual([["a", ["p", "q"]], ["b", []]]);
  });
});
