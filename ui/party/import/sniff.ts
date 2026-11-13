import { readBuffer } from "../../src/lib/files";
import { looksLikeFiveTools } from "../../src/lib/importRouting";

/**
 * What a file handed to the Import screen is (plan I9), decided by its bytes first and its name only when the bytes
 * don't say. Phones hand over names like "document:1234" or "Download (2).txt", so the extension is a weak hint.
 * - `character`: an Aurora `.dnd5e` (XML with a `<character>` root) or a native character JSON;
 * - `bundle`: a `.borealis` file (a zip with `manifest.json` at its root);
 * - `pack`: Aurora content (XML, `.index`, or a zip of them) or a Borealis content-pack JSON;
 * - `fivetools`: a 5etools-format homebrew or data JSON;
 * - `fivetools-zip`: a zip of a 5etools `data/` folder (it holds a `books.json`);
 * - `other`: nothing here reads it.
 */
export type FileKind = "character" | "bundle" | "pack" | "fivetools" | "fivetools-zip" | "other";

/** How much of a file is read to tell what it is. A JSON file that fits is parsed whole; a bigger one is guessed from its start. */
export const HEAD_BYTES = 512 * 1024;

const ZIP_LOCAL = 0x04034b50;
const ZIP_EMPTY = 0x06054b50;
const ZIP_EOCD = 0x06054b50;
const ZIP64_LOCATOR = 0x07064b50;
const ZIP_CENTRAL = 0x02014b50;
/** The end record is 22 bytes plus a comment of at most 65535. */
const EOCD_SEARCH = 22 + 0xffff;
/** No real central directory is this large; a bigger one is not read (the server checks the file anyway). */
const MAX_DIRECTORY = 32 * 1024 * 1024;

const extOf = (name: string) => {
  const base = name.trim().toLowerCase().split(/[\\/]/).pop() ?? "";
  return base.includes(".") ? base.split(".").pop()! : "";
};

const bytesOf = async (blob: Blob, start: number, end: number) => new Uint8Array(await readBuffer(blob.slice(start, end)));
const u32 = (b: Uint8Array, at: number) => (b[at]! | (b[at + 1]! << 8) | (b[at + 2]! << 16) | (b[at + 3]! << 24)) >>> 0;
const u16 = (b: Uint8Array, at: number) => b[at]! | (b[at + 1]! << 8);
const u64 = (b: Uint8Array, at: number) => u32(b, at) + u32(b, at + 4) * 2 ** 32;

export function isZip(head: Uint8Array): boolean {
  if (head.length < 4) return false;
  const sig = u32(head, 0);
  return sig === ZIP_LOCAL || sig === ZIP_EMPTY;
}

/**
 * The names of the entries in a zip, read from its central directory at the end of the file (only that part is
 * read). Null when the file has no readable directory.
 */
export async function zipEntryNames(blob: Blob): Promise<string[] | null> {
  const size = blob.size;
  const tailStart = Math.max(0, size - EOCD_SEARCH);
  const tail = await bytesOf(blob, tailStart, size);
  let eocd = -1;
  for (let i = tail.length - 22; i >= 0; i--) {
    if (u32(tail, i) === ZIP_EOCD) { eocd = i; break; }
  }
  if (eocd < 0) return null;
  let count = u16(tail, eocd + 10);
  let dirSize = u32(tail, eocd + 12);
  let dirOffset = u32(tail, eocd + 16);
  if (dirOffset === 0xffffffff || dirSize === 0xffffffff || count === 0xffff) {
    // Zip64: the locator just before the end record points at the zip64 end record.
    const loc = eocd - 20;
    if (loc < 0 || u32(tail, loc) !== ZIP64_LOCATOR) return null;
    const recordAt = u64(tail, loc + 8);
    const record = await bytesOf(blob, recordAt, recordAt + 56);
    if (record.length < 56) return null;
    count = u64(record, 32);
    dirSize = u64(record, 40);
    dirOffset = u64(record, 48);
  }
  if (dirSize > MAX_DIRECTORY || dirOffset + dirSize > size) return null;
  const dir = await bytesOf(blob, dirOffset, dirOffset + dirSize);
  const names: string[] = [];
  const text = new TextDecoder();
  let at = 0;
  while (at + 46 <= dir.length && names.length < count && u32(dir, at) === ZIP_CENTRAL) {
    const nameLen = u16(dir, at + 28);
    const extraLen = u16(dir, at + 30);
    const commentLen = u16(dir, at + 32);
    names.push(text.decode(dir.subarray(at + 46, at + 46 + nameLen)));
    at += 46 + nameLen + extraLen + commentLen;
  }
  return names;
}

/** What a zip is, by its entry names: a bundle has `manifest.json` at its root, 5etools data a `books.json`. */
export function classifyZip(names: readonly string[], fileName: string): FileKind {
  const clean = names.map((n) => n.replace(/\\/g, "/")).filter((n) => !n.startsWith("__MACOSX/"));
  if (clean.includes("manifest.json")) return "bundle";
  if (clean.some((n) => n === "books.json" || n.endsWith("/books.json"))) return "fivetools-zip";
  if (extOf(fileName) === "borealis") return "bundle";
  return "pack";
}

/** A byte order mark (U+FEFF), which TextDecoder keeps when it is not at the very start. */
export const BOM = String.fromCharCode(0xfeff);
const withoutBom = (text: string) => (text.startsWith(BOM) ? text.slice(1) : text);

/** The name of an XML document's root element, past the declaration, comments and a doctype. */
export function xmlRoot(text: string): string | null {
  const body = withoutBom(text)
    .replace(/<\?[\s\S]*?\?>/g, "")
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<!DOCTYPE[^>]*>/gi, "")
    .trimStart();
  const m = /^<([A-Za-z_][\w.:-]*)/.exec(body);
  return m ? m[1]!.toLowerCase() : null;
}

/** A Borealis content pack has a content-format marker or lists elements; a native character has neither. */
function looksLikePackJson(value: unknown): boolean {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const o = value as Record<string, unknown>;
  return (typeof o.format === "string" && /content/i.test(o.format)) || Array.isArray(o.elements);
}

const FIVETOOLS_KEYS = /"(_meta|spell|item|baseitem|magicvariant|monster|class|subclass|classFeature|subclassFeature|race|subrace|feat|background|optionalfeature|language|condition)"\s*:\s*[[{]/;

/** A JSON document: whole when `complete`, else only its start, where the keys that tell are usually found. */
export function classifyJson(text: string, complete: boolean): FileKind {
  if (complete) {
    let value: unknown;
    try { value = JSON.parse(text); } catch { return "other"; }
    if (looksLikePackJson(value)) return "pack";
    if (looksLikeFiveTools(text)) return "fivetools";
    return value && typeof value === "object" && !Array.isArray(value) ? "character" : "other";
  }
  if (/"format"\s*:\s*"[^"]*content/i.test(text) || /"elements"\s*:\s*\[/.test(text)) return "pack";
  if (FIVETOOLS_KEYS.test(text)) return "fivetools";
  return "character";
}

/** A file whose bytes said nothing, by its name. */
function byName(fileName: string): FileKind {
  switch (extOf(fileName)) {
    case "borealis": return "bundle";
    case "dnd5e": return "character";
    case "xml":
    case "index": return "pack";
    default: return "other";
  }
}

/** What `file` is (see {@link FileKind}). Reads at most its first {@link HEAD_BYTES} and, for a zip, its directory. */
export async function sniffFile(file: Blob & { name: string }): Promise<FileKind> {
  const head = await bytesOf(file, 0, Math.min(file.size, HEAD_BYTES));
  if (isZip(head)) {
    const names = await zipEntryNames(file);
    return names ? classifyZip(names, file.name) : byName(file.name) === "bundle" ? "bundle" : "pack";
  }
  const text = withoutBom(new TextDecoder().decode(head));
  const start = text.trimStart();
  if (start.startsWith("<")) {
    const root = xmlRoot(start);
    if (root === "character") return "character";
    return root ? "pack" : byName(file.name);
  }
  if (start.startsWith("{") || start.startsWith("[")) {
    const kind = classifyJson(text, file.size <= HEAD_BYTES);
    return kind === "other" ? byName(file.name) : kind;
  }
  return byName(file.name);
}

/** What a pasted address is: an index (`.index`), 5etools-format data (a GitHub repository or a `data/` folder), or unsure. */
export type LinkKind = "index" | "fivetools" | "unknown";

export interface Link {
  kind: LinkKind;
  url: string;
}

const GITHUB_HOSTS = new Set(["github.com", "www.github.com", "raw.githubusercontent.com"]);

/**
 * Classifies a pasted address. An address without a scheme gets `https://`; any other scheme is passed on as typed,
 * so the server's own refusal says why. Null for text that is no address at all.
 */
export function classifyLink(text: string): Link | null {
  const typed = text.trim();
  if (!typed || /\s/.test(typed)) return null;
  const url = /^[a-z][a-z0-9+.-]*:\/\//i.test(typed) ? typed : `https://${typed}`;
  let parsed: URL;
  try { parsed = new URL(url); } catch { return null; }
  if (!parsed.hostname) return null;
  const path = parsed.pathname.toLowerCase();
  if (path.endsWith(".index")) return { kind: "index", url };
  if (GITHUB_HOSTS.has(parsed.hostname.toLowerCase())) return { kind: "fivetools", url };
  if (/\/data\/?$/.test(path)) return { kind: "fivetools", url };
  return { kind: "unknown", url };
}

/**
 * Where "Get the default 5etools data" downloads from: the public 5etools source repository on GitHub. The server's
 * importer resolves its newest commit and downloads only the files it reads; Update later re-imports the same packs.
 */
export const LATEST_5ETOOLS_URL = "https://github.com/5etools-mirror-3/5etools-src";

/** The default 5etools source as the Import screen names it. */
export const DEFAULT_5ETOOLS = { name: "5etools-src on GitHub (5etools-mirror-3)", url: LATEST_5ETOOLS_URL } as const;

/**
 * What the server takes as a 5etools source (`parse_location` in src-tauri/src/content_fivetools_remote.rs): an
 * https:// address of a GitHub repository (`github.com/owner/repo`), a branch or folder in one (`…/tree/branch/folder`),
 * the same on raw.githubusercontent.com, or any https:// folder that holds `books.json` (a file in it stands for the
 * folder). Private and LAN addresses are refused by the server, which says so in its own words.
 */
export type SourceCheck = { url: string; error?: undefined } | { url?: undefined; error: string };

export function checkFiveToolsSource(text: string): SourceCheck {
  const typed = text.trim();
  if (!typed) return { error: "Enter the address of your 5etools source." };
  if (/\s/.test(typed)) return { error: "An address has no spaces in it." };
  const url = /^[a-z][a-z0-9+.-]*:\/\//i.test(typed) ? typed : `https://${typed}`;
  let parsed: URL;
  try { parsed = new URL(url); } catch { return { error: "That is not a web address." }; }
  if (parsed.protocol !== "https:") return { error: "Use an https:// address." };
  if (!parsed.hostname) return { error: "That is not a web address." };
  const path = parsed.pathname.replace(/\/+$/, "");
  if (path.toLowerCase().endsWith(".index")) return { error: "That is an index file. Add it under Paste a link." };
  const parts = path.split("/").filter(Boolean);
  const host = parsed.hostname.toLowerCase();
  if (host === "github.com" || host === "www.github.com") {
    const [, , tree, ref] = parts;
    const repoOnly = parts.length === 2;
    const inTree = parts.length >= 4 && tree === "tree" && !!ref;
    if (!repoOnly && !inTree) {
      return { error: "On GitHub, use the repository's address (github.com/owner/repo) or a branch or folder in it (…/tree/branch/folder)." };
    }
  } else if (host === "raw.githubusercontent.com" && parts.length < 3) {
    return { error: "A raw.githubusercontent.com address needs the owner, the repository and the branch." };
  }
  return { url };
}
