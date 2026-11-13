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
