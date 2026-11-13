import { act, cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAIN_TABS as UPSTREAM_TABS } from "../../src/components/shell/sections";
import { rememberDmRole, resetDmRole } from "../api/dmRole";
import { partiesDeps, resetParties } from "../api/overrides/parties";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";

// Upstream's tab bars as the party build gives them (party/vite.config.ts swaps their "./sections" for
// entry/partySections.ts); a test has no swap, so the module is answered with the party's tabs here.
vi.mock("../../src/components/shell/sections", async (importOriginal) => {
  const real = await importOriginal<typeof import("../../src/components/shell/sections")>();
  const { dmRole } = await import("../api/dmRole");
  const { activeIn, withEncounters } = await import("./encountersTab");
  return {
    ...real,
    get MAIN_TABS() { return withEncounters(real.MAIN_TABS, dmRole()); },
    activeTab: (path: string) => activeIn(withEncounters(real.MAIN_TABS, dmRole()), path),
  };
});
vi.mock("../../src/components/shell/useShellActions", () => ({ useShellActions: () => ({ snapshot: null }) }));

const { MainTabs } = await import("../../src/components/shell/Chrome");
const { BottomBar } = await import("../../src/components/shell/MobileChrome");
const { activeIn, COMBAT_TAB, ENCOUNTERS_TAB, withEncounters } = await import("./encountersTab");
const { DmOnly } = await import("../pages/EncounterWorkspace");

let rolesAsked = 0;
let serverDm = false;

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "u1", username: "u1", email: null, admin: false, mustChangePassword: false } });
  rolesAsked = 0;
  serverDm = false;
  partiesDeps.fetch = async (_s, _m, path) => {
    if (path === "/api/me/roles") { rolesAsked++; return { dm: serverDm }; }
    return Promise.reject({ code: "not_found" });
  };
});

afterEach(() => {
  cleanup();
  resetDmRole();
  resetParties();
  vi.unstubAllGlobals();
});

const tabIds = () => [...document.querySelectorAll("nav[aria-label='Main'] [data-testid^='nav-']")].map((e) => e.getAttribute("data-testid"));

describe("the main tabs", () => {
  it("add Encounters and Combat after Play for a DM, and nothing for anyone else (or unknown)", () => {
    expect(withEncounters(UPSTREAM_TABS, true).map((t) => t.id)).toEqual([...UPSTREAM_TABS.map((t) => t.id), "encounters", "combat"]);
    expect(withEncounters(UPSTREAM_TABS, false)).toBe(UPSTREAM_TABS);
    expect(withEncounters(UPSTREAM_TABS, null)).toBe(UPSTREAM_TABS);
    const tabs = withEncounters(UPSTREAM_TABS, true);
    expect(activeIn(tabs, "/encounters/p1/e1")).toBe(ENCOUNTERS_TAB);
    expect(activeIn(tabs, "/combat/c1")).toBe(COMBAT_TAB);
    expect(activeIn(tabs, "/combat")).toBe(COMBAT_TAB);
    // On phones both live under More.
    expect(ENCOUNTERS_TAB.secondary && COMBAT_TAB.secondary).toBe(true);
  });

  it("a DM sees the Encounters and Combat tabs on the first paint, from the device's last answer", () => {
    rememberDmRole(true);
    render(<MemoryRouter initialEntries={["/encounters/p1"]}><MainTabs /></MemoryRouter>);
    expect(tabIds().slice(-2)).toEqual(["nav-encounters", "nav-combat"]);
    expect(screen.getByTestId("nav-encounters").getAttribute("aria-current")).toBe("page");
    expect(screen.getByTestId("nav-encounters").getAttribute("href")).toBe("/encounters");
    expect(screen.getByTestId("nav-combat").getAttribute("href")).toBe("/combat");
  });

  it("a player sees neither, and neither does an account this device hasn't asked about yet", () => {
    render(<MemoryRouter initialEntries={["/characters"]}><MainTabs /></MemoryRouter>);
    expect(tabIds()).not.toContain("nav-encounters");
    cleanup();
    rememberDmRole(false);
    render(<MemoryRouter initialEntries={["/characters"]}><MainTabs /></MemoryRouter>);
    expect(tabIds()).not.toContain("nav-encounters");
    expect(tabIds()).not.toContain("nav-combat");
  });

  it("the phone's bottom bar keeps its five tabs and lights More on the DM tabs", () => {
    rememberDmRole(true);
    render(<MemoryRouter initialEntries={["/combat"]}><BottomBar /></MemoryRouter>);
    const items = [...document.querySelectorAll(".aur-bottombar .bb-item")].map((e) => e.textContent);
    expect(items).toEqual(["Start", "Build", "Magic", "Gear", "Play", "More"]);
    expect(screen.getByTestId("nav-more").getAttribute("data-selected")).toBe("true");
  });
});

describe("the DM-only routes", () => {
  const guarded = (path: string) => render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/encounters" element={<DmOnly><p>Workspace</p></DmOnly>} />
        <Route path="/combat" element={<DmOnly><p>Combats</p></DmOnly>} />
        <Route path="/" element={<p>Start page</p>} />
      </Routes>
    </MemoryRouter>,
  );

  it("let a DM in at once, without asking the server again", () => {
    rememberDmRole(true);
    guarded("/encounters");
    expect(screen.getByText("Workspace")).toBeTruthy();
    expect(rolesAsked).toBe(0);
  });

  it("send a player to the start page", () => {
    rememberDmRole(false);
    guarded("/combat");
    expect(screen.getByText("Start page")).toBeTruthy();
    expect(screen.queryByText("Combats")).toBeNull();
  });

  it("ask the server once when the device doesn't know, then follow it", async () => {
    serverDm = true;
    guarded("/encounters");
    expect(screen.getByText("Loading…")).toBeTruthy();
    expect(await screen.findByText("Workspace")).toBeTruthy();
    expect(rolesAsked).toBe(1);
    cleanup();
    resetDmRole();
    serverDm = false;
    guarded("/encounters");
    expect(await screen.findByText("Start page")).toBeTruthy();
  });

  it("send everyone away when the server can't say (parties off)", async () => {
    partiesDeps.fetch = async () => Promise.reject({ code: "not_found" });
    guarded("/combat");
    expect(await screen.findByText("Start page")).toBeTruthy();
  });

  it("close again when the switch goes off while open", () => {
    rememberDmRole(true);
    guarded("/encounters");
    expect(screen.getByText("Workspace")).toBeTruthy();
    act(() => rememberDmRole(false));
    expect(screen.getByText("Start page")).toBeTruthy();
  });
});
