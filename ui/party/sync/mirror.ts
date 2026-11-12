/**
 * What the app remembers about its copy of the account (localStorage `party.mirror.<userId>`). The copy itself is in
 * the app's own backend; this is only how its files relate to the server's. Lost, it is rebuilt from the files
 * (each copy keeps the server's id as `meta.id`), at the price of fetching everything again.
 */

/** One character: the server's id S is the key, `local` is the id L the app's backend gave the copy. */
export interface MirroredCharacter {
  local: string;
  /** The server file's sha256 the copy was made from; null when unknown (rebuilt from the files). */
  serverSha: string | null;
  /** sha256 of the copy as upstream exports it, right after it was made: a later difference is an edit here. */
  localSha: string;
  /** The copy's `updatedAt` at that time. */
  localMtime: string;
  /** Found again after a lost record: `serverSha` is the server's file, not known to be this copy's base. */
  baseUnknown?: boolean;
}

export interface Mirror {
  userId: string;
  /** Server clock minus this device's, measured at the last sync. */
  offsetMs: number;
  lastSyncMs: number;
  characters: Record<string, MirroredCharacter>;
  /** Pack id to the server's pack digest the copy was made from. */
  packs: Record<string, string>;
  /** Server ids being copied: a copy of one of them that is in no entry here is a spare left by a stopped sync. */
  pending: string[];
  /** What the last sync could not do, by character or pack id (the Account page shows it). */
  problems: Record<string, string>;
}

const key = (userId: string) => `party.mirror.${userId}`;
const noticesKey = (userId: string) => `party.notices.${userId}`;
const MAX_NOTICES = 20;

export function emptyMirror(userId: string): Mirror {
  return { userId, offsetMs: 0, lastSyncMs: 0, characters: {}, packs: {}, pending: [], problems: {} };
}

/**
 * Edits made here that the server did not take, newest first, until dismissed on the Account page. Kept apart from
 * the record above, which a running sync writes from memory.
 */
export function loadNotices(userId: string): string[] {
  try {
    const list = JSON.parse(localStorage.getItem(noticesKey(userId)) ?? "[]") as unknown;
    return Array.isArray(list) ? list.filter((n): n is string => typeof n === "string") : [];
  } catch { return []; }
}

export function addNotices(userId: string, notices: string[]): void {
  if (notices.length === 0) return;
  const next = [...[...notices].reverse(), ...loadNotices(userId)].slice(0, MAX_NOTICES);
  try { localStorage.setItem(noticesKey(userId), JSON.stringify(next)); } catch { /* not kept */ }
}

export function clearNotices(userId: string): void {
  try { localStorage.removeItem(noticesKey(userId)); } catch { /* nothing kept */ }
}

export function loadMirror(userId: string): Mirror | null {
  let text: string | null = null;
  try { text = localStorage.getItem(key(userId)); } catch { /* storage refused: rebuilt from the files */ }
  if (!text) return null;
  try {
    const m = JSON.parse(text) as Partial<Mirror>;
    if (m.userId !== userId || typeof m.characters !== "object" || !m.characters || typeof m.packs !== "object" || !m.packs) return null;
    return {
      ...emptyMirror(userId), ...m,
      pending: Array.isArray(m.pending) ? m.pending : [],
      problems: m.problems && typeof m.problems === "object" ? m.problems : {},
    } as Mirror;
  } catch { return null; }
}

/**
 * The copies on this device that another account's record holds. A sync of this account never takes one over or
 * removes it, even when it carries an id this account's server also has (two accounts that opened the same bundle).
 */
export function heldByOthers(userId: string): Set<string> {
  const held = new Set<string>();
  try {
    for (let i = 0; i < localStorage.length; i += 1) {
      const k = localStorage.key(i);
      if (!k?.startsWith("party.mirror.") || k === key(userId)) continue;
      const other = loadMirror(k.slice("party.mirror.".length));
      for (const c of Object.values(other?.characters ?? {})) held.add(c.local);
    }
  } catch { /* storage refused: no record to keep apart */ }
  return held;
}

/** A whole copy to play with offline: one sync of this account ran to the end (a first sync cut off part-way is not). */
export function hasCopy(userId: string): boolean {
  return (loadMirror(userId)?.lastSyncMs ?? 0) > 0;
}

export function saveMirror(mirror: Mirror): void {
  try { localStorage.setItem(key(mirror.userId), JSON.stringify(mirror)); } catch { /* rebuilt from the files next time */ }
}
