import { listen } from "@tauri-apps/api/event";
import { EVENTS, type FileDrop } from "../../src/api/contract";
import { createTauriApi } from "../../src/api/tauri";
import { closeTopOverlay } from "../../src/components/controls/overlayStack";
import { base64ToFile } from "../../src/lib/files";
import { isTauri } from "../../src/lib/tauri";
import { HOMEBREW_BUTTON, ZIP_BUTTON } from "../../src/pages/content/FiveToolsTab";
import { errorMessage } from "../../src/state/useAsync";
import { ask } from "../api/ask";
import { openImport, sendFiles, sendLink, type ImportFocus, type LinkFor } from "../import/door";

/**
 * Every place upstream starts an import leads to the one Import section, at the top of Additional Content (plan I9; one page since 0.2.1), so each kind of file and link
 * is read, sniffed and reported the same way. Upstream's buttons and menu items stay where they are and look the same;
 * a click on one (capturing phase, before upstream's handler) opens the Import section instead, with the file chooser
 * open for a file pick. Upstream's rows carry no attribute to match, so they are matched by their text, as
 * ./hideTransfer.ts does. If upstream rewords one, it falls back to upstream's own import, which still works.
 *
 * Kept where they are: the Additional Content page's lists, Update, Remove, enable and Clear (management).
 * Not redirected: "Import my content folder" and "Scan folder..." (desktop-only rows upstream hides in the party, which
 * the server reports as mobile; the Import section takes files, not folders).
 */

/** A file pick that opens the Import section's chooser instead. */
interface ClickRule {
  /** What the rule is, for tests and the docs. */
  name: string;
  selector: string;
  labels: readonly string[];
  /** The element's label: its text unless said otherwise. */
  label?: (el: HTMLElement) => string;
  focus: ImportFocus;
}

const text = (el: HTMLElement) => el.textContent?.replace(/\s+/g, " ").trim() ?? "";
const MENU_ITEM = ".aur-menu-item, .menu-tree-item";

export const CLICK_RULES: readonly ClickRule[] = [
  // Start page (CharactersPage): "Import characters / Load one or more .dnd5e files".
  { name: "start-import-characters", selector: ".v4-newbtn", labels: ["Import characters"], label: (el) => text(el.querySelector<HTMLElement>(".t") ?? el), focus: "files" },
  // File menu (desktop bar and the phone Menu sheet): Import Characters, Load Character and Open bundle... all pick files.
  { name: "menu-import", selector: MENU_ITEM, labels: ["Import Characters", "Load Character", "Open bundle...", "Open bundle…"], focus: "files" },
  // GlobalImport's floating "+".
  { name: "fab", selector: ".fab-import", labels: ["Import content"], label: (el) => el.getAttribute("aria-label") ?? "", focus: "files" },
  // Additional Content > Custom content: Import files.
  { name: "content-import-files", selector: ".v4-content .btn", labels: ["Import files"], focus: "files" },
  // Additional Content > 5etools-format import: the .zip and the homebrew .json choosers.
  { name: "content-5etools-files", selector: ".v4-content .btn", labels: [ZIP_BUTTON, HOMEBREW_BUTTON], focus: "files" },
];

/** A link form whose Fetch / Import takes the typed address to the Import section. */
export const LINK_FORMS: readonly { name: string; input: string; kind: LinkFor }[] = [
  // Additional Content > Index files: "Remote .index file URL" + Fetch.
  { name: "content-index-link", input: "#index-url", kind: "index" },
  // Additional Content > 5etools-format import: "Import from an address".
  { name: "content-5etools-link", input: "#fivetools-url", kind: "fivetools" },
];

/** The rule a click on `target` falls under, with the element it matched. */
export function matchClick(target: EventTarget | null): { rule: ClickRule; el: HTMLElement } | null {
  if (!(target instanceof Element)) return null;
  for (const rule of CLICK_RULES) {
    const el = target.closest<HTMLElement>(rule.selector);
    if (!el || (el as HTMLButtonElement).disabled) continue;
    if (rule.labels.includes((rule.label ?? text)(el))) return { rule, el };
  }
  return null;
}

/** Closes the menu a redirected item sits in, as upstream's own click would have. */
function closeMenu(el: HTMLElement): void {
  if (el.closest(".menu-tree")) closeTopOverlay();
  else if (el.closest(".aur-menu-panel")) el.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
}

const hasFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");

function onClick(e: MouseEvent): void {
  const hit = matchClick(e.target);
  if (!hit) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  closeMenu(hit.el);
  openImport(hit.rule.focus);
}

function onSubmit(e: Event): void {
  const form = e.target;
  if (!(form instanceof HTMLFormElement)) return;
  const rule = LINK_FORMS.find((f) => form.querySelector(f.input));
  if (!rule) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  const url = form.querySelector<HTMLInputElement>(rule.input)?.value ?? "";
  if (url.trim()) sendLink({ url, kind: rule.kind });
  // The address goes along and runs there; without one, the screen shows where to paste it.
  openImport(url.trim() ? null : rule.kind === "index" ? "link" : "fivetools");
}

function onDragOver(e: DragEvent): void {
  if (hasFiles(e)) e.preventDefault();
}

/**
 * Files dropped anywhere on the page go to the Import section. Listening on the document runs after errors.ts's size
 * guard (window, capturing), which stops a drop that is too large, and before upstream's GlobalImport (window, bubbling).
 */
function onDrop(e: DragEvent): void {
  if (!hasFiles(e)) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  const files = Array.from(e.dataTransfer?.files ?? []);
  if (files.length === 0) return;
  sendFiles(files);
  openImport(null);
}

export const DROP_LIMIT_TITLE = "Not imported from the drop";

/**
 * The desktop app: the window takes drops natively (Tauri) and its backend tells this page, which the server's event
 * stream doesn't relay, so the page listens to the device itself. Files are read from the device and go to the Import
 * screen. A dropped `.borealis` or folder can't be read into the page (upstream opens those by path on the device), so
 * the user is told to choose it on the Import section instead.
 */
export async function takeNativeDrop(drop: FileDrop, read = (tokens: string[]) => createTauriApi().takePickedFiles({ purpose: "dropped-file", tokens })): Promise<void> {
  const left = [...drop.bundles.map((b) => b.name), ...drop.folders.map((f) => f.name)];
  let files: File[] = [];
  let error = "";
  if (drop.files.length > 0) {
    try { files = (await read(drop.files.map((f) => f.token))).map((o) => base64ToFile(o.fileName, o.contentBase64)); }
    catch (err) { error = errorMessage(err); }
  }
  sendFiles(files);
  if (files.length > 0 || left.length > 0 || error || drop.rejected.length > 0) openImport(null);
  const notes: string[] = [];
  if (error) notes.push(error);
  if (left.length > 0) notes.push(`${left.join(", ")}: a bundle or folder can't be dropped here. Choose its files with Choose files in the Import section of Additional Content.`);
  if (drop.rejected.length > 0) notes.push(`${drop.rejected.join(", ")}: nothing here reads ${drop.rejected.length === 1 ? "this file" : "these files"}.`);
  if (notes.length > 0) await ask(DROP_LIMIT_TITLE, notes.join(" "), [{ id: "ok", label: "OK" }]);
}

let started = false;

/** Installs the redirects for the life of the page (once; returns a cleanup for tests). */
export function unifyImports(doc: Document = document): () => void {
  if (started) return () => undefined;
  started = true;
  doc.addEventListener("click", onClick, true);
  doc.addEventListener("submit", onSubmit, true);
  doc.addEventListener("dragover", onDragOver, true);
  doc.addEventListener("drop", onDrop, true);
  let unlisten: (() => void) | null = null;
  let stopped = false;
  if (isTauri()) {
    void listen<FileDrop>(EVENTS["files:dropped"], (event) => void takeNativeDrop(event.payload))
      .then((off) => { if (stopped) off(); else unlisten = off; })
      .catch(() => undefined);
  }
  return () => {
    stopped = true;
    started = false;
    unlisten?.();
    doc.removeEventListener("click", onClick, true);
    doc.removeEventListener("submit", onSubmit, true);
    doc.removeEventListener("dragover", onDragOver, true);
    doc.removeEventListener("drop", onDrop, true);
  };
}
