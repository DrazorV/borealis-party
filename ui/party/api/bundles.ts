import type { ApiError, BundleExport, BundlePreview, BundleShare, CharacterSummary, ImportReport, PickedPath } from "../../src/api/contract";
import { downloadBase64, fileToBase64 } from "../../src/lib/files";
import { openBundleFile } from "../../src/pages/transfer/openBundle";
import { serverFetch } from "./http";
import type { ServerSession } from "./session";

/**
 * `.borealis` bundles (desktop File > Export to bundle...) on a server. The browser has the file's bytes but no path, and
 * upstream's preview reads a bundle by `bundle-file` token, so the bytes go up first (`stage_bundle`) and come back as a
 * token in this account's own folder. From there upstream's Open bundle window (BundleHost) does the rest unchanged.
 * Export goes the other way: the server builds the file with upstream's builder and the bytes come down as a download.
 */

type Session = Pick<ServerSession, "baseUrl" | "token">;

/** What the server's `bundle_export` answers: upstream's `BundleExport` without a path, with the file's bytes. */
interface BundleFile {
  fileName: string;
  characters: number;
  packs: number;
  bytes: number;
  dataBase64: string;
}

/**
 * File > Export to bundle...: the server builds the bundle and the browser saves it as a download (the desktop app's
 * web view asks where). The answer names the file as its `path`, so upstream's dialog closes and the status bar says so.
 */
export async function exportBundle(session: Session, args: Record<string, unknown>): Promise<BundleExport> {
  const file = await serverFetch(session, "POST", "/api/rpc/bundle_export", args) as BundleFile;
  downloadBase64(file.fileName, BUNDLE_MIME, file.dataBase64);
  return { fileName: file.fileName, path: file.fileName, characters: file.characters, packs: file.packs, bytes: file.bytes };
}

const BUNDLE_MIME = "application/octet-stream";

/**
 * The same file for upstream's phone path of that dialog, which every party client takes (the server reports `mobile`
 * for all of them). On a phone the bytes go back for the web view's share sheet; elsewhere they are saved as a download
 * at once, so the dialog closes as shared.
 */
export async function shareBundle(session: Session, args: Record<string, unknown>, phone: boolean): Promise<BundleShare> {
  const file = await serverFetch(session, "POST", "/api/rpc/bundle_export", args) as BundleFile;
  const info = { fileName: file.fileName, characters: file.characters, packs: file.packs, bytes: file.bytes };
  if (phone) return { ...info, shared: false, dataBase64: file.dataBase64 };
  downloadBase64(file.fileName, BUNDLE_MIME, file.dataBase64);
  return { ...info, shared: true, dataBase64: null };
}

/** Uploads a bundle into this account's folder and returns a `bundle-file` token for `bundleOpen`. */
export async function stageBundle(session: Session, fileName: string, contentBase64: string): Promise<{ token: string; name: string }> {
  return await serverFetch(session, "POST", "/api/rpc/stage_bundle", { fileName, contentBase64 }) as { token: string; name: string };
}

/** Marks the file input this module adds, so a leftover one (a browser that never fires "cancel") is removed next time. */
const INPUT_MARK = "data-party-bundle-input";

/**
 * File > Open bundle...: the browser's own file chooser, then the upload. Resolves `[]` when nothing was chosen. No
 * `accept` filter: Android's web view crashes on an extension it has no MIME type for, and the server refuses non-bundles.
 * The click happens before any await, so it still counts as the user's gesture.
 */
export function pickBundleFile(session: Session): Promise<PickedPath[]> {
  document.querySelectorAll(`input[${INPUT_MARK}]`).forEach((old) => old.remove());
  const input = document.createElement("input");
  input.type = "file";
  input.setAttribute(INPUT_MARK, "");
  input.hidden = true;
  document.body.appendChild(input);
  return new Promise<PickedPath[]>((resolve, reject) => {
    input.addEventListener("cancel", () => { input.remove(); resolve([]); });
    input.addEventListener("change", () => {
      const file = input.files?.[0];
      input.remove();
      if (!file) { resolve([]); return; }
      fileToBase64(file)
        .then((b64) => stageBundle(session, file.name, b64))
        .then((s) => resolve([{ token: s.token, name: s.name, path: "" }]), reject);
    });
    input.click();
  });
}

/**
 * A bundle by name, or, for a name with no extension at all (Android's opaque provider ids), by content: a zip
 * ("PK\x03\x04" is "UEsD" in base64). Upstream's inbox sniffs only such names too, so a content `.zip` or a
 * misnamed `.json` still gets upstream's own import message.
 */
export function looksLikeBundle(fileName: string, contentBase64: string): boolean {
  const name = fileName.trim();
  return /\.borealis$/i.test(name) || (!name.includes(".") && contentBase64.startsWith("UEsD"));
}

/** A preview opened for Import characters, until the Open bundle window is accepted or cancelled. */
interface Waiting {
  preview: BundlePreview;
  done: (outcome: "accepted" | "cancelled") => void;
}

/**
 * Bundles handed to Import characters, by token until BundleHost opens them, then by bundle id. http.ts routes
 * `bundleOpen`, `bundleAccept` and `bundleDiscard` through {@link importWatch} so the import waits for the window.
 */
const byToken = new Map<string, Waiting>();
const byId = new Map<string, Waiting>();

/** The bundle calls BundleHost makes, so an import waiting on its window learns how it closed. */
export const importWatch = {
  /** BundleHost reopening a preview the import already made: the same preview, no second read of the token. */
  open(token: string): BundlePreview | null {
    const w = byToken.get(token);
    if (!w) return null;
    byToken.delete(token);
    byId.set(w.preview.bundleId, w);
    return w.preview;
  },
  settle(bundleId: string, outcome: "accepted" | "cancelled"): void {
    const w = byId.get(bundleId);
    byId.delete(bundleId);
    w?.done(outcome);
  },
};

const plural = (n: number, one: string) => (n === 1 ? `1 ${one}` : `${n} ${one}s`);

/**
 * A bundle handed to Import characters: uploaded and checked first, so a bad file gets only the import report's message.
 * A good one opens in upstream's Open bundle window, and the import waits for it, so the report comes after the window
 * (not under it) and tells what happened: the character that came in, or that nothing changed.
 */
export async function openBundleFromImport(session: Session, fileName: string, contentBase64: string): Promise<ImportReport> {
  const staged = await stageBundle(session, fileName, contentBase64);
  const preview = await serverFetch(session, "POST", "/api/rpc/bundle_open", { token: staged.token }) as BundlePreview;
  const outcome = await new Promise<"accepted" | "cancelled">((done) => {
    byToken.set(staged.token, { preview, done });
    openBundleFile({ token: staged.token, name: staged.name });
  });
  const items = preview.preview.items;
  const characters = items.filter((i) => i.kind === "character");
  if (outcome === "cancelled") {
    throw { code: "invalid_argument", message: "it is a Borealis bundle. The Open bundle window was cancelled, so nothing was changed." } satisfies ApiError;
  }
  const first = characters[0];
  if (!first) {
    const packs = items.length;
    throw { code: "invalid_argument", message: `it is a Borealis bundle with no characters. Its ${plural(packs, "content pack")} went into Additional Content.` } satisfies ApiError;
  }
  const list = await serverFetch(session, "POST", "/api/rpc/list_characters", {}) as CharacterSummary[];
  const summary = list.find((c) => c.id === first.key) ?? list.find((c) => c.name === first.label);
  if (!summary) throw { code: "not_found", message: `the bundle was applied, but ${first.label} is not in the list.` } satisfies ApiError;
  const warnings: string[] = [];
  if (first.action === "keep_local") warnings.push("The copy already here was edited more recently, so it was kept.");
  if (first.action === "identical") warnings.push("The copy already here was the same, so nothing changed.");
  const missing = [...new Set(characters.flatMap((c) => c.missingPacks))];
  if (missing.length > 0) warnings.push(`Content it needs that was not in the bundle: ${missing.join(", ")}.`);
  const others = characters.slice(1).map((c) => c.label);
  if (others.length > 0) warnings.push(`Also in the bundle: ${others.join(", ")}.`);
  return { summary, divergences: [], warnings, missingContent: [], unmappedSlots: [], invalidSelections: [], selectionsImported: 0 };
}
