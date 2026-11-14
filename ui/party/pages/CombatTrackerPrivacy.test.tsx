import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { combatDeps, resetCombatDeps } from "../api/overrides/combat";
import { saveSession } from "../api/session";
import { resetCastFocus } from "../combat/dm/castFocus";
import { execute } from "../combat/dm/mockEngine";
import { mockCombatServer, type MockServer } from "../combat/dm/mockServer";
import { redactCombatant } from "../combat/dm/privacy";
import type { Combatant, DmCombat } from "../combat/dm/types";
import { memoryStorage } from "../test/memoryStorage";

/**
 * The DM screen never shows a player's HP or resources (user, 2026-11-14), even when a server sends them: here the
 * server answers the FULL engine state (HP, temp HP, death saves, slots, resources, economy on both PCs and on a
 * player's wolf) and nothing of it may render.
 */

const toasts: string[] = [];
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (t: string) => { toasts.push(t); } }) }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => "confirm" }));

const { CombatTracker } = await import("./CombatTracker");

let server: MockServer;
const combat = () => server.combats.get("c1")!;
const typed = (total: number) => ({ kind: "typed" as const, total });
const row = (id: string) => document.querySelector<HTMLElement>(`[data-combatant="${id}"]`)!;
const card = () => screen.getByRole("region", { name: /details$/ });

/** Numbers no creature in the sample fight has, so finding one on screen means a player's value leaked. */
const SECRET = { aria: { current: 37, max: 45, temp: 6 }, bram: { current: 3, max: 41, temp: 0 }, wolf: { current: 19, max: 23, temp: 0 } };

function fullState() {
  const s = combat().view.state;
  const aria = s.combatants.find((c) => c.id === "pc-aria")!;
  const bram = s.combatants.find((c) => c.id === "pc-bram")!;
  Object.assign(aria, {
    hp: SECRET.aria, spell_slots: { "1": { max: 4, remaining: 2 }, "2": { max: 3, remaining: 1 } },
    resources: { ki: { name: "Ki points", pool: { max: 9, remaining: 7 } } }, economy: { action: true, bonus_action: false, reaction: true },
    pending_concentration: [{ dc: 17, damage: 33 }], concentration: { name: "Bless" },
  });
  Object.assign(bram, { initiative: 4, hp: SECRET.bram, life: { state: "dying", successes: 2, failures: 1 } });
  const wolf: Combatant = {
    ...structuredClone(s.combatants.find((c) => c.id === "cultist1")!), id: "wolf", label: "Wolf", public_name: "Wolf", visible: true,
    owner_id: "pc-aria", controller: { kind: "player", pc: "pc-aria" }, hp: SECRET.wolf, initiative_mode: "after_owner",
    spell_slots: { "1": { max: 2, remaining: 2 } },
  };
  s.combatants.push(wolf);
  s.effects.push({ id: 900, name: "Bless", source: "pc-aria", target: "pc-bram", concentration: true });
  for (const [id, n] of [["knight", 18], ["dragon", 12], ["priest", 10], ["cultist1", 9], ["cultist2", 8], ["mage", 5], ["druid", 3]] as const) {
    execute(combat(), { command: "set_initiative", id, roll: typed(n) });
  }
  execute(combat(), { command: "start" });
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "dm1", username: "dana", email: null, admin: false, mustChangePassword: false } });
  server = mockCombatServer();
  resetCastFocus();
  // A server that sends everything: the DM view with the engine's whole state, players' HP and resources included.
  const fetch = server.deps.fetch;
  Object.assign(combatDeps, server.deps, {
    fetch: async (...a: Parameters<typeof fetch>) => {
      const out = (await fetch(...a)) as DmCombat | undefined;
      return out && typeof out === "object" && "state" in out ? { ...out, state: structuredClone(combat().view.state) } : out;
    },
  });
  toasts.length = 0;
});
afterEach(() => { resetCombatDeps(); vi.unstubAllGlobals(); });

function page() {
  return render(<MemoryRouter initialEntries={["/combat/c1"]}><Routes><Route path="/combat/:id" element={<CombatTracker />} /></Routes></MemoryRouter>);
}

/** Nothing of a player's HP, death saves or resources is on the screen. */
function expectNothingPrivate() {
  const text = document.body.textContent ?? "";
  for (const hp of Object.values(SECRET)) {
    expect(text).not.toContain(`${hp.current}/${hp.max}`);
    expect(text).not.toMatch(new RegExp(`\\b${hp.max}\\b`));
  }
  for (const id of ["pc-aria", "pc-bram", "wolf"]) {
    const r = row(id);
    expect(r.querySelector(".cb-row-hp, .cb-econ-mini, [role=meter], [role=progressbar]")).toBeNull();
    expect(within(r).queryByLabelText(/hit points/)).toBeNull();
    expect(within(r).queryByRole("button", { name: /5 (damage|healing) to/ })).toBeNull();
  }
  expect(text).not.toMatch(/Death saves|Ki points|7\/9|CON save DC 17|Spell slots/);
}

describe("DM screen privacy", () => {
  it("no PC or player creature HP, death save, slot or resource renders, given a full state", async () => {
    fullState();
    page();
    await waitFor(() => expect(row("pc-aria")).toBeTruthy());
    expectNothingPrivate();
    // What the DM does see: AC, initiative, concentration and effects (Aria's Bless on Bram, as a chip).
    expect(within(row("pc-aria")).getByText("16")).toBeTruthy();
    expect(within(row("pc-aria")).getByTitle(/Concentrating on Bless/)).toBeTruthy();
    expect(within(row("pc-bram")).getByTitle("Bless, from Aria")).toBeTruthy();
    for (const id of ["pc-aria", "pc-bram", "wolf"]) {
      fireEvent.click(within(row(id)).getAllByRole("button")[0]!);
      await waitFor(() => expect(card().getAttribute("aria-label")).toContain(id === "wolf" ? "Wolf" : id === "pc-aria" ? "Aria" : "Bram"));
      expectNothingPrivate();
      expect(within(card()).queryByLabelText(/hit points/)).toBeNull();
      expect(within(card()).queryByRole("button", { name: /^(Damage|Heal|Set exact|Temp HP|−10|−5|\+5|Stabilize)$/ })).toBeNull();
      expect(within(card()).getByRole("button", { name: "Send damage" })).toBeTruthy();
    }
  });

  it("strips a player's combatant to what the DM may see", () => {
    fullState();
    const aria = redactCombatant(combat().view.state.combatants.find((c) => c.id === "pc-aria")!);
    expect(aria.hp).toEqual({ current: 0, max: 0, temp: 0 });
    for (const k of ["life", "spell_slots", "resources", "economy", "pending_concentration"] as const) expect(aria[k]).toBeUndefined();
    expect(aria.ac).toBe(16);
    expect(aria.concentration).toEqual({ name: "Bless" });
    const dragon = combat().view.state.combatants.find((c) => c.id === "dragon")!;
    expect(redactCombatant(dragon)).toBe(dragon);
  });

  it("a player's new effect says so to the DM once", async () => {
    fullState();
    page();
    await waitFor(() => expect(row("pc-aria")).toBeTruthy());
    execute(combat(), { command: "add_effect", effect: { name: "Shield of Faith", source: "pc-aria", target: "pc-aria" } });
    server.notify("c1");
    await waitFor(() => expect(toasts).toContain("Aria cast Shield of Faith: Aria."));
    server.notify("c1");
    await waitFor(() => expect(toasts.filter((t) => t.includes("Shield of Faith"))).toHaveLength(1));
  });
});
