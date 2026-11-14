import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { combatDeps, resetCombatDeps } from "../api/overrides/combat";
import { saveSession } from "../api/session";
import { byId } from "../combat/dm/derive";
import { execute } from "../combat/dm/mockEngine";
import { answerPrompt, mockCombatServer, type MockServer } from "../combat/dm/mockServer";
import { resetCastFocus } from "../combat/dm/castFocus";
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
  resetCastFocus();
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
    expect(ids).toEqual(["knight", "warhorse", "pc-aria", "dragon", "priest", "cultist1", "cultist2", "mage", "druid", "pc-bram"]);
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
    expect(combat().view.log.filter((e) => e.event === "initiative" && (e.roll as { source: string }).source === "app").length).toBeGreaterThan(3);
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
    fireEvent.change(within(card()).getByLabelText(/Amount for Aria/), { target: { value: "7" } });
    fireEvent.change(within(card()).getByLabelText("Damage type"), { target: { value: "slashing" } });
    fireEvent.click(within(card()).getByRole("button", { name: "Send damage" }));
    await waitFor(() => expect(combat().playerEvents.at(-1)).toMatchObject({ event: "damage_prompt", id: "pc-aria", amount: 7 }));
    expect(byId(state(), "pc-aria")!.hp.current).toBe(28);
    // Delivered, then accepted once the player takes it; never their HP.
    const sent = await within(card()).findByRole("list", { name: "Sent to Aria" });
    await within(sent).findByText(/Delivered/);
    answerPrompt(combat(), "prompt-" + (combat().playerEvents.length - 1), true);
    server.notify("c1");
    await within(card()).findByText("Accepted");
    expect(card().textContent).not.toMatch(/28/);
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

  it("legendary actions per creature: each has its own count on its row and in the panel (C16)", async () => {
    readyAndStart();
    page();
    const panel = await screen.findByRole("complementary", { name: "Resources" });
    const dragon = within(panel).getByRole("region", { name: "Adult Red Dragon legendary actions" });
    const knight = within(panel).getByRole("region", { name: "Knight legendary actions" });
    expect(within(dragon).getByLabelText("Adult Red Dragon: 3 of 3 left")).toBeTruthy();
    expect(within(knight).getByLabelText("Knight: 2 of 2 left")).toBeTruthy();
    expect(within(panel).queryByText("LINKED")).toBeNull();
    // The knight's turn: its own option is refused, the dragon's is not.
    const charge = within(knight).getByRole("button", { name: /Lance Charge/ });
    expect((charge as HTMLButtonElement).disabled).toBe(true);
    expect(charge.textContent).toMatch(/Not on its own turn/);
    fireEvent.click(within(dragon).getByRole("button", { name: /Wing Attack/ }));
    await waitFor(() => expect(state().legendary_pools.dragon!.remaining).toBe(1));
    expect(state().legendary_pools.knight!.remaining).toBe(2);
    await within(dragon).findByLabelText("Adult Red Dragon: 1 of 3 left");
    expect(within(knight).getByLabelText("Knight: 2 of 2 left")).toBeTruthy();
    // The roster rows show each creature's own pips.
    expect(within(row("dragon")).getByLabelText("Adult Red Dragon legendary actions: 1 of 3 left").textContent).toBe("LA ●○○");
    expect(within(row("knight")).getByLabelText("Knight legendary actions: 2 of 2 left").textContent).toBe("LA ●●");
    expect(within(row("mage")).queryByText(/^LA /)).toBeNull();
  });

  it("a guessed count is marked, and the DM sets each creature's own count (C16)", async () => {
    combat().view.state.legendary_pools.knight = { max: 3, remaining: 3, members: ["knight"], guessed: true };
    readyAndStart();
    page();
    const panel = await screen.findByRole("complementary", { name: "Resources" });
    const knight = within(panel).getByRole("region", { name: "Knight legendary actions" });
    const dragon = within(panel).getByRole("region", { name: "Adult Red Dragon legendary actions" });
    expect(within(knight).getByText("GUESS")).toBeTruthy();
    expect(within(dragon).queryByText("GUESS")).toBeNull();
    fireEvent.click(within(knight).getByRole("button", { name: "Knight: one fewer legendary action per round" }));
    await waitFor(() => expect(state().legendary_pools.knight).toMatchObject({ max: 2, remaining: 2 }));
    expect(state().legendary_pools.knight!.guessed).toBeFalsy();
    expect(state().legendary_pools.dragon).toMatchObject({ max: 3, remaining: 3 });
    await within(knight).findByLabelText("Knight: 2 of 2 left");
    expect(within(knight).queryByText("GUESS")).toBeNull();
  });

  it("a pool the DM linked shows once, as LINKED (C16, opt-in)", async () => {
    execute(combat(), { command: "set_legendary_pool", pool: "heads", value: { max: 3, remaining: 3, members: ["dragon", "knight"] } });
    readyAndStart();
    page();
    const panel = await screen.findByRole("complementary", { name: "Resources" });
    const linked = within(panel).getByRole("region", { name: "Adult Red Dragon + Knight legendary actions" });
    expect(within(linked).getByText("LINKED")).toBeTruthy();
    expect(within(linked).getByRole("button", { name: /Knight: Lance Charge/ })).toBeTruthy();
    expect(within(panel).getAllByText(/· Legendary actions/)).toHaveLength(1);
  });

  it("a turn button spends the economy and says why an ability can't be used (C14, C5; 0.3.2)", async () => {
    readyAndStart();
    page();
    await screen.findByRole("region", { name: "Knight details" });
    const great = () => document.querySelector<HTMLButtonElement>('[data-turn-button="greatsword"]')!;
    expect(great().textContent).toMatch(/Greatsword\+5 · 2d6\+3/);
    fireEvent.click(great());
    const sheet = await screen.findByRole("dialog", { name: "Knight: Greatsword" });
    fireEvent.click(within(sheet).getByRole("button", { name: "Aria" }));
    fireEvent.click(within(sheet).getByRole("button", { name: "Attack" }));
    await waitFor(() => expect(byId(state(), "knight")!.economy!.action).toBe(true));
    // The attack waits for the table roll: its resolution sheet opens.
    await screen.findByRole("dialog", { name: "Greatsword · Knight" });
    fireEvent.click(screen.getByRole("button", { name: "Later" }));
    await waitFor(() => expect(great().disabled).toBe(true));
    expect(great().closest("li")!.textContent).toMatch(/Action spent/);
    expect(within(card()).getByRole("button", { name: "Action: spent" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "1 pending" })).toBeTruthy();
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
    const breath = c.querySelector<HTMLElement>('[data-turn-button="fire_breath"]')!.closest("li")!;
    expect(breath.textContent).toMatch(/Fire Breath 5–6/);
    expect(within(breath).getByText("Needs recharge")).toBeTruthy();
    fireEvent.click(within(c).getByRole("button", { name: "Roll for me" }));
    await waitFor(() => expect(combat().view.log.at(-1)!.event).toBe("recharge"));
    expect((combat().view.log.at(-1)!.roll as { source: string }).source).toBe("app");
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
  it("has no Summon button: Add creature does that job (summon spells come in 0.3.2)", async () => {
    readyAndStart();
    page();
    await loaded();
    fireEvent.click(within(row("mage")).getByRole("button", { name: /^Mage/ }));
    await within(card()).findByRole("button", { name: "Rename" });
    expect(within(card()).queryByRole("button", { name: /Summon/ })).toBeNull();
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

