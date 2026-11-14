import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ConfirmProvider } from "../../src/components/Confirm";
import { ToastProvider } from "../../src/components/Toast";
import type { DeletedItem } from "../api/party";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { deletedLabel, KEPT_NOTE, RecentlyDeletedCard, restoreNote } from "./RecentlyDeleted";

/** Account › Recently deleted (0.3.6): the caller's own deleted parties, encounters and characters, Restore and Delete forever. */

const BASE = "https://party.example.com";
const DAY = 24 * 60 * 60 * 1000;
const NOW = Date.now();
const PARTY: DeletedItem = { kind: "party", id: "p1", name: "The Lantern", detail: "", deletedAtMs: NOW - DAY, purgeAtMs: NOW + 29 * DAY };
const ENCOUNTER: DeletedItem = { kind: "encounter", id: "e1", name: "Bridge", detail: "The Lantern", deletedAtMs: NOW - 2 * DAY, purgeAtMs: NOW + 28 * DAY };
const CHARACTER: DeletedItem = { kind: "character", id: "c_1", name: "", detail: "", deletedAtMs: NOW - 3 * DAY, purgeAtMs: NOW + 27 * DAY };

let requests: { method: string; path: string; body: unknown }[];
let answers: Record<string, { status: number; body: unknown }>;
const settle = () => act(() => new Promise((r) => setTimeout(r, 20)));

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: BASE, token: "session", user: { id: "u1", username: "dm", email: null, admin: false, mustChangePassword: false } });
  requests = [];
  answers = {};
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const path = url.replace(BASE, "");
    const method = init?.method ?? "GET";
    requests.push({ method, path, body: init?.body ? JSON.parse(String(init.body)) : undefined });
    const a = answers[`${method} ${path}`] ?? { status: 404, body: { code: "not_found", message: "That isn't in Recently deleted (any more)." } };
    return new Response(JSON.stringify(a.body), { status: a.status, headers: { "content-type": "application/json" } });
  }));
});

afterEach(() => { vi.unstubAllGlobals(); });

const wrap = () => render(<ToastProvider><ConfirmProvider><RecentlyDeletedCard /></ConfirmProvider></ToastProvider>);

it("names rows and says what a restore brings back", () => {
  expect(deletedLabel(CHARACTER)).toBe("Character");
  expect(deletedLabel(PARTY)).toBe("The Lantern");
  expect(restoreNote(ENCOUNTER)).toBe("It goes back into The Lantern.");
  expect(restoreNote(PARTY)).toMatch(/members, invites and encounters/);
  expect(KEPT_NOTE).toMatch(/Recently deleted for 30 days/);
});

it("lists the account's deleted items with when they go", async () => {
  answers["GET /api/me/deleted"] = { status: 200, body: { items: [PARTY, ENCOUNTER, CHARACTER], retentionDays: 30 } };
  wrap();
  await settle();
  const rows = document.querySelectorAll(".bp-deleted-row");
  expect([...rows].map((r) => r.getAttribute("data-kind"))).toEqual(["party", "encounter", "character"]);
  expect(rows[1]!.textContent).toContain("Encounter in The Lantern");
  expect(rows[0]!.textContent).toContain("Removed in 29 days");
});

it("never shows more than the retention when the server's clock is ahead", async () => {
  answers["GET /api/me/deleted"] = { status: 200, body: { items: [{ ...PARTY, deletedAtMs: NOW + 5000, purgeAtMs: NOW + 30 * DAY + 5000 }], retentionDays: 30 } };
  wrap();
  await settle();
  expect(document.querySelector(".bp-deleted-row")!.textContent).toContain("Removed in 30 days");
});

it("restores after an app confirm, then reloads the list", async () => {
  answers["GET /api/me/deleted"] = { status: 200, body: { items: [ENCOUNTER], retentionDays: 30 } };
  answers["POST /api/me/deleted/restore"] = { status: 200, body: { kind: "encounter", id: "e1", name: "Bridge" } };
  wrap();
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Restore" }));
  const ask = await screen.findByRole("dialog", { name: /Restore Bridge\?/ });
  expect(ask.textContent).toContain("It goes back into The Lantern.");
  answers["GET /api/me/deleted"] = { status: 200, body: { items: [], retentionDays: 30 } };
  fireEvent.click(within(ask).getByRole("button", { name: "Restore" }));
  await settle();
  expect(requests.find((r) => r.method === "POST")).toEqual({ method: "POST", path: "/api/me/deleted/restore", body: { kind: "encounter", id: "e1" } });
  expect(screen.getByText("Nothing deleted lately")).toBeTruthy();
  expect(screen.getByText("Bridge is back.")).toBeTruthy();
});

it("Delete forever asks first (a danger confirm) and can be cancelled", async () => {
  answers["GET /api/me/deleted"] = { status: 200, body: { items: [PARTY], retentionDays: 30 } };
  answers["DELETE /api/me/deleted/party/p1"] = { status: 200, body: { deleted: true } };
  wrap();
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Delete forever" }));
  let ask = await screen.findByRole("dialog", { name: /Delete The Lantern forever\?/ });
  expect(ask.textContent).toContain("its deleted encounters");
  fireEvent.click(within(ask).getByRole("button", { name: "Cancel" }));
  await settle();
  expect(requests.some((r) => r.method === "DELETE")).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Delete forever" }));
  ask = await screen.findByRole("dialog", { name: /Delete The Lantern forever\?/ });
  fireEvent.click(within(ask).getByRole("button", { name: "Delete forever" }));
  await settle();
  expect(requests.some((r) => r.method === "DELETE" && r.path === "/api/me/deleted/party/p1")).toBe(true);
});

it("shows the server's refusal", async () => {
  answers["GET /api/me/deleted"] = { status: 200, body: { items: [ENCOUNTER], retentionDays: 30 } };
  answers["POST /api/me/deleted/restore"] = { status: 409, body: { code: "conflict", message: "Its party “The Lantern” is deleted too. Restore the party first." } };
  wrap();
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Restore" }));
  const ask = await screen.findByRole("dialog", { name: /Restore Bridge\?/ });
  fireEvent.click(within(ask).getByRole("button", { name: "Restore" }));
  await settle();
  expect(screen.getByText(/Restore the party first/)).toBeTruthy();
});
