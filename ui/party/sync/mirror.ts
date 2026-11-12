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
}

const key = (userId: string) => `party.mirror.${userId}`;

export function emptyMirror(userId: string): Mirror {
  return { userId, offsetMs: 0, lastSyncMs: 0, characters: {}, packs: {}, pending: [] };
}

export function loadMirror(userId: string): Mirror | null {
  let text: string | null = null;
  try { text = localStorage.getItem(key(userId)); } catch { /* storage refused: rebuilt from the files */ }
  if (!text) return null;
  try {
    const m = JSON.parse(text) as Partial<Mirror>;
    if (m.userId !== userId || typeof m.characters !== "object" || !m.characters || typeof m.packs !== "object" || !m.packs) return null;
    return { ...emptyMirror(userId), ...m, pending: Array.isArray(m.pending) ? m.pending : [] } as Mirror;
  } catch { return null; }
}

export function saveMirror(mirror: Mirror): void {
  try { localStorage.setItem(key(mirror.userId), JSON.stringify(mirror)); } catch { /* rebuilt from the files next time */ }
}
