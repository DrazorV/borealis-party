import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { combatDeps, resetCombatDeps } from "../api/overrides/combat";
import { saveSession } from "../api/session";
import { byId } from "../combat/dm/derive";
import { execute } from "../combat/dm/mockEngine";
import { mockCombatServer, type MockServer } from "../combat/dm/mockServer";
import { openCasts } from "../combat/dm/spellTypes";
import { resetCastFocus } from "../combat/dm/castFocus";
import { memoryStorage } from "../test/memoryStorage";

/**
 * 0.3.2 in the DM tracker, against the mock combat server: the turn panel's buttons, the cast sheet, the resolution
 * sheet with typed table rolls first, the summon panel and Add creature, and the effect builder. SRD names only.
 */

const toasts: string[] = [];
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (t: string) => { toasts.push(t); } }) }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => "confirm" }));

const { CombatTracker } = await import("./CombatTracker");

let server: MockServer;
const typed = (total: number) => ({ kind: "typed" as const, total });
const combat = () => server.combats.get("c1")!;
const state = () => combat().view.state;

function page() {
  return render(<MemoryRouter initialEntries={["/combat/c1"]}><Routes><Route path="/combat/:id" element={<CombatTracker />} /></Routes></MemoryRouter>);
}
/** Starts the sample fight and steps to `who`'s turn. */
function turnOf(who: string) {
  for (const [id, n] of [["knight", 18], ["dragon", 12], ["priest", 10], ["cultist1", 9], ["cultist2", 8], ["mage", 5], ["druid", 4]] as const) {
    execute(combat(), { command: "set_initiative", id, roll: typed(n) });
  }
  execute(combat(), { command: "start" });
  while (state().current !== who) execute(combat(), { command: "next_turn" });
}
const turnButton = (key: string) => document.querySelector<HTMLButtonElement>(`[data-turn-button="${key}"]`)!;

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "dm1", username: "dana", email: null, admin: false, mustChangePassword: false } });
  server = mockCombatServer();
  resetCastFocus();
  Object.assign(combatDeps, server.deps);
  toasts.length = 0;
});
afterEach(() => { resetCombatDeps(); vi.unstubAllGlobals(); });

describe("spells and mechanics in the tracker (0.3.2)", () => {
  it("casts Fireball at L4: slot chips with a live preview, capped targets, then typed table rolls with Enter to the next row", async () => {
    turnOf("mage");
    page();
    await screen.findByRole("region", { name: "Mage details" });
    expect(turnButton("fireball").textContent).toContain("Fireball L3+");
    fireEvent.click(turnButton("fireball"));
    const sheet = await screen.findByRole("dialog", { name: "Cast Fireball" });
    // Only the legal slots: 3, 4 and 5.
    expect(within(sheet).getAllByRole("radio").map((r) => r.textContent?.slice(0, 2))).toEqual(["L3", "L4", "L5"]);
    fireEvent.click(within(sheet).getByRole("radio", { name: /L4/ }));
    expect(sheet.querySelector("[data-preview]")!.textContent).toBe("9d6 fire at L4 · DEX save, half on a success · 20-ft sphere");
    fireEvent.click(within(sheet).getByRole("button", { name: "Knight" }));
    fireEvent.click(within(sheet).getByRole("button", { name: "Cultist 1" }));
    fireEvent.click(within(sheet).getByRole("button", { name: "Aria" }));
    fireEvent.click(within(sheet).getByRole("button", { name: "Cast" }));
    await waitFor(() => expect(byId(state(), "mage")!.spell_slots!["4"]!.remaining).toBe(2));

    const res = await screen.findByRole("dialog", { name: "Fireball · Mage" });
    // 0.3.5: a save is an outcome: roll at the table, press Saved or Failed (no number box).
    expect(within(res).queryByLabelText("Knight: DEX save: typed result")).toBeNull();
    fireEvent.click(within(res.querySelector<HTMLElement>('[data-target="knight"]')!).getByRole("button", { name: "Saved" }));
    await waitFor(() => expect(openCasts(state())[0]!.rows[0]!.outcome).toBe("saved"));
    fireEvent.click(within(res.querySelector<HTMLElement>('[data-target="cultist1"]')!).getByRole("button", { name: "Failed" }));
    // A player's row waits for the player; the DM can press their result.
    const aria = res.querySelector<HTMLElement>('[data-target="pc-aria"]')!;
    expect(within(aria).getByText(/Waiting for Aria's player/)).toBeTruthy();
    fireEvent.click(within(aria).getByRole("button", { name: "Failed" }));
    await waitFor(() => expect(openCasts(state())[0]!.rows.every((r) => r.outcome)).toBe(true));
    // One damage roll, typed: half for the knight, full for the others.
    fireEvent.change(within(res).getByLabelText("Fireball damage: typed total"), { target: { value: "30" } });
    fireEvent.click(within(res).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(byId(state(), "knight")!.hp.current).toBe(37));
    expect(byId(state(), "cultist1")!.life?.state).toBe("dead");
    expect(combat().playerEvents.at(-1)).toMatchObject({ event: "damage_prompt", id: "pc-aria", amount: 30 });
    expect(screen.queryByRole("button", { name: /pending/ })).toBeNull();
  });

  it("Roll for me and Roll all creature saves are there for people without dice", async () => {
    turnOf("dragon");
    page();
    await screen.findByRole("region", { name: "Adult Red Dragon details" });
    fireEvent.click(turnButton("fire_breath"));
    const sheet = await screen.findByRole("dialog", { name: "Adult Red Dragon: Fire Breath" });
    for (const n of ["Knight", "Cultist 1", "Cultist 2"]) fireEvent.click(within(sheet).getByRole("button", { name: n }));
    fireEvent.click(within(sheet).getByRole("button", { name: "Use" }));
    const res = await screen.findByRole("dialog", { name: "Fire Breath · Adult Red Dragon" });
    fireEvent.click(within(res).getByRole("button", { name: /Roll all creature saves \(3\)/ }));
    await waitFor(() => expect(openCasts(state())[0]!.rows.every((r) => r.outcome && r.roll?.source === "app")).toBe(true));
    fireEvent.click(within(res).getByRole("button", { name: "Roll for me" }));
    await waitFor(() => expect(openCasts(state())).toHaveLength(0));
  });

  it("Later keeps the cast: the pending badge reopens it", async () => {
    turnOf("mage");
    page();
    await screen.findByRole("region", { name: "Mage details" });
    fireEvent.click(turnButton("hold_person"));
    const sheet = await screen.findByRole("dialog", { name: "Cast Hold Person" });
    fireEvent.click(within(sheet).getByRole("button", { name: "Cultist 2" }));
    // Capped at one target at L2.
    expect((within(sheet).getByRole("button", { name: "Knight" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(within(sheet).getByRole("button", { name: "Cast" }));
    fireEvent.click(within(await screen.findByRole("dialog", { name: "Hold Person · Mage" })).getByRole("button", { name: "Later" }));
    fireEvent.click(await screen.findByRole("button", { name: "1 pending" }));
    expect(await screen.findByRole("dialog", { name: "Hold Person · Mage" })).toBeTruthy();
  });

  it("Mage Armor: the AC chip shows why", async () => {
    turnOf("mage");
    page();
    await screen.findByRole("region", { name: "Mage details" });
    fireEvent.click(turnButton("mage_armor"));
    const sheet = await screen.findByRole("dialog", { name: "Cast Mage Armor" });
    fireEvent.click(within(sheet).getByRole("button", { name: "Mage" }));
    fireEvent.click(within(sheet).getByRole("button", { name: "Cast" }));
    await waitFor(() => expect(document.querySelector(".cb-ac")!.textContent).toBe("AC 15 = Mage Armor 13 + DEX 2"));
  });

  it("a summon spell spawns nothing: its options open Add creature pre-filtered, bound to the caster's concentration", async () => {
    turnOf("druid");
    page();
    await screen.findByRole("region", { name: "Druid details" });
    fireEvent.click(turnButton("conjure_animals"));
    fireEvent.click(within(await screen.findByRole("dialog", { name: "Cast Conjure Animals" })).getByRole("button", { name: "Cast" }));
    const panel = await screen.findByRole("status", { name: "Conjure Animals options" });
    const before = state().combatants.length;
    expect(within(panel).getAllByRole("button").map((b) => b.textContent)).toContain("8 × CR 1/4 beasts…");
    fireEvent.click(within(panel).getByRole("button", { name: "2 × CR 1 beasts…" }));
    const add = await screen.findByRole("dialog", { name: "Add creatures: Conjure Animals" });
    expect((within(add).getByLabelText("CR up to") as HTMLInputElement).value).toBe("1");
    expect((within(add).getByLabelText("Controlled by") as HTMLSelectElement).value).toBe("druid");
    expect((within(add).getByRole("checkbox", { name: /Remove when Druid's concentration on Conjure Animals ends/ }) as HTMLInputElement).checked).toBe(true);
    fireEvent.click(await within(add).findByRole("button", { name: /Brown Bear/ }));
    expect(within(add).queryByRole("button", { name: /Giant Elk/ })).toBeNull();
    fireEvent.click(within(add).getByRole("button", { name: "Add 2 Brown Bear" }));
    await waitFor(() => expect(state().combatants.length).toBe(before + 2));
    const bears = state().combatants.filter((c) => c.label.startsWith("Brown Bear"));
    expect(bears.every((b) => b.owner_id === "druid")).toBe(true);
    execute(combat(), { command: "end_concentration", id: "druid" });
    expect(state().combatants.filter((c) => c.label.startsWith("Brown Bear") && !c.removed)).toHaveLength(0);
  });

  it("the Spells tab shows each spell's coverage, and Define effect saves a definition to the account", async () => {
    turnOf("mage");
    page();
    const card = await screen.findByRole("region", { name: "Mage details" });
    fireEvent.click(within(card).getByRole("button", { name: "Spells" }));
    const row = (key: string) => card.querySelector<HTMLElement>(`[data-spell="${key}"]`)!;
    expect(within(row("fireball")).getByText("Auto")).toBeTruthy();
    expect(within(row("haste")).getByText("Partial")).toBeTruthy();
    expect(within(row("counterspell")).getByText("Ruling")).toBeTruthy();
    expect(await within(row("cone_of_cold")).findByText("Suggested")).toBeTruthy();
    expect(within(row("arcane_word")).getByText("Unlinked")).toBeTruthy();
    fireEvent.click(within(row("misty_step")).getByRole("button", { name: "Define effect…" }));
    const b = await screen.findByRole("dialog", { name: "Define effect: Misty Step" });
    fireEvent.click(await within(b).findByRole("button", { name: /Damage \+ save/ }));
    fireEvent.change(within(b).getByLabelText("Dice"), { target: { value: "2d6" } });
    fireEvent.change(within(b).getByLabelText("Damage type"), { target: { value: "psychic" } });
    expect(b.querySelector("[data-preview]")!.textContent).toContain("2d6 psychic at L2");
    fireEvent.click(within(b).getByRole("button", { name: "Save effect" }));
    await waitFor(() => expect(server.spells.rows.find((r) => r.spellKey === "ID_SRD_SPELL_MISTY_STEP")).toMatchObject({
      kind: "def", ruleset: "dnd2024", origin: "dm", def: { v: 1, name: "Misty Step", level: 2, on_land: [{ kind: "damage", dice: "2d6", damage_type: "psychic" }] },
    }));
  });

  it("a Suggested spell opens its tag draft with fields marked check", async () => {
    turnOf("mage");
    page();
    const card = await screen.findByRole("region", { name: "Mage details" });
    fireEvent.click(within(card).getByRole("button", { name: "Spells" }));
    fireEvent.click(await within(card.querySelector<HTMLElement>('[data-spell="cone_of_cold"]')!).findByRole("button", { name: "Check and confirm…" }));
    const b = await screen.findByRole("dialog", { name: "Define effect: Cone of Cold" });
    await waitFor(() => expect((within(b).getByLabelText("Dice") as HTMLInputElement).value).toBe("8d8+(slot_above)d8"));
    expect(within(b).getAllByText("check").length).toBeGreaterThan(0);
    fireEvent.click(within(b).getByRole("button", { name: "Save effect" }));
    await waitFor(() => expect(server.spells.rows.find((r) => r.spellKey === "ID_SRD_SPELL_CONE_OF_COLD")).toMatchObject({ origin: "suggested", sourceHash: "mockhash" }));
    expect(within(b).getByRole("button", { name: "60 ft" })).toBeTruthy();
  });

  it("effect chips: End, Dismiss spell, and the It attacked: end reminder", async () => {
    turnOf("mage");
    execute(combat(), { command: "add_effect", effect: { name: "Invisibility", source: "mage", target: "mage", ends_on_attack: true, spell: { key: "ID_SRD_SPELL_INVISIBILITY", level: 2 } } });
    page();
    const card = await screen.findByRole("region", { name: "Mage details" });
    const fx = card.querySelector<HTMLElement>("[data-effect]")!;
    expect(within(fx).getByRole("button", { name: "End Invisibility" })).toBeTruthy();
    expect(within(fx).getByRole("button", { name: "Dismiss spell" })).toBeTruthy();
    fireEvent.click(within(fx).getByRole("button", { name: "It attacked: end" }));
    await waitFor(() => expect(state().effects.find((e) => e.name === "Invisibility")).toBeUndefined());
  });
});
