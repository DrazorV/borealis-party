import { describe, expect, it } from "vitest";
import type { FiveToolsImportReport, ImportNote } from "../../src/api/contract";
import { elementName, explainNotes, notesLine, viewReport } from "./reportNotes";

const note = (reason: string, element = "Goblin|SRDX", kind = "monster"): ImportNote => ({ element, kind, reason });
const many = (n: number, reason: string) => Array.from({ length: n }, (_, i) => note(reason, `Creature ${i}|SRDX`));

function report(over: Partial<FiveToolsImportReport> = {}): FiveToolsImportReport {
  return { converted: 16746, replaced: 0, partial: [], skipped: [], packs: [{ id: "a", name: "SRD Sample" }] as never, ...over };
}

describe("explainNotes", () => {
  it("names each group in plain words and says what it means", () => {
    const groups = explainNotes([
      ...many(411, "creature section: variant sidebars not imported"),
      ...many(21, "creature section: mythic actions not imported"),
      ...many(20, "creature section: the reactions' introduction not imported"),
      ...many(3, "creature section: the actions' introduction not imported"),
      ...many(11, "the challenge rating in a coven not imported"),
      ...many(10, "creature field: fly speeds not imported"),
      note("character option (Supernatural Gift) kept as text: no builder choice offers it", "Gift|SRDX", "feat"),
    ], "fallback");
    expect(groups.map((g) => [g.title, g.notes.length])).toEqual([
      ["Variant sidebars", 411], ["Action introductions", 23], ["Mythic actions", 21], ["Coven challenge ratings", 11],
      ["Special speeds", 10], ["Character options kept as text", 1],
    ]);
    expect(groups[0]!.meaning).toMatch(/stats are complete/);
    expect(groups[2]!.meaning).toMatch(/work without them/);
  });

  it("keeps the importer's own words, in sentence case, for a reason it doesn't know", () => {
    const [g] = explainNotes([note("something new 3 times not imported")], "The rest works.");
    expect(g!.title).toBe("Something new N times not imported");
    expect(g!.meaning).toBe("The rest works.");
  });

  it("shows an element as its name and source", () => {
    expect(elementName("Goblin|SRDX")).toBe("Goblin (SRDX)");
    expect(elementName("plain")).toBe("plain");
  });
});

describe("viewReport", () => {
  it("is a success with notes when the import brought content in", () => {
    const view = viewReport(report({ partial: many(833, "creature section: variant sidebars not imported") }));
    expect(view.tone).toBe("success");
    expect(view.headline).toBe("Imported 16746 elements into 1 content pack.");
    expect(notesLine(view)).toBe("833 details couldn't be carried over. They don't stop anything from working.");
  });

  it("counts entries left out apart, and not the ones the core rules already have", () => {
    const view = viewReport(report({
      skipped: [note("creature stat block: the creature type is a choice"), note("already in the core pack", "Common|PHB", "language")],
    }));
    expect(view.tone).toBe("success");
    expect(view.skippedCount).toBe(1);
    expect(view.expected).toBe(1);
    expect(notesLine(view)).toBe("1 entry couldn't be read and was left out.");
  });

  it("warns when nothing was imported", () => {
    expect(viewReport(report({ converted: 0, packs: [] })).tone).toBe("warning");
    expect(viewReport(report({ converted: 0, packs: [] })).headline).toMatch(/^Nothing was imported/);
  });

  it("has no notes line without notes", () => {
    expect(notesLine(viewReport(report()))).toBe("");
  });
});
