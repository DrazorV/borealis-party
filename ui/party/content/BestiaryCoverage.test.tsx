import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Coverage, PackCoverage } from "../api/bestiary";

const state = vi.hoisted(() => ({
  coverage: null as unknown as Coverage,
  handlers: [] as (() => void)[],
  calls: 0,
}));
vi.mock("../../src/state/ApiContext", () => ({
  useApi: () => ({ subscribe: (_e: string, h: () => void) => { state.handlers.push(h); return () => {}; } }),
}));
vi.mock("../api/bestiary", () => ({
  bestiaryCoverage: async () => { state.calls += 1; return state.coverage; },
}));

const { BestiaryCoverage, summaryLine } = await import("./BestiaryCoverage");

const NO_EXTRAS = { environment: 0, page: 0, mythic: 0, covenCr: 0, altSpeeds: 0, variants: 0, reactionIntro: 0, fluff: 0, tokens: 0 };
const pack = (over: Partial<PackCoverage>): PackCoverage => ({
  packId: "srd-5.2.1-monsters", name: "SRD 5.2.1 monsters", kind: "bundled", enabled: true, packRev: "r", status: "complete",
  monsters: 330, companions: 0, inData: 330, skipped: 0, partial: 0, skippedReasons: [], partialNotes: [], sources: [],
  byCr: [{ value: "0", count: 20 }, { value: "1/4", count: 40 }], byType: [{ value: "beast", count: 80 }], rulesets: [],
  extras: NO_EXTRAS, captured: false, ...over,
});
const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  state.handlers = [];
  state.calls = 0;
  state.coverage = {
    totals: { packs: 3, monsters: 834, companions: 361 },
    packs: [
      pack({}),
      pack({
        packId: "5etools-btest", name: "5etools-format: Bestiary Test", kind: "5etools", status: "partial", monsters: 504,
        inData: 506, skipped: 2, partial: 5, captured: true,
        skippedReasons: [{ value: "the str score is not a number", count: 2 }],
        partialNotes: [{ value: "alternate speeds", count: 4 }, { value: "mythic actions", count: 1 }],
        extras: { ...NO_EXTRAS, environment: 504, mythic: 1, covenCr: 3 },
      }),
      pack({ packId: "dl-aurora", name: "Aurora Legacy", kind: "index", status: "companions_only", monsters: 0, companions: 361, inData: null, byCr: [], byType: [] }),
    ],
  };
});

describe("summaryLine", () => {
  it("says how much of a book came in, in plain words", () => {
    expect(summaryLine(pack({}))).toBe("330 of 330 creatures");
    expect(summaryLine(state.coverage.packs[1]!)).toBe("504 of 506 creatures · 2 not imported · 5 with details kept as notes");
    expect(summaryLine(state.coverage.packs[2]!)).toBe("361 companions as Aurora lists them (no typed stat blocks).");
    expect(summaryLine(pack({ status: "installed", inData: null, monsters: 2, companions: 1 }))).toBe("2 creatures · 1 companion");
    expect(summaryLine(pack({ status: "unknown", inData: null, monsters: 1 }))).toMatch(/^1 creature\. Import or update this source again/);
    expect(summaryLine(pack({ status: "installed", inData: null, monsters: 2 }))).toBe("2 creatures");
  });
});

describe("BestiaryCoverage", () => {
  it("lists each source with its status and folds the details away", async () => {
    render(<BestiaryCoverage />);
    await settle();
    expect(screen.getByText(/834 creatures with stat blocks and 361 companions in 3 sources/)).toBeTruthy();
    const list = screen.getByRole("list", { name: "Bestiary by source" });
    const test = within(list).getByRole("listitem", { name: "5etools-format: Bestiary Test" });
    expect(within(test).getByText("Partly imported")).toBeTruthy();
    expect(within(test).queryByRole("group", { name: "Kept as notes" })).toBeNull();
    fireEvent.click(within(test).getByRole("button", { name: /Bestiary Test/ }));
    const notes = within(test).getByRole("group", { name: "Kept as notes" });
    expect(notes.textContent).toContain("alternate speeds");
    expect(within(test).getByRole("group", { name: "Extra details kept" }).textContent).toContain("with a coven CR");
    expect(within(test).getByRole("group", { name: "Not imported" }).textContent).toContain("2 the str score is not a number");
    const aurora = within(list).getByRole("listitem", { name: "Aurora Legacy" });
    expect(within(aurora).getByText("Companions only")).toBeTruthy();
  });

  it("loads again when the content reloads", async () => {
    render(<BestiaryCoverage />);
    await settle();
    expect(state.calls).toBe(1);
    state.handlers.forEach((h) => h());
    await settle();
    expect(state.calls).toBe(2);
  });
});
