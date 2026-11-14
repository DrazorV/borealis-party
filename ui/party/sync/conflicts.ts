import { useSyncExternalStore } from "react";
import { loadMirror, type SyncConflict } from "./mirror";

/**
 * Offline conflict safety (0.3.6). A character edited here offline whose server copy changed meanwhile (another
 * device, or the DM) is never decided for the user: the server takes nothing (`party_sync_push` with `ask`), the copy
 * here waits, and a dialog asks which one stays. The choice is kept here, apart from the mirror (which a running sync
 * writes from memory), together with the two copies it was made about; the next sync carries it out (./sync.ts) only
 * while the copy here is still the one the user saw, else it asks again:
 *  - `mine`: the copy here is sent again with `forceOver` (the server copy the user saw): written only while the
 *    server still holds that copy, else it is a new conflict;
 *  - `server`: the copy here goes to the account's trash on the server (`discard`), then the server's replaces it;
 *  - `both`: the copy here goes to the server as a new character ("<name> (this device)", or "(this device 2)"...
 *    when that name is taken), and the server's copy replaces it here. Not offered for a character deleted there.
 * A chosen action the server refuses clears the choice: the dialog asks again and says what went wrong.
 */
export type ConflictChoice = "mine" | "server" | "both";

/** A choice and what it was made about. */
export interface ChoiceRecord {
  choice: ConflictChoice;
  /** The copy here when the user chose: an edit made since is not covered. */
  localSha: string;
  /** The server copy the user chose over (null: it was deleted). */
  serverSha: string | null;
  /** Keep both: the new character's name, fixed before it is sent, so a repeat finds it instead of a second copy. */
  copyName?: string;
}

export interface PendingConflict extends SyncConflict {
  /** The character's server id. */
  id: string;
}

const key = (userId: string) => `party.conflictChoices.${userId}`;
const watchers = new Set<() => void>();
let version = 0;
let round = 0;

/** What is pending changed: after a sync (`fromSync`, which also brings back conflicts closed for later) or a choice. */
export function conflictsChanged(fromSync = false): void {
  version += 1;
  if (fromSync) round += 1;
  watchers.forEach((w) => w());
}

/** Syncs finished so far on this page: a conflict closed "for later" comes back after the next one. */
export const syncRound = (): number => round;

export function loadChoices(userId: string): Record<string, ChoiceRecord> {
  try {
    const value = JSON.parse(localStorage.getItem(key(userId)) ?? "{}") as unknown;
    if (!value || typeof value !== "object") return {};
    return Object.fromEntries(Object.entries(value as Record<string, Partial<ChoiceRecord>>)
      .filter((e): e is [string, ChoiceRecord] => ["mine", "server", "both"].includes(String(e[1]?.choice)) && typeof e[1]?.localSha === "string"));
  } catch { return {}; }
}

function saveChoices(userId: string, choices: Record<string, ChoiceRecord>): void {
  try {
    if (Object.keys(choices).length === 0) localStorage.removeItem(key(userId));
    else localStorage.setItem(key(userId), JSON.stringify(choices));
  } catch { /* not kept: the dialog asks again */ }
}

export function saveChoice(userId: string, id: string, record: ChoiceRecord): void {
  saveChoices(userId, { ...loadChoices(userId), [id]: record });
}

/** The user's answer to the dialog, about the conflict as the last sync found it. */
export function chooseConflict(userId: string, id: string, choice: ConflictChoice): void {
  const conflict = loadMirror(userId)?.conflicts[id];
  if (!conflict) return;
  saveChoice(userId, id, { choice, localSha: conflict.localSha, serverSha: conflict.serverSha });
  conflictsChanged();
}

/** A choice the sync carried out, or one that no longer applies. */
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
    .map(([id, c]) => ({ ...c, id, deleted: Boolean(c.deleted) }));
}

export function useConflictsVersion(): number {
  return useSyncExternalStore((w) => { watchers.add(w); return () => watchers.delete(w); }, () => version);
}

const COPY_SUFFIX = / \(this device(?: \d+)?\)$/;

/** "<name> (this device)", or "(this device 2)", "(this device 3)"... when taken; a copy of a copy is not "(this device) (this device)". */
export function copyName(name: string, taken: Iterable<string>): string {
  const base = name.replace(COPY_SUFFIX, "") || "A character";
  const names = new Set(taken);
  for (let n = 1; ; n += 1) {
    const candidate = `${base} (this device${n === 1 ? "" : ` ${n}`})`;
    if (!names.has(candidate)) return candidate;
  }
}

/** The copy here as a new character beside the server's: no id (the server gives it one), and its new name. */
export function asCopy(base64: string, name: string): string {
  const text = new TextDecoder().decode(Uint8Array.from(atob(base64), (c) => c.charCodeAt(0)));
  const value = JSON.parse(text) as { meta?: { id?: unknown; name?: unknown } };
  const meta = { ...(value.meta ?? {}) };
  delete meta.id;
  meta.name = name;
  const bytes = new TextEncoder().encode(JSON.stringify({ ...value, meta }));
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}
