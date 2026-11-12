import { closeTopOverlay, openOverlayCount, subscribeOverlays } from "../../src/components/controls/overlayStack";
import { installKeyboardInset } from "../../src/lib/keyboardInset";
import { detectPhoneOs, deviceShortSide, installPlatformClasses, type PhoneOs, type PlatformEnv } from "../../src/lib/phone";
import { isTauri } from "../../src/lib/tauri";
import "./phoneBrowser.css";

/**
 * Plan M4 (D8): phone browsers get upstream's phone layer too. Upstream's `startPhone()` gives it only to the iOS and
 * Android builds and to the web mock (`?mock=mobile`); the party's server also serves phone browsers, and Safari is the
 * only way onto an iPhone until an iOS app ships. So the party entry installs the same platform classes and keyboard
 * inset for a phone browser, and lets the browser's Back close the top sheet or dialog, as the Android app's Back does.
 */

export interface BrowserEnv extends PlatformEnv {
  /** `(pointer: coarse)` matches: the main input is a finger. */
  coarse: boolean;
}

/**
 * The phone OS of a plain phone browser, or null: in the apps, in a desktop browser, and with `?mock=mobile` (upstream's
 * `startPhone()` already sets the layer up there). A phone or tablet user agent counts only with a touch screen, so a
 * desktop browser that only borrows a phone's user agent stays desktop.
 */
export function detectBrowserPhoneOs(env: BrowserEnv): PhoneOs | null {
  if (env.isTauri || detectPhoneOs(env) !== null) return null;
  if (!env.coarse && env.maxTouchPoints < 1) return null;
  // Upstream reads the platform from the user agent in the apps; a phone browser's user agent says the same.
  return detectPhoneOs({ ...env, isTauri: true });
}

const noMedia = () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined });

function browserEnv(win: Window): BrowserEnv {
  const coarse = typeof win.matchMedia === "function" && win.matchMedia("(pointer: coarse)").matches;
  return { userAgent: win.navigator.userAgent, maxTouchPoints: win.navigator.maxTouchPoints ?? 0, search: win.location.search, isTauri: isTauri(), coarse };
}

/**
 * Party startup for browsers, run once from the entry module (before upstream's `startPhone()`, which then does nothing
 * in a plain browser). Returns a cleanup (tests).
 */
export function startPhoneBrowser(win: Window = window): () => void {
  if (isTauri()) return () => undefined;
  const env = browserEnv(win);
  const cleanups: (() => void)[] = [];
  const os = detectBrowserPhoneOs(env);
  if (os) {
    const root = win.document.documentElement;
    // installPlatformClasses reads the platform from the user agent the way it does in the apps.
    cleanups.push(
      installPlatformClasses(root, { ...env, isTauri: true }, typeof win.matchMedia === "function" ? (q) => win.matchMedia(q) : noMedia, () => deviceShortSide(win)),
      installKeyboardInset(win, root),
    );
  }
  // A phone browser, real or the web mock, gets Back for its sheets and dialogs.
  if (os || detectPhoneOs(env) !== null) cleanups.push(installBrowserBack(win));
  return () => cleanups.forEach((c) => c());
}

/** The parts of `window` the browser Back handler uses. */
export interface BackWindow {
  history: Pick<History, "state" | "pushState" | "go" | "back" | "forward">;
  location: { href: string };
  addEventListener(type: "popstate", listener: () => void): void;
  removeEventListener(type: "popstate", listener: () => void): void;
  setTimeout(handler: () => void, ms?: number): unknown;
}

/** The history-state key of the entries pushed for open overlays. */
export const OVERLAY_KEY = "partyOverlay";

type State = Record<string, unknown> | null;
const markOf = (state: unknown): number | null => {
  const v = (state as State)?.[OVERLAY_KEY];
  return typeof v === "number" ? v : null;
};
const idxOf = (state: unknown): number | null => {
  const v = (state as State)?.idx;
  return typeof v === "number" ? v : null;
};

/**
 * Browser Back closes the top overlay. Each sheet, dialog or menu that opens pushes a history entry with the same URL
 * (the router's state is kept, so it sees no navigation), and Back pops it: the top overlay closes through its own
 * onClose, so an unsaved-changes prompt still runs. An overlay closed another way (its button, Escape, the scrim) takes
 * its entry back off, so the next Back leaves the page as before. When a sheet's choice navigates (a Menu item), the new
 * page sits above the overlay's entry; that entry is then skipped in either direction, so no Back press is lost to it.
 */
export function installBrowserBack(win: BackWindow): () => void {
  const h = win.history;
  /** Marks of the entries pushed for the overlays open now, oldest first. */
  let entries: number[] = [];
  /** Entries left behind by an overlay that closed under a new page, with the router index they carry. */
  const stale = new Set<number>();
  let seq = 0;
  /** Overlay count Back asked for; closes go one at a time, since an overlay leaves the stack only after it unmounts. */
  let backTarget: number | null = null;
  let lastCount = openOverlayCount();
  let lastIdx = idxOf(h.state);

  const sync = () => {
    const count = openOverlayCount();
    const closedOne = count < lastCount;
    lastCount = count;
    if (backTarget !== null) {
      if (count > backTarget && closedOne) { closeTopOverlay(); return; }
      // Done, or the overlay stayed (it asked first, perhaps in a new dialog): what is open now gets its entries again.
      backTarget = null;
    }
    while (entries.length < count) {
      const mark = ++seq;
      h.pushState({ ...((h.state as State) ?? {}), [OVERLAY_KEY]: mark }, "", win.location.href);
      entries.push(mark);
    }
    if (entries.length > count) {
      const dropped = entries.slice(count);
      entries = entries.slice(0, count);
      const top = dropped[dropped.length - 1]!;
      // Later, so a navigation made by the same tap (a Menu item) has pushed its page first; then it stays.
      win.setTimeout(() => {
        if (markOf(h.state) === top) h.go(-dropped.length);
        else {
          dropped.forEach((m) => stale.add(m));
          lastIdx = idxOf(h.state);
        }
      }, 0);
    }
  };

  const onPop = () => {
    const state = h.state;
    const mark = markOf(state);
    const idx = idxOf(state);
    const from = lastIdx;
    lastIdx = idx;
    if (mark !== null && stale.has(mark)) {
      // Back from the newer page goes on past it, Forward from the older one goes on to the newer page.
      if (idx !== null && from !== null && from > idx) h.back();
      else if (idx !== null && from !== null) h.forward();
      return;
    }
    const at = mark === null ? -1 : entries.indexOf(mark);
    entries = entries.slice(0, at + 1);
    const target = entries.length;
    if (openOverlayCount() > target) {
      backTarget = target;
      closeTopOverlay();
    }
  };

  const off = subscribeOverlays(sync);
  win.addEventListener("popstate", onPop);
  sync();
  return () => {
    off();
    win.removeEventListener("popstate", onPop);
  };
}
