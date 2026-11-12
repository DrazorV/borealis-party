/**
 * The one way into the Import screen (`/import-files`, ../pages/ImportHub.tsx), for every place that starts an import:
 * upstream's buttons and menu items (../entry/unifyImports.ts), drops on the window and "Open with"
 * (../api/overrides/opened.ts). Each hands over what it has, and the screen sniffs and lists it the same way.
 *
 * - Files go into an inbox the screen empties (one import at a time; a pick while it is busy waits).
 * - A link goes in with what it was meant for (an index, or 5etools-format data).
 * - `focus` says which part of the screen to show: the file chooser, the link field, or the 5etools card.
 */

export const IMPORT_PATH = "/import-files";

export type ImportFocus = "files" | "link" | "fivetools";
export type LinkFor = "index" | "fivetools";

export interface InboxLink {
  url: string;
  kind: LinkFor;
}

let files: File[] = [];
let links: InboxLink[] = [];
const listeners = new Set<() => void>();
const changed = () => listeners.forEach((l) => l());

/** Hand files to the Import screen. */
export function sendFiles(list: readonly File[]): void {
  if (list.length === 0) return;
  files = [...files, ...list];
  changed();
}

/** Hand a link to the Import screen. */
export function sendLink(link: InboxLink): void {
  if (!link.url.trim()) return;
  links = [...links, { url: link.url.trim(), kind: link.kind }];
  changed();
}

/** What waits for the Import screen, emptied. */
export function takeInbox(): { files: File[]; links: InboxLink[] } {
  const out = { files, links };
  files = [];
  links = [];
  return out;
}

export const inboxEmpty = () => files.length === 0 && links.length === 0;

/** Told when something arrives. */
export function onInbox(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

type Navigate = (to: string) => void;
/** The router's navigate (the party App registers it); a hash change without it (HashRouter). */
let navigate: Navigate | null = null;
export function setImportNavigate(nav: Navigate | null): void {
  navigate = nav;
}

/** Counts the doors opened, so the screen can tell a new one from a re-render of the same address. */
let opened = 0;
export const doorCount = () => opened;
let lastFocus: ImportFocus | null = null;
/** The focus the last door asked for, once. */
export function takeFocus(): ImportFocus | null {
  const f = lastFocus;
  lastFocus = null;
  return f;
}

/** The chooser's input, made once and kept outside React, so a click can open it while the tap still counts. */
let chooser: HTMLInputElement | null = null;
function fileChooser(): HTMLInputElement {
  if (chooser?.isConnected) return chooser;
  const input = document.createElement("input");
  input.type = "file";
  input.multiple = true;
  input.hidden = true;
  // No accept filter: Android's web view fails on extensions it has no type for, and the files are sniffed anyway.
  input.setAttribute("aria-label", "Choose files for the Import screen");
  input.setAttribute("data-party-import-chooser", "");
  input.addEventListener("change", () => {
    const list = Array.from(input.files ?? []);
    input.value = "";
    sendFiles(list);
  });
  document.body.appendChild(input);
  chooser = input;
  return input;
}

/**
 * Opens the Import screen. With `focus: "files"` the file chooser opens at once: call it from the tap itself (a web
 * view ignores a chooser opened later), and the chosen files arrive on the screen.
 */
export function openImport(focus: ImportFocus | null = null): void {
  opened += 1;
  lastFocus = focus;
  if (focus === "files") fileChooser().click();
  if (navigate) navigate(IMPORT_PATH);
  else if (typeof window !== "undefined") window.location.hash = `#${IMPORT_PATH}`;
  changed();
}

/** Tests: start over. */
export function resetDoor(): void {
  files = [];
  links = [];
  listeners.clear();
  navigate = null;
  opened = 0;
  lastFocus = null;
  chooser?.remove();
  chooser = null;
}
