import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FiveToolsUrl } from "../../src/api/contract";
import { memoryStorage } from "../test/memoryStorage";
import { ImportedFrom, whenText } from "./ImportedFrom";

const PLACES = ["Ashvale", "Brindlewood", "Dunmarrow", "Emberfall", "Frostwhistle", "Glimmerdeep", "Hollowpine", "Lanternmoor", "Mossgate", "Quillhaven", "Saltreach", "Thornmere", "Wyrmwater", "Ysolde"];
const PACK_IDS = PLACES.map((p) => `pack-${p}`);
const NAMES = Object.fromEntries(PLACES.map((p) => [`pack-${p}`, `5etools-format: Tales of ${p}`]));
const SOURCE: FiveToolsUrl = { url: "https://example.com/data/", revision: "abc", sources: [], packs: PACK_IDS, lastChecked: null, lastImported: "2026-10-02T08:00:00Z", problem: null };

beforeEach(() => { vi.stubGlobal("localStorage", memoryStorage()); });

describe("ImportedFrom", () => {
  it("shows a source on one line, its packs folded, and remembers it opened", () => {
    const onUpdate = vi.fn();
    const view = render(<ImportedFrom sources={[SOURCE]} packNames={NAMES} title={(u) => u} busy={false} onUpdate={onUpdate} />);
    expect(screen.getByText(/^5etools-format data · 14 packs · updated /)).toBeTruthy();
    const show = screen.getByRole("button", { name: "Show packs" });
    expect(show.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("Tales of Ashvale")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Update from https://example.com/data/" }));
    expect(onUpdate).toHaveBeenCalledWith("https://example.com/data/");

    fireEvent.click(show);
    const list = screen.getByRole("list", { name: "Packs" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(14);
    expect(list.textContent).not.toContain("5etools-format");
    fireEvent.change(screen.getByRole("searchbox", { name: "Search packs" }), { target: { value: "moor" } });
    expect(within(screen.getByRole("list", { name: "Packs" })).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["Tales of Lanternmoor"]);

    view.unmount();
    render(<ImportedFrom sources={[SOURCE]} packNames={NAMES} title={(u) => u} busy={false} onUpdate={onUpdate} />);
    expect(screen.getByRole("button", { name: "Hide packs" }).getAttribute("aria-expanded")).toBe("true");
  });

  it("has no search for a few packs, and no date when there is none", () => {
    render(<ImportedFrom sources={[{ ...SOURCE, packs: PACK_IDS.slice(0, 3), lastImported: null }]} packNames={NAMES} title={(u) => u} busy={false} onUpdate={() => {}} />);
    expect(screen.getByText("5etools-format data · 3 packs")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show packs" }));
    expect(screen.queryByRole("searchbox")).toBeNull();
    expect(whenText({ lastImported: "not a date", lastChecked: null })).toBeNull();
  });

  it("is the one place a 5etools source is listed: Update and Remove each (0.3.3)", () => {
    const onUpdate = vi.fn();
    const onRemove = vi.fn();
    render(<ImportedFrom sources={[SOURCE]} packNames={NAMES} title={(u) => u} busy={false} onUpdate={onUpdate} onRemove={onRemove} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove https://example.com/data/" }));
    expect(onRemove).toHaveBeenCalledWith("https://example.com/data/");
    expect(onUpdate).not.toHaveBeenCalled();
  });
});
