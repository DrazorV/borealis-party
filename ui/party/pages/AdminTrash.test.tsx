import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ConfirmProvider } from "../../src/components/Confirm";
import { ToastProvider } from "../../src/components/Toast";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { purgeIn, Trash, trashLabel } from "./AdminTrash";
import { SignOutEverywhereCard, signedOutSummary } from "./SignOutEverywhere";

const BASE = "https://party.example.com";
const DAY = 24 * 60 * 60 * 1000;
const NOW = Date.UTC(2026, 10, 14);
const ITEM = {
  accountId: "a1",
  username: "mira",
  kind: "character" as const,
  characterId: "c_0123456789abcdef0123456789abcdef",
  name: "Lost Hero",
  deletedAtMs: NOW - 2 * DAY,
  purgeAtMs: NOW + 28 * DAY,
};

let requests: { method: string; path: string; body: unknown }[];
let answers: Record<string, { status: number; body: unknown }>;
const settle = () => act(() => new Promise((r) => setTimeout(r, 20)));

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: BASE, token: "session", user: { id: "admin", username: "admin", email: null, admin: true, mustChangePassword: false } });
  requests = [];
  answers = {};
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const path = url.replace(BASE, "");
    const method = init?.method ?? "GET";
    requests.push({ method, path, body: init?.body ? JSON.parse(String(init.body)) : undefined });
    const a = answers[`${method} ${path}`] ?? { status: 404, body: { code: "not_found", message: "nope" } };
    return new Response(JSON.stringify(a.body), { status: a.status, headers: { "content-type": "application/json" } });
  }));
});

afterEach(() => { vi.unstubAllGlobals(); });

const wrap = (ui: React.ReactNode) => render(<ToastProvider><ConfirmProvider>{ui}</ConfirmProvider></ToastProvider>);

it("says when a copy goes, in whole days", () => {
  expect(purgeIn(NOW + 28 * DAY, NOW)).toBe("in 28 days");
  expect(purgeIn(NOW + DAY, NOW)).toBe("in 1 day");
  expect(purgeIn(NOW + DAY / 2, NOW)).toBe("in 1 day");
  expect(purgeIn(NOW - 1, NOW)).toBe("today");
  expect(trashLabel({ ...ITEM, name: null })).toBe(ITEM.characterId);
  expect(trashLabel({ ...ITEM, name: "  " })).toBe(ITEM.characterId);
});

it("lists deleted characters and restores one after asking", async () => {
  answers["GET /api/admin/trash"] = { status: 200, body: { items: [ITEM], retentionDays: 30 } };
  answers["POST /api/admin/trash/restore"] = { status: 200, body: { characterId: ITEM.characterId, name: ITEM.name } };
  wrap(<Trash />);
  await settle();
  expect(screen.getByText("Lost Hero")).toBeTruthy();
  expect(screen.getByText(/mira/)).toBeTruthy();
  expect(screen.getByText(/Removed in \d+ days?/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Restore" }));
  await settle();
  // An app dialog first; nothing is sent until it's confirmed.
  expect(requests.filter((r) => r.method === "POST")).toEqual([]);
  answers["GET /api/admin/trash"] = { status: 200, body: { items: [], retentionDays: 30 } };
  const buttons = screen.getAllByRole("button", { name: "Restore" });
  fireEvent.click(buttons[buttons.length - 1]!);
  await settle();
  expect(requests.filter((r) => r.method === "POST")).toEqual([
    { method: "POST", path: "/api/admin/trash/restore", body: { accountId: "a1", characterId: ITEM.characterId } },
  ]);
  expect(screen.getByText("The trash is empty")).toBeTruthy();
});

it("shows an empty trash", async () => {
  answers["GET /api/admin/trash"] = { status: 200, body: { items: [], retentionDays: 30 } };
  wrap(<Trash />);
  await settle();
  expect(screen.getByText("The trash is empty")).toBeTruthy();
});

it("signs out everywhere after asking, and says what ended", async () => {
  answers["POST /api/me/sign-out-everywhere"] = { status: 200, body: { sessions: 2, apiTokens: 1, apps: 1 } };
  wrap(<SignOutEverywhereCard />);
  fireEvent.click(screen.getByRole("button", { name: "Sign out everywhere" }));
  await settle();
  expect(requests).toEqual([]);
  const buttons = screen.getAllByRole("button", { name: "Sign out everywhere" });
  fireEvent.click(buttons[buttons.length - 1]!);
  await settle();
  expect(requests).toEqual([{ method: "POST", path: "/api/me/sign-out-everywhere", body: {} }]);
  expect(screen.getByText("Signed out: 2 other sign-ins, 1 AI app, 1 API token. This device stays signed in.")).toBeTruthy();
});

it("summarises nothing to end", () => {
  expect(signedOutSummary({ sessions: 0, apiTokens: 0, apps: 0 })).toBe("Nothing else was signed in. This device stays signed in.");
  expect(signedOutSummary({ sessions: 1, apiTokens: 3, apps: 0 })).toBe("Signed out: 1 other sign-in, 0 AI apps, 3 API tokens. This device stays signed in.");
});
