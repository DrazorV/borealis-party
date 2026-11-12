import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Api } from "../../src/api/contract";
import { emptyMirror, saveMirror } from "../sync/mirror";
import { memoryStorage } from "../test/memoryStorage";
import { connectivity, decideBoot, goOnline, goSignInAgain, isOffline, offlinePageApi, openCopies, probe, resetConnectivity } from "./connectivity";
import { serverFetch, serverWatch } from "./http";
import { loadSession, saveSession, wantsSignInAgain, type ServerSession } from "./session";

const user = { id: "u1", username: "ana", email: null, admin: false, mustChangePassword: false };
const session: ServerSession = { baseUrl: "http://server", token: "t", user };
let reload: ReturnType<typeof vi.fn>;
/** What the server answers `/api/me`: a status, or "down" for no answer at all. */
let answer: number | "down";
let fetched: number;

const withCopy = () => {
  const m = emptyMirror("u1");
  m.characters.c_s = { local: "c_l", serverSha: "x", localSha: "y", localMtime: "z" };
  m.lastSyncMs = 1;
  saveMirror(m);
};
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  vi.stubGlobal("sessionStorage", memoryStorage());
  reload = vi.fn();
  vi.stubGlobal("location", { ...window.location, reload });
  answer = 200;
  fetched = 0;
  vi.stubGlobal("fetch", vi.fn(async () => {
    fetched += 1;
    if (answer === "down") throw new TypeError("Failed to fetch");
    return new Response(answer === 200 ? JSON.stringify(user) : "{}", { status: answer });
  }));
  saveSession(session);
  resetConnectivity();
});

afterEach(() => {
  resetConnectivity();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

it("a probe tells an answer, no answer and a refused token apart", async () => {
  expect(await probe(session)).toBe("ok");
  answer = 401;
  expect(await probe(session)).toBe("expired");
  answer = 503;
  expect(await probe(session)).toBe("network");
  answer = "down";
  expect(await probe(session)).toBe("network");
});

it("a probe that gets no answer in time is no answer", async () => {
  vi.stubGlobal("fetch", vi.fn((_url: string, init: RequestInit) => new Promise((_, reject) => {
    init.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
  })));
  expect(await probe(session, 10)).toBe("network");
});

it("a browser is always online and never asks", async () => {
  answer = "down";
  expect(await decideBoot(session, false)).toBe("online");
  expect(fetched).toBe(0);
});

it("the app starts online when the server answers", async () => {
  withCopy();
  expect(await decideBoot(session, true)).toBe("online");
  expect(isOffline()).toBe(false);
});

it("no answer: offline with a copy, a message without one", async () => {
  answer = "down";
  expect(await decideBoot(session, true)).toBe("no-copy");
  resetConnectivity();
  withCopy();
  expect(await decideBoot(session, true)).toBe("offline");
  expect(isOffline()).toBe(true);
  expect(document.documentElement.classList.contains("party-offline-mode")).toBe(true);
});

it("a first sync cut off part-way is no copy: no answer gives the message, not an offline app", async () => {
  const m = emptyMirror("u1");
  m.characters.c_s = { local: "c_l", serverSha: "x", localSha: "y", localMtime: "z" };
  saveMirror(m);
  answer = "down";
  expect(await decideBoot(session, true)).toBe("no-copy");
});

it("a record whose characters are gone from the device is no copy", async () => {
  answer = "down";
  withCopy();
  expect(await decideBoot(session, true, { listCharacters: async () => [] })).toBe("no-copy");
  resetConnectivity();
  const kept = [{ id: "c_l" }] as Awaited<ReturnType<Api["listCharacters"]>>;
  expect(await decideBoot(session, true, { listCharacters: async () => kept })).toBe("offline");
});

it("a refused token: offline with the copy and 'sign in again'; without a copy, signed out", async () => {
  answer = 401;
  withCopy();
  expect(await decideBoot(session, true)).toBe("offline");
  expect(loadSession()).toMatchObject({ token: "t", expired: true });
  // Next start: no probe, still offline until signing in again.
  resetConnectivity();
  fetched = 0;
  expect(await decideBoot(loadSession()!, true)).toBe("offline");
  expect(fetched).toBe(0);

  localStorage.clear();
  saveSession(session);
  resetConnectivity();
  expect(await decideBoot(session, true)).toBe("signed-out");
  expect(loadSession()).toBeNull();
});

it("online, a failed request alone does not switch: a failed probe is needed too", async () => {
  withCopy();
  await decideBoot(session, true);
  // The request failed but the server answers the probe (a blip).
  serverWatch.network?.();
  await flush();
  expect(reload).not.toHaveBeenCalled();
  // Now it is really gone.
  answer = "down";
  await expect(serverFetch(session, "POST", "/api/rpc/list_characters", {})).rejects.toMatchObject({ code: "network" });
  await flush();
  expect(reload).toHaveBeenCalledOnce();
});

it("online in the app, a refused token keeps the session for offline play instead of wiping it", async () => {
  withCopy();
  await decideBoot(session, true);
  answer = 401;
  await expect(serverFetch(session, "POST", "/api/rpc/list_characters", {})).rejects.toBeTruthy();
  expect(loadSession()).toMatchObject({ token: "t", expired: true });
  expect(reload).toHaveBeenCalledOnce();
});

it("without a copy, a refused token signs out as before", async () => {
  await decideBoot(session, true);
  answer = 401;
  await expect(serverFetch(session, "POST", "/api/rpc/list_characters", {})).rejects.toBeTruthy();
  expect(loadSession()).toBeNull();
});

/** The page's offline API over a local backend that answers every command with null. */
const nullLocal = () => new Proxy({}, { get: (_, n) => (n === "subscribe" ? () => () => {} : async () => null) }) as unknown as Api;

it("offline, the server answering again reloads online when no character is open", async () => {
  vi.useFakeTimers();
  withCopy();
  answer = "down";
  await decideBoot(session, true);
  offlinePageApi(nullLocal());
  answer = 200;
  window.dispatchEvent(new Event("online"));
  await vi.runOnlyPendingTimersAsync();
  expect(reload).toHaveBeenCalledOnce();
});

it("offline with a character open: a banner instead of a reload, and its button saves first", async () => {
  vi.useFakeTimers();
  withCopy();
  answer = "down";
  await decideBoot(session, true);
  const api = offlinePageApi(nullLocal());
  await api.loadCharacter({ characterId: "c_s" });
  answer = 200;
  await vi.advanceTimersByTimeAsync(20_000);
  expect(reload).not.toHaveBeenCalled();
  expect(connectivity()).toEqual({ mode: "offline", backOnline: true, expired: false });
  await goOnline();
  expect(api.openCharacters()).toEqual([]);
  expect(reload).toHaveBeenCalledOnce();
});

it("with the banner up, closing the character switches online on its own", async () => {
  vi.useFakeTimers();
  withCopy();
  answer = "down";
  await decideBoot(session, true);
  const api = offlinePageApi(nullLocal());
  await api.loadCharacter({ characterId: "c_s" });
  answer = 200;
  await vi.advanceTimersByTimeAsync(20_000);
  expect(connectivity().backOnline).toBe(true);
  expect(reload).not.toHaveBeenCalled();
  await api.closeCharacter({ characterId: "c_s", discard: false });
  expect(reload).toHaveBeenCalledOnce();
});

it("offline, signed out on the server meanwhile: play goes on, the banner asks for a sign-in and probing stops", async () => {
  vi.useFakeTimers();
  withCopy();
  answer = "down";
  await decideBoot(session, true);
  const api = offlinePageApi(nullLocal());
  await api.loadCharacter({ characterId: "c_s" });
  answer = 401;
  await vi.advanceTimersByTimeAsync(20_000);
  expect(connectivity()).toEqual({ mode: "offline", backOnline: false, expired: true });
  expect(loadSession()).toMatchObject({ token: "t", expired: true });
  expect(reload).not.toHaveBeenCalled();
  fetched = 0;
  await vi.advanceTimersByTimeAsync(60_000);
  expect(fetched).toBe(0);
  // The open character is the copy's, by its own id; "Sign in again" saves it and asks for the sign-in on reload.
  expect([...openCopies()]).toEqual(["c_l"]);
  await goSignInAgain();
  expect(api.openCharacters()).toEqual([]);
  expect(wantsSignInAgain()).toBe(true);
  expect(reload).toHaveBeenCalledOnce();
});
