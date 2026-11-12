import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ContentPack, ContentSource } from "../../src/api/contract";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { loadMirror } from "./mirror";
import { startSync } from "./runner";
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
    listCharacters: async () => [...files].map(([id, f]) => ({ id, updatedAt: f.updatedAt }) as never),
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
  };
  const sha = (text: string) => `sha(${text})`;
  const call: ServerCall = async (command, args) => {
    state.calls.push(args?.id ? `${command} ${String(args.id)}` : command);
    switch (command) {
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
  // Upstream rewrote the file when it found the content it was missing.
  local.state.files.set(m.local, { text: character("c_a", "Ana", 11), updatedAt: "later" });
  server.state.packs = [{ id: "homebrew", sha256: "h1", enabled: true, indexUrl: null }];
  await sync();
  const after = loadMirror("u1")!.characters.c_a!;
  expect(after.local).toBe(m.local);
  expect(after.localMtime).toBe("later");
  expect(after.localSha).not.toBe(m.localSha);
});

it("the server's settings replace the app's, less its own folders and flags", async () => {
  await sync();
  expect(local.state.settings).toEqual({ theme: "light", documentsDir: "/device/docs", mobile: true });
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
