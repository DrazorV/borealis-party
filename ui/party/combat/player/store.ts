import { useSyncExternalStore } from "react";
import { holdReload } from "../../api/connectivity";
import {
  actingId, combatPlayerDeps, liveCombat, myCombatant, watchCombats, type PlayerCombat,
} from "./api";
import { loadSession } from "../../api/session";

/**
 * The live combats of the signed-in player's parties, for the Play tab's Combat sub-tab, the Live chip and the
 * "Your turn" banner (plan 4.4, 0.3.0 part F). One watcher per page: it lists the parties the account has a character
 * in, asks each for its live combat, and follows the combat events on the account's shared event stream.
 *
 * Losing the signal (D44): the last combat seen stays on screen with an "Offline" chip, and nothing is dropped
 * because a request failed. The watcher asks again when the stream reopens or the next event comes. While a combat
 * is live the page doesn't reload on its own (connectivity.ts `holdReload`), the screen stays on (Screen Wake Lock),
 * and the phone buzzes when the turn comes to one of the player's characters.
 */

export interface CombatStore {
  /** Live combats (setup or active) with one of the player's characters in them, by party. */
  combats: PlayerCombat[];
  /** A request failed for want of the server: what's shown is the last state seen. */
  offline: boolean;
  /** When `offline` began (ms). */
  offlineSince: number | null;
  /** When the last answer came (ms). */
  updatedAt: number | null;
  /** The first look at the parties is done. */
  ready: boolean;
}

const EMPTY: CombatStore = { combats: [], offline: false, offlineSince: null, updatedAt: null, ready: false };

let state: CombatStore = EMPTY;
const subscribers = new Set<() => void>();

function publish(next: Partial<CombatStore>): void {
  state = { ...state, ...next };
  subscribers.forEach((s) => s());
  device();
}

export const combatStore = (): CombatStore => state;

export function useCombatStore(): CombatStore {
  return useSyncExternalStore((s) => { subscribers.add(s); return () => subscribers.delete(s); }, () => state);
}

/** What the page can do on the device; tests swap it. */
export const deviceDeps = {
  now: () => Date.now(),
  /** D44: offline longer than this, what's shown is called old. */
  staleMs: 60 * 1000,
  vibrate: (pattern: number[]) => {
    try { navigator.vibrate?.(pattern); } catch { /* not on this device */ }
  },
  wakeLock: async (): Promise<{ release: () => Promise<void> } | null> => {
    try {
      const nav = navigator as Navigator & { wakeLock?: { request: (type: "screen") => Promise<{ release: () => Promise<void> }> } };
      return (await nav.wakeLock?.request("screen")) ?? null;
    } catch {
      return null;
    }
  },
};

/** The buzz on "Your turn". */
export const YOUR_TURN_BUZZ = [200, 100, 200];

const isLive = (c: PlayerCombat) => c.status !== "ended" && c.pointer.kind !== "ended";
const isNetwork = (e: unknown) => (e as { code?: string } | null)?.code === "network";

/** Parties with one of the account's characters in them: theirs to play in. */
let parties = new Map<string, string>();
const lastSeq = new Map<string, number>();
/** The turn each party's combat was on last time, to buzz once when it becomes the player's. */
const lastActing = new Map<string, string | null>();

async function listParties(): Promise<void> {
  const session = loadSession();
  if (!session) return;
  const out = await combatPlayerDeps.fetch(session, "GET", "/api/parties") as { parties?: { id: string; name: string; myCharacters?: string[] }[] };
  parties = new Map((out.parties ?? []).filter((p) => (p.myCharacters ?? []).length > 0).map((p) => [p.id, p.name]));
}

function wentOffline(): void {
  if (!state.offline) publish({ offline: true, offlineSince: deviceDeps.now() });
}

function put(partyId: string, combat: PlayerCombat | null): void {
  const others = state.combats.filter((c) => c.partyId !== partyId);
  const keep = combat && isLive(combat) && myCombatant(combat) ? [{ ...combat, partyName: combat.partyName ?? parties.get(partyId) }] : [];
  if (combat) lastSeq.set(partyId, combat.seq);
  else lastSeq.delete(partyId);
  const was = lastActing.get(partyId) ?? null;
  const now = keep[0] ? actingId(keep[0]) : null;
  lastActing.set(partyId, now);
  if (keep[0] && now && now !== was && keep[0].combatants.some((x) => x.mine && x.id === now)) deviceDeps.vibrate(YOUR_TURN_BUZZ);
  publish({ combats: [...others, ...keep], offline: false, offlineSince: null, updatedAt: deviceDeps.now() });
}

/** Ask one party for its combat again. A network failure keeps what's shown (D44). */
async function refresh(partyId: string): Promise<void> {
  try {
    put(partyId, await liveCombat(partyId));
  } catch (e) {
    if (isNetwork(e)) wentOffline();
    // Any other refusal: the party is gone or no longer this account's; its combat goes too.
    else put(partyId, null);
  }
}

async function refreshAll(): Promise<void> {
  try {
    await listParties();
  } catch (e) {
    if (isNetwork(e)) { wentOffline(); return; }
    parties = new Map();
  }
  // Combats of parties the account left go.
  const gone = state.combats.filter((c) => !parties.has(c.partyId));
  if (gone.length > 0) publish({ combats: state.combats.filter((c) => parties.has(c.partyId)) });
  await Promise.all([...parties.keys()].map(refresh));
  if (!state.ready) publish({ ready: true });
}

/** A player write answered the new projection: show it at once. */
export function adopt(combat: PlayerCombat): void {
  put(combat.partyId, combat);
}

/** Tests and a reconnect: ask everything again. */
export const refreshCombats = (): Promise<void> => refreshAll();

let running: (() => void) | null = null;
let first: Promise<void> = Promise.resolve();
/** Tests: the watcher's first look at the parties. */
export const combatWatchReady = (): Promise<void> => first;

/** Starts following the account's combats; answers a stop. Once per page (a second start shares the first). */
export function startCombatWatch(): () => void {
  if (running) return running;
  const stopEvents = watchCombats((e) => {
    if (e.type === "combat") {
      if (!parties.has(e.party)) { void refreshAll(); return; }
      if (e.seq && e.seq <= (lastSeq.get(e.party) ?? -1) && !state.offline) return;
      void refresh(e.party);
    } else void refreshAll();
  });
  first = refreshAll();
  running = () => {
    stopEvents();
    running = null;
  };
  return running;
}

/** Tests: start over. */
export function resetCombatStore(): void {
  running?.();
  running = null;
  parties = new Map();
  lastSeq.clear();
  lastActing.clear();
  state = EMPTY;
  release();
  subscribers.forEach((s) => s());
}

// The device side: no reload and the screen on while a combat is live.
let reloadHold: (() => void) | null = null;
let lock: { release: () => Promise<void> } | null = null;
let locking = false;

function release(): void {
  reloadHold?.();
  reloadHold = null;
  void lock?.release().catch(() => undefined);
  lock = null;
}

function device(): void {
  const live = state.combats.length > 0;
  if (!live) { release(); return; }
  reloadHold ??= holdReload("combat");
  if (!lock && !locking) {
    locking = true;
    void deviceDeps.wakeLock().then((l) => {
      locking = false;
      if (state.combats.length > 0) lock = l;
      else void l?.release().catch(() => undefined);
    });
  }
}

// A wake lock ends when the page is hidden; take it again when the page comes back.
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") { lock = null; device(); }
  });
}

/** The live combat that has this character in it, or null. */
export function combatFor(store: CombatStore, characterId: string | null | undefined): PlayerCombat | null {
  if (!characterId) return null;
  return store.combats.find((c) => myCombatant(c, characterId)) ?? null;
}

export function useCombatFor(characterId: string | null | undefined): PlayerCombat | null {
  return combatFor(useCombatStore(), characterId);
}

/** The first live combat where it's one of the player's characters' turn, or null. */
export function myTurn(store: CombatStore): { combat: PlayerCombat; characterId: string | null } | null {
  for (const c of store.combats) {
    const id = actingId(c);
    const mine = c.combatants.find((x) => x.mine && x.id === id);
    if (mine) return { combat: c, characterId: mine.characterId ?? null };
  }
  return null;
}
