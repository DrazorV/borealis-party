import { MENUS, type MenuEntry } from "../../../src/components/shell/sections";
import { setStatus } from "../../../src/state/status";
import { errorMessage } from "../../../src/state/useAsync";
import { ask } from "../ask";
import { LOCAL } from "../http";
import { checkForUpdate, installUpdate, offerUpdate, updatesPossible, type AvailableUpdate } from "../desktopUpdate";
import { override } from "./registry";

/**
 * Help → Check for updates… in the desktop app (party/desktop/updates.md). Upstream's menus have no extension point,
 * so this adds an entry to upstream's Help menu list (`MENUS` in src/components/shell/sections.ts, read on every render)
 * after Privacy Policy. The entry is a web-page entry whose address is {@link CHECK_LINK}: upstream sends it to
 * `openLink`, which this module answers by checking the party feed at once instead of opening a page. Browsers and
 * phones get no entry ({@link updatesPossible}).
 * - Nothing newer: an app dialog says so, with this version.
 * - An update: an app dialog offers Install and restart (the update bar then shows the download) or Later (the bar
 *   keeps offering it).
 * - The feed can't be read, or this copy has no updater (a deb, an unpackaged build): an app dialog says why.
 */

export const CHECK_LINK = "borealis-party:check-for-updates";
export const CHECK_ENTRY: MenuEntry = { id: "party-check-updates", label: "Check for updates...", link: CHECK_LINK };
export const CHECK_TITLE = "Check for updates";

export interface CheckDeps {
  possible: () => boolean;
  check: () => Promise<AvailableUpdate | null>;
  ask: typeof ask;
  version: () => string;
  offer: typeof offerUpdate;
  install: typeof installUpdate;
}

const DEFAULT_DEPS: CheckDeps = {
  possible: updatesPossible,
  check: checkForUpdate,
  ask,
  version: () => __APP_VERSION__,
  offer: offerUpdate,
  install: installUpdate,
};

export const checkDeps: CheckDeps = { ...DEFAULT_DEPS };

/** Tests: start over. */
export function resetCheck(): void {
  Object.assign(checkDeps, DEFAULT_DEPS);
}

/** Puts the entry in upstream's Help menu when this copy can update itself, and takes it out otherwise. Idempotent. */
export function placeHelpEntry(possible: boolean): void {
  const help = MENUS.find((m) => m.id === "help");
  if (!help) return;
  const at = help.entries.findIndex((e) => e.id === CHECK_ENTRY.id);
  if (at >= 0) help.entries.splice(at, 1);
  if (!possible) return;
  const privacy = help.entries.findIndex((e) => e.id === "privacy");
  help.entries.splice(privacy >= 0 ? privacy + 1 : help.entries.length, 0, CHECK_ENTRY);
}

const OK = [{ id: "close", label: "OK" }];

/** Help → Check for updates…: asks the feed now and shows the answer in an app dialog. */
export async function checkNow(deps: CheckDeps = checkDeps): Promise<void> {
  setStatus("Checking for updates…");
  let update: AvailableUpdate | null;
  try {
    update = await deps.check();
  } catch (e) {
    setStatus("");
    await deps.ask(CHECK_TITLE, `Borealis Connect couldn't check for updates: ${errorMessage(e)}`, OK);
    return;
  }
  setStatus("");
  if (!update) {
    await deps.ask(CHECK_TITLE, `You're up to date (Borealis Connect ${deps.version()}).`, OK);
    return;
  }
  const answer = await deps.ask(CHECK_TITLE,
    `Borealis Connect ${update.version} is available (this is ${update.currentVersion}). Your characters stay as they are.`,
    [{ id: "install", label: "Install and restart", variant: "highlight" }, { id: "later", label: "Later" }]);
  const install = answer === "install";
  if (deps.offer({ update, install }) || !install) return;
  // No update bar on screen to show the download: install with the status line instead.
  setStatus(`Downloading Borealis Connect ${update.version}…`);
  try {
    await deps.install(update, (f) => setStatus(`Downloading Borealis Connect ${update.version}${f === null ? "…" : ` (${Math.round(f * 100)}%)`}`));
  } catch (e) {
    setStatus("");
    await deps.ask(CHECK_TITLE, `The update could not be installed: ${errorMessage(e)}`, OK);
  }
}

const isCheck = (args?: Record<string, unknown>) => args?.url === CHECK_LINK;
let offlineWrapped = false;

override("updates", (methods) => {
  placeHelpEntry(checkDeps.possible());
  const inner = methods.openLink;
  methods.openLink = (args?: Record<string, unknown>) => (isCheck(args) ? checkNow() : inner(args));
  // The offline pages (../offline.ts) answer openLink from LOCAL, not from these methods. Read here, not at import:
  // ../http.ts imports this module before it defines LOCAL.
  if (offlineWrapped) return;
  offlineWrapped = true;
  const local = LOCAL.openLink;
  LOCAL.openLink = (args) => (isCheck(args) ? checkNow() : local?.(args));
});
