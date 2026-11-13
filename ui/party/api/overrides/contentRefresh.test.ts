import { beforeEach, describe, expect, it, vi } from "vitest";
import { contentSync, resetContentSync } from "../../content/contentSync";
import { applyContentRefresh, lastLists } from "./contentRefresh";
import type { Methods } from "./registry";

vi.mock("../liveEvents", () => ({ liveEvents: () => () => undefined }));

/** A server whose answers wait for `release()`, counting the calls. */
function server() {
  const calls: string[] = [];
  let packs = [{ id: "a", name: "Alpha", enabled: true }, { id: "b", name: "Beta", enabled: true }];
  let gates: (() => void)[] = [];
  let refuse: string | null = null;
  const later = <T>(value: () => T) => new Promise<T>((resolve, reject) => {
    gates.push(() => (refuse ? reject({ code: "internal", message: refuse }) : resolve(value())));
  });
  const methods = {
    listPacks: () => { calls.push("listPacks"); return later(() => packs); },
    listIndexes: () => { calls.push("listIndexes"); return Promise.resolve([{ url: "https://x.example/a.index", name: "A" }]); },
    list5etoolsUrls: () => Promise.resolve([]),
    removePack: (args?: Record<string, unknown>) => {
      calls.push(`removePack ${String(args?.packId)}`);
      return later(() => { packs = packs.filter((p) => p.id !== args?.packId); return packs; });
    },
    setPackEnabled: () => later(() => packs),
    addIndex: (args?: Record<string, unknown>) => { calls.push(`addIndex ${String(args?.url)}`); return Promise.resolve([]); },
    updateIndexes: () => later(() => ({ updated: 1 })),
    importPack: () => Promise.resolve({}),
  } as unknown as Methods;
  applyContentRefresh(methods);
  return {
    methods, calls,
    release: () => { const g = gates; gates = []; g.forEach((f) => f()); },
    refuse: (m: string) => { refuse = m; },
  };
}
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  resetContentSync();
  lastLists.packs = [];
  lastLists.indexes = [];
  lastLists.fiveTools = [];
});

describe("the content lists", () => {
  it("share one request among the calls made at the same content version", async () => {
    const s = server();
    const one = s.methods.listPacks() as Promise<unknown[]>;
    const two = s.methods.listPacks() as Promise<unknown[]>;
    s.release();
    expect(await one).toHaveLength(2);
    expect(await two).toHaveLength(2);
    expect(s.calls).toEqual(["listPacks"]);
    // Answered: the next call asks again.
    const three = s.methods.listPacks() as Promise<unknown[]>;
    s.release();
    await three;
    expect(s.calls).toEqual(["listPacks", "listPacks"]);
  });
});

describe("removing a pack", () => {
  it("answers at once without the pack, then the server's own list follows", async () => {
    const s = server();
    const listed = s.methods.listPacks();
    s.release();
    await listed;
    const answer = await (s.methods.removePack({ packId: "a" }) as Promise<{ id: string }[]>);
    expect(answer.map((p) => p.id)).toEqual(["b"]);
    expect(contentSync.get().activity).toMatchObject([{ state: "working", label: "Removing “Alpha”…" }]);
    // Upstream's reload right after: answered from the last list, with no request, while the server works.
    const reload = s.methods.listPacks() as Promise<{ id: string }[]>;
    expect((await reload).map((p) => p.id)).toEqual(["b"]);
    expect(s.calls).toEqual(["listPacks", "removePack a"]);
    // The next one asks the server again; the pack stays hidden until the removal ends.
    const again = s.methods.listPacks() as Promise<{ id: string }[]>;
    s.release();
    expect((await again).map((p) => p.id)).toEqual(["b"]);
    await flush();
    expect(contentSync.get().activity).toMatchObject([{ state: "done", message: "Removed “Alpha”." }]);
  });

  it("brings the pack back with the server's message when it refuses", async () => {
    const s = server();
    const listed = s.methods.listPacks();
    s.release();
    await listed;
    await s.methods.removePack({ packId: "a" });
    s.refuse("That content pack is not installed.");
    s.release();
    await flush();
    expect(contentSync.get().activity).toMatchObject([{ state: "failed", message: "“Alpha” wasn't removed: That content pack is not installed." }]);
    expect(contentSync.packs(lastLists.packs).map((p) => p.id)).toEqual(["a", "b"]);
  });

  it("a switch shows at once and goes back if refused", async () => {
    const s = server();
    const listed = s.methods.listPacks();
    s.release();
    await listed;
    const answer = await (s.methods.setPackEnabled({ packId: "b", enabled: false }) as Promise<{ id: string; enabled: boolean }[]>);
    expect(answer.find((p) => p.id === "b")!.enabled).toBe(false);
    s.refuse("no");
    s.release();
    await flush();
    expect(contentSync.packs(lastLists.packs).find((p) => p.id === "b")!.enabled).toBe(true);
  });
});

describe("additions", () => {
  it("an index file being added shows an activity row; the lists are fetched again after", async () => {
    vi.useFakeTimers();
    try {
      const s = server();
      const added = s.methods.addIndex({ url: "https://x.example/b.index" }) as Promise<unknown>;
      expect(contentSync.get().activity).toMatchObject([{ area: "indexes", state: "working", label: "Adding the index file from x.example…" }]);
      await added;
      expect(contentSync.get().activity).toMatchObject([{ state: "done" }]);
      await vi.advanceTimersByTimeAsync(200);
      expect(contentSync.get()).toMatchObject({ version: 1, loud: 1 });
    } finally { vi.useRealTimers(); }
  });

  it("a download shows progress and keeps the update's own change away from upstream's Index files", async () => {
    vi.useFakeTimers();
    try {
      const s = server();
      const update = s.methods.updateIndexes() as Promise<unknown>;
      expect(contentSync.get().activity).toMatchObject([{ area: "indexes", label: "Downloading index files…", progress: true }]);
      s.release();
      await update;
      await vi.advanceTimersByTimeAsync(200);
      expect(contentSync.get()).toMatchObject({ version: 1, loud: 0 });
    } finally { vi.useRealTimers(); }
  });

  it("a failed import leaves no row: its caller shows the error", async () => {
    const methods = { listPacks: () => Promise.resolve([]), listIndexes: () => Promise.resolve([]), list5etoolsUrls: () => Promise.resolve([]), importPack: () => Promise.reject({ code: "x", message: "bad" }) } as unknown as Methods;
    applyContentRefresh(methods);
    await expect(methods.importPack({})).rejects.toMatchObject({ message: "bad" });
    expect(contentSync.get().activity).toEqual([]);
  });
});
