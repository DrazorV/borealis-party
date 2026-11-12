import { useSyncExternalStore } from "react";
import type { Api } from "../../src/api/contract";
import { createTauriApi } from "../../src/api/tauri";
import { isTauri } from "../../src/lib/tauri";
import { hasCopy, loadMirror } from "../sync/mirror";
import { serverWatch } from "./http";
import { createOfflineApi, type OfflineApi } from "./offline";
import { askSignInAgain, clearSession, loadSession, markExpired, type ServerSession } from "./session";

/**
 * Online or offline, decided when the page starts and kept until it reloads: one page never mixes the server and the
 * app's own backend. The apps go offline when the server can't be reached and they have a copy of the account
 * (sync/); a browser always stays online.
 *
 * Losing the signal on a phone never reloads the page under the user (plan M6):
 * - a failed request or a dropped event stream only starts probing. The server counts as gone after two failed probes
 *   with a backoff between them (`link: "down"`, the banner says "Reconnecting…"). Probing goes on with a growing
 *   backoff, and at once when the OS says the network is back or the app comes to the front, until it answers
 *   (`"back"`, then `"up"`);
 * - a switch between online and offline needs a reload, and it waits until that disturbs nobody: the Start page,
 *   nothing being typed, no dialog, no open character (offline) and no {@link holdReload} (live party and combat
 *   views). Until then the banner offers it (`waiting`).
 */

/** What one request to the server found: it answered, it could not be reached, or it refused the token. */
export type Reach = "ok" | "network" | "expired";

const PROBE_MS = 4000;
/** Offline: how often the app asks whether the server is back. */
const RECONNECT_MS = 20 * 1000;
/** Online: the wait after the first failed probe, doubled after each one, up to the cap. */
const BACKOFF_MS = 2000;
const BACKOFF_CAP_MS = 30 * 1000;
/** Failed probes in a row before the server counts as gone (and, offline, before "Back online" is taken back). */
const FAILS_DOWN = 2;
/** How long "Back online" stays up. */
export const BACK_MS = 4000;
/** A reload that waits for a safe moment looks again this often (and at once on a blur, a route change, a close). */
const SAFE_POLL_MS = 1500;

/** `GET /api/me`: can this device reach the server, and does it still take the token? */
export async function probe(session: Pick<ServerSession, "baseUrl" | "token">, timeoutMs = PROBE_MS): Promise<Reach> {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), timeoutMs);
  try {
    const response = await fetch(`${session.baseUrl}/api/me`, { headers: { authorization: `Bearer ${session.token}` }, signal: abort.signal });
    if (response.status === 401) return "expired";
    // A proxy in front of a server that is down.
    if (response.status === 502 || response.status === 503 || response.status === 504) return "network";
    return "ok";
  } catch {
    return "network";
  } finally {
    clearTimeout(timer);
  }
}

/** The wait before the next probe after `fails` failed ones in a row: 2, 4, 8, 16, then 30 seconds. */
export function backoffMs(fails: number): number {
  return Math.min(BACKOFF_MS * 2 ** Math.max(0, fails - 1), BACKOFF_CAP_MS);
}

/** How the page starts: `no-copy` when the server can't be reached and there is nothing to play offline with. */
export type Boot = "online" | "offline" | "no-copy" | "signed-out";

/**
 * The server as an online page sees it: `up`; `checking` (a request failed and a probe is out; nothing is shown yet,
 * so a blip shows nothing); `down` (two probes failed: "Reconnecting…"); `back` (it answers again after `down`: "Back
 * online" for {@link BACK_MS}).
 */
export type Link = "up" | "checking" | "down" | "back";

/**
 * A reload that waits for {@link safeToReload}: `offline` (online page, the server is gone and the app has a copy),
 * `online` (offline page, the server is back), `again` (the server signed this device out; the next page plays offline
 * with "sign in again").
 */
export type Switch = "offline" | "online" | "again";

export interface ConnectivityState {
  mode: "online" | "offline";
  /** Offline, and the server answers again: the banner offers "Sync now" until the switch happens. */
  backOnline: boolean;
  /** The server no longer takes the session (signed out there, or the account was turned off). */
  expired: boolean;
  /** Online: whether the server can be reached right now. Offline it stays `up`; `backOnline` says the rest. */
  link: Link;
  /** A switch that waits for a moment that disturbs nobody; the banner offers to do it now. */
  waiting: Switch | null;
}

const START: ConnectivityState = { mode: "online", backOnline: false, expired: false, link: "up", waiting: null };
let state: ConnectivityState = START;
const watchers = new Set<() => void>();
let offlineApi: OfflineApi | null = null;

function publish(next: Partial<ConnectivityState>): void {
  const merged = { ...state, ...next };
  if ((Object.keys(merged) as (keyof ConnectivityState)[]).every((k) => merged[k] === state[k])) return;
  state = merged;
  watchers.forEach((w) => w());
}

export function useConnectivity(): ConnectivityState {
  return useSyncExternalStore((w) => { watchers.add(w); return () => watchers.delete(w); }, () => state);
}

export const connectivity = (): ConnectivityState => state;
export const isOffline = (): boolean => state.mode === "offline";
/** Online, and the server can't be reached right now (or is being checked after a failure). */
export const isReconnecting = (): boolean => state.mode === "online" && (state.link === "checking" || state.link === "down");

/** Run when the server answers again after "Reconnecting…" (the sync catches up, event streams reopen). */
const backHandlers = new Set<() => void>();
export function onBack(handler: () => void): () => void {
  backHandlers.add(handler);
  return () => { backHandlers.delete(handler); };
}

/**
 * Keep the page from reloading on its own while held: live party and combat views (plan 4.4) and a sync that is
 * writing files. A switch waits meanwhile, and the banner offers it. Returns how to let go.
 */
const holds = new Set<symbol>();
export function holdReload(why = "hold"): () => void {
  const hold = Symbol(why);
  holds.add(hold);
  return () => {
    holds.delete(hold);
    trySwitch();
  };
}

/** The page's API offline: the app's own backend, under the server's ids. */
export function offlinePageApi(local: Api = createTauriApi()): OfflineApi {
  if (!offlineApi) {
    const session = loadSession();
    const mirror = session ? loadMirror(session.user.id) : null;
    const api = createOfflineApi(local, new Map(Object.entries(mirror?.characters ?? {}).map(([s, m]) => [s, m.local])));
    const close = api.closeCharacter;
    api.closeCharacter = async (args) => {
      await close(args);
      closedHook?.();
    };
    offlineApi = api;
  }
  return offlineApi;
}

const reload = () => window.location.reload();

/** The upstream route the page shows (a HashRouter: `#/characters?x`). */
function route(): string {
  return window.location.hash.replace(/^#/, "").split("?")[0] || "/";
}

/**
 * Whether reloading now disturbs nobody: the Start page, nothing being typed, no dialog open, no character open on an
 * offline page, and nothing holding the page ({@link holdReload}).
 */
export function safeToReload(): boolean {
  if (holds.size > 0) return false;
  if (route() !== "/" && route() !== "/characters") return false;
  const active = document.activeElement as HTMLElement | null;
  if (active && active !== document.body && (active.matches("input, textarea, select") || active.isContentEditable)) return false;
  if (document.querySelector("[role=dialog], [role=alertdialog], dialog[open]")) return false;
  if ((offlineApi?.openCharacters().length ?? 0) > 0) return false;
  return true;
}

let stopPending: (() => void) | null = null;

function trySwitch(): void {
  if (!state.waiting || !safeToReload()) return;
  stopPending?.();
  stopWatching?.();
  reload();
}

/** Switch as soon as that disturbs nobody: now, or on a blur, a route change, a closed character, a released hold. */
function wantSwitch(to: Switch): void {
  if (state.waiting !== to) {
    cancelSwitch();
    publish({ waiting: to });
    const look = () => trySwitch();
    const every = setInterval(look, SAFE_POLL_MS);
    window.addEventListener("hashchange", look);
    document.addEventListener("focusout", look);
    stopPending = () => {
      clearInterval(every);
      window.removeEventListener("hashchange", look);
      document.removeEventListener("focusout", look);
      stopPending = null;
    };
  }
  trySwitch();
}

function cancelSwitch(): void {
  stopPending?.();
  publish({ waiting: null });
}

/**
 * A record whose characters are all gone from the device is no copy: the WebView can lose a storage write when the app
 * is killed right after it (a sign-out's wipe). When the files can't be listed, the record is taken as it is.
 */
async function copyOnDisk(userId: string, local: Pick<Api, "listCharacters">): Promise<boolean> {
  const wanted = Object.values(loadMirror(userId)?.characters ?? {}).map((m) => m.local);
  if (wanted.length === 0) return true;
  try {
    const onDisk = new Set((await local.listCharacters()).map((c) => c.id));
    return wanted.some((id) => onDisk.has(id));
  } catch {
    return true;
  }
}

/** The page starts. Apps only: a browser is always online (and watches the server from {@link watchConnection}). */
export async function decideBoot(session: ServerSession, inApp = isTauri(), local?: Pick<Api, "listCharacters">): Promise<Boot> {
  if (!inApp) return "online";
  const reach: Reach = session.expired ? "expired" : await probe(session);
  if (reach === "ok") {
    watchOnline(session, true);
    return "online";
  }
  if (!hasCopy(session.user.id) || !await copyOnDisk(session.user.id, local ?? createTauriApi())) {
    if (reach === "network") return "no-copy";
    clearSession();
    return "signed-out";
  }
  if (reach === "expired") markExpired();
  // party.css and entry/hideTransfer.ts hide what upstream can't refuse with a message (delete, duplicate).
  document.documentElement.classList.add("party-offline-mode");
  publish({ mode: "offline", expired: reach === "expired" });
  watchOffline(session);
  return "offline";
}

/** An online page, once it shows (OfflineBanner, live event streams): watch the server, in a browser too. Idempotent. */
export function watchConnection(inApp = isTauri()): void {
  if (stopWatching || state.mode !== "online" || state.expired) return;
  const session = loadSession();
  if (session && !session.expired) watchOnline(session, inApp);
}

let stopWatching: (() => void) | null = null;
/** Online: something says the server may be gone (a failed request, a dropped stream, the OS). */
let suspectHook: (() => void) | null = null;
/** Online: something reached the server (an event stream opened). */
let reachedHook: (() => void) | null = null;
/** Probe now instead of after the backoff (the banner's "Try now"). */
let nowHook: (() => void) | null = null;
/** Offline: run after the page closes a character (with the banner up, that is the moment to switch). */
let closedHook: (() => void) | null = null;

/** The server may be gone: an event stream dropped and could not reopen (liveEvents.ts). */
export const suspectServer = (): void => suspectHook?.();
/** The server answered: an event stream opened. */
export const serverReached = (): void => reachedHook?.();
/** The banner's "Try now": probe at once. */
export const retryNow = (): void => nowHook?.();

/**
 * Online: a failed request starts probing. Two failed probes in a row, with a backoff between them, make the server
 * `down`; in the app with a copy the page then switches offline once that disturbs nobody. A browser just waits.
 */
function watchOnline(session: ServerSession, inApp: boolean): void {
  stopWatching?.();
  let fails = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let backTimer: ReturnType<typeof setTimeout> | null = null;
  let probing = false;
  let stopped = false;

  const clear = () => {
    if (timer) clearTimeout(timer);
    timer = null;
  };
  const up = () => {
    clear();
    fails = 0;
    if (state.waiting === "offline") cancelSwitch();
    if (state.link === "down") {
      publish({ link: "back" });
      if (backTimer) clearTimeout(backTimer);
      backTimer = setTimeout(() => { backTimer = null; if (state.link === "back") publish({ link: "up" }); }, BACK_MS);
      backHandlers.forEach((h) => h());
    } else if (state.link === "checking") {
      publish({ link: "up" });
    }
  };
  const check = async () => {
    if (probing || stopped) return;
    probing = true;
    const reach = await probe(session);
    probing = false;
    if (stopped) return;
    if (reach === "ok") { up(); return; }
    if (reach === "expired") { expired(); return; }
    fails += 1;
    if (fails >= FAILS_DOWN) {
      publish({ link: "down" });
      if (inApp && hasCopy(session.user.id)) wantSwitch("offline");
    }
    clear();
    timer = setTimeout(() => { timer = null; void check(); }, backoffMs(fails));
  };
  const suspect = () => {
    if (stopped || state.expired) return;
    if (state.link === "up" || state.link === "back") {
      if (backTimer) clearTimeout(backTimer);
      backTimer = null;
      publish({ link: "checking" });
      void check();
    }
    // Already checking or down: the next probe is on its way.
  };
  const now = () => {
    if (state.link === "up" || state.link === "back") return;
    clear();
    void check();
  };
  // A refused token keeps the copy to play with: offline with "sign in again", once that disturbs nobody.
  const expired = () => {
    clear();
    fails = 0;
    markExpired();
    publish({ expired: true, link: "up" });
    wantSwitch("again");
  };
  const front = () => { if (document.visibilityState === "visible") now(); };

  serverWatch.network = suspect;
  serverWatch.expired = () => {
    if (!inApp || !hasCopy(session.user.id)) return false;
    if (!state.expired) expired();
    return true;
  };
  suspectHook = suspect;
  reachedHook = () => { if (state.link === "checking" || state.link === "down") up(); };
  nowHook = now;
  window.addEventListener("offline", suspect);
  window.addEventListener("online", now);
  document.addEventListener("visibilitychange", front);
  stopWatching = () => {
    stopped = true;
    clear();
    if (backTimer) clearTimeout(backTimer);
    window.removeEventListener("offline", suspect);
    window.removeEventListener("online", now);
    document.removeEventListener("visibilitychange", front);
    suspectHook = reachedHook = nowHook = null;
    stopWatching = null;
  };
}

/**
 * Offline: try the server every 20 seconds, when the network comes back and when the app comes to the front. Once it
 * answers, the page reloads online if that disturbs nobody; otherwise the banner offers "Sync now" and the switch
 * waits. A server that goes away again (two failed probes) takes the offer back.
 */
function watchOffline(session: ServerSession): void {
  if (session.expired) return;
  let probing = false;
  let fails = 0;
  const check = async () => {
    if (probing) return;
    probing = true;
    const reach = await probe(session);
    probing = false;
    if (!stopWatching) return;
    if (reach === "expired") {
      // Signed out on the server while this device was offline: play goes on, and the banner asks for a sign-in.
      markExpired();
      cancelSwitch();
      stopWatching();
      publish({ expired: true, backOnline: false });
      return;
    }
    if (reach !== "ok") {
      fails += 1;
      if (fails >= FAILS_DOWN && state.backOnline) {
        cancelSwitch();
        publish({ backOnline: false });
      }
      return;
    }
    fails = 0;
    publish({ backOnline: true });
    wantSwitch("online");
  };
  const front = () => { if (document.visibilityState === "visible") void check(); };
  const online = () => void check();
  const every = setInterval(online, RECONNECT_MS);
  window.addEventListener("online", online);
  document.addEventListener("visibilitychange", front);
  closedHook = () => { if (state.waiting) trySwitch(); else void check(); };
  nowHook = online;
  stopWatching = () => {
    closedHook = null;
    nowHook = null;
    clearInterval(every);
    window.removeEventListener("online", online);
    document.removeEventListener("visibilitychange", front);
    stopWatching = null;
  };
}

/** Back online with a character open: save it on the device, then start again online (where the sync takes over). */
export async function goOnline(): Promise<void> {
  cancelSwitch();
  try { await offlineApi?.closeAll(); } finally { reload(); }
}

/** Online in the app with the server gone: start again from the device's copy now (the banner's "Play offline"). */
export function goOffline(): void {
  cancelSwitch();
  reload();
}

/** Signed out by the server: save what is open on the device, then the next page asks for the sign-in (ConnectPage). */
export async function goSignInAgain(): Promise<void> {
  cancelSwitch();
  try { await offlineApi?.closeAll(); } finally {
    askSignInAgain();
    reload();
  }
}

/** The app's own ids of the copies open on this page (offline); none online. */
export const openCopies = (): Set<string> => new Set(offlineApi?.openCopies() ?? []);

/** Tests: start over. */
export function resetConnectivity(): void {
  stopWatching?.();
  stopWatching = null;
  stopPending?.();
  closedHook = suspectHook = reachedHook = nowHook = null;
  holds.clear();
  backHandlers.clear();
  document.documentElement.classList.remove("party-offline-mode");
  state = START;
  offlineApi = null;
  delete serverWatch.network;
  delete serverWatch.expired;
}
