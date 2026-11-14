import { useSyncExternalStore } from "react";
import { loadMirror, type SyncConflict } from "./mirror";

/**
 * Offline conflict safety (0.3.6). A character edited here offline whose server copy changed meanwhile (another
 * device, or the DM) is never decided for the user: the server takes nothing (`party_sync_push` with `ask`), the copy
 * here waits, and a dialog asks which one stays. The choice is kept here, apart from the mirror (which a running sync
 * writes from memory), and the next sync carries it out (./sync.ts):
 *  - `mine`: the copy here is sent again with `force`, over the server's (which goes to the account's trash);
 *  - `server`: the server's copy replaces the one here;
 *  - `both`: the copy here goes to the server as a new character ("<name> (this device)"), and the server's copy
 *    replaces it here. Not offered for a character deleted on the server.
 */
export type ConflictChoice = "mine" | "server" | "both";

export interface PendingConflict extends SyncConflict {
  /** The character's server id. */
  id: string;
}

const key = (userId: string) => `party.conflictChoices.${userId}`;
const watchers = new Set<() => void>();
let version = 0;

/** A sync or a choice changed what is pending: the dialog looks again. */
export function conflictsChanged(): void {
  version += 1;
  watchers.forEach((w) => w());
}

export function loadChoices(userId: string): Record<string, ConflictChoice> {
  try {
    const value = JSON.parse(localStorage.getItem(key(userId)) ?? "{}") as unknown;
    if (!value || typeof value !== "object") return {};
    return Object.fromEntries(Object.entries(value as Record<string, unknown>)
      .filter((e): e is [string, ConflictChoice] => e[1] === "mine" || e[1] === "server" || e[1] === "both"));
  } catch { return {}; }
}

function saveChoices(userId: string, choices: Record<string, ConflictChoice>): void {
  try {
    if (Object.keys(choices).length === 0) localStorage.removeItem(key(userId));
    else localStorage.setItem(key(userId), JSON.stringify(choices));
  } catch { /* not kept: the dialog asks again */ }
}

export function chooseConflict(userId: string, id: string, choice: ConflictChoice): void {
  saveChoices(userId, { ...loadChoices(userId), [id]: choice });
  conflictsChanged();
}

/** A choice the sync carried out (or one for a conflict that is gone). */
export function clearChoice(userId: string, id: string): void {
  const choices = loadChoices(userId);
  if (!(id in choices)) return;
  delete choices[id];
  saveChoices(userId, choices);
}

/** Conflicts still waiting for the user's choice. */
export function pendingConflicts(userId: string): PendingConflict[] {
  const chosen = loadChoices(userId);
  return Object.entries(loadMirror(userId)?.conflicts ?? {})
    .filter(([id]) => !chosen[id])
    .map(([id, c]) => ({ id, name: c.name, deleted: Boolean(c.deleted) }));
}

export function useConflictsVersion(): number {
  return useSyncExternalStore((w) => { watchers.add(w); return () => watchers.delete(w); }, () => version);
}

/** The copy here as a new character beside the server's: no id (the server gives it one), "(this device)" after its name. */
export function asCopy(base64: string, fallbackName: string): string {
  const text = new TextDecoder().decode(Uint8Array.from(atob(base64), (c) => c.charCodeAt(0)));
  const value = JSON.parse(text) as { meta?: { id?: unknown; name?: unknown } };
  const meta = { ...(value.meta ?? {}) };
  delete meta.id;
  meta.name = `${String(meta.name || fallbackName)} (this device)`;
  const bytes = new TextEncoder().encode(JSON.stringify({ ...value, meta }));
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}
