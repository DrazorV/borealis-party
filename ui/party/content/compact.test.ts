import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PackCoverage } from "../api/bestiary";
import { memoryStorage } from "../test/memoryStorage";
import { matches, pickSources, readPref, savePref, stripFormat } from "./compact";

const pack = (name: string, over: Partial<PackCoverage> = {}): PackCoverage => ({
  packId: name, name, kind: "5etools", enabled: true, packRev: "r", status: "complete", monsters: 10, companions: 0, inData: 10,
  skipped: 0, partial: 0, skippedReasons: [], partialNotes: [], sources: [], byCr: [], byType: [], rulesets: [],
  extras: { environment: 0, page: 0, mythic: 0, covenCr: 0, altSpeeds: 0, variants: 0, reactionIntro: 0, fluff: 0, tokens: 0 },
  captured: true, ...over,
});

describe("stripFormat", () => {
  it("drops the format label every 5etools pack carries", () => {
    expect(stripFormat("5etools-format: Tales of the Copper Fen")).toBe("Tales of the Copper Fen");
    expect(stripFormat("5ETOOLS-FORMAT:Sample")).toBe("Sample");
    expect(stripFormat("SRD 5.2.1 Monsters")).toBe("SRD 5.2.1 Monsters");
    // A name that is only the label keeps it.
    expect(stripFormat("5etools-format: ")).toBe("5etools-format: ");
  });
});

describe("matches", () => {
  it("ignores case, accents and the format label", () => {
    expect(matches("5etools-format: Ruins of Émberfall", "emberFALL")).toBe(true);
    expect(matches("5etools-format: Ruins of Emberfall", "format")).toBe(false);
    expect(matches("Anything", "   ")).toBe(true);
  });
});

describe("pickSources", () => {
  const packs = [
    pack("5etools-format: Beasts of Mossgate", { monsters: 40 }),
    pack("SRD 5.1 Monsters", { kind: "bundled", monsters: 317 }),
    pack("5etools-format: Atlas of Ashvale", { status: "partial", monsters: 5, inData: 6, skipped: 1 }),
    pack("5etools-format: Wardens of Thornmere", { status: "partial", partial: 2, monsters: 900 }),
  ];
  const names = (l: PackCoverage[]) => l.map((p) => stripFormat(p.name));

  it("puts SRD first, then by name or by count", () => {
    expect(names(pickSources(packs, { text: "", filter: "all", sort: "name" }))).toEqual(["SRD 5.1 Monsters", "Atlas of Ashvale", "Beasts of Mossgate", "Wardens of Thornmere"]);
    expect(names(pickSources(packs, { text: "", filter: "all", sort: "count" }))).toEqual(["SRD 5.1 Monsters", "Wardens of Thornmere", "Beasts of Mossgate", "Atlas of Ashvale"]);
  });

  it("filters by status and notes, and by the search text", () => {
    expect(names(pickSources(packs, { text: "", filter: "complete", sort: "name" }))).toEqual(["SRD 5.1 Monsters", "Beasts of Mossgate"]);
    expect(names(pickSources(packs, { text: "", filter: "partial", sort: "name" }))).toEqual(["Atlas of Ashvale", "Wardens of Thornmere"]);
    expect(names(pickSources(packs, { text: "", filter: "notes", sort: "name" }))).toEqual(["Wardens of Thornmere"]);
    expect(names(pickSources(packs, { text: "of", filter: "all", sort: "name" }))).toEqual(["Atlas of Ashvale", "Beasts of Mossgate", "Wardens of Thornmere"]);
  });
});

describe("prefs", () => {
  beforeEach(() => { vi.stubGlobal("localStorage", memoryStorage()); });

  it("keeps values and falls back on a missing, broken or mistyped one", () => {
    expect(readPref("x", false)).toBe(false);
    savePref("x", true);
    expect(readPref("x", false)).toBe(true);
    localStorage.setItem("party.content.y", "{not json");
    expect(readPref("y", "all")).toBe("all");
    localStorage.setItem("party.content.z", "3");
    expect(readPref("z", "all")).toBe("all");
  });

  it("does without storage", () => {
    const blocked = () => { throw new Error("blocked"); };
    vi.stubGlobal("localStorage", { getItem: blocked, setItem: blocked, removeItem: blocked, clear: blocked, key: blocked, length: 0 });
    expect(() => savePref("x", true)).not.toThrow();
    expect(readPref("x", false)).toBe(false);
  });
});
