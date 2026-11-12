import type { ContentPack } from "../../src/api/contract";
import { phoneOs } from "../../src/lib/phone";
import { serverFetch } from "../api/http";
import { loadSession } from "../api/session";

/**
 * Party wording in upstream's export dialog (TransferDialog), which every party client opens on its phone path
 * ("Share as a file", as the server reports every client as mobile). Upstream's markup carries no hook for this, so,
 * like entry/hideTransfer.ts, it is matched by text. Only text nodes' values change, never React's elements, so React
 * keeps its own nodes; the one element added (the licence line) sits after upstream's help line.
 *
 * - E9: off the phone apps the file is a download, so the dialog says "Export to bundle" and its button "Download".
 * - Everywhere: the help line says a bundle is for other players too.
 * - E6: packs marked not redistributable get a warning chip while packs go into the file.
 * - E4: a "Share with a code" button: no file at all, which is the way out on Android (it closes the dialog and opens
 *   pages/SharePage.tsx).
 */

export const PHONE_TITLE = "Share as a file";
export const EXPORT_TITLE = "Export to bundle";
export const DOWNLOAD = "Download";
export const UPSTREAM_HELP = "Homebrew and imported packs travel only between your own devices.";
export const PARTY_HELP = "A bundle is for your other devices and for other players: they open it with Borealis (Open bundle..., or from the Files or share sheet on a phone). The content packs you choose travel in it.";
export const LICENCE_LEAD = "Not redistributable:";
export const LICENCE_TAIL = "Send these only to players who own the source they came from.";

const LICENCE_MARK = "data-party-licence";
const CODE_MARK = "data-party-share-code";
export const CODE_LEAD = "No file needed:";
export const CODE_BUTTON = "Share with a code";

/** The line with the "Share with a code" button, once per dialog, after the help line. */
function codeLine(dialog: Element, help: Element): void {
  if (dialog.querySelector(`[${CODE_MARK}]`)) return;
  const line = document.createElement("p");
  line.className = "aur-set-help";
  line.setAttribute(CODE_MARK, "");
  line.append(`${CODE_LEAD} `);
  const button = document.createElement("button");
  button.type = "button";
  button.className = "btn small";
  button.textContent = CODE_BUTTON;
  button.addEventListener("click", () => {
    dialog.querySelector<HTMLButtonElement>(".dialog-head button[aria-label='Close']")?.click();
    window.location.hash = "#/share";
  });
  line.append(button, " another player types the code on any device.");
  help.after(line);
}

/** Reads the account's packs (tests pass their own). */
export type PackLister = () => Promise<Pick<ContentPack, "id" | "name" | "redistributable">[]>;

const serverPacks: PackLister = async () => {
  const session = loadSession();
  if (!session) return [];
  return await serverFetch(session, "POST", "/api/rpc/list_packs", {}) as ContentPack[];
};

let listPacks: PackLister = serverPacks;
/** Each open export dialog reads the packs once. */
const packsOf = new WeakMap<Element, { names: string[] | null }>();

export function setPackListerForTests(next: PackLister | null): void {
  listPacks = next ?? serverPacks;
}

function setText(root: Element, from: string, to: string): void {
  const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    if (n.nodeValue?.trim() === from) n.nodeValue = to;
  }
}

const offPhoneApps = () => phoneOs() === null;

/** The export dialog's pick step chose "No content packs". */
function noPacks(dialog: Element): boolean {
  const select = dialog.querySelector<HTMLSelectElement>(".aur-combo select");
  if (select) return select.value === "none";
  return dialog.querySelector(".aur-combo")?.textContent?.trim() === "No content packs";
}

function licenceLine(dialog: Element, help: Element, run: () => void): void {
  let entry = packsOf.get(dialog);
  if (!entry) {
    const fresh: { names: string[] | null } = { names: null };
    entry = fresh;
    packsOf.set(dialog, fresh);
    void listPacks()
      .then((packs) => { fresh.names = packs.filter((p) => p.redistributable === false).map((p) => p.name || p.id); })
      .catch(() => { fresh.names = []; })
      .finally(run);
  }
  const existing = dialog.querySelector(`[${LICENCE_MARK}]`);
  const names = entry.names ?? [];
  const wanted = names.length > 0 && !noPacks(dialog);
  const key = names.join("\n");
  if (!wanted) { existing?.remove(); return; }
  if (existing?.getAttribute(LICENCE_MARK) === key && existing.previousElementSibling === help) return;
  existing?.remove();
  const line = document.createElement("p");
  line.className = "aur-set-help";
  line.setAttribute(LICENCE_MARK, key);
  line.setAttribute("role", "note");
  line.append(`${LICENCE_LEAD} `);
  for (const name of names) {
    const chip = document.createElement("span");
    chip.className = "chip warning";
    chip.textContent = name;
    line.append(chip, " ");
  }
  line.append(LICENCE_TAIL);
  help.after(line);
}

/** One pass over the page: run on every change (entry/hideTransfer.ts observes the page). */
export function tweakExportDialog(run: () => void): void {
  for (const title of document.querySelectorAll(".dialog.xfer-dlg .dialog-head h2")) {
    const text = title.textContent?.trim();
    if (text !== PHONE_TITLE && text !== EXPORT_TITLE) continue;
    const dialog = title.closest(".dialog")!;
    if (offPhoneApps()) {
      setText(title, PHONE_TITLE, EXPORT_TITLE);
      const foot = dialog.querySelector(".dialog-foot");
      if (foot) setText(foot, "Share", DOWNLOAD);
    }
    const help = [...dialog.querySelectorAll(".aur-set-help:not([data-party-licence]):not([data-party-share-code])")]
      .find((p) => [UPSTREAM_HELP, PARTY_HELP].includes(p.textContent?.trim() ?? ""));
    if (!help) continue;
    setText(help, UPSTREAM_HELP, PARTY_HELP);
    codeLine(dialog, help);
    licenceLine(dialog, help, run);
  }
  // The phone More page's row (only the phone apps have that page; iOS keeps upstream's words).
  if (offPhoneApps()) {
    for (const row of document.querySelectorAll(".v5-row")) setText(row, PHONE_TITLE, EXPORT_TITLE);
  }
}
