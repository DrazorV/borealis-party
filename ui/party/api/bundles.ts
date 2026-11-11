import type { ApiError, PickedPath } from "../../src/api/contract";
import { fileToBase64 } from "../../src/lib/files";
import { openBundleFile } from "../../src/pages/transfer/openBundle";
import { serverFetch } from "./http";
import type { ServerSession } from "./session";

/**
 * `.borealis` bundles (desktop File > Export to bundle...) on a server. The browser has the file's bytes but no path, and
 * upstream's preview reads a bundle by `bundle-file` token, so the bytes go up first (`stage_bundle`) and come back as a
 * token in this account's own folder. From there upstream's Open bundle window (BundleHost) does the rest unchanged.
 */

type Session = Pick<ServerSession, "baseUrl" | "token">;

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

/**
 * A bundle handed to Import characters: uploaded and shown in the Open bundle window. The import report still gets a
 * row for it, which it prefixes with "Could not import this file:", so the message is worded to follow that.
 */
export async function openBundleFromImport(session: Session, fileName: string, contentBase64: string): Promise<never> {
  const staged = await stageBundle(session, fileName, contentBase64);
  openBundleFile({ token: staged.token, name: staged.name });
  const what = /\.borealis$/i.test(fileName.trim()) ? "it is a Borealis bundle" : "it looks like a Borealis bundle";
  throw { code: "invalid_argument", message: `${what}, so it opened in the Open bundle window for review.` } satisfies ApiError;
}
