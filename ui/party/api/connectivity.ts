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
 */

/** What one request to the server found: it answered, it could not be reached, or it refused the token. */
export type Reach = "ok" | "network" | "expired";

const PROBE_MS = 4000;
const RECONNECT_MS = 20 * 1000;

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

/** How the page starts: `no-copy` when the server can't be reached and there is nothing to play offline with. */
export type Boot = "online" | "offline" | "no-copy" | "signed-out";

export interface ConnectivityState {
  mode: "online" | "offline";
  /** Offline, and the server answers again while a character is open: the banner offers to switch. */
  backOnline: boolean;
  /** Offline because the server no longer takes the session (signed out there, or the account was turned off). */
  expired: boolean;
}

let state: ConnectivityState = { mode: "online", backOnline: false, expired: false };
const watchers = new Set<() => void>();
let offlineApi: OfflineApi | null = null;

function publish(next: Partial<ConnectivityState>): void {
  state = { ...state, ...next };
  watchers.forEach((w) => w());
}

export function useConnectivity(): ConnectivityState {
  return useSyncExternalStore((w) => { watchers.add(w); return () => watchers.delete(w); }, () => state);
}

export const connectivity = (): ConnectivityState => state;
export const isOffline = (): boolean => state.mode === "offline";

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

/** The page starts. Apps only: a browser is always online. */
export async function decideBoot(session: ServerSession, inApp = isTauri(), local?: Pick<Api, "listCharacters">): Promise<Boot> {
  if (!inApp) return "online";
  const reach: Reach = session.expired ? "expired" : await probe(session);
  if (reach === "ok") {
    watchOnline(session);
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

let checking = false;
let stopWatching: (() => void) | null = null;
/** Offline: run after the page closes a character (with the banner up, that is the moment to switch). */
let closedHook: (() => void) | null = null;

/** Online in the app: a request that could not reach the server, and one probe agrees, reloads the page offline. */
function watchOnline(session: ServerSession): void {
  serverWatch.network = () => {
    if (checking || !hasCopy(session.user.id)) return;
    checking = true;
    void probe(session).then((reach) => {
      checking = false;
      if (reach === "network") reload();
    });
  };
  // A refused token keeps the copy to play with: offline, with "sign in again" (and no reload into an empty app).
  serverWatch.expired = () => {
    if (!hasCopy(session.user.id)) return false;
    markExpired();
    reload();
    return true;
  };
}

/**
 * Offline: try the server every 20 seconds, when the network comes back and when the app comes to the front. Once
 * it answers, the page reloads online, unless a character is open: then the banner offers it.
 */
function watchOffline(session: ServerSession): void {
  if (session.expired) return;
  const check = () => {
    if (checking) return;
    // The banner is up: the switch waits for the character to be closed.
    if (state.backOnline) {
      if ((offlineApi?.openCharacters().length ?? 0) === 0) { stopWatching?.(); reload(); }
      return;
    }
    checking = true;
    void probe(session).then((reach) => {
      checking = false;
      if (reach === "expired") {
        // Signed out on the server while this device was offline: play goes on, and the banner asks for a sign-in.
        markExpired();
        stopWatching?.();
        publish({ expired: true, backOnline: false });
        return;
      }
      if (reach !== "ok") return;
      if ((offlineApi?.openCharacters().length ?? 0) > 0) {
        publish({ backOnline: true });
      } else {
        stopWatching?.();
        reload();
      }
    });
  };
  const front = () => { if (document.visibilityState === "visible") check(); };
  const every = setInterval(check, RECONNECT_MS);
  window.addEventListener("online", check);
  document.addEventListener("visibilitychange", front);
  closedHook = check;
  stopWatching = () => {
    closedHook = null;
    clearInterval(every);
    window.removeEventListener("online", check);
    document.removeEventListener("visibilitychange", front);
  };
}

/** Back online with a character open: save it on the device, then start again online (where the sync takes over). */
export async function goOnline(): Promise<void> {
  try { await offlineApi?.closeAll(); } finally { reload(); }
}

/** Signed out by the server: save what is open on the device, then the next page asks for the sign-in (ConnectPage). */
export async function goSignInAgain(): Promise<void> {
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
  closedHook = null;
  document.documentElement.classList.remove("party-offline-mode");
  state = { mode: "online", backOnline: false, expired: false };
  offlineApi = null;
  checking = false;
  delete serverWatch.network;
  delete serverWatch.expired;
}
