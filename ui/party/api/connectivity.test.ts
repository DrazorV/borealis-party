import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Api } from "../../src/api/contract";
import { emptyMirror, saveMirror } from "../sync/mirror";
import { memoryStorage } from "../test/memoryStorage";
import {
  backoffMs, BACK_MS, connectivity, decideBoot, goOffline, goOnline, goSignInAgain, holdReload, isOffline, offlinePageApi, onBack, openCopies, probe,
  resetConnectivity, retryNow, safeToReload, suspectServer, watchConnection,
} from "./connectivity";
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


afterEach(() => {
  document.body.innerHTML = "";
});

/** The page shows this upstream route (a HashRouter). */
const at = (route: string) => {
  vi.stubGlobal("location", { ...window.location, hash: `#${route}`, reload });
  window.dispatchEvent(new Event("hashchange"));
};
/** Probes so far: every fetch in these tests is a probe of `/api/me`. */
const probes = () => fetched;

it("the backoff between probes grows to a cap", () => {
  expect([1, 2, 3, 4, 5, 6, 9].map(backoffMs)).toEqual([2000, 4000, 8000, 16000, 30000, 30000, 30000]);
});

it("online, a blip shows nothing and switches nothing: the probe after the failed request answers", async () => {
  vi.useFakeTimers();
  withCopy();
  await decideBoot(session, true);
  serverWatch.network?.();
  expect(connectivity().link).toBe("checking");
  await vi.advanceTimersByTimeAsync(0);
  expect(connectivity()).toMatchObject({ link: "up", waiting: null });
  await vi.advanceTimersByTimeAsync(60_000);
  expect(reload).not.toHaveBeenCalled();
});

it("online, offline only after two failed probes with a backoff between them", async () => {
  vi.useFakeTimers();
  withCopy();
  await decideBoot(session, true);
  answer = "down";
  fetched = 0;
  await expect(serverFetch(session, "POST", "/api/rpc/list_characters", {})).rejects.toMatchObject({ code: "network" });
  await vi.advanceTimersByTimeAsync(0);
  // The request and one probe failed: still only checking.
  expect(connectivity().link).toBe("checking");
  expect(reload).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1999);
  expect(reload).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  // The second probe failed too: down, and on the Start page with nothing going on the app switches offline.
  expect(connectivity().link).toBe("down");
  expect(reload).toHaveBeenCalledOnce();
});

it("online, the server gone while typing waits for the field to be left, then switches offline", async () => {
  vi.useFakeTimers();
  withCopy();
  await decideBoot(session, true);
  const field = document.body.appendChild(document.createElement("input"));
  field.focus();
  answer = "down";
  serverWatch.network?.();
  await vi.advanceTimersByTimeAsync(2000);
  expect(connectivity()).toMatchObject({ link: "down", waiting: "offline" });
  await vi.advanceTimersByTimeAsync(30_000);
  expect(reload).not.toHaveBeenCalled();
  field.blur();
  expect(reload).toHaveBeenCalledOnce();
});

it("online, the server gone with a dialog open or away from the Start page: no reload until both are over", async () => {
  vi.useFakeTimers();
  withCopy();
  await decideBoot(session, true);
  at("/sheet");
  const dialog = document.body.appendChild(document.createElement("div"));
  dialog.setAttribute("role", "dialog");
  answer = "down";
  serverWatch.network?.();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(connectivity()).toMatchObject({ link: "down", waiting: "offline" });
  expect(reload).not.toHaveBeenCalled();
  at("/characters");
  expect(reload).not.toHaveBeenCalled();
  dialog.remove();
  await vi.advanceTimersByTimeAsync(1500);
  expect(reload).toHaveBeenCalledOnce();
});

it("online, the banner's Play offline switches at once, wherever the page is", async () => {
  vi.useFakeTimers();
  withCopy();
  await decideBoot(session, true);
  at("/play");
  answer = "down";
  serverWatch.network?.();
  await vi.advanceTimersByTimeAsync(2000);
  expect(reload).not.toHaveBeenCalled();
  goOffline();
  expect(reload).toHaveBeenCalledOnce();
});

it("online, the server back before the switch takes the switch back and says so", async () => {
  vi.useFakeTimers();
  withCopy();
  await decideBoot(session, true);
  at("/sheet");
  const back = vi.fn();
  onBack(back);
  answer = "down";
  serverWatch.network?.();
  await vi.advanceTimersByTimeAsync(2000);
  expect(connectivity()).toMatchObject({ link: "down", waiting: "offline" });
  answer = 200;
  window.dispatchEvent(new Event("online"));
  await vi.advanceTimersByTimeAsync(0);
  expect(connectivity()).toMatchObject({ link: "back", waiting: null });
  expect(back).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(BACK_MS);
  expect(connectivity().link).toBe("up");
  at("/characters");
  await vi.advanceTimersByTimeAsync(10_000);
  expect(reload).not.toHaveBeenCalled();
});

it("a browser never reloads: Reconnecting… with a growing backoff, then Back online", async () => {
  vi.useFakeTimers();
  watchConnection(false);
  answer = "down";
  fetched = 0;
  window.dispatchEvent(new Event("offline"));
  await vi.advanceTimersByTimeAsync(0);
  expect(connectivity().link).toBe("checking");
  // Probes at 0, 2, 6, 14, 30, 60 and 90 seconds.
  await vi.advanceTimersByTimeAsync(90_000);
  expect(probes()).toBe(7);
  expect(connectivity()).toMatchObject({ mode: "online", link: "down", waiting: null });
  // The banner's Try now asks at once.
  answer = 200;
  retryNow();
  await vi.advanceTimersByTimeAsync(0);
  expect(connectivity().link).toBe("back");
  await vi.advanceTimersByTimeAsync(BACK_MS);
  expect(connectivity().link).toBe("up");
  expect(reload).not.toHaveBeenCalled();
});

it("flapping: a signal that comes and goes between probes never shows Reconnecting… or reloads", async () => {
  vi.useFakeTimers();
  withCopy();
  await decideBoot(session, true);
  const seen = new Set<string>();
  for (let i = 0; i < 10; i++) {
    answer = "down";
    serverWatch.network?.();
    window.dispatchEvent(new Event("offline"));
    await vi.advanceTimersByTimeAsync(0);
    seen.add(connectivity().link);
    // The signal is back before the next probe.
    answer = 200;
    await vi.advanceTimersByTimeAsync(2000);
    seen.add(connectivity().link);
  }
  expect([...seen].sort()).toEqual(["checking", "up"]);
  expect(reload).not.toHaveBeenCalled();
});

it("flapping while down: one good probe brings it back, the next loss starts over with two probes", async () => {
  vi.useFakeTimers();
  watchConnection(false);
  answer = "down";
  suspectServer();
  await vi.advanceTimersByTimeAsync(2000);
  expect(connectivity().link).toBe("down");
  answer = 200;
  await vi.advanceTimersByTimeAsync(4000);
  expect(connectivity().link).toBe("back");
  answer = "down";
  suspectServer();
  await vi.advanceTimersByTimeAsync(0);
  expect(connectivity().link).toBe("checking");
  await vi.advanceTimersByTimeAsync(2000);
  expect(connectivity().link).toBe("down");
});

it("a held page (a live view) is never reloaded on its own; letting go switches", async () => {
  vi.useFakeTimers();
  withCopy();
  await decideBoot(session, true);
  const release = holdReload("combat");
  expect(safeToReload()).toBe(false);
  answer = "down";
  serverWatch.network?.();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(connectivity().link).toBe("down");
  expect(reload).not.toHaveBeenCalled();
  release();
  expect(reload).toHaveBeenCalledOnce();
});

it("online in the app, a refused token keeps the session for offline play instead of wiping it", async () => {
  withCopy();
  await decideBoot(session, true);
  answer = 401;
  await expect(serverFetch(session, "POST", "/api/rpc/list_characters", {})).rejects.toBeTruthy();
  expect(loadSession()).toMatchObject({ token: "t", expired: true });
  expect(connectivity()).toMatchObject({ mode: "online", expired: true });
  // On the Start page with nothing going on, the page starts again offline at once.
  expect(reload).toHaveBeenCalledOnce();
});

it("online in the app, a refused token while typing waits before the page starts again offline", async () => {
  withCopy();
  await decideBoot(session, true);
  const field = document.body.appendChild(document.createElement("textarea"));
  field.focus();
  answer = 401;
  await expect(serverFetch(session, "POST", "/api/rpc/list_characters", {})).rejects.toBeTruthy();
  await expect(serverFetch(session, "POST", "/api/rpc/list_characters", {})).rejects.toBeTruthy();
  expect(connectivity()).toMatchObject({ expired: true, waiting: "again" });
  expect(reload).not.toHaveBeenCalled();
  field.blur();
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

it("offline, the server answering again reloads online on the Start page with no character open", async () => {
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

it("offline, the server back while away from the Start page or in a dialog: Back online, Sync now, no reload", async () => {
  vi.useFakeTimers();
  withCopy();
  answer = "down";
  await decideBoot(session, true);
  offlinePageApi(nullLocal());
  at("/content");
  answer = 200;
  await vi.advanceTimersByTimeAsync(20_000);
  expect(connectivity()).toMatchObject({ mode: "offline", backOnline: true, waiting: "online" });
  const dialog = document.body.appendChild(document.createElement("div"));
  dialog.setAttribute("role", "alertdialog");
  at("/characters");
  await vi.advanceTimersByTimeAsync(10_000);
  expect(reload).not.toHaveBeenCalled();
  dialog.remove();
  await vi.advanceTimersByTimeAsync(1500);
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
  expect(connectivity()).toMatchObject({ mode: "offline", backOnline: true, expired: false });
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

it("offline, a server that answers and goes again takes Back online back after two failed probes", async () => {
  vi.useFakeTimers();
  withCopy();
  answer = "down";
  await decideBoot(session, true);
  const api = offlinePageApi(nullLocal());
  await api.loadCharacter({ characterId: "c_s" });
  answer = 200;
  await vi.advanceTimersByTimeAsync(20_000);
  expect(connectivity()).toMatchObject({ backOnline: true, waiting: "online" });
  answer = "down";
  await vi.advanceTimersByTimeAsync(20_000);
  expect(connectivity().backOnline).toBe(true);
  await vi.advanceTimersByTimeAsync(20_000);
  expect(connectivity()).toMatchObject({ backOnline: false, waiting: null });
  await api.closeCharacter({ characterId: "c_s", discard: false });
  await vi.advanceTimersByTimeAsync(0);
  expect(reload).not.toHaveBeenCalled();
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
  expect(connectivity()).toMatchObject({ mode: "offline", backOnline: false, expired: true, waiting: null });
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
