import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { combatDeps, resetCombatDeps } from "../api/overrides/combat";
import { saveSession } from "../api/session";
import { fightOver } from "../combat/dm/fightOver";
import { execute } from "../combat/dm/mockEngine";
import { mockCombatServer, type MockServer } from "../combat/dm/mockServer";
import { memoryStorage } from "../test/memoryStorage";

/** Ending a combat (0.3.2): End combat in the header, the "All enemies are down" hint, and the way back. */

let phone = false;
const asked: string[] = [];
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: () => {} }) }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async (o: { title: string }) => { asked.push(o.title); return "confirm"; } }));
vi.mock("../../src/lib/phone", async (orig) => ({ ...(await orig<object>()), usePhone: () => phone }));

const { CombatTracker } = await import("./CombatTracker");

let server: MockServer;
const typed = (total: number) => ({ kind: "typed" as const, total });
const combat = () => server.combats.get("c1")!;
const state = () => combat().view.state;
const HOSTILE = ["knight", "warhorse", "dragon", "priest", "cultist1", "cultist2", "mage", "druid"];

function Where() { return <span data-testid="where">{useLocation().pathname}</span>; }
function page() {
  return render(<MemoryRouter initialEntries={["/combat/c1"]}><Routes>
    <Route path="/combat/:id" element={<CombatTracker />} />
    <Route path="*" element={<Where />} />
  </Routes></MemoryRouter>);
}
function start() {
  for (const [id, n] of [["knight", 18], ["dragon", 12], ["priest", 10], ["cultist1", 9], ["cultist2", 8], ["mage", 5], ["druid", 4]] as const) {
    execute(combat(), { command: "set_initiative", id, roll: typed(n) });
  }
  execute(combat(), { command: "start" });
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "dm1", username: "dana", email: null, admin: false, mustChangePassword: false } });
  server = mockCombatServer();
  Object.assign(combatDeps, server.deps);
  phone = false;
  asked.length = 0;
});
afterEach(() => { resetCombatDeps(); vi.unstubAllGlobals(); });

describe("ending a combat", () => {
  it("End combat is in the header (with the same confirm), and the ended note leads back to the encounter", async () => {
    start();
    page();
    const header = await screen.findByRole("banner");
    fireEvent.click(within(header).getByRole("button", { name: "End combat" }));
    await waitFor(() => expect(state().status).toBe("ended"));
    expect(asked).toEqual(["End this combat?"]);
    const note = await screen.findByRole("status", { name: "Combat ended" });
    expect(within(header).queryByRole("button", { name: "End combat" })).toBeNull();
    fireEvent.click(within(note).getByRole("link", { name: "Back to encounters" }));
    expect(screen.getByTestId("where").textContent).toBe("/encounters/p1/e1");
  });

  it("the log panel no longer hides it", async () => {
    start();
    page();
    const tools = (await screen.findByText("Log, export and import")).closest("details")!;
    expect(within(tools).queryByRole("button", { name: "End combat" })).toBeNull();
  });

  it("when every enemy is down it offers to end, never ends by itself, and Keep going hides the hint", async () => {
    start();
    for (const id of HOSTILE.slice(1)) execute(combat(), { command: "remove_combatant", id });
    page();
    await screen.findByRole("banner");
    expect(screen.queryByRole("status", { name: "All enemies are down" })).toBeNull();
    execute(combat(), { command: "set_hp", id: "knight", current: 0 });
    server.notify("c1");
    const hint = await screen.findByRole("status", { name: "All enemies are down" });
    expect(state().status).toBe("active");
    fireEvent.click(within(hint).getByRole("button", { name: "Keep going" }));
    await waitFor(() => expect(screen.queryByRole("status", { name: "All enemies are down" })).toBeNull());
    expect(state().status).toBe("active");
  });

  it("the hint's End combat ends it", async () => {
    start();
    for (const id of HOSTILE) execute(combat(), { command: "remove_combatant", id });
    page();
    fireEvent.click(within(await screen.findByRole("status", { name: "All enemies are down" })).getByRole("button", { name: "End combat" }));
    await waitFor(() => expect(state().status).toBe("ended"));
  });

  it("on a phone End combat is in the header's More menu", async () => {
    phone = true;
    start();
    page();
    const header = await screen.findByRole("banner");
    expect(within(header).queryByRole("button", { name: "End combat" })).toBeNull();
    fireEvent.click(within(header).getByRole("button", { name: "⋯ More" }));
    fireEvent.click(within(screen.getByRole("menu", { name: "More combat actions" })).getByRole("menuitem", { name: "End combat" }));
    await waitFor(() => expect(state().status).toBe("ended"));
  });
});

describe("fightOver", () => {
  it("counts a PC's creatures with the party; never says the party is down (the DM never sees players' HP)", () => {
    start();
    const s = structuredClone(state());
    for (const c of s.combatants) if (HOSTILE.includes(c.id)) c.removed = true;
    s.combatants.push({ ...s.combatants.find((c) => c.id === "knight")!, id: "wolf", removed: false, owner_id: "pc-aria", controller: { kind: "player", pc: "pc-aria" } });
    expect(fightOver(s)).toBe("enemies");
    const t = structuredClone(state());
    for (const c of t.combatants) if (c.kind === "character") c.life = { state: "dying", successes: 0, failures: 0 };
    for (const c of t.combatants) if (c.kind === "character") c.hp.current = 0;
    expect(fightOver(t)).toBeNull();
    expect(fightOver(state())).toBeNull();
  });
});
