import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { packsFrom, type PackSource } from "../packRemoval";
import { askRemoval, type SourceRemovalDeps } from "./sourceRemoval";

const SOURCES: PackSource[] = [
  { id: "srd-a", indexUrl: null, fiveToolsUrl: "https://github.com/example/srd-data" },
  { id: "srd-b", indexUrl: null, fiveToolsUrl: "https://github.com/example/srd-data/" },
  { id: "idx", indexUrl: "https://content.example/srd.index", fiveToolsUrl: null },
];

/** An ask that shows the dialog's text, lets `act` tick or untick, and answers `answer`. */
function deps(answer: string, act?: () => void): SourceRemovalDeps & { asked: string[] } {
  const asked: string[] = [];
  return {
    asked,
    packSources: async () => SOURCES,
    removePacks: async () => ({ removed: 0, skipped: [], packs: [] }),
    ask: async (title: string, message: ReactNode) => {
      const view = render(<>{message}</>);
      asked.push(`${title}: ${view.container.textContent}`);
      act?.();
      view.unmount();
      return answer;
    },
  } as never;
}

afterEach(() => { document.body.innerHTML = ""; });

describe("packsFrom", () => {
  it("finds a source's packs, with or without a trailing slash", () => {
    expect(packsFrom(SOURCES, "https://github.com/example/srd-data")).toEqual(["srd-a", "srd-b"]);
    expect(packsFrom(SOURCES, "https://content.example/srd.index")).toEqual(["idx"]);
    expect(packsFrom(SOURCES, "https://other.example/x.index")).toEqual([]);
  });
});

describe("removing a source", () => {
  it("offers to remove its packs too, ticked, with the count", async () => {
    const d = deps("remove");
    expect(await askRemoval("https://github.com/example/srd-data", d)).toEqual(["srd-a", "srd-b"]);
    expect(d.asked[0]).toMatch(/^Remove source: Stop following https:\/\/github.com\/example\/srd-data\?/);
    expect(d.asked[0]).toMatch(/Also remove the 2 packs that came from it/);
  });

  it("keeps the packs when unticked", async () => {
    const d = deps("remove", () => fireEvent.click(screen.getByRole("checkbox", { name: /Also remove the 1 pack that came from it/ })));
    expect(await askRemoval("https://content.example/srd.index", d)).toEqual([]);
  });

  it("does nothing when cancelled", async () => {
    expect(await askRemoval("https://content.example/srd.index", deps("cancel"))).toBe("cancel");
  });

  it("asks nothing when no pack came from the source", async () => {
    const d = deps("remove");
    expect(await askRemoval("https://other.example/x.index", d)).toEqual([]);
    expect(d.asked).toEqual([]);
  });
});

describe("the removal itself", () => {
  const URL = "https://content.example/srd.index";
  async function setUp(refuse: string | null) {
    const { OVERRIDES } = await import("./registry");
    const { sourceRemovalDeps } = await import("./sourceRemoval");
    const { contentSync, resetContentSync } = await import("../../content/contentSync");
    resetContentSync();
    let answer = () => {};
    const gate = new Promise<void>((r) => { answer = r; });
    const removed: string[][] = [];
    Object.assign(sourceRemovalDeps, deps("remove"), {
      removePacks: async (ids: string[]) => { removed.push(ids); return { removed: ids.length, skipped: [], packs: [] }; },
    });
    const methods = {
      listIndexes: async () => contentSync.indexes([{ url: URL }, { url: "https://other.example/o.index" }]),
      removeIndex: async () => { await gate; if (refuse) throw { code: "internal", message: refuse }; return []; },
    } as never as Record<string, (a?: unknown) => Promise<{ url: string }[]>>;
    OVERRIDES.find((o) => o.name === "sourceRemoval")!.apply(methods as never, {} as never);
    return { methods, answer, removed, contentSync };
  }

  it("takes the index and its packs off the lists at once, and removes the packs after the index", async () => {
    const { methods, answer, removed, contentSync } = await setUp(null);
    const list = await methods.removeIndex!({ url: URL });
    expect(list.map((i) => i.url)).toEqual(["https://other.example/o.index"]);
    expect(contentSync.packs([{ id: "idx" }, { id: "srd-a" }]).map((p) => p.id)).toEqual(["srd-a"]);
    expect(contentSync.get().activity).toMatchObject([{ area: "indexes", state: "working", label: "Removing the index file from content.example and its pack…" }]);
    expect(removed).toEqual([]);
    answer();
    await new Promise((r) => setTimeout(r, 0));
    expect(removed).toEqual([["idx"]]);
    expect(contentSync.get().activity).toMatchObject([{ state: "done", message: "Removed the index file from content.example and its pack." }]);
  });

  it("brings them back with the server's message when it refuses", async () => {
    const { methods, answer, removed, contentSync } = await setUp("The server is busy.");
    await methods.removeIndex!({ url: URL });
    answer();
    await new Promise((r) => setTimeout(r, 0));
    expect(removed).toEqual([]);
    expect((await methods.listIndexes!()).map((i) => i.url)).toContain(URL);
    expect(contentSync.packs([{ id: "idx" }])).toHaveLength(1);
    expect(contentSync.get().activity).toMatchObject([{ state: "failed", message: "The index file from content.example and its pack weren't removed: The server is busy." }]);
  });
});
