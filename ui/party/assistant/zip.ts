/**
 * A custom index's files as a zip for the person to download (the user's D47: the app never hosts or publishes an
 * index). `index_build` returns the files as data; this packs them uncompressed ("stored"), which every unzip tool
 * reads, so no compression library is needed.
 */

export interface ZipFile { path: string; bytes: Uint8Array }

const TEXT = new TextEncoder();

/** index_build's files, as `{ files: [{ path | name, content | text | contentBase64 }] }`. Paths are kept relative. */
export function indexFiles(result: unknown): ZipFile[] {
  const list = (result as { files?: unknown } | null)?.files;
  if (!Array.isArray(list)) return [];
  const out: ZipFile[] = [];
  for (const f of list as Record<string, unknown>[]) {
    const raw = typeof f?.path === "string" ? f.path : typeof f?.name === "string" ? f.name : "";
    // No absolute paths, no "..": a zip that unpacks outside its folder is refused by this app.
    const path = raw.replace(/\\/g, "/").split("/").filter((p) => p && p !== "." && p !== "..").join("/");
    if (!path) continue;
    if (typeof f.contentBase64 === "string") out.push({ path, bytes: Uint8Array.from(atob(f.contentBase64), (c) => c.charCodeAt(0)) });
    else if (typeof f.content === "string") out.push({ path, bytes: TEXT.encode(f.content) });
    else if (typeof f.text === "string") out.push({ path, bytes: TEXT.encode(f.text) });
  }
  return out;
}

let table: Uint32Array | null = null;

/** CRC-32 (IEEE), as zip wants. */
export function crc32(bytes: Uint8Array): number {
  if (!table) {
    table = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      table[n] = c >>> 0;
    }
  }
  let crc = 0xffffffff;
  for (const b of bytes) crc = table[(crc ^ b) & 0xff]! ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

/** The files as one zip, stored without compression, names in UTF-8. */
export function storedZip(files: ZipFile[]): Uint8Array {
  const parts: Uint8Array[] = [];
  const central: Uint8Array[] = [];
  let offset = 0;
  for (const f of files) {
    const name = TEXT.encode(f.path);
    const crc = crc32(f.bytes);
    const local = new DataView(new ArrayBuffer(30));
    local.setUint32(0, 0x04034b50, true);
    local.setUint16(4, 20, true);
    local.setUint16(6, 0x0800, true); // UTF-8 names
    local.setUint32(14, crc, true);
    local.setUint32(18, f.bytes.length, true);
    local.setUint32(22, f.bytes.length, true);
    local.setUint16(26, name.length, true);
    parts.push(new Uint8Array(local.buffer), name, f.bytes);
    const dir = new DataView(new ArrayBuffer(46));
    dir.setUint32(0, 0x02014b50, true);
    dir.setUint16(4, 20, true);
    dir.setUint16(6, 20, true);
    dir.setUint16(8, 0x0800, true);
    dir.setUint32(16, crc, true);
    dir.setUint32(20, f.bytes.length, true);
    dir.setUint32(24, f.bytes.length, true);
    dir.setUint16(28, name.length, true);
    dir.setUint32(42, offset, true);
    central.push(new Uint8Array(dir.buffer), name);
    offset += 30 + name.length + f.bytes.length;
  }
  const size = central.reduce((n, p) => n + p.length, 0);
  const end = new DataView(new ArrayBuffer(22));
  end.setUint32(0, 0x06054b50, true);
  end.setUint16(8, files.length, true);
  end.setUint16(10, files.length, true);
  end.setUint32(12, size, true);
  end.setUint32(16, offset, true);
  const all = [...parts, ...central, new Uint8Array(end.buffer)];
  const out = new Uint8Array(all.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of all) { out.set(p, at); at += p.length; }
  return out;
}

export function toBase64(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}
