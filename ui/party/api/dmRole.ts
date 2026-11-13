import { useSyncExternalStore } from "react";
import { loadSession } from "./session";

/**
 * Whether the signed-in account has the Dungeon Master switch on (plan D2, `account_roles`), as the UI needs it at once:
 * the top bar's Encounters tab and the `/encounters` routes are for DMs only (0.3.1). The answer is the server's
 * (`GET /api/me/roles`, and the switch's own answers); this keeps the last one on the device, per account, so a reload
 * draws the tab on its first paint, with no flash, and offline too. `null`: never asked on this device yet.
 *
 * The server checks every encounter call itself (the party's DM only); this only decides what the UI offers.
 */
const KEY = "party.dmRole";

type Remembered = Record<string, boolean>;

function readAll(): Remembered {
  try {
    const text = localStorage.getItem(KEY);
    const parsed = text ? (JSON.parse(text) as unknown) : null;
    return parsed && typeof parsed === "object" ? (parsed as Remembered) : {};
  } catch { return {}; }
}

const listeners = new Set<() => void>();
let version = 0;

/** The remembered answer for the signed-in account, or null when there is none (or nobody is signed in). */
export function dmRole(): boolean | null {
  const id = loadSession()?.user.id;
  if (!id) return null;
  const known = readAll()[id];
  return typeof known === "boolean" ? known : null;
}

/** The server said so: remember it for the signed-in account and redraw what depends on it. */
export function rememberDmRole(dm: boolean): void {
  const id = loadSession()?.user.id;
  if (!id) return;
  const all = readAll();
  if (all[id] === dm) return;
  all[id] = dm;
  try { localStorage.setItem(KEY, JSON.stringify(all)); } catch { /* private window: it lasts as long as the page */ }
  version++;
  for (const l of [...listeners]) l();
}

export function onDmRoleChanged(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** The signed-in account's DM switch for a component; it redraws when it changes. */
export function useDmRole(): boolean | null {
  useSyncExternalStore(onDmRoleChanged, () => version, () => version);
  return dmRole();
}

/** Tests: forget every account's answer. */
export function resetDmRole(): void {
  try { localStorage.removeItem(KEY); } catch { /* nothing kept */ }
  version++;
  for (const l of [...listeners]) l();
}
