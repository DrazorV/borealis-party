import { listen } from "@tauri-apps/api/event";
import { EVENTS, type Api, type OpenedFile } from "../../../src/api/contract";
import { createTauriApi } from "../../../src/api/tauri";
import { base64ToFile } from "../../../src/lib/files";
import { isTauri } from "../../../src/lib/tauri";
import { openImport, sendFiles } from "../../import/door";
import { ask } from "../ask";
import type { ServerSession } from "../session";
import { override } from "./registry";

/**
 * "Open with" / "Share to Borealis Party" (plan I4, decision D11). The OS hands the app a file; the app keeps it in the
 * device's queue (Android's activity, or Rust on iOS and the desktop) until the page asks with `take_opened_files`.
 * The apps are built without device-link, so that is the only queue: `take_opened_bundles` does not exist.
 * - Android reads every opened file into that queue as bytes, `.borealis` included.
 * - iOS and the desktop read only `.xml`, `.index`, `.zip`, `.dnd5e` and `.json` (content_preview.rs
 *   `push_opened_file`) and drop a `.borealis` without a word, so party/ios/Info.plist no longer claims `.borealis`.
 *
 * Online, the files are taken from the device and, after an app dialog ("Import X into <account>?"), handed to the
 * Import section (../../import/door.ts), which sniffs them and lists each result, as it does for files chosen or dropped
 * (0.1.5): characters, bundles (the Open bundle window), Aurora content (the import preview) and 5etools-format data.
 * Upstream's GlobalImport asks too, but gets none: the party drains the queue itself (`watchOpened`), so a file opened
 * while Additional Content is showing is taken as well.
 *
 * Offline, the page answers `take_opened_files` itself with none (../offline.ts), so the files stay in the device's
 * queue, and this module says they will be imported when the app is connected (the online page takes them on start).
 */

/** Names the device queue takes in (content_preview.rs `push_opened_file`, plus `.borealis` on Android). */
const IMPORTABLE = [".dnd5e", ".xml", ".index", ".zip", ".json", ".borealis"];

const importable = (name: string) => IMPORTABLE.some((e) => name.toLowerCase().endsWith(e));

export const OFFLINE_TITLE = "Will import when connected";
export const ASK_TITLE = "Import into your account";

export interface OpenedDeps {
  /** The device's queue (`take_opened_files` of the app's own backend). */
  take: () => Promise<OpenedFile[]>;
  ask: typeof ask;
  /** Hand the files to the Import section and show it. */
  toImport: (files: OpenedFile[]) => void;
  inApp: () => boolean;
}

const DEFAULT_DEPS: OpenedDeps = {
  take: () => createTauriApi().takeOpenedFiles(),
  ask,
  toImport: (files) => {
    sendFiles(files.map((f) => base64ToFile(f.fileName, f.contentBase64)));
    openImport(null);
  },
  inApp: isTauri,
};

export const openedDeps: OpenedDeps = { ...DEFAULT_DEPS };

/** "Hero.dnd5e", "Hero.dnd5e and SRD.xml", "3 files (a, b and c)". */
export function describeFiles(names: readonly string[]): string {
  const list = names.length <= 1 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return names.length > 2 ? `${names.length} files (${list})` : list;
}

/**
 * The opened files the Import section should take, after asking. Files no importer reads pass straight through, so the
 * screen says it can't import them; the rest wait for the answer, and Cancel drops them.
 */
export async function takeOpened(session: ServerSession, deps: OpenedDeps = openedDeps): Promise<OpenedFile[]> {
  if (!deps.inApp()) return [];
  const files = await deps.take().catch(() => [] as OpenedFile[]);
  if (files.length === 0) return [];
  const wanted = files.filter((f) => importable(f.fileName));
  const others = files.filter((f) => !importable(f.fileName));
  if (wanted.length === 0) return others;
  const names = wanted.map((f) => f.fileName);
  const answer = await deps.ask(ASK_TITLE, `Import ${describeFiles(names)} into ${session.user.username}?`, [
    { id: "import", label: "Import" },
    { id: "cancel", label: "Cancel" },
  ]);
  if (answer !== "import") return others;
  return [...wanted, ...others];
}

/** Takes the device's queue and hands it to the Import section, one question at a time. */
export function drainOpened(session: ServerSession, deps: OpenedDeps = openedDeps): Promise<void> {
  return oneAtATime(async () => {
    const files = await takeOpened(session, deps);
    if (files.length > 0) deps.toImport(files);
  });
}

/**
 * Online, from the party App: the queue is drained now (a file that started the app) and whenever the device says a
 * file was opened. Returns a cleanup.
 */
export function watchOpened(session: ServerSession): () => void {
  if (!isTauri()) return () => undefined;
  const drain = () => void drainOpened(session);
  drain();
  window.addEventListener("borealis-open-file", drain);
  let off: (() => void) | null = null;
  let stopped = false;
  void listen(EVENTS["content:open-file"], drain).then((u) => { if (stopped) u(); else off = u; }).catch(() => undefined);
  return () => {
    stopped = true;
    off?.();
    window.removeEventListener("borealis-open-file", drain);
  };
}

/** One question at a time: a second "Open with" while the first dialog is up waits for it. */
let queue: Promise<unknown> = Promise.resolve();
function oneAtATime<T>(work: () => Promise<T>): Promise<T> {
  const next = queue.then(work, work);
  queue = next.catch(() => undefined);
  return next;
}

/** The page is offline (../connectivity.ts sets this class for the whole page). */
const offline = () => document.documentElement.classList.contains("party-offline-mode");

let offlineNoticeShown = false;
/** Told once per page: the files stay in the device's queue until the app starts online. */
export async function noticeOffline(deps: Pick<OpenedDeps, "ask"> = openedDeps): Promise<void> {
  if (!offline() || offlineNoticeShown) return;
  offlineNoticeShown = true;
  await deps.ask(OFFLINE_TITLE, "The server can't be reached, so the file you opened is kept on this device. Borealis Party imports it into your account when it is connected again (open the app once it is back online).", [{ id: "ok", label: "OK" }]);
}

let watching = false;
function watchOffline(): void {
  if (watching || typeof window === "undefined") return;
  watching = true;
  // Android's activity nudges the page with this event; iOS and the desktop send the backend's event.
  window.addEventListener("borealis-open-file", () => void noticeOffline());
  if (isTauri()) void listen(EVENTS["content:open-file"], () => void noticeOffline()).catch(() => undefined);
}

/** Tests: start over. */
export function resetOpened(): void {
  offlineNoticeShown = false;
  queue = Promise.resolve();
  Object.assign(openedDeps, DEFAULT_DEPS);
}

watchOffline();

override("opened", (methods) => {
  // Upstream's GlobalImport asks on start and when nudged: the party has taken the files already (watchOpened).
  methods.takeOpenedFiles = async () => [] as OpenedFile[];
  // Nothing is ever set aside as a bundle here: Android hands `.borealis` over with the other files (above).
  methods.takeOpenedBundles = async () => [] as Awaited<ReturnType<Api["takeOpenedBundles"]>>;
});
