import { listen } from "@tauri-apps/api/event";
import { EVENTS, type Api, type OpenedFile } from "../../../src/api/contract";
import { createTauriApi } from "../../../src/api/tauri";
import { errorMessage } from "../../../src/state/useAsync";
import { isTauri } from "../../../src/lib/tauri";
import { openBundleFile } from "../../../src/pages/transfer/openBundle";
import { ask } from "../ask";
import { stageBundle } from "../bundles";
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
 * Online, the files are taken from the device and, after an app dialog ("Import X into <account>?"), handed to
 * upstream's own import routing (GlobalImport), whose commands go to this account on the server: characters to
 * `import_character`, Aurora content to the import preview, 5etools files to Additional Content. A `.borealis` goes up
 * as a staged bundle and opens upstream's Open bundle window, as File > Open bundle... does. ./errors.ts checks the
 * sizes and words the failures.
 *
 * Offline, the page answers `take_opened_files` itself with none (../offline.ts), so the files stay in the device's
 * queue, and this module says they will be imported when the app is connected (the online page takes them on start).
 */

/** Names the device queue takes in (content_preview.rs `push_opened_file`, plus `.borealis` on Android). */
const IMPORTABLE = [".dnd5e", ".xml", ".index", ".zip", ".json", ".borealis"];

const isBundle = (name: string) => name.toLowerCase().endsWith(".borealis");
const importable = (name: string) => IMPORTABLE.some((e) => name.toLowerCase().endsWith(e));

export const OFFLINE_TITLE = "Will import when connected";
export const ASK_TITLE = "Import into your account";

export interface OpenedDeps {
  /** The device's queue (`take_opened_files` of the app's own backend). */
  take: () => Promise<OpenedFile[]>;
  ask: typeof ask;
  /** Upload a `.borealis` and open upstream's Open bundle window on it. */
  openBundle: (session: Pick<ServerSession, "baseUrl" | "token">, file: OpenedFile) => Promise<void>;
  inApp: () => boolean;
}

const DEFAULT_DEPS: OpenedDeps = {
  take: () => createTauriApi().takeOpenedFiles(),
  ask,
  openBundle: async (session, file) => openBundleFile(await stageBundle(session, file.fileName, file.contentBase64)),
  inApp: isTauri,
};

export const openedDeps: OpenedDeps = { ...DEFAULT_DEPS };

/** "Hero.dnd5e", "Hero.dnd5e and SRD.xml", "3 files (a, b and c)". */
export function describeFiles(names: readonly string[]): string {
  const list = names.length <= 1 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return names.length > 2 ? `${names.length} files (${list})` : list;
}

/**
 * The opened files the page should import, after asking. Files no importer reads pass straight through, so upstream
 * says it can't import them; the rest wait for the answer, and Cancel drops them.
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
  for (const bundle of wanted.filter((f) => isBundle(f.fileName))) {
    try {
      await deps.openBundle(session, bundle);
    } catch (e) {
      await deps.ask(`${bundle.fileName} was not opened`, errorMessage(e), [{ id: "ok", label: "OK" }]);
    }
  }
  return [...wanted.filter((f) => !isBundle(f.fileName)), ...others];
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

override("opened", (methods, session) => {
  methods.takeOpenedFiles = () => oneAtATime(() => takeOpened(session));
  // Nothing is ever set aside as a bundle here: Android hands `.borealis` over with the other files (above).
  methods.takeOpenedBundles = async () => [] as Awaited<ReturnType<Api["takeOpenedBundles"]>>;
});
