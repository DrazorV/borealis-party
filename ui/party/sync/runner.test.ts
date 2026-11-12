import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { resetConnectivity, safeToReload, suspectServer, watchConnection } from "../api/connectivity";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { requestSync, startSync, useSyncStatus, type SyncStatus } from "./runner";
import type { LocalApi, ServerCall } from "./sync";

/** The Sync chip's state, read the way the account bar reads it. */
let status: () => SyncStatus;
vi.mock("react", async (real) => ({
  ...(await real<typeof import("react")>()),
  useSyncExternalStore: (_subscribe: unknown, get: () => SyncStatus) => { status = get; return get(); },
}));

/** The app's backend with nothing on it: every list is empty, every change does nothing. */
const local = new Proxy({}, { get: () => async () => [] }) as unknown as LocalApi;
let reachable: boolean;
let calls: number;
/** A server with no characters or packs; with `reachable` false every call gets no answer. */
const server: ServerCall = async (command) => {
  calls += 1;
  if (!reachable) throw { code: "network", message: "The server can't be reached. Check your connection." };
  if (command === "party_sync_state") return { nowMs: Date.now(), characters: [], packs: [] };
  if (command === "get_settings") return {};
  return [];
};

let stop: () => void = () => {};
beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  vi.stubGlobal("sessionStorage", memoryStorage());
  vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("Failed to fetch"); }));
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "u1", username: "a", email: null, admin: false, mustChangePassword: false } });
  resetConnectivity();
  reachable = true;
  calls = 0;
  useSyncStatus();
});

afterEach(() => {
  stop();
  resetConnectivity();
  vi.unstubAllGlobals();
});

it("no signal is no sync problem: the Sync chip keeps what it said", async () => {
  stop = startSync({ local, server, userId: "u1" });
  await requestSync();
  expect(status()).toMatchObject({ state: "synced", waiting: 0 });
  reachable = false;
  await requestSync();
  expect(status()).toMatchObject({ state: "synced", waiting: 0 });
});

it("a real refusal still shows as a sync problem", async () => {
  stop = startSync({ local, server: async () => { throw { code: "internal", message: "Disk full." }; }, userId: "u1" });
  await requestSync();
  expect(status()).toEqual({ state: "problem", message: "Disk full." });
});

it("while Reconnecting… shows, a failed sync leaves the chip alone too", async () => {
  watchConnection(false);
  suspectServer();
  stop = startSync({ local, server: async () => { throw { code: "internal", message: "The server answered 502." }; }, userId: "u1" });
  await requestSync();
  expect(status().state).not.toBe("problem");
});

it("a sync that is running holds off any reload", async () => {
  let release: (() => void) | undefined;
  const gate = new Promise<void>((r) => { release = r; });
  const slow: ServerCall = async (command, args) => {
    if (command === "party_sync_state") await gate;
    return server(command, args);
  };
  stop = startSync({ local, server: slow, userId: "u1" });
  await Promise.resolve();
  expect(safeToReload()).toBe(false);
  release!();
  await requestSync();
  expect(safeToReload()).toBe(true);
});
