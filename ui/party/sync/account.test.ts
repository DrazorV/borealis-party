import { beforeEach, expect, it, vi } from "vitest";
import type { ContentPack } from "../../src/api/contract";
import { memoryStorage } from "../test/memoryStorage";
import { othersUnsentEdits, removeCopy, removeOtherCopies, unsentEdits } from "./account";
import { addNotices, emptyMirror, loadMirror, loadNotices, saveMirror, type MirroredCharacter } from "./mirror";
import { sha256, type LocalApi } from "./sync";

/** The app's backend: files with a date, open sessions that may hold edits not on file, packs and indexes. */
function fakeLocal() {
  const files = new Map<string, { text: string; updatedAt: string; name: string }>();
  const sessions = new Map<string, string>();
  const state = {
    files, sessions, calls: [] as string[],
    packs: [{ id: "srd", kind: "bundled" }, { id: "hb", kind: "homebrew" }, { id: "dl-x", kind: "downloaded" }] as ContentPack[],
    indexes: ["https://index.example/a.xml"],
  };
  const api = {
    listCharacters: async () => [...files].map(([id, f]) => ({ id, name: f.name, updatedAt: f.updatedAt })),
    loadCharacter: async ({ characterId }: { characterId: string }) => {
      if (!sessions.has(characterId)) sessions.set(characterId, files.get(characterId)!.text);
      return {};
    },
    exportCharacter: async ({ characterId }: { characterId: string }) => ({ dataBase64: btoa(sessions.get(characterId)!) }),
    closeCharacter: async ({ characterId, discard }: { characterId: string; discard: boolean }) => {
      state.calls.push(`close ${characterId} ${discard}`);
      const text = sessions.get(characterId);
      if (text !== undefined && !discard && text !== files.get(characterId)?.text) files.set(characterId, { ...files.get(characterId)!, text, updatedAt: "saved" });
      sessions.delete(characterId);
    },
    deleteCharacter: async ({ characterId }: { characterId: string }) => {
      state.calls.push(`delete ${characterId}`);
      sessions.delete(characterId);
      if (!files.delete(characterId)) throw { code: "not_found", message: "gone" };
    },
    listPacks: async () => state.packs,
    removePack: async ({ packId }: { packId: string }) => { state.calls.push(`removePack ${packId}`); state.packs = state.packs.filter((p) => p.id !== packId); return []; },
    listIndexes: async () => state.indexes.map((url) => ({ url })),
    removeIndex: async ({ url }: { url: string }) => { state.calls.push(`removeIndex ${url}`); state.indexes = state.indexes.filter((u) => u !== url); return []; },
  } as unknown as LocalApi;
  return { api, state };
}

const sha = (text: string) => sha256(btoa(text));

/** A copy on the device as a sync left it: its file and its entry in the account's record. */
async function copy(local: ReturnType<typeof fakeLocal>, userId: string, server: string, localId: string, name: string, extra: Partial<MirroredCharacter> = {}) {
  const text = JSON.stringify({ meta: { id: server, name } });
  local.state.files.set(localId, { text, updatedAt: "t1", name });
  const m = loadMirror(userId) ?? { ...emptyMirror(userId), lastSyncMs: 1 };
  m.characters[server] = { local: localId, serverSha: "s", localSha: await sha(text), localMtime: "t1", ...extra };
  saveMirror(m);
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
});

it("names only the copies with edits the server does not have, and reads open ones without closing them", async () => {
  const local = fakeLocal();
  await copy(local, "u1", "s1", "l1", "Untouched");
  await copy(local, "u1", "s2", "l2", "Only re-dated");
  await copy(local, "u1", "s3", "l3", "Edited");
  await copy(local, "u1", "s4", "l4", "Open with an edit");
  await copy(local, "u1", "s5", "l5", "Found again", { baseUnknown: true });
  local.state.files.get("l2")!.updatedAt = "t2";
  local.state.files.set("l3", { text: '{"hp":1}', updatedAt: "t2", name: "Edited" });
  local.state.sessions.set("l4", '{"hp":2}');
  expect(await unsentEdits(local.api, "u1", new Set(["l4"]))).toEqual(["Edited", "Open with an edit", "Found again"]);
  // The page's open copy was neither closed nor saved: a Cancel leaves it as it was.
  expect(local.state.sessions.get("l4")).toBe('{"hp":2}');
  expect(local.state.calls.filter((c) => c.startsWith("close l4"))).toEqual([]);
  expect(local.state.files.get("l4")!.updatedAt).toBe("t1");
});

it("an edit still open from an earlier page goes on file first, and counts", async () => {
  const local = fakeLocal();
  await copy(local, "u1", "s1", "l1", "Left open");
  local.state.sessions.set("l1", '{"hp":3}');
  expect(await unsentEdits(local.api, "u1")).toEqual(["Left open"]);
  expect(local.state.files.get("l1")!.text).toBe('{"hp":3}');
});

it("no record: nothing to name", async () => {
  expect(await unsentEdits(fakeLocal().api, "nobody")).toEqual([]);
});

it("removing the only account's copy takes its characters, spares, packs and indexes, and forgets the record", async () => {
  const local = fakeLocal();
  await copy(local, "u1", "s1", "l1", "Hero");
  local.state.files.set("spare", { text: "{}", updatedAt: "t", name: "" });
  local.state.sessions.set("l1", "{}");
  addNotices("u1", ["a notice"]);
  await removeCopy(local.api, "u1");
  expect(local.state.files.size).toBe(0);
  expect(local.state.sessions.size).toBe(0);
  expect(local.state.packs.map((p) => p.id)).toEqual(["srd"]);
  expect(local.state.indexes).toEqual([]);
  expect(loadMirror("u1")).toBeNull();
  expect(loadNotices("u1")).toEqual([]);
});

it("another account's copy on the device stays, with the packs it needs", async () => {
  const local = fakeLocal();
  await copy(local, "u1", "s1", "l1", "Mine");
  await copy(local, "u2", "s9", "l9", "Theirs");
  await removeCopy(local.api, "u1");
  expect([...local.state.files.keys()]).toEqual(["l9"]);
  expect(local.state.packs).toHaveLength(3);
  expect(loadMirror("u2")).not.toBeNull();
});

it("a step that fails does not stop the rest", async () => {
  const local = fakeLocal();
  await copy(local, "u1", "s1", "l1", "A");
  await copy(local, "u1", "s2", "l2", "B");
  const del = local.api.deleteCharacter;
  local.api.deleteCharacter = async (args) => { if (args.characterId === "l1") throw { code: "io_error", message: "busy" }; return del(args); };
  await removeCopy(local.api, "u1");
  expect([...local.state.files.keys()]).toEqual(["l1"]);
  expect(local.state.indexes).toEqual([]);
  expect(loadMirror("u1")).toBeNull();
});

it("another account signing in: the earlier account's unsent edits are named, then its copy goes and the new one's stays", async () => {
  const local = fakeLocal();
  await copy(local, "u1", "s1", "l1", "Old hero");
  await copy(local, "u2", "s2", "l2", "New hero");
  local.state.files.set("l1", { text: '{"hp":1}', updatedAt: "t2", name: "Old hero" });
  local.state.files.set("l2", { text: '{"hp":1}', updatedAt: "t2", name: "New hero" });
  expect(await othersUnsentEdits(local.api, "u2")).toEqual(["Old hero"]);
  // Seen from the other side, the other account's edit is the one named.
  expect(await othersUnsentEdits(local.api, "u1")).toEqual(["New hero"]);
  await removeOtherCopies(local.api, "u2");
  expect([...local.state.files.keys()]).toEqual(["l2"]);
  expect(loadMirror("u1")).toBeNull();
  expect(loadMirror("u2")).not.toBeNull();
});
