import type { Api, ApiError, ContentPack, ContentSource } from "../../src/api/contract";
import { emptyMirror, loadMirror, saveMirror, type Mirror } from "./mirror";

/**
 * One sync of the app's copy of the account, while online: the server is read, the app's own backend is written.
 * Characters are imported with upstream `import_character`, which gives each copy an id of its own (the map in
 * ./mirror.ts); packs from an index are downloaded by the app from the same index, other packs come from the server
 * as one content file for upstream `import_pack`; the server's settings replace the app's.
 * The characters are copied before the index downloads, which can take minutes: upstream re-checks saved characters
 * when packs are installed.
 * Online, nobody edits the copy, so this may replace it freely.
 */

/** The app's own backend, as far as a sync uses it. */
export type LocalApi = Pick<Api,
  | "listCharacters" | "loadCharacter" | "exportCharacter" | "closeCharacter" | "deleteCharacter" | "importCharacter"
  | "listPacks" | "importPack" | "removePack" | "setPackEnabled" | "listIndexes" | "addIndex" | "removeIndex" | "updateIndexes"
  | "listSources" | "setSourceEdition" | "getSettings" | "saveSettings">;

/** A command of the server (`POST /api/rpc/<command>`). */
export type ServerCall = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

export interface SyncDeps {
  local: LocalApi;
  server: ServerCall;
  userId: string;
  /** Characters copied so far, of how many (for "Downloading your characters..."). */
  onProgress?: (done: number, total: number) => void;
  now?: () => number;
}

export interface SyncReport {
  /** The device had no copy of this account yet. */
  first: boolean;
  pulled: number;
  removed: number;
  packs: number;
  /** What did not copy, by character or pack id; it is tried again next time. */
  problems: Record<string, string>;
  /** What copied another way than planned. */
  notes: string[];
}

interface ServerState {
  nowMs: number;
  characters: { id: string; sha256: string }[];
  packs: ServerPack[];
}

interface ServerPack { id: string; sha256: string; enabled: boolean; indexUrl: string | null }

interface ServerFile {
  sha256: string;
  contentBase64: string;
}

/** Settings the server keeps for itself (party settings.rs FOLDERS and FIXED): never copied to the app. */
const SERVER_OWNED = ["documentsDir", "secondaryContentDir", "portraitLibraryDir", "vaultDir", "checkAppUpdates", "ai", "mobile", "storeBuild", "aiAvailable"];

const message = (e: unknown) => (e as Partial<ApiError>)?.message ?? String(e);

async function sha256(base64: string): Promise<string> {
  const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** A copy as upstream exports it: its digest and the server id it carries. */
async function exported(local: LocalApi, id: string): Promise<{ sha: string; metaId: string }> {
  await local.loadCharacter({ characterId: id });
  try {
    const file = await local.exportCharacter({ characterId: id });
    const data = file.dataBase64 ?? "";
    let metaId = "";
    try {
      const text = new TextDecoder().decode(Uint8Array.from(atob(data), (c) => c.charCodeAt(0)));
      metaId = String((JSON.parse(text) as { meta?: { id?: unknown } }).meta?.id ?? "");
    } catch { /* not ours */ }
    return { sha: await sha256(data), metaId };
  } finally {
    await local.closeCharacter({ characterId: id, discard: true });
  }
}

/** Delete a copy here; one already gone is fine. */
async function drop(local: LocalApi, id: string): Promise<void> {
  try {
    await local.deleteCharacter({ characterId: id });
  } catch (e) {
    if ((e as Partial<ApiError>)?.code !== "not_found") throw e;
  }
}

function withoutServerOwned(settings: object): Record<string, unknown> {
  const copy: Record<string, unknown> = { ...(settings as Record<string, unknown>) };
  for (const k of SERVER_OWNED) delete copy[k];
  return copy;
}

/** JSON with sorted keys, to compare settings. */
function stable(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((k) => `${JSON.stringify(k)}:${stable((value as Record<string, unknown>)[k])}`).join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}

/** The server's packs this device has no current copy of. */
async function stalePacks(local: LocalApi, st: ServerState, mirror: Mirror, fromIndex: boolean): Promise<ServerPack[]> {
  const here = new Set((await local.listPacks()).map((p) => p.id));
  return st.packs.filter((p) => Boolean(p.indexUrl) === fromIndex && (mirror.packs[p.id] !== p.sha256 || !here.has(p.id)));
}

/** Packs copied from the server as one content file each. True when content changed here. */
async function copyMerged(deps: SyncDeps, packs: ServerPack[], mirror: Mirror, report: SyncReport): Promise<boolean> {
  let changed = false;
  for (const p of packs) {
    try {
      const file = await deps.server("party_sync_pack", { id: p.id }) as ServerFile & { fileName: string };
      const result = await deps.local.importPack({ request: { fileName: file.fileName, contentBase64: file.contentBase64, format: "json" } });
      changed = true;
      if (result.pack?.id !== p.id) throw new Error(`it was installed as ${result.pack?.id ?? "nothing"}`);
      mirror.packs[p.id] = file.sha256;
      report.packs += 1;
    } catch (e) {
      report.problems[p.id] = `The pack ${p.id} could not be copied: ${message(e)}`;
    }
  }
  return changed;
}

/**
 * Packs from an index, downloaded here from the same address (which gives the same pack id); then what the server no
 * longer has, the on/off flags and the editions. True when content changed here.
 */
async function syncIndexPacks(deps: SyncDeps, st: ServerState, mirror: Mirror, report: SyncReport): Promise<boolean> {
  const { local, server } = deps;
  const fromIndex = await stalePacks(local, st, mirror, true);
  let changed = false;
  if (fromIndex.length > 0) {
    const urls = [...new Set(fromIndex.map((p) => p.indexUrl as string))];
    let failed: string[] = urls;
    let missing = new Map<string, number>();
    try {
      const known = new Set((await local.listIndexes()).map((i) => i.url));
      for (const url of urls) if (!known.has(url)) await local.addIndex({ url });
      failed = (await local.updateIndexes()).failed;
      missing = new Map((await local.listIndexes()).map((i) => [i.url, i.updatesAvailable]));
    } catch (e) {
      failed = urls;
      report.notes.push(`Downloading from the content indexes failed (${message(e)}).`);
    }
    changed = true;
    const here = new Set((await local.listPacks()).map((p) => p.id));
    const fallback: ServerPack[] = [];
    for (const p of fromIndex) {
      const url = p.indexUrl as string;
      // An Aurora index that missed files is held or partly installed: upstream lists the missed files in
      // `failed`, not the index, and leaves updates available on it.
      const complete = !failed.includes(url) && missing.get(url) === 0;
      if (here.has(p.id) && complete) {
        mirror.packs[p.id] = p.sha256;
        report.packs += 1;
      } else if (here.has(p.id)) {
        report.problems[p.id] = `The pack ${p.id} did not download completely from its index; it is tried again next time.`;
      } else {
        report.notes.push(`The pack ${p.id} did not download from its index here, so it was copied from the server.`);
        fallback.push(p);
      }
    }
    if (await copyMerged(deps, fallback, mirror, report)) changed = true;
  }

  // One refused command is a problem of its own: it does not stop the rest.
  const attempt = async (key: string, what: string, step: () => Promise<void>) => {
    try { await step(); } catch (e) { report.problems[key] = `${what}: ${message(e)}`; }
  };

  // What the server no longer has. Nothing else adds packs or indexes here: content changes need the server.
  const onServer = new Set(st.packs.map((p) => p.id));
  for (const p of await local.listPacks()) {
    if (p.kind !== "bundled" && !onServer.has(p.id)) {
      await attempt(p.id, `The pack ${p.id} could not be removed`, async () => { await local.removePack({ packId: p.id }); changed = true; });
    }
  }
  for (const id of Object.keys(mirror.packs)) if (!onServer.has(id)) delete mirror.packs[id];
  const usedIndexes = new Set(st.packs.map((p) => p.indexUrl).filter(Boolean));
  for (const i of await local.listIndexes()) {
    if (!usedIndexes.has(i.url)) await attempt(i.url, `The index ${i.url} could not be removed`, async () => { await local.removeIndex({ url: i.url }); });
  }

  const localPacks = await local.listPacks();
  for (const p of st.packs) {
    const here: ContentPack | undefined = localPacks.find((l) => l.id === p.id);
    if (here && here.enabled !== p.enabled) {
      await attempt(p.id, `The pack ${p.id} could not be turned ${p.enabled ? "on" : "off"}`, async () => {
        await local.setPackEnabled({ packId: p.id, enabled: p.enabled });
        changed = true;
      });
    }
  }

  // The editions the account chose for books of index packs (merged packs carry them in the file).
  if (changed && fromIndex.length > 0) {
    await attempt("editions", "The editions of the books could not be copied", async () => {
      const ids = new Set(fromIndex.map((p) => p.id));
      const chosen = (await server("list_sources") as ContentSource[]).filter((s) => s.editionSetByUser && s.edition && ids.has(s.packId));
      const here = new Map((await local.listSources()).map((s) => [s.id, s]));
      for (const s of chosen) {
        if (here.has(s.id) && here.get(s.id)?.edition !== s.edition) await local.setSourceEdition({ sourceId: s.id, edition: s.edition! });
      }
    });
  }
  return changed;
}

/** Copy the account to the app. Throws when the server or the app's backend can't be reached; nothing is lost then. */
export async function syncOnce(deps: SyncDeps): Promise<SyncReport> {
  const { local, server, userId } = deps;
  const now = deps.now ?? Date.now;
  const known = loadMirror(userId);
  const mirror = known ?? emptyMirror(userId);
  const report: SyncReport = { first: !known, pulled: 0, removed: 0, packs: 0, problems: {}, notes: [] };

  const t0 = now();
  const st = await server("party_sync_state") as ServerState;
  mirror.offsetMs = Math.round(st.nowMs - (t0 + now()) / 2);
  const onServer = new Set(st.characters.map((c) => c.id));

  const localList = await local.listCharacters();
  const localIds = new Set(localList.map((c) => c.id));
  for (const [s, m] of Object.entries(mirror.characters)) if (!localIds.has(m.local)) delete mirror.characters[s];
  if (!known || mirror.pending.length > 0) {
    // A lost or new mirror: copies already here are found by the server id they carry, and fetched again.
    // A stopped sync: a copy of a server id it was copying that is in no entry is a spare, and goes.
    const mapped = new Set(Object.values(mirror.characters).map((m) => m.local));
    for (const c of localList) {
      if (mapped.has(c.id)) continue;
      try {
        const { sha, metaId } = await exported(local, c.id);
        if (mirror.pending.includes(metaId)) {
          await drop(local, c.id);
        } else if (!known && onServer.has(metaId) && !mirror.characters[metaId]) {
          mirror.characters[metaId] = { local: c.id, serverSha: null, localSha: sha, localMtime: c.updatedAt };
        }
      } catch { /* unreadable: left as it is */ }
    }
    mirror.pending = [];
  }

  // Merged packs are quick and come first; the index downloads wait until the characters are here.
  let packsChanged = await copyMerged(deps, await stalePacks(local, st, mirror, false), mirror, report);
  saveMirror(mirror);

  const pulledNow = new Set<string>();
  const todo = st.characters.filter((c) => mirror.characters[c.id]?.serverSha !== c.sha256);
  deps.onProgress?.(0, todo.length);
  for (const [n, c] of todo.entries()) {
    let spare: string | null = null;
    try {
      const file = await server("party_sync_get", { id: c.id }) as ServerFile;
      // Saved before the import: a sync stopped before the entry below leaves a spare the next sync removes.
      mirror.pending = [...new Set([...mirror.pending, c.id])];
      saveMirror(mirror);
      const imported = await local.importCharacter({ fileName: `${c.id}.json`, contentBase64: file.contentBase64 });
      spare = imported.summary.id;
      const id = spare;
      const { sha } = await exported(local, id);
      const before = mirror.characters[c.id];
      mirror.characters[c.id] = { local: id, serverSha: file.sha256, localSha: sha, localMtime: imported.summary.updatedAt };
      spare = null;
      // Saved before the old copy goes: a crash in between leaves a spare copy, never no copy.
      saveMirror(mirror);
      if (before && before.local !== id) await drop(local, before.local);
      mirror.pending = mirror.pending.filter((s) => s !== c.id);
      pulledNow.add(id);
      report.pulled += 1;
    } catch (e) {
      // The new copy is in no entry: it goes now, or else at the next sync by the server id it carries.
      if (spare) {
        try {
          await drop(local, spare);
          mirror.pending = mirror.pending.filter((s) => s !== c.id);
        } catch { /* the next sync */ }
      }
      // Deleted on the server since the state was read: the next sync drops it.
      if ((e as Partial<ApiError>)?.code !== "not_found") report.problems[c.id] = `A character could not be copied: ${message(e)}`;
    }
    saveMirror(mirror);
    deps.onProgress?.(n + 1, todo.length);
  }

  // Deleted on the server.
  for (const [s, m] of Object.entries(mirror.characters)) {
    if (onServer.has(s)) continue;
    try {
      await drop(local, m.local);
      delete mirror.characters[s];
      report.removed += 1;
    } catch (e) {
      report.problems[s] = `A character deleted on the server could not be removed here: ${message(e)}`;
    }
  }
  saveMirror(mirror);

  // The server's settings win, less the folders and flags that belong to each machine.
  const wanted = withoutServerOwned(await server("get_settings") as object);
  if (stable(wanted) !== stable(withoutServerOwned(await local.getSettings()))) await local.saveSettings({ patch: wanted });

  if (await syncIndexPacks(deps, st, mirror, report)) packsChanged = true;
  saveMirror(mirror);

  // New content can rewrite character files here (missing content found), and a copy made before its packs were
  // here may export differently now: neither is an edit.
  if (packsChanged) {
    for (const c of await local.listCharacters()) {
      const m = Object.values(mirror.characters).find((x) => x.local === c.id);
      if (m && (m.localMtime !== c.updatedAt || pulledNow.has(c.id))) {
        m.localSha = (await exported(local, c.id)).sha;
        m.localMtime = c.updatedAt;
      }
    }
  }

  mirror.lastSyncMs = now();
  saveMirror(mirror);
  return report;
}
