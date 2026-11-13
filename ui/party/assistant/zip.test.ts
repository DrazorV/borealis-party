import { describe, expect, it } from "vitest";
import { crc32, indexFiles, storedZip } from "./zip";

describe("the index zip", () => {
  it("packs index_build's files, keeping paths inside the zip", () => {
    const files = indexFiles({ files: [
      { path: "my.index", content: "<index/>" },
      { path: "../../etc/passwd", content: "x" },
      { name: "/abs\\elements/a.xml", text: "<elements/>" },
      { path: "b.bin", contentBase64: "AAE=" },
      { path: "", content: "skipped" },
    ] });
    expect(files.map((f) => f.path)).toEqual(["my.index", "etc/passwd", "abs/elements/a.xml", "b.bin"]);
    expect(Array.from(files[3]!.bytes)).toEqual([0, 1]);
    expect(indexFiles({})).toEqual([]);
  });

  it("writes a zip with correct checksums and a central directory", () => {
    expect(crc32(new TextEncoder().encode("123456789"))).toBe(0xcbf43926);
    const zip = storedZip([{ path: "a.txt", bytes: new TextEncoder().encode("hello") }]);
    const view = new DataView(zip.buffer);
    expect(view.getUint32(0, true)).toBe(0x04034b50);
    expect(view.getUint32(14, true)).toBe(crc32(new TextEncoder().encode("hello")));
    const end = zip.length - 22;
    expect(view.getUint32(end, true)).toBe(0x06054b50);
    expect(view.getUint16(end + 10, true)).toBe(1);
    expect(new TextDecoder().decode(zip.subarray(30, 35))).toBe("a.txt");
  });
});
