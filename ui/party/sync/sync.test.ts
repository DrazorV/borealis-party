import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ContentPack, ContentSource } from "../../src/api/contract";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { chooseConflict, loadChoices, pendingConflicts } from "./conflicts";
import { clearNotices, loadMirror, loadNotices } from "./mirror";
import { lastSync, requestSync, resumeSync, startSync } from "./runner";
import { syncOnce, type LocalApi, type ServerCall } from "./sync";

const b64 = (text: string) => btoa(text);
const unb64 = (data: string) => atob(data);
const character = (id: string, name: string, hp = 10) => JSON.stringify({ meta: { id, name }, hp });
const pack = (id: string, kind: ContentPack["kind"] = "homebrew", enabled = true) => ({ id, kind, enabled }) as ContentPack;

/** The app's backend in memory: what upstream's commands do to its files, as far as a sync can tell. */
function fakeLocal() {
  let next = 0;
  let clock = 1000;
  const files = new Map<string, { text: string; updatedAt: string }>();
  const open = new Set<string>();
  const state = {
    files,
    packs: [pack("srd", "bundled")] as ContentPack[],
    indexes: [] as string[],
    /** What `update_indexes` installs, by index URL. */
    indexPacks: {} as Record<string, string[]>,
    /** Indexes whose update misses files (held: nothing installed or changed). */
    held: [] as string[],
    sources: [] as ContentSource[],
    settings: { theme: "dark", documentsDir: "/device/docs", mobile: true } as Record<string, unknown>,
    calls: [] as string[],
  };
  const stamp = () => new Date((clock += 1000)).toISOString();
  const api: LocalApi = {
    listCharacters: async () => [...files].map(([id, f]) => {
      let name = "";
      try { name = String((JSON.parse(f.text) as { meta?: { name?: string } }).meta?.name ?? ""); } catch { /* not JSON */ }
      return { id, name, updatedAt: f.updatedAt } as never;
    }),
    loadCharacter: async ({ characterId }) => {
      if (!files.has(characterId)) throw { code: "not_found", message: "gone" };
      open.add(characterId);
      return {} as never;
    },
    exportCharacter: async ({ characterId }) => {
      if (!open.has(characterId)) throw { code: "not_found", message: "not open" };
      return { fileName: "x.borealis.json", mime: "application/json", path: null, dataBase64: b64(files.get(characterId)!.text) };
    },
    closeCharacter: async ({ characterId }) => { open.delete(characterId); },
    deleteCharacter: async ({ characterId }) => {
      state.calls.push(`delete ${characterId}`);
      if (!files.delete(characterId)) throw { code: "not_found", message: "gone" };
    },
    importCharacter: async ({ contentBase64 }) => {
      const id = `L${(next += 1)}`;
      const updatedAt = stamp();
      files.set(id, { text: unb64(contentBase64), updatedAt });
      state.calls.push(`import ${id}`);
      return { summary: { id, updatedAt } } as never;
    },
    listPacks: async () => state.packs.map((p) => ({ ...p })),
    importPack: async ({ request }) => {
      const id = request.fileName.replace(/\.json$/, "");
      state.calls.push(`importPack ${id}`);
      state.packs = [...state.packs.filter((p) => p.id !== id), pack(id)];
      return { pack: pack(id), report: {} } as never;
    },
    removePack: async ({ packId }) => {
      state.calls.push(`removePack ${packId}`);
      state.packs = state.packs.filter((p) => p.id !== packId);
      return [];
    },
    setPackEnabled: async ({ packId, enabled }) => {
      state.calls.push(`setPackEnabled ${packId} ${enabled}`);
      state.packs = state.packs.map((p) => (p.id === packId ? { ...p, enabled } : p));
      return [];
    },
    listIndexes: async () => state.indexes.map((url) => ({ url, updatesAvailable: state.held.includes(url) ? 3 : 0 }) as never),
    addIndex: async ({ url }) => { state.calls.push(`addIndex ${url}`); state.indexes.push(url); return []; },
    removeIndex: async ({ url }) => { state.calls.push(`removeIndex ${url}`); state.indexes = state.indexes.filter((u) => u !== url); return []; },
    updateIndexes: async () => {
      state.calls.push("updateIndexes");
      const failed: string[] = [];
      for (const url of state.indexes) {
        // Upstream holds an Aurora index that missed files and lists the files, not the index, as failed.
        if (state.held.includes(url)) { failed.push(`${url}/missed.xml`); continue; }
        for (const id of state.indexPacks[url] ?? []) if (!state.packs.some((p) => p.id === id)) state.packs.push(pack(id, "downloaded"));
      }
      return { updated: 1, failed, reloaded: true, changed: [], newHosts: [], reports: [] };
    },
    listSources: async () => state.sources,
    setSourceEdition: async ({ sourceId, edition }) => {
      state.calls.push(`setSourceEdition ${sourceId} ${edition}`);
      state.sources = state.sources.map((s) => (s.id === sourceId ? { ...s, edition } : s));
      return [];
    },
    getSettings: async () => ({ ...state.settings }) as never,
    saveSettings: async ({ patch }) => {
      state.calls.push("saveSettings");
      state.settings = { ...state.settings, ...patch };
      return state.settings as never;
    },
  };
  return { api, state };
}

/** The server: an account's characters and packs, answering the party_sync_* commands as P2 does. */
function fakeServer() {
  const state = {
    characters: new Map<string, string>(),
    packs: [] as { id: string; sha256: string; enabled: boolean; indexUrl: string | null }[],
    sources: [] as ContentSource[],
    settings: { theme: "light", documentsDir: null, mobile: true, checkAppUpdates: false } as Record<string, unknown>,
    calls: [] as string[],
    /** Edit stamps of the server's files (0 when not set). */
    edited: new Map<string, number>(),
    /** Delete records: id to the time of the delete. */
    deleted: new Map<string, number>(),
    /** What a push left in the account's trash. */
    trash: [] as string[],
    pushes: [] as { id: string; baseSha: string | null; editedAtMs: number }[],
    /** A push answered with this error. */
    refuse: null as null | { code: string; message: string },
    /** Runs as a push arrives, after the state was read (a change made on the server in between). */
    beforePush: null as null | ((id: string) => void),
    /** A 0.3.6 server: a push with `ask` from a stale base is a conflict, `force` writes it over. Off: the server before. */
    asks: false,
    /** Characters made by `import_character`, by id. */
    imported: [] as string[],
    /** `import_character` makes the character, then answers with this error (a lost answer, or a refusal). */
    importThrows: null as null | { code: string; message: string },
  };
  const sha = (text: string) => `sha(${text})`;
  /** party_sync_push as the server decides it (sync.rs). */
  const push = (args: Record<string, unknown>) => {
    const id = String(args.id);
    const text = unb64(String(args.contentBase64));
    const baseSha = (args.baseSha ?? null) as string | null;
    const editedAtMs = Number(args.editedAtMs);
    state.pushes.push({ id, baseSha, editedAtMs });
    if (state.refuse) throw state.refuse;
    state.beforePush?.(id);
    const current = state.characters.get(id);
    const take = () => {
      state.characters.set(id, text);
      state.edited.set(id, editedAtMs);
      return { result: "applied", sha256: sha(text) };
    };
    if (state.asks && args.ask) {
      if (args.discard) {
        state.trash.push(text);
        return { result: "discarded", sha256: current === undefined ? null : sha(current) };
      }
      if (current === text) return { result: "identical", sha256: sha(text) };
      if (typeof args.forceOver === "string" && args.forceOver === (current === undefined ? "" : sha(current))) return take();
      if (current !== undefined && baseSha === sha(current)) return take();
      if (current === undefined && !state.deleted.has(id)) return take();
      return { result: "conflict", sha256: current === undefined ? null : sha(current) };
    }
    if (current !== undefined) {
      if (current === text) return { result: "identical", sha256: sha(text) };
      if (baseSha === sha(current) || editedAtMs > (state.edited.get(id) ?? 0)) return take();
      state.trash.push(text);
      return { result: "kept-server", sha256: sha(current) };
    }
    const at = state.deleted.get(id);
    if (at !== undefined && editedAtMs <= at) {
      state.trash.push(text);
      return { result: "deleted", sha256: null };
    }
    return take();
  };
  const call: ServerCall = async (command, args) => {
    state.calls.push(args?.id ? `${command} ${String(args.id)}` : command);
    switch (command) {
      case "party_sync_push": return push(args ?? {});
      case "party_sync_state":
        return {
          nowMs: 5000,
          characters: [...state.characters].map(([id, text]) => ({ id, sha256: sha(text) })),
          packs: state.packs,
          deleted: [],
        };
      case "party_sync_get": {
        const text = state.characters.get(String(args?.id));
        if (!text) throw { code: "not_found", message: "That character no longer exists." };
        return { sha256: sha(text), contentBase64: b64(text) };
      }
      case "party_sync_pack": {
        const p = state.packs.find((x) => x.id === args?.id);
        if (!p) throw { code: "not_found", message: "That pack is not installed." };
        return { fileName: `${p.id}.json`, sha256: p.sha256, contentBase64: b64("{}") };
      }
      case "import_character": {
        const text = unb64(String(args?.contentBase64));
        const id = `c_new${state.imported.length + 1}`;
        state.imported.push(id);
        state.characters.set(id, text);
        if (state.importThrows) throw state.importThrows;
        return { summary: { id } };
      }
      case "list_characters":
        return [...state.characters].map(([id, text]) => ({ id, name: (JSON.parse(text) as { meta: { name: string } }).meta.name }));
      case "list_sources": return state.sources;
      case "get_settings": return state.settings;
      default: throw { code: "not_found", message: "The server does not offer that." };
    }
  };
  return { call, state };
}

let local: ReturnType<typeof fakeLocal>;
let server: ReturnType<typeof fakeServer>;
const sync = () => syncOnce({ local: local.api, server: server.call, userId: "u1", now: () => 4000 });
const localTexts = () => [...local.state.files.values()].map((f) => f.text).sort();

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  local = fakeLocal();
  server = fakeServer();
  server.state.characters.set("c_a", character("c_a", "Ana"));
  server.state.characters.set("c_b", character("c_b", "Bo"));
});

afterEach(() => { vi.unstubAllGlobals(); });

it("first sync copies every character and keeps the server id of each copy", async () => {
  const progress: string[] = [];
  const report = await syncOnce({ local: local.api, server: server.call, userId: "u1", now: () => 4000, onProgress: (d, t) => progress.push(`${d}/${t}`) });
  expect(report).toMatchObject({ first: true, pulled: 2, removed: 0, problems: {} });
  expect(progress).toEqual(["0/2", "1/2", "2/2"]);
  expect(localTexts()).toEqual([character("c_a", "Ana"), character("c_b", "Bo")]);
  const mirror = loadMirror("u1")!;
  expect(Object.keys(mirror.characters).sort()).toEqual(["c_a", "c_b"]);
  expect(mirror.characters.c_a).toMatchObject({ serverSha: `sha(${character("c_a", "Ana")})` });
  expect(local.state.files.get(mirror.characters.c_a!.local)?.text).toContain("Ana");
  // Server clock 5000 at device 4000.
  expect(mirror.offsetMs).toBe(1000);
});

it("a later sync fetches only what changed, replaces the copy and drops the old one", async () => {
  await sync();
  const before = loadMirror("u1")!.characters;
  server.state.calls = [];
  server.state.characters.set("c_b", character("c_b", "Bo", 7));
  const report = await sync();
  expect(report).toMatchObject({ first: false, pulled: 1, removed: 0 });
  expect(server.state.calls.filter((c) => c.startsWith("party_sync_get"))).toEqual(["party_sync_get c_b"]);
  const after = loadMirror("u1")!.characters;
  expect(after.c_a).toEqual(before.c_a);
  expect(after.c_b!.local).not.toBe(before.c_b!.local);
  expect(local.state.files.has(before.c_b!.local)).toBe(false);
  expect(localTexts()).toEqual([character("c_a", "Ana"), character("c_b", "Bo", 7)]);
});

it("nothing changed: nothing is fetched or written", async () => {
  await sync();
  server.state.calls = [];
  local.state.calls = [];
  const report = await sync();
  expect(report).toMatchObject({ pulled: 0, removed: 0, packs: 0 });
  expect(server.state.calls).toEqual(["party_sync_state", "get_settings"]);
  expect(local.state.calls).toEqual([]);
});

it("a character deleted on the server goes from the device", async () => {
  await sync();
  const gone = loadMirror("u1")!.characters.c_a!.local;
  server.state.characters.delete("c_a");
  const report = await sync();
  expect(report.removed).toBe(1);
  expect(local.state.files.has(gone)).toBe(false);
  expect(loadMirror("u1")!.characters.c_a).toBeUndefined();
});

it("a copy removed on the device is fetched again", async () => {
  await sync();
  local.state.files.delete(loadMirror("u1")!.characters.c_a!.local);
  expect((await sync()).pulled).toBe(1);
  expect(localTexts()).toEqual([character("c_a", "Ana"), character("c_b", "Bo")]);
});

it("a lost mirror is rebuilt from the copies' server ids, with no copy twice", async () => {
  await sync();
  localStorage.clear();
  const report = await sync();
  expect(report.first).toBe(true);
  expect(localTexts()).toEqual([character("c_a", "Ana"), character("c_b", "Bo")]);
  expect(Object.keys(loadMirror("u1")!.characters).sort()).toEqual(["c_a", "c_b"]);
});

it("a lost mirror after an offline edit keeps the edited copy, waiting, and later syncs leave it too", async () => {
  await sync();
  const m = loadMirror("u1")!.characters.c_a!;
  local.state.files.set(m.local, { text: character("c_a", "Ana", 3), updatedAt: "offline edit" });
  const closed: string[] = [];
  const close = local.api.closeCharacter;
  local.api.closeCharacter = async (args) => { closed.push(`${args.characterId} ${args.discard}`); return close(args); };
  localStorage.clear();
  const report = await sync();
  expect(report.waiting).toEqual(["c_a"]);
  // Saved before it was read: an open session's edits are not discarded.
  expect(closed[0]).toBe(`${m.local} false`);
  expect(local.state.files.get(m.local)?.text).toBe(character("c_a", "Ana", 3));
  expect(localTexts()).toEqual([character("c_a", "Ana", 3), character("c_b", "Bo")]);
  expect(loadMirror("u1")!.characters.c_a!.local).toBe(m.local);

  // Not known to be made from the server's file: sent without a base, so the server weighs the edit times.
  server.state.characters.set("c_a", character("c_a", "Ana", 20));
  server.state.edited.set("c_a", 999_999_999);
  const again = await sync();
  expect(server.state.pushes).toEqual([{ id: "c_a", baseSha: null, editedAtMs: expect.any(Number) }]);
  expect(again).toMatchObject({ waiting: [], pulled: 1 });
  // The server's edit was newer: its copy is here now, and this device's is in the trash there.
  expect(server.state.trash).toEqual([character("c_a", "Ana", 3)]);
  expect(localTexts()).toEqual([character("c_a", "Ana", 20), character("c_b", "Bo")]);
});

it("a character deleted between the state and the fetch is no problem", async () => {
  const get = server.call;
  server.call = async (command, args) => {
    if (command === "party_sync_get" && args?.id === "c_a") throw { code: "not_found", message: "That character no longer exists." };
    return get(command, args);
  };
  const report = await syncOnce({ local: local.api, server: server.call, userId: "u1" });
  expect(report).toMatchObject({ pulled: 1, problems: {} });
});

it("packs: merged packs are imported, index packs come from the same index, extras are removed, flags follow", async () => {
  const INDEX = "https://example.org/legacy.index";
  server.state.packs = [
    { id: "homebrew", sha256: "h1", enabled: true, indexUrl: null },
    { id: "dl-legacy-1234abcd", sha256: "d1", enabled: false, indexUrl: INDEX },
  ];
  local.state.indexPacks[INDEX] = ["dl-legacy-1234abcd", "dl-legacy-other"];
  local.state.packs.push(pack("old-pack"));
  local.state.indexes.push("https://example.org/forgotten.index");
  server.state.sources = [{ id: "src-legacy", packId: "dl-legacy-1234abcd", edition: "dnd2024", editionSetByUser: true } as ContentSource];
  local.state.sources = [{ id: "src-legacy", packId: "dl-legacy-1234abcd", edition: "dnd2014", editionSetByUser: false } as ContentSource];

  const report = await sync();
  expect(report).toMatchObject({ packs: 2, problems: {}, notes: [] });
  expect(local.state.packs.map((p) => `${p.id}:${p.enabled}`).sort()).toEqual(["dl-legacy-1234abcd:false", "homebrew:true", "srd:true"]);
  expect(local.state.indexes).toEqual([INDEX]);
  expect(local.state.calls).toEqual(expect.arrayContaining([
    `addIndex ${INDEX}`, "updateIndexes", "importPack homebrew", "removePack old-pack", "removePack dl-legacy-other",
    "removeIndex https://example.org/forgotten.index", "setPackEnabled dl-legacy-1234abcd false", "setSourceEdition src-legacy dnd2024",
  ]));
  expect(loadMirror("u1")!.packs).toEqual({ homebrew: "h1", "dl-legacy-1234abcd": "d1" });

  // Unchanged packs are left alone; a changed one is fetched again; one the server dropped goes.
  local.state.calls = [];
  server.state.packs = [{ id: "homebrew", sha256: "h2", enabled: true, indexUrl: null }];
  await sync();
  expect(local.state.calls.filter((c) => !c.startsWith("import ") && !c.startsWith("delete "))).toEqual([
    "importPack homebrew", "removePack dl-legacy-1234abcd", `removeIndex ${INDEX}`,
  ]);
  expect(loadMirror("u1")!.packs).toEqual({ homebrew: "h2" });
});

it("an index pack that installs under another id here is copied from the server instead, and said so", async () => {
  const INDEX = "https://example.org/legacy.index";
  server.state.packs = [{ id: "dl-legacy-1234abcd", sha256: "d1", enabled: true, indexUrl: INDEX }];
  local.state.indexPacks[INDEX] = ["dl-legacy-ffffffff"];
  const report = await sync();
  expect(report.notes).toEqual(["The pack dl-legacy-1234abcd did not download from its index here, so it was copied from the server."]);
  expect(local.state.calls).toContain("importPack dl-legacy-1234abcd");
  expect(loadMirror("u1")!.packs).toEqual({ "dl-legacy-1234abcd": "d1" });
});

it("a pack the server can't merge is a problem, tried again next time", async () => {
  server.state.packs = [{ id: "odd", sha256: "o1", enabled: true, indexUrl: null }];
  const get = server.call;
  server.call = async (command, args) => {
    if (command === "party_sync_pack") throw { code: "invalid_argument", message: "The pack \"odd\" cannot be copied to a device: its id is not a plain name." };
    return get(command, args);
  };
  const report = await syncOnce({ local: local.api, server: server.call, userId: "u1" });
  expect(report.problems.odd).toContain("cannot be copied");
  expect(report.pulled).toBe(2);
  expect(loadMirror("u1")!.packs).toEqual({});
});

it("a pack change that rewrites a copy is not taken for an edit", async () => {
  await sync();
  const m = loadMirror("u1")!.characters.c_a!;
  // Upstream rewrites the file when the new pack has the content it was missing.
  const importPack = local.api.importPack;
  local.api.importPack = async (args) => {
    local.state.files.set(m.local, { text: character("c_a", "Ana", 11), updatedAt: "later" });
    return importPack(args);
  };
  server.state.packs = [{ id: "homebrew", sha256: "h1", enabled: true, indexUrl: null }];
  const report = await sync();
  expect(report.waiting).toEqual([]);
  const after = loadMirror("u1")!.characters.c_a!;
  expect(after.local).toBe(m.local);
  expect(after.localMtime).toBe("later");
  expect(after.localSha).not.toBe(m.localSha);
});

it("a copy whose edit the server refused waits: a server change does not replace it, a server delete does not remove it", async () => {
  await sync();
  const m = loadMirror("u1")!.characters.c_a!;
  // Played offline: the file on the device changed.
  local.state.files.set(m.local, { text: character("c_a", "Ana", 3), updatedAt: "offline edit" });
  server.state.characters.set("c_a", character("c_a", "Ana", 20));
  server.state.calls = [];
  server.state.refuse = { code: "invalid_argument", message: "The character can't be taken: not a Borealis character." };
  const report = await sync();
  expect(report.waiting).toEqual(["c_a"]);
  expect(report.problems.c_a).toBe("Your changes to Ana could not be sent: The character can't be taken: not a Borealis character.");
  expect(loadMirror("u1")!.problems).toEqual(report.problems);
  expect(report.pulled).toBe(0);
  expect(server.state.calls).not.toContain("party_sync_get c_a");
  expect(local.state.files.get(m.local)?.text).toBe(character("c_a", "Ana", 3));
  expect(loadMirror("u1")!.characters.c_a).toEqual(m);

  server.state.characters.delete("c_a");
  // A pack change re-reads the copies afterwards: still not taken for the base.
  server.state.packs = [{ id: "homebrew", sha256: "h1", enabled: true, indexUrl: null }];
  const again = await sync();
  expect(again).toMatchObject({ waiting: ["c_a"], removed: 0 });
  expect(local.state.files.get(m.local)?.text).toBe(character("c_a", "Ana", 3));
  expect(loadMirror("u1")!.characters.c_a).toEqual(m);
});

it("a copy with a new date and the same content is not an edit", async () => {
  await sync();
  const m = loadMirror("u1")!.characters.c_a!;
  local.state.files.set(m.local, { text: local.state.files.get(m.local)!.text, updatedAt: "touched" });
  server.state.characters.set("c_a", character("c_a", "Ana", 20));
  const report = await sync();
  expect(report).toMatchObject({ waiting: [], pulled: 1 });
  expect(localTexts()).toContain(character("c_a", "Ana", 20));
});

it("a copy left open by an offline page is saved before it is looked at", async () => {
  await sync();
  const closed: string[] = [];
  const close = local.api.closeCharacter;
  local.api.closeCharacter = async (args) => {
    closed.push(`${args.characterId} ${args.discard}`);
    return close(args);
  };
  await sync();
  const mirror = loadMirror("u1")!;
  expect(closed.slice(0, 2).sort()).toEqual([`${mirror.characters.c_a!.local} false`, `${mirror.characters.c_b!.local} false`].sort());
});

it("the server's settings replace the app's, less its own folders and flags", async () => {
  await sync();
  expect(local.state.settings).toEqual({ theme: "light", documentsDir: "/device/docs", mobile: true });
  local.state.calls = [];
  await sync();
  expect(local.state.calls).not.toContain("saveSettings");
});

it("the party's own account settings are not the app's: a sync with nothing changed writes no settings", async () => {
  // The app's backend has no field for them and drops them, so they never compare equal.
  server.state.settings = { ...server.state.settings, skipPersonalityChoices: false };
  await sync();
  expect(local.state.settings).not.toHaveProperty("skipPersonalityChoices");
  local.state.calls = [];
  await sync();
  expect(local.state.calls).not.toContain("saveSettings");
});

it("a server that can't be reached changes nothing on the device", async () => {
  await sync();
  const files = new Map(local.state.files);
  server.call = async () => { throw { code: "network", message: "The server can't be reached. Check your connection." }; };
  await expect(syncOnce({ local: local.api, server: server.call, userId: "u1" })).rejects.toMatchObject({ code: "network" });
  expect(local.state.files).toEqual(files);
});

it("each account has a copy record of its own", async () => {
  await sync();
  const first = loadMirror("u1")!;
  const other = fakeServer();
  other.state.characters.set("c_z", character("c_z", "Zed"));
  await syncOnce({ local: local.api, server: other.call, userId: "u2", now: () => 4000 });
  expect(Object.keys(loadMirror("u2")!.characters)).toEqual(["c_z"]);
  expect(loadMirror("u1")).toEqual(first);
  // The first account's copies are still on the device (P4 decides when they go).
  expect(localTexts()).toEqual([character("c_a", "Ana"), character("c_b", "Bo"), character("c_z", "Zed")]);
});

it("an index update that missed files is not taken for a copy: tried again next time", async () => {
  const INDEX = "https://example.org/legacy.index";
  server.state.packs = [{ id: "dl-legacy-1234abcd", sha256: "d1", enabled: true, indexUrl: INDEX }];
  local.state.indexPacks[INDEX] = ["dl-legacy-1234abcd"];
  await sync();
  expect(loadMirror("u1")!.packs).toEqual({ "dl-legacy-1234abcd": "d1" });

  // The server's pack changed; the device's download is held (the old pack stays).
  server.state.packs = [{ id: "dl-legacy-1234abcd", sha256: "d2", enabled: true, indexUrl: INDEX }];
  local.state.held = [INDEX];
  const report = await sync();
  expect(report.problems["dl-legacy-1234abcd"]).toContain("did not download completely");
  expect(local.state.calls).not.toContain("importPack dl-legacy-1234abcd");
  expect(loadMirror("u1")!.packs).toEqual({ "dl-legacy-1234abcd": "d1" });

  local.state.held = [];
  local.state.calls = [];
  expect((await sync()).problems).toEqual({});
  expect(local.state.calls).toContain("updateIndexes");
  expect(loadMirror("u1")!.packs).toEqual({ "dl-legacy-1234abcd": "d2" });
});

it("characters are copied before the index downloads", async () => {
  const INDEX = "https://example.org/legacy.index";
  server.state.packs = [{ id: "dl-legacy-1234abcd", sha256: "d1", enabled: true, indexUrl: INDEX }];
  local.state.indexPacks[INDEX] = ["dl-legacy-1234abcd"];
  await sync();
  const calls = local.state.calls;
  expect(calls.indexOf("import L2")).toBeGreaterThanOrEqual(0);
  expect(calls.indexOf("import L2")).toBeLessThan(calls.indexOf("updateIndexes"));
});

it("a refused pack command is a problem, and the characters and settings still copy", async () => {
  local.state.packs.push(pack("old-pack"));
  local.api.removePack = async () => { throw { code: "io_error", message: "disk full" }; };
  const report = await sync();
  expect(report.problems["old-pack"]).toContain("disk full");
  expect(report.pulled).toBe(2);
  expect(local.state.settings.theme).toBe("light");
});

it("a copy that fails after its import is removed: one copy per character", async () => {
  const exportCharacter = local.api.exportCharacter;
  let fail = true;
  local.api.exportCharacter = async (args) => {
    if (fail) { fail = false; throw { code: "internal", message: "no digest here" }; }
    return exportCharacter(args);
  };
  const report = await sync();
  expect(Object.keys(report.problems)).toHaveLength(1);
  expect(local.state.files.size).toBe(1);
  await sync();
  expect(localTexts()).toEqual([character("c_a", "Ana"), character("c_b", "Bo")]);
  expect(loadMirror("u1")!.pending).toEqual([]);
});

it("a sync stopped right after an import leaves no spare copy after the next sync", async () => {
  await sync();
  server.state.characters.set("c_b", character("c_b", "Bo", 7));
  const importCharacter = local.api.importCharacter;
  // The app is killed once the file is written, before the copy is recorded.
  local.api.importCharacter = async (args) => { await importCharacter(args); throw new Error("killed"); };
  await sync().catch(() => {});
  expect(local.state.files.size).toBe(3);
  local.api.importCharacter = importCharacter;
  await sync();
  expect(localTexts()).toEqual([character("c_a", "Ana"), character("c_b", "Bo", 7)]);
  expect(loadMirror("u1")!.pending).toEqual([]);
});

/** Edit the device's copy of `s` as an offline page does, at device time `atMs`. */
function editHere(s: string, text: string, atMs = 3500) {
  const m = loadMirror("u1")!.characters[s]!;
  local.state.files.set(m.local, { text, updatedAt: new Date(atMs).toISOString() });
  return m.local;
}

it("only edited copies are sent, before anything is fetched, and the server's answer is the new base", async () => {
  await sync();
  server.state.calls = [];
  const edited = character("c_a", "Ana", 3);
  const here = editHere("c_a", edited);
  // Changed on the server too: c_b is fetched after the push.
  server.state.characters.set("c_b", character("c_b", "Bo", 8));
  const report = await sync();
  expect(report).toMatchObject({ pushed: 1, waiting: [], pulled: 1, problems: {}, notices: [] });
  expect(server.state.calls).toEqual(["party_sync_state", "party_sync_push c_a", "party_sync_get c_b", "get_settings"]);
  expect(server.state.pushes).toEqual([{ id: "c_a", baseSha: `sha(${character("c_a", "Ana")})`, editedAtMs: 3500 + 1000 }]);
  expect(server.state.characters.get("c_a")).toBe(edited);
  const m = loadMirror("u1")!.characters.c_a!;
  expect(m).toMatchObject({ local: here, serverSha: `sha(${edited})` });
  expect(local.state.files.get(here)?.text).toBe(edited);
  // Nothing left to do.
  server.state.calls = [];
  expect(await sync()).toMatchObject({ pushed: 0, pulled: 0 });
  expect(server.state.calls).toEqual(["party_sync_state", "get_settings"]);
});

it("the edit time sent is on the server's clock: a device clock 2 hours fast is corrected", async () => {
  await sync();
  const fast = 2 * 60 * 60 * 1000;
  editHere("c_a", character("c_a", "Ana", 3), 10_000_000);
  // Server clock 5000 at device 5000 + 2 h.
  await syncOnce({ local: local.api, server: server.call, userId: "u1", now: () => 5000 + fast });
  expect(loadMirror("u1")!.offsetMs).toBe(-fast);
  expect(server.state.pushes[0]?.editedAtMs).toBe(10_000_000 - fast);
});

it("a server that can't be reached during the pushes stops the sync and changes nothing here", async () => {
  await sync();
  const a = editHere("c_a", character("c_a", "Ana", 3));
  const b = editHere("c_b", character("c_b", "Bo", 4));
  const files = new Map(local.state.files);
  const before = loadMirror("u1")!.characters;
  const call = server.call;
  server.call = async (command, args) => {
    if (command === "party_sync_push") throw { code: "network", message: "The server can't be reached. Check your connection." };
    return call(command, args);
  };
  await expect(syncOnce({ local: local.api, server: server.call, userId: "u1", now: () => 4000 })).rejects.toMatchObject({ code: "network" });
  expect(local.state.files).toEqual(files);
  expect(loadMirror("u1")!.characters).toEqual(before);
  // Once it answers, both go.
  server.call = call;
  expect(await sync()).toMatchObject({ pushed: 2, waiting: [] });
  expect(server.state.characters.get("c_a")).toBe(local.state.files.get(a)?.text);
  expect(server.state.characters.get("c_b")).toBe(local.state.files.get(b)?.text);
});

it("a newer edit on the server wins: its copy replaces this one, and a notice says so", async () => {
  await sync();
  const old = editHere("c_a", character("c_a", "Ana", 3), 2500);
  server.state.characters.set("c_a", character("c_a", "Ana", 20));
  server.state.edited.set("c_a", 9000);
  const report = await sync();
  expect(report).toMatchObject({ pushed: 0, waiting: [], pulled: 1 });
  expect(server.state.trash).toEqual([character("c_a", "Ana", 3)]);
  expect(local.state.files.has(old)).toBe(false);
  expect(localTexts()).toEqual([character("c_a", "Ana", 20), character("c_b", "Bo")]);
  expect(report.notices).toEqual(["Ana: a newer edit on the server was kept over the changes made on this device. This device's copy is in the account's trash on the server for 30 days."]);
  expect(loadNotices("u1")).toEqual(report.notices);
  clearNotices("u1");
  expect(loadNotices("u1")).toEqual([]);
});

it("an edit here newer than the server's wins over it", async () => {
  await sync();
  const mine = character("c_a", "Ana", 3);
  editHere("c_a", mine, 9500);
  server.state.characters.set("c_a", character("c_a", "Ana", 20));
  server.state.edited.set("c_a", 2000);
  const report = await sync();
  expect(report).toMatchObject({ pushed: 1, pulled: 0, notices: [] });
  expect(server.state.characters.get("c_a")).toBe(mine);
  expect(localTexts()).toEqual([mine, character("c_b", "Bo")]);
});

it("deleted on the server: an unchanged copy goes, one edited before the delete goes with a notice, one edited after comes back", async () => {
  await sync();
  // Edited before the delete.
  const before = editHere("c_a", character("c_a", "Ana", 3), 2500);
  server.state.characters.delete("c_a");
  server.state.deleted.set("c_a", 6000);
  // Edited after the delete.
  const after = character("c_b", "Bo", 4);
  const kept = editHere("c_b", after, 9500);
  server.state.characters.delete("c_b");
  server.state.deleted.set("c_b", 6000);
  const report = await sync();
  expect(report.notices).toEqual(["Ana was deleted on the server after it was changed on this device. This device's copy is in the account's trash on the server for 30 days."]);
  expect(local.state.files.has(before)).toBe(false);
  expect(loadMirror("u1")!.characters.c_a).toBeUndefined();
  expect(report).toMatchObject({ pushed: 1, removed: 1, waiting: [] });
  expect(server.state.characters.get("c_b")).toBe(after);
  expect(local.state.files.get(kept)?.text).toBe(after);
  expect(loadMirror("u1")!.characters.c_b).toMatchObject({ local: kept, serverSha: `sha(${after})` });
});

it("deleted on the server between the state and the push: the copy goes in the same sync, sent once", async () => {
  await sync();
  const here = editHere("c_a", character("c_a", "Ana", 3), 2500);
  server.state.beforePush = (id) => {
    server.state.characters.delete(id);
    server.state.deleted.set(id, 6000);
  };
  const report = await sync();
  expect(report).toMatchObject({ removed: 1, pushed: 0, waiting: [], problems: {} });
  expect(report.notices).toHaveLength(1);
  expect(local.state.files.has(here)).toBe(false);
  expect(loadMirror("u1")!.characters.c_a).toBeUndefined();
  server.state.pushes = [];
  expect((await sync()).notices).toEqual([]);
  expect(server.state.pushes).toEqual([]);
});

it("another account's copy on this device is never taken over, sent or removed by this account's sync", async () => {
  await sync();
  const theirs = loadMirror("u1")!.characters.c_a!.local;
  const edited = character("c_a", "Ana", 3);
  editHere("c_a", edited);
  // A second account whose server has a character of the same id (both opened the same bundle).
  const other = fakeServer();
  other.state.characters.set("c_a", character("c_a", "Ana", 20));
  await syncOnce({ local: local.api, server: other.call, userId: "u2", now: () => 4000 });
  await syncOnce({ local: local.api, server: other.call, userId: "u2", now: () => 4000 });
  expect(other.state.pushes).toEqual([]);
  expect(loadMirror("u2")!.characters.c_a!.local).not.toBe(theirs);
  expect(local.state.files.get(theirs)?.text).toBe(edited);
  expect(other.state.characters.get("c_a")).toBe(character("c_a", "Ana", 20));
});

it("a browser keeps no copy: nothing starts outside the app", async () => {
  saveSession({ baseUrl: "", token: "t", user: { id: "u1", username: "a", email: null, admin: false, mustChangePassword: false } });
  const fetch = vi.fn();
  vi.stubGlobal("fetch", fetch);
  const stop = startSync();
  await Promise.resolve();
  stop();
  expect(fetch).not.toHaveBeenCalled();
  expect(loadMirror("u1")).toBeNull();
});

it("signing out: one last sync after the one running, then none until the sign-out is cancelled", async () => {
  saveSession({ baseUrl: "", token: "t", user: { id: "u1", username: "a", email: null, admin: false, mustChangePassword: false } });
  const local = fakeLocal();
  const server = fakeServer();
  const stop = startSync({ local: local.api, server: server.call, userId: "u1" });
  const states = () => server.state.calls.filter((c) => c === "party_sync_state").length;
  await lastSync();
  // The sync started by startSync, then the last one.
  expect(states()).toBe(2);
  await requestSync();
  expect(states()).toBe(2);
  resumeSync();
  await requestSync();
  expect(states()).toBe(3);
  stop();
});

describe("offline conflict safety (0.3.6)", () => {
  beforeEach(() => { server.state.asks = true; });

  it("a stale edit is never sent over the server's copy, nor replaced by it: the copy waits and the user is asked", async () => {
    await sync();
    const mine = character("c_a", "Ana", 3);
    const here = editHere("c_a", mine);
    const theirs = character("c_a", "Ana", 20);
    server.state.characters.set("c_a", theirs);
    // Even a later edit time here changes nothing: the base decides.
    server.state.edited.set("c_a", 1);
    const report = await sync();
    expect(report).toMatchObject({ pushed: 0, pulled: 0, conflicts: ["c_a"], waiting: ["c_a"], notices: [] });
    expect(server.state.pushes.at(-1)).toMatchObject({ id: "c_a", baseSha: `sha(${character("c_a", "Ana")})` });
    expect(server.state.characters.get("c_a")).toBe(theirs);
    expect(server.state.trash).toEqual([]);
    expect(local.state.files.get(here)?.text).toBe(mine);
    expect(pendingConflicts("u1")).toMatchObject([{ id: "c_a", name: "Ana", deleted: false, serverSha: `sha(${theirs})` }]);
    // Until the user chooses, every sync asks again and keeps both as they are.
    await sync();
    expect(server.state.characters.get("c_a")).toBe(theirs);
    expect(local.state.files.get(here)?.text).toBe(mine);
  });

  it("Keep mine: the copy here is sent with force and goes over the server's", async () => {
    await sync();
    const mine = character("c_a", "Ana", 3);
    const here = editHere("c_a", mine);
    server.state.characters.set("c_a", character("c_a", "Ana", 20));
    await sync();
    chooseConflict("u1", "c_a", "mine");
    expect(pendingConflicts("u1")).toEqual([]);
    const report = await sync();
    expect(report).toMatchObject({ pushed: 1, conflicts: [], waiting: [] });
    expect(server.state.characters.get("c_a")).toBe(mine);
    expect(local.state.files.get(here)?.text).toBe(mine);
    expect(loadMirror("u1")!.conflicts).toEqual({});
    expect(loadChoices("u1")).toEqual({});
  });

  it("Use the server's: the server's copy replaces the one here", async () => {
    await sync();
    const here = editHere("c_a", character("c_a", "Ana", 3));
    const theirs = character("c_a", "Ana", 20);
    server.state.characters.set("c_a", theirs);
    await sync();
    chooseConflict("u1", "c_a", "server");
    const pushes = server.state.pushes.length;
    const report = await sync();
    // One push only: the copy here, to the trash.
    expect(server.state.pushes.length).toBe(pushes + 1);
    expect(report).toMatchObject({ pulled: 1, conflicts: [], waiting: [] });
    expect(local.state.files.has(here)).toBe(false);
    expect(localTexts()).toEqual([theirs, character("c_b", "Bo")]);
    expect(server.state.characters.get("c_a")).toBe(theirs);
    // The copy here is in the account's trash, not lost.
    expect(server.state.trash).toEqual([character("c_a", "Ana", 3)]);
  });

  it("Keep both: the copy here becomes a new character beside the server's, and both are on the device", async () => {
    await sync();
    editHere("c_a", character("c_a", "Ana", 3));
    const theirs = character("c_a", "Ana", 20);
    server.state.characters.set("c_a", theirs);
    await sync();
    chooseConflict("u1", "c_a", "both");
    const report = await sync();
    expect(server.state.imported).toEqual(["c_new1"]);
    const copy = JSON.parse(server.state.characters.get("c_new1")!) as { meta: { id?: string; name: string }; hp: number };
    expect(copy).toEqual({ meta: { name: "Ana (this device)" }, hp: 3 });
    expect(server.state.characters.get("c_a")).toBe(theirs);
    expect(report).toMatchObject({ conflicts: [], waiting: [] });
    expect(localTexts()).toEqual([theirs, character("c_b", "Bo"), server.state.characters.get("c_new1")].sort());
  });

  it("deleted on the server: asked too, with no Keep both; Use the server's removes the copy here", async () => {
    await sync();
    const here = editHere("c_a", character("c_a", "Ana", 3));
    server.state.characters.delete("c_a");
    server.state.deleted.set("c_a", 9000);
    const report = await sync();
    expect(report).toMatchObject({ conflicts: ["c_a"], removed: 0 });
    expect(local.state.files.has(here)).toBe(true);
    expect(pendingConflicts("u1")).toMatchObject([{ id: "c_a", name: "Ana", deleted: true, serverSha: null }]);
    chooseConflict("u1", "c_a", "server");
    expect(await sync()).toMatchObject({ removed: 1, conflicts: [] });
    expect(local.state.files.has(here)).toBe(false);
    expect(server.state.characters.has("c_a")).toBe(false);
  });

  it("a fresh edit (the server copy unchanged) is sent as before, with no question", async () => {
    await sync();
    const mine = character("c_a", "Ana", 3);
    editHere("c_a", mine);
    const report = await sync();
    expect(report).toMatchObject({ pushed: 1, conflicts: [] });
    expect(server.state.characters.get("c_a")).toBe(mine);
    expect(pendingConflicts("u1")).toEqual([]);
  });

  /** Ana edited here and on the server: one conflict, as the dialog shows it. */
  async function conflictOnAna(mine = character("c_a", "Ana", 3), theirs = character("c_a", "Ana", 20)) {
    await sync();
    const here = editHere("c_a", mine);
    server.state.characters.set("c_a", theirs);
    await sync();
    return { here, mine, theirs };
  }

  it("Keep mine goes only over the server copy the user saw: a later server change is asked about again", async () => {
    const { mine } = await conflictOnAna();
    chooseConflict("u1", "c_a", "mine");
    // The DM changes Ana again before the choice is carried out.
    const later = character("c_a", "Ana", 25);
    server.state.characters.set("c_a", later);
    const report = await sync();
    expect(server.state.characters.get("c_a")).toBe(later);
    expect(report.conflicts).toEqual(["c_a"]);
    expect(pendingConflicts("u1")).toMatchObject([{ id: "c_a", serverSha: `sha(${later})` }]);
    // Chosen again, over the copy now seen: written.
    chooseConflict("u1", "c_a", "mine");
    await sync();
    expect(server.state.characters.get("c_a")).toBe(mine);
  });

  it("a choice covers the copy the user saw: an edit made here after it is asked about again", async () => {
    await conflictOnAna();
    chooseConflict("u1", "c_a", "server");
    const more = character("c_a", "Ana", 1);
    const here = editHere("c_a", more, 3600);
    const report = await sync();
    expect(report.conflicts).toEqual(["c_a"]);
    expect(local.state.files.get(here)?.text).toBe(more);
    expect(server.state.trash).toEqual([]);
    expect(loadChoices("u1")).toEqual({});
    expect(pendingConflicts("u1")).toMatchObject([{ id: "c_a", localSha: expect.any(String) }]);
  });

  it("Use the server's whose fetch fails is not asked again: the next sync fetches it", async () => {
    const { here, theirs } = await conflictOnAna();
    chooseConflict("u1", "c_a", "server");
    const call = server.call;
    let fail = true;
    const flaky: typeof call = async (command, args) => {
      if (command === "party_sync_get" && fail) throw { code: "internal", message: "busy" };
      return call(command, args);
    };
    const report = await syncOnce({ local: local.api, server: flaky, userId: "u1", now: () => 4000 });
    expect(report.problems.c_a).toBeDefined();
    expect(local.state.files.has(here)).toBe(true);
    fail = false;
    const pushes = server.state.pushes.length;
    const next = await sync();
    expect(next.conflicts).toEqual([]);
    expect(server.state.pushes.length).toBe(pushes);
    expect(localTexts()).toEqual([theirs, character("c_b", "Bo")]);
  });

  it("a refused choice comes back to the dialog with why, and can be chosen again", async () => {
    await conflictOnAna();
    chooseConflict("u1", "c_a", "both");
    server.state.importThrows = { code: "invalid", message: "Too many characters" };
    await sync();
    expect(loadChoices("u1")).toEqual({});
    expect(pendingConflicts("u1")).toMatchObject([{ id: "c_a", error: expect.stringContaining("Too many characters") }]);
  });

  it("Keep both after a lost answer makes no second copy, and copy names do not repeat", async () => {
    server.state.characters.set("c_x", character("c_x", "Ana (this device)"));
    await conflictOnAna();
    chooseConflict("u1", "c_a", "both");
    // The import lands, but its answer is lost.
    server.state.importThrows = { code: "network", message: "lost" };
    await expect(sync()).rejects.toMatchObject({ code: "network" });
    server.state.importThrows = null;
    await sync();
    expect(server.state.imported).toEqual(["c_new1"]);
    const copy = JSON.parse(server.state.characters.get("c_new1")!) as { meta: { name: string } };
    expect(copy.meta.name).toBe("Ana (this device 2)");
    expect(pendingConflicts("u1")).toEqual([]);
  });
});
