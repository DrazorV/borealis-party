import { act, cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { MAIN_TABS as UPSTREAM_TABS } from "../../src/components/shell/sections";
import { rememberDmRole, resetDmRole } from "../api/dmRole";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import * as partySections from "./partySections";
import { useLocation } from "./shellRouter";

const signIn = (id: string) =>
  saveSession({ baseUrl: "http://server", token: "t", user: { id, username: id, email: null, admin: false, mustChangePassword: false } });

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  signIn("u1");
  partySections.refreshMainTabs();
});

afterEach(() => {
  cleanup();
  resetDmRole();
  vi.unstubAllGlobals();
});

const ids = () => partySections.MAIN_TABS.map((t) => t.id);

it("follows the DM switch: the live binding the shell's tab bars read", () => {
  expect(partySections.MAIN_TABS).toBe(UPSTREAM_TABS);
  rememberDmRole(true);
  expect(ids()).toEqual([...UPSTREAM_TABS.map((t) => t.id), "encounters", "combat"]);
  expect(partySections.activeTab("/encounters/p1").id).toBe("encounters");
  expect(partySections.activeTab("/combat/c1").id).toBe("combat");
  rememberDmRole(false);
  expect(partySections.MAIN_TABS).toBe(UPSTREAM_TABS);
  expect(partySections.activeTab("/encounters").id).toBe("start");
  // Everything else is upstream's.
  expect(partySections.START_SUBNAV.length).toBeGreaterThan(0);
});

it("each account on a device has its own answer", () => {
  rememberDmRole(true);
  signIn("u2");
  partySections.refreshMainTabs();
  expect(ids()).not.toContain("encounters");
  signIn("u1");
  partySections.refreshMainTabs();
  expect(ids()).toContain("encounters");
});

it("the shell redraws when the switch changes (shellRouter's useLocation listens)", () => {
  function Probe() {
    useLocation();
    return <span data-testid="tabs">{partySections.MAIN_TABS.map((t) => t.id).join(",")}</span>;
  }
  render(<MemoryRouter><Probe /></MemoryRouter>);
  expect(screen.getByTestId("tabs").textContent).not.toContain("encounters");
  act(() => rememberDmRole(true));
  expect(screen.getByTestId("tabs").textContent).toContain("encounters,combat");
  act(() => rememberDmRole(false));
  expect(screen.getByTestId("tabs").textContent).not.toContain("encounters");
});
