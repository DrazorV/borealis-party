import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { combatDeps, resetCombatDeps } from "../api/overrides/combat";
import { saveSession } from "../api/session";
import { byId } from "../combat/dm/derive";
import { execute } from "../combat/dm/mockEngine";
import { mockCombatServer, type MockServer } from "../combat/dm/mockServer";
import { memoryStorage } from "../test/memoryStorage";

/**
 * The DM tracker against the mock combat server: every button is a server write and the screen shows what the
 * server answers. SRD creatures only (combat/dm/fixtures.ts).
 */

const toasts: string[] = [];
let confirmAnswer = "confirm";
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (t: string) => { toasts.push(t); } }) }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => confirmAnswer }));

const { CombatTracker } = await import("./CombatTracker");

let server: MockServer;

function Where() {
  return <span data-testid="where">{useLocation().pathname}</span>;
}

function page(path = "/combat/c1") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes><Route path="/combat/:id" element={<><CombatTracker /><Where /></>} /></Routes>
    </MemoryRouter>,
  );
}

const typed = (total: number) => ({ kind: "typed" as const, total });
const combat = () => server.combats.get("c1")!;
const state = () => combat().view.state;
/** Puts the sample fight in order (as a DM would by typing table rolls) and starts it. */
function readyAndStart() {
  for (const [id, n] of [["knight", 18], ["dragon", 12], ["priest", 10], ["cultist1", 9], ["cultist2", 8], ["mage", 5]] as const) {
    execute(combat(), { command: "set_initiative", id, roll: typed(n) });
  }
  execute(combat(), { command: "start" });
}
const row = (id: string) => document.querySelector<HTMLElement>(`[data-combatant="${id}"]`)!;
/** The page has its first state. */
const loaded = () => waitFor(() => expect(document.querySelector("[data-combatant]")).not.toBeNull());
const card = () => screen.getByRole("region", { name: /details$/ });

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "dm1", username: "dana", email: null, admin: false, mustChangePassword: false } });
  server = mockCombatServer();
  Object.assign(combatDeps, server.deps);
  toasts.length = 0;
  confirmAnswer = "confirm";
});
afterEach(() => { resetCombatDeps(); vi.unstubAllGlobals(); });

describe("CombatTracker", () => {
  it("lists everyone by initiative; a PC without one is not in the fight yet (C1)", async () => {
    readyAndStart();
    page();
    await screen.findByText("ROUND 1");
    const ids = [...document.querySelectorAll("[data-combatant]")].map((e) => e.getAttribute("data-combatant"));
    expect(ids).toEqual(["knight", "warhorse", "pc-aria", "dragon", "priest", "cultist1", "cultist2", "mage", "pc-bram"]);
    expect(within(row("pc-bram")).getByText(/no initiative/)).toBeTruthy();
    expect(row("knight").getAttribute("aria-current")).toBe("true");
    expect(within(row("warhorse")).getByTitle(/Acts right after Knight/)).toBeTruthy();
  });

  it("starts and steps turns with a round counter (C4)", async () => {
    for (const [id, n] of [["knight", 18], ["dragon", 12], ["priest", 10], ["cultist1", 9], ["cultist2", 8], ["mage", 5]] as const) {
      execute(combat(), { command: "set_initiative", id, roll: typed(n) });
    }
    page();
    fireEvent.click(await screen.findByRole("button", { name: /Start combat/ }));
    await screen.findByText("ROUND 1");
    await waitFor(() => expect(state().current).toBe("knight"));
    fireEvent.click(screen.getByRole("button", { name: /Next turn/ }));
    await waitFor(() => expect(state().current).toBe("warhorse"));
    await screen.findByRole("heading", { name: "Warhorse's turn" });
  });

  it("rolls initiative in the app or takes a typed table roll (D21, D43)", async () => {
    page();
    await screen.findByText(/Ready to start/);
    // Typed in the roster.
    const box = within(row("mage")).getByLabelText("Mage initiative");
    fireEvent.change(box, { target: { value: "16" } });
    fireEvent.blur(box);
    await waitFor(() => expect(byId(state(), "mage")!.initiative).toBe(16));
    expect(combat().view.log.at(-1)!.roll).toMatchObject({ source: "typed", total: 16 });
    // Rolled by the server for every creature.
    fireEvent.click(screen.getByRole("button", { name: "Roll initiatives" }));
    await waitFor(() => expect(byId(state(), "dragon")!.initiative).not.toBeNull());
    expect(combat().view.log.filter((e) => e.event === "initiative" && (e.roll as { source: string }).source === "rolled").length).toBeGreaterThan(3);
  });

  it("damages with a typed amount and a type, and asks for the concentration save with its DC (C6, C11)", async () => {
    readyAndStart();
    execute(combat(), { command: "set_concentration", id: "mage", name: "Haste" });
    page();
    await loaded();
    fireEvent.click(await within(row("mage")).findByRole("button", { name: /^Mage/ }));
    const c = card();
    fireEvent.change(within(c).getByLabelText(/Damage or healing/), { target: { value: "24" } });
    fireEvent.click(within(c).getByRole("button", { name: "slashing" }));
    fireEvent.click(within(c).getByRole("button", { name: "Damage" }));
    await waitFor(() => expect(byId(state(), "mage")!.hp.current).toBe(16));
    await within(card()).findByText(/CON save DC 12/);
    // A typed failed save ends it.
    fireEvent.change(within(card()).getByLabelText("CON save: typed result"), { target: { value: "8" } });
    fireEvent.click(within(card()).getByRole("button", { name: "Enter" }));
    await waitFor(() => expect(byId(state(), "mage")!.concentration).toBeUndefined());
  });

  it("damage to a PC becomes a prompt for its player, not a change to their HP (D7)", async () => {
    readyAndStart();
    page();
    await loaded();
    fireEvent.click(await within(row("pc-aria")).findByRole("button", { name: /^Aria/ }));
    fireEvent.change(within(card()).getByLabelText(/Damage or healing/), { target: { value: "7" } });
    fireEvent.click(within(card()).getByRole("button", { name: "Ask to take" }));
    await waitFor(() => expect(combat().playerEvents.at(-1)).toMatchObject({ event: "damage_prompt", id: "pc-aria", amount: 7 }));
    expect(byId(state(), "pc-aria")!.hp.current).toBe(28);
  });

  it("ending concentration ends every effect it held (the cascade, C11/C12)", async () => {
    readyAndStart();
    for (const target of ["knight", "dragon"]) {
      execute(combat(), { command: "add_effect", effect: { name: "Haste", source: "mage", target, concentration: true, modifiers: [{ kind: "ac", value: 2 }] } });
    }
    page();
    await loaded();
    fireEvent.click(await within(row("knight")).findByRole("button", { name: /^Knight/ }));
    expect(within(card()).getByText("AC 20")).toBeTruthy();
    fireEvent.click(within(row("mage")).getByRole("button", { name: /^Mage/ }));
    await waitFor(() => expect(screen.getByRole("region", { name: "Mage details" })).toBeTruthy());
    fireEvent.click(within(card()).getByRole("button", { name: "End" }));
    await waitFor(() => expect(state().effects).toHaveLength(0));
    fireEvent.click(within(row("knight")).getByRole("button", { name: /^Knight/ }));
    await within(card()).findByText("AC 18");
  });

  it("the shared legendary pool: one pool for two creatures, refused on a member's own turn (C16)", async () => {
    readyAndStart();
    page();
    const panel = await screen.findByRole("complementary", { name: "Resources" });
    expect(within(panel).getByText("SHARED")).toBeTruthy();
    const charge = within(panel).getByRole("button", { name: /Knight: Lance Charge/ });
    expect((charge as HTMLButtonElement).disabled).toBe(true);
    expect(charge.textContent).toMatch(/Not on its own turn/);
    fireEvent.click(within(panel).getByRole("button", { name: /Adult Red Dragon: Wing Attack/ }));
    await waitFor(() => expect(state().legendary_pools.court!.remaining).toBe(1));
    expect(within(panel).getByLabelText("1 of 3 left")).toBeTruthy();
  });

  it("Use spends the economy and says why an ability can't be used (C14, C5)", async () => {
    readyAndStart();
    page();
    const c = await screen.findByRole("region", { name: "Knight details" });
    const great = within(c).getByText("Greatsword").closest("li")!;
    fireEvent.click(within(great).getByRole("button", { name: "Use" }));
    await waitFor(() => expect(byId(state(), "knight")!.economy!.action).toBe(true));
    await within(great).findByText("Action spent");
    expect(within(card()).getByRole("button", { name: /Action: spent/ })).toBeTruthy();
  });

  it("reveals and hides a creature (D20 b), and asks before hiding one players have seen", async () => {
    readyAndStart();
    page();
    await loaded();
    fireEvent.click(await within(row("dragon")).findByRole("button", { name: "Reveal Adult Red Dragon to players" }));
    await waitFor(() => expect(byId(state(), "dragon")!.visible).toBe(true));
    expect(combat().playerEvents.at(-1)).toEqual({ event: "appeared", id: "dragon" });
    fireEvent.click(within(row("dragon")).getByRole("button", { name: /^Adult Red Dragon/ }));
    confirmAnswer = "cancel";
    fireEvent.click(await within(card()).findByRole("button", { name: "Hide from players" }));
    await act(async () => { await new Promise((r) => setTimeout(r, 10)); });
    expect(byId(state(), "dragon")!.visible).toBe(true);
    confirmAnswer = "confirm";
    fireEvent.click(within(card()).getByRole("button", { name: "Hide from players" }));
    await waitFor(() => expect(byId(state(), "dragon")!.visible).toBe(false));
  });

  it("undo reverts the last write (C8)", async () => {
    readyAndStart();
    page();
    await loaded();
    fireEvent.click(await within(row("mage")).findByRole("button", { name: "5 damage to Mage" }));
    await waitFor(() => expect(byId(state(), "mage")!.hp.current).toBe(35));
    fireEvent.click(screen.getByRole("button", { name: /Undo/ }));
    await waitFor(() => expect(byId(state(), "mage")!.hp.current).toBe(40));
  });

  it("shows the no-signal state and keeps the last state on screen (D44)", async () => {
    readyAndStart();
    page();
    await screen.findByText("ROUND 1");
    server.offline = true;
    fireEvent.click(within(row("mage")).getByRole("button", { name: "5 damage to Mage" }));
    await screen.findByText(/No signal: 1 change waiting/);
    // The tap shows at once (optimistic); the server's state stays underneath until it answers.
    expect(within(row("mage")).getByText("35")).toBeTruthy();
    expect(byId(state(), "mage")!.hp.current).toBe(40);
  });

  it("a recharge rolls in the app (C15)", async () => {
    readyAndStart();
    execute(combat(), { command: "next_turn" });
    execute(combat(), { command: "next_turn" });
    execute(combat(), { command: "next_turn" });
    execute(combat(), { command: "use_ability", id: "dragon", ability: "fire_breath" });
    page();
    const c = await screen.findByRole("region", { name: "Adult Red Dragon details" });
    const breath = within(c).getByText("Fire Breath").closest("li")!;
    expect(within(breath).getByText("Needs recharge")).toBeTruthy();
    fireEvent.click(within(breath).getByRole("button", { name: "Roll in app" }));
    await waitFor(() => expect(combat().view.log.at(-1)!.event).toBe("recharge"));
    expect((combat().view.log.at(-1)!.roll as { source: string }).source).toBe("rolled");
  });

  it("exports the combat and imports it as a new combat (D45)", async () => {
    readyAndStart();
    page();
    fireEvent.click(await screen.findByRole("button", { name: "Export combat" }));
    const text = (await screen.findByLabelText("Exported combat") as HTMLTextAreaElement).value;
    expect(JSON.parse(text)).toMatchObject({ format: "borealis-party-combat", schema_version: 1 });
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!);
    fireEvent.click(screen.getByRole("button", { name: "Import combat" }));
    fireEvent.change(screen.getByLabelText("Combat JSON"), { target: { value: text } });
    fireEvent.click(screen.getByRole("button", { name: "Import as a new combat" }));
    await waitFor(() => expect(screen.getByTestId("where").textContent).toBe("/combat/imported-1"));
    expect(server.combats.get("imported-1")!.view.seq).toBe(0);
    expect(server.combats.get("c1")!.view.state.status).toBe("active");
  });

  it("shows the encounter rule as a banner, and it ends when its creature drops (C20)", async () => {
    readyAndStart();
    page();
    const banner = await screen.findByRole("region", { name: /Encounter rule/ });
    expect(within(banner).getByText("● ACTIVE")).toBeTruthy();
    execute(combat(), { command: "set_hp", id: "priest", current: 0 });
    server.notify("c1");
    await within(banner).findByText("✕ ENDED");
  });
  it("summons a quick block under its owner, nested in the roster (controlled creatures)", async () => {
    readyAndStart();
    page();
    await loaded();
    fireEvent.click(within(row("mage")).getByRole("button", { name: /^Mage/ }));
    fireEvent.click(await within(card()).findByRole("button", { name: "Summon…" }));
    const dialog = await screen.findByRole("dialog", { name: "Summon for Mage" });
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Imp" } });
    fireEvent.change(within(dialog).getByLabelText("HP"), { target: { value: "10" } });
    fireEvent.change(within(dialog).getByLabelText("AC"), { target: { value: "13" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Summon" }));
    await waitFor(() => expect(state().combatants.some((c) => c.label === "Imp" && c.owner_id === "mage")).toBe(true));
    const imp = state().combatants.find((c) => c.label === "Imp")!;
    await waitFor(() => expect(row(imp.id)).toBeTruthy());
    const ids = [...document.querySelectorAll("[data-combatant]")].map((e) => e.getAttribute("data-combatant"));
    expect(ids[ids.indexOf("mage") + 1]).toBe(imp.id);
    expect(row(imp.id).className).toMatch(/cb-depth-1/);
    expect(imp.visible).toBe(false);
  });

  it("asks what happens to a pet when its owner drops: keep, own initiative or dismiss", async () => {
    readyAndStart();
    execute(combat(), { command: "summon", owner: "mage", combatant: { id: "imp", kind: "creature", label: "Imp", public_name: "Imp", visible: false, hp: { current: 10, max: 10 }, ac: 13 } });
    page();
    await loaded();
    execute(combat(), { command: "set_hp", id: "mage", current: 0 });
    server.notify("c1");
    const dialog = await screen.findByRole("dialog", { name: "Mage is down" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Dismiss it" }));
    await waitFor(() => expect(byId(state(), "imp")!.removed).toBe(true));
    expect(screen.queryByRole("dialog", { name: "Mage is down" })).toBeNull();
  });
});

