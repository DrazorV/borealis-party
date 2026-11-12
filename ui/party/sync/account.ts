import { forgetMirror, heldByOthers, loadMirror, mirrorOwners } from "./mirror";
import { drop, exported, sha256, type LocalApi } from "./sync";

/**
 * The app's copy of an account when the account leaves the device: what would be lost, and removing it. A copy goes
 * when its account signs out here, and when another account signs in here (after a warning when it has edits the
 * server does not have). Signing in again as the same account keeps it, and the next sync sends its edits.
 */

/**
 * The names of the account's characters with edits made on this device that the server does not have. `open` are the
 * copies open on the page (local ids): read as they are, never closed or saved, so a Cancel leaves the page as it was.
 */
export async function unsentEdits(local: LocalApi, userId: string, open: ReadonlySet<string> = new Set()): Promise<string[]> {
  const mirror = loadMirror(userId);
  if (!mirror) return [];
  const entries = Object.values(mirror.characters);
  // Open in the backend from an earlier page (not this one): its edits go on file first, as a sync does.
  for (const m of entries) if (!open.has(m.local)) await local.closeCharacter({ characterId: m.local, discard: false });
  const listed = new Map((await local.listCharacters()).map((c) => [c.id, c]));
  const names: string[] = [];
  for (const m of entries) {
    const c = listed.get(m.local);
    if (!c) continue;
    const name = c.name || "A character";
    // Found again after a lost record and different from the server's file: waiting to be sent.
    if (m.baseUnknown) { names.push(name); continue; }
    const isOpen = open.has(m.local);
    if (!isOpen && c.updatedAt === m.localMtime) continue;
    try {
      const sha = isOpen ? await sha256((await local.exportCharacter({ characterId: m.local })).dataBase64 ?? "") : (await exported(local, m.local)).sha;
      if (sha !== m.localSha) names.push(name);
    } catch {
      names.push(name); // unreadable: it may hold an edit
    }
  }
  return names;
}

/** What other accounts' copies on this device hold that their server does not have, before one signs in here. */
export async function othersUnsentEdits(local: LocalApi, userId: string): Promise<string[]> {
  const names: string[] = [];
  for (const other of mirrorOwners()) if (other !== userId) names.push(...await unsentEdits(local, other));
  return names;
}

/**
 * Remove the account's copy from this device: its characters, and the packs and indexes when no other account keeps a
 * copy here. One step that fails does not stop the rest; what is left is in no account's record and never shown.
 */
export async function removeCopy(local: LocalApi, userId: string): Promise<void> {
  const others = heldByOthers(userId);
  const alone = mirrorOwners().every((o) => o === userId);
  // The record goes first: the WebView may lose a late storage write, and files in no record are never shown.
  forgetMirror(userId);
  const step = async (run: () => Promise<unknown>) => { try { await run(); } catch { /* left over */ } };
  await step(async () => {
    // An account's copies are the ones in its record, and spares of a stopped sync that are in no record.
    for (const c of await local.listCharacters()) if (!others.has(c.id)) await step(() => drop(local, c.id));
  });
  if (alone) {
    await step(async () => {
      for (const p of await local.listPacks()) if (p.kind !== "bundled") await step(() => local.removePack({ packId: p.id }));
    });
    await step(async () => {
      for (const i of await local.listIndexes()) await step(() => local.removeIndex({ url: i.url }));
    });
  }
}

/** Before another account signs in here: every other account's copy goes. */
export async function removeOtherCopies(local: LocalApi, userId: string): Promise<void> {
  for (const other of mirrorOwners()) if (other !== userId) await removeCopy(local, other);
}
