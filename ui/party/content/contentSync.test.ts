import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ServerEvent } from "../api/http";
import { contentSync, contentSyncDeps, DONE_SHOWN_MS, HIDE_AFTER_MS, resetContentSync, SETTLE_MS, UPDATE_QUIET_MS } from "./contentSync";

let clock = 0;
const timers: { at: number; f: () => void }[] = [];
/** Run the timers due by `ms` from now. */
function advance(ms: number) {
  clock += ms;
  for (;;) {
    const due = timers.filter((t) => t.at <= clock).sort((a, b) => a.at - b.at)[0];
    if (!due) return;
    timers.splice(timers.indexOf(due), 1);
    due.f();
  }
}

let onEvent: ((e: ServerEvent) => void) | null = null;
let onOpen: ((again: boolean) => void) | null = null;
let streams = 0;

beforeEach(() => {
  resetContentSync();
  clock = 1_000_000;
  timers.length = 0;
  streams = 0;
  Object.assign(contentSyncDeps, {
    now: () => clock,
    setTimeout: (f: () => void, ms: number) => { const t = { at: clock + ms, f }; timers.push(t); return t; },
    clearTimeout: (t: unknown) => { const i = timers.indexOf(t as never); if (i >= 0) timers.splice(i, 1); },
    events: (e: (e: ServerEvent) => void, o: (again: boolean) => void) => { onEvent = e; onOpen = o; streams += 1; return () => { streams -= 1; }; },
  });
});
afterEach(() => resetContentSync());

const PACKS = [{ id: "a", enabled: true }, { id: "b", enabled: true }, { id: "c", enabled: false }];

describe("changed", () => {
  it("makes one new version for several changes close together", () => {
    contentSync.changed();
    contentSync.changed(true);
    advance(SETTLE_MS - 1);
    expect(contentSync.get().version).toBe(0);
    contentSync.changed();
    advance(1);
    expect(contentSync.get()).toMatchObject({ version: 1, loud: 1 });
    advance(SETTLE_MS * 5);
    expect(contentSync.get().version).toBe(1);
  });

  it("leaves the Index files version alone during an index update and just after, unless forced", () => {
    const end = contentSync.updating();
    contentSync.changed();
    advance(SETTLE_MS);
    expect(contentSync.get()).toMatchObject({ version: 1, loud: 0 });
    end();
    contentSync.changed();
    advance(SETTLE_MS);
    expect(contentSync.get()).toMatchObject({ version: 2, loud: 0 });
    contentSync.changed(true);
    advance(SETTLE_MS);
    expect(contentSync.get()).toMatchObject({ version: 3, loud: 3 });
    advance(UPDATE_QUIET_MS);
    contentSync.changed();
    advance(SETTLE_MS);
    expect(contentSync.get()).toMatchObject({ version: 4, loud: 4 });
  });
});

describe("removals", () => {
  it("hide at once, stay hidden a while after they end, then show what the server says", () => {
    const op = contentSync.begin("packs", "Removing “A”…", { packs: ["a"] });
    expect(contentSync.packs(PACKS).map((p) => p.id)).toEqual(["b", "c"]);
    expect(contentSync.get().activity).toEqual([{ id: op, area: "packs", label: "Removing “A”…", state: "working" }]);
    advance(SETTLE_MS);
    expect(contentSync.get().version).toBe(1);
    contentSync.done(op, "Removed “A”.");
    expect(contentSync.get().activity[0]).toMatchObject({ state: "done", message: "Removed “A”." });
    advance(DONE_SHOWN_MS);
    expect(contentSync.get().activity).toEqual([]);
    expect(contentSync.packs(PACKS).map((p) => p.id)).toEqual(["b", "c"]);
    advance(HIDE_AFTER_MS);
    expect(contentSync.packs(PACKS).map((p) => p.id)).toEqual(["a", "b", "c"]);
  });

  it("come back with the message when the server refuses, and the lists are fetched again", () => {
    const op = contentSync.begin("indexes", "Removing the index file from x…", { indexes: ["https://x.example/a.index/"], packs: ["b"] });
    expect(contentSync.indexes([{ url: "https://x.example/a.index" }])).toEqual([]);
    expect(contentSync.packs(PACKS).map((p) => p.id)).toEqual(["a", "c"]);
    advance(SETTLE_MS);
    contentSync.fail(op, "It wasn't removed: no.");
    expect(contentSync.indexes([{ url: "https://x.example/a.index" }])).toHaveLength(1);
    expect(contentSync.packs(PACKS)).toHaveLength(3);
    expect(contentSync.get().activity).toMatchObject([{ state: "failed", message: "It wasn't removed: no." }]);
    advance(DONE_SHOWN_MS * 10);
    expect(contentSync.get().activity).toHaveLength(1);
    advance(SETTLE_MS);
    expect(contentSync.get()).toMatchObject({ version: 2, loud: 2 });
    contentSync.dismiss(op);
    expect(contentSync.get().activity).toEqual([]);
  });

  it("the next fetch of a list a removal or switch changed is answered from the last list, once", () => {
    contentSync.begin("indexes", "x", { indexes: ["u"], packs: ["a"] });
    expect([contentSync.takeCached("packs"), contentSync.takeCached("indexes"), contentSync.takeCached("fiveTools")]).toEqual([true, true, false]);
    expect(contentSync.takeCached("packs")).toBe(false);
    contentSync.switching("b", false, contentSync.begin("packs", "y"));
    expect(contentSync.takeCached("packs")).toBe(true);
  });

  it("an addition lets an ended removal's items show again", () => {
    contentSync.done(contentSync.begin("packs", "x", { packs: ["a"] }));
    contentSync.adding();
    expect(contentSync.packs(PACKS)).toHaveLength(3);
  });

  it("a pending switch shows at once and goes with its activity", () => {
    const op = contentSync.begin("packs", "Turning off “B”…");
    contentSync.switching("b", false, op);
    expect(contentSync.packs(PACKS).find((p) => p.id === "b")!.enabled).toBe(false);
    contentSync.fail(op, "no");
    expect(contentSync.packs(PACKS).find((p) => p.id === "b")!.enabled).toBe(true);
  });
});

describe("watch", () => {
  it("opens one stream for every watcher, follows content changes, resyncs and reopens, and closes after the last", () => {
    const stopA = contentSync.watch();
    const stopB = contentSync.watch();
    expect(streams).toBe(1);
    onEvent!({ event: "party://content-changed", payload: { account: "me", kind: "indexes" } });
    onEvent!({ event: "party://characters-changed", payload: {} });
    advance(SETTLE_MS);
    expect(contentSync.get()).toMatchObject({ version: 1, loud: 1 });
    onEvent!({ event: "party://resync", payload: {} });
    advance(SETTLE_MS);
    expect(contentSync.get().version).toBe(2);
    onOpen!(false);
    advance(SETTLE_MS);
    expect(contentSync.get().version).toBe(2);
    onOpen!(true);
    advance(SETTLE_MS);
    expect(contentSync.get().version).toBe(3);
    stopA();
    stopA();
    expect(streams).toBe(1);
    stopB();
    expect(streams).toBe(0);
  });

  it("a content change from another device during this tab's index update still reaches Index files", () => {
    contentSync.watch();
    const end = contentSync.updating();
    onEvent!({ event: "party://content-changed", payload: { account: "me", kind: "indexes" } });
    advance(SETTLE_MS);
    end();
    expect(contentSync.get().loud).toBe(1);
  });

  it("the update's own change is left to upstream's Index files in the tab that runs it", () => {
    contentSync.watch();
    const end = contentSync.updating();
    end();
    onEvent!({ event: "party://content-changed", payload: { account: "me", kind: "indexes", command: "update_indexes" } });
    advance(SETTLE_MS);
    expect(contentSync.get()).toMatchObject({ version: 1, loud: 0 });
  });
});

vi.mock("../api/liveEvents", () => ({ liveEvents: () => () => undefined }));
