import { describe, expect, it } from "vitest";
import type { MissingContentItem } from "../../src/api/contract";
import { groupMissingAcross, nextActions, type ResultLike, summarize } from "./results";

const item = (id: string, name: string, source: string): MissingContentItem => ({ id, name, source, type: "Spell", context: "selection" } as MissingContentItem);

describe("import results summary", () => {
  it("counts each outcome and names the worst", () => {
    const rows: ResultLike[] = [
      { title: "a.dnd5e", tone: "success", category: "character" },
      { title: "b.dnd5e", tone: "warning", category: "character" },
      { title: "c.txt", tone: "error", category: "other" },
    ];
    const s = summarize(rows);
    expect(s.counts).toMatchObject({ success: 1, warning: 1, error: 1 });
    expect(s.tone).toBe("error");
    expect(s.headline).toBe("Import finished: 3 items, 1 done, 1 with warnings, 1 failed.");
  });

  it("is in progress while any row works, and says when nothing came in", () => {
    expect(summarize([{ title: "a", tone: "working", category: "content" }, { title: "b", tone: "success", category: "content" }]).tone).toBe("working");
    expect(summarize([{ title: "a", tone: "error", category: "other" }]).headline).toBe("Nothing imported: 1 item, 1 failed.");
    expect(summarize([{ title: "a", tone: "success", category: "content" }]).tone).toBe("success");
  });

  it("groups missing content once across characters, by source, with who uses it", () => {
    const shared = item("ID_FIREBALL", "Fireball", "SRD");
    const rows: ResultLike[] = [
      { title: "ana.dnd5e", who: "Ana", tone: "warning", category: "character", missing: [shared, item("ID_X", "Unknown thing", "")] },
      { title: "bo.dnd5e", who: "Bo", tone: "warning", category: "character", missing: [shared, item("ID_SHIELD", "Shield", "SRD")] },
    ];
    const groups = groupMissingAcross(rows);
    expect(groups.map((g) => g.source)).toEqual(["SRD", "Unknown source"]);
    expect(groups[0]!.items.map((i) => [i.item.name, i.usedBy])).toEqual([["Fireball", ["Ana", "Bo"]], ["Shield", ["Bo"]]]);
  });

  it("what next: add the missing content first, then the characters", () => {
    const rows: ResultLike[] = [{ title: "ana", tone: "warning", category: "character", missing: [item("ID_A", "A", "SRD")] }];
    expect(nextActions(rows).map((a) => [a.id, a.primary])).toEqual([["add-missing", true], ["characters", false], ["parties", false]]);
  });

  it("what next: content only leads to a new character; nothing while a row waits or works", () => {
    expect(nextActions([{ title: "srd.xml", tone: "success", category: "content" }]).map((a) => a.id)).toEqual(["new-character"]);
    expect(nextActions([{ title: "x", tone: "info", category: "content", waiting: true }])).toEqual([]);
    expect(nextActions([{ title: "x", tone: "working", category: "content" }])).toEqual([]);
    expect(nextActions([{ title: "x", tone: "error", category: "other" }])).toEqual([]);
  });
});
