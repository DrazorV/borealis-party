import { describe, expect, it } from "vitest";
import { planImport } from "./plan";
import { BOM, classifyJson, classifyLink, classifyZip, HEAD_BYTES, sniffFile, xmlRoot, zipEntryNames } from "./sniff";
import {
  AURORA_CHARACTER, AURORA_INDEX, AURORA_PACK, AURORA_ZIP, BOREALIS_PACK, BUNDLE_ZIP, fileOf, FIVETOOLS_HOMEBREW,
  FIVETOOLS_ZIP, NATIVE_CHARACTER, zipOf,
} from "./testFiles";

describe("sniffFile", () => {
  it("tells files by their names when the names are right", async () => {
    expect(await sniffFile(fileOf("Ana.dnd5e", AURORA_CHARACTER))).toBe("character");
    expect(await sniffFile(fileOf("srd.xml", AURORA_PACK))).toBe("pack");
    expect(await sniffFile(fileOf("srd.index", AURORA_INDEX))).toBe("pack");
    expect(await sniffFile(fileOf("content.borealis", BUNDLE_ZIP()))).toBe("bundle");
    expect(await sniffFile(fileOf("srd.zip", AURORA_ZIP()))).toBe("pack");
    expect(await sniffFile(fileOf("data.zip", FIVETOOLS_ZIP()))).toBe("fivetools-zip");
    expect(await sniffFile(fileOf("brew.json", FIVETOOLS_HOMEBREW))).toBe("fivetools");
    expect(await sniffFile(fileOf("ana.json", NATIVE_CHARACTER))).toBe("character");
    expect(await sniffFile(fileOf("pack.json", BOREALIS_PACK))).toBe("pack");
  });

  it("goes by content when the name is wrong or missing", async () => {
    // Android's providers hand over opaque names; downloads get renamed.
    expect(await sniffFile(fileOf("document:1234", AURORA_CHARACTER))).toBe("character");
    expect(await sniffFile(fileOf("Ana.xml", AURORA_CHARACTER))).toBe("character");
    expect(await sniffFile(fileOf("Download (2).zip", BUNDLE_ZIP()))).toBe("bundle");
    expect(await sniffFile(fileOf("msf:42", BUNDLE_ZIP()))).toBe("bundle");
    expect(await sniffFile(fileOf("content.txt", AURORA_PACK))).toBe("pack");
    expect(await sniffFile(fileOf("brew.txt", FIVETOOLS_HOMEBREW))).toBe("fivetools");
    expect(await sniffFile(fileOf("Ana.dnd5e", NATIVE_CHARACTER))).toBe("character");
    expect(await sniffFile(fileOf("data.borealis", FIVETOOLS_ZIP()))).toBe("fivetools-zip");
  });

  it("reads past a byte order mark and leading space", async () => {
    expect(await sniffFile(fileOf("a", `${BOM}  \n${AURORA_PACK}`))).toBe("pack");
    expect(await sniffFile(fileOf("b", `${BOM}${FIVETOOLS_HOMEBREW}`))).toBe("fivetools");
    expect(xmlRoot(`${BOM}<elements/>`)).toBe("elements");
  });

  it("falls back to the name for unreadable bytes, and says other when neither tells", async () => {
    expect(await sniffFile(fileOf("broken.borealis", "not a zip"))).toBe("bundle");
    expect(await sniffFile(fileOf("photo.png", new Uint8Array([0x89, 0x50, 0x4e, 0x47, 1, 2, 3])))).toBe("other");
    expect(await sniffFile(fileOf("notes.txt", "hello"))).toBe("other");
    expect(await sniffFile(fileOf("empty.json", ""))).toBe("other");
    expect(await sniffFile(fileOf("list.json", "[1,2]"))).toBe("other");
  });

  it("guesses a large JSON from its start", async () => {
    const big = `{"_meta":{"sources":[]},"spell":[${Array.from({ length: 30000 }, (_, i) => `{"name":"Spell ${i}","source":"SRDX"}`).join(",")}]}`;
    expect(big.length).toBeGreaterThan(HEAD_BYTES);
    expect(await sniffFile(fileOf("all.json", big))).toBe("fivetools");
  });
});

describe("zips", () => {
  it("lists entries, past a comment", async () => {
    const zip = zipOf({ "a.xml": "<elements/>", "dir/b.index": "<index/>" }, "made by a test");
    expect(await zipEntryNames(new Blob([zip]))).toEqual(["a.xml", "dir/b.index"]);
  });

  it("answers null for a zip with no end record", async () => {
    expect(await zipEntryNames(new Blob([new Uint8Array([0x50, 0x4b, 3, 4, 0, 0, 0])]))).toBeNull();
  });

  it("classifies by entry names", () => {
    expect(classifyZip(["manifest.json", "characters/a.json"], "x.zip")).toBe("bundle");
    expect(classifyZip(["data/books.json"], "x.zip")).toBe("fivetools-zip");
    expect(classifyZip(["books.json"], "x.zip")).toBe("fivetools-zip");
    expect(classifyZip(["__MACOSX/manifest.json", "srd.xml"], "x.zip")).toBe("pack");
    expect(classifyZip(["srd.xml"], "x.borealis")).toBe("bundle");
    expect(classifyZip(["sub\\manifest.json"], "x.zip")).toBe("pack");
  });
});

describe("text", () => {
  it("finds the XML root", () => {
    expect(xmlRoot(AURORA_CHARACTER)).toBe("character");
    expect(xmlRoot("<!DOCTYPE x><index/>")).toBe("index");
    expect(xmlRoot("plain")).toBeNull();
  });

  it("classifies the start of a JSON file", () => {
    expect(classifyJson('{"format":"borealis-content/1","elem', false)).toBe("pack");
    expect(classifyJson('{"monster": [ {"name":"Goblin"', false)).toBe("fivetools");
    expect(classifyJson('{"name":"Ana","level":3,', false)).toBe("character");
    expect(classifyJson("{not json", true)).toBe("other");
  });
});

describe("classifyLink", () => {
  it("tells index files from 5etools sources", () => {
    expect(classifyLink("https://example.com/content/srd.index")).toEqual({ kind: "index", url: "https://example.com/content/srd.index" });
    expect(classifyLink("https://raw.githubusercontent.com/someone/repo/main/srd.INDEX")?.kind).toBe("index");
    expect(classifyLink("https://github.com/someone/data-repo")?.kind).toBe("fivetools");
    expect(classifyLink("https://github.com/someone/repo/tree/main/data")?.kind).toBe("fivetools");
    expect(classifyLink("https://example.com/my-data/data/")?.kind).toBe("fivetools");
    expect(classifyLink("https://example.com/something")?.kind).toBe("unknown");
  });

  it("adds https:// when the scheme is missing, and keeps another scheme for the server to refuse", () => {
    expect(classifyLink("  example.com/srd.index ")).toEqual({ kind: "index", url: "https://example.com/srd.index" });
    expect(classifyLink("http://192.168.1.10/srd.index")).toEqual({ kind: "index", url: "http://192.168.1.10/srd.index" });
  });

  it("answers null for text that is no address", () => {
    expect(classifyLink("")).toBeNull();
    expect(classifyLink("two words")).toBeNull();
    expect(classifyLink("https://")).toBeNull();
  });
});

describe("planImport", () => {
  it("routes every file of one pick to its importer", async () => {
    const files = [
      fileOf("Ana.dnd5e", AURORA_CHARACTER), fileOf("srd.xml", AURORA_PACK), fileOf("srd.index", AURORA_INDEX),
      fileOf("content.borealis", BUNDLE_ZIP()), fileOf("brew.json", FIVETOOLS_HOMEBREW), fileOf("data.zip", FIVETOOLS_ZIP()),
      fileOf("photo.png", "png"),
    ];
    const plan = await planImport(files);
    const names = (list: File[]) => list.map((f) => f.name);
    expect(names(plan.characters)).toEqual(["Ana.dnd5e"]);
    expect(names(plan.packs)).toEqual(["srd.xml", "srd.index"]);
    expect(names(plan.bundles)).toEqual(["content.borealis"]);
    expect(names(plan.fiveToolsFiles)).toEqual(["brew.json"]);
    expect(names(plan.fiveToolsZips)).toEqual(["data.zip"]);
    expect(names(plan.others)).toEqual(["photo.png"]);
    expect(plan.kinds.get(files[0]!)).toBe("character");
  });

  it("treats a file it could not read as other", async () => {
    const plan = await planImport([fileOf("x", "")], () => Promise.reject(new Error("read failed")));
    expect(plan.others).toHaveLength(1);
  });
});
