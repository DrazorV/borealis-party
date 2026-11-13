import { Channel, invoke } from "@tauri-apps/api/core";
import { phoneOs } from "../../src/lib/phone";
import { isTauri } from "../../src/lib/tauri";

/**
 * In-app updates for the party desktop apps (party/desktop/updates.md). The party config (tauri.party.desktop.json)
 * gives upstream's updater plugin the party release feed and key, so upstream registers it for a copy that can replace
 * itself (the Windows NSIS install, an AppImage) and leaves it out elsewhere (a deb, an unpackaged build, phones). The
 * web view drives it with the plugin's own two commands: `check` reads the signed feed and compares with this build's
 * version, `download_and_install` checks the signature and installs. Upstream's own update notice asks upstream's
 * GitHub about upstream's version, so it is not used here.
 */

/** What the feed offers: the plugin's `check` answer. */
export interface AvailableUpdate {
  rid: number;
  currentVersion: string;
  version: string;
  body?: string | null;
}

/** The plugin's download events. */
type DownloadEvent =
  | { event: "Started"; data: { contentLength?: number | null } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" };

/** Whether this copy can be asked at all: the desktop app, not a browser tab or a phone. */
export function updatesPossible(): boolean {
  return isTauri() && phoneOs() === null;
}

/**
 * The newer version the feed offers, or null when there is none. Throws when this copy has no updater (a deb, an
 * unpackaged build) or the feed can't be read; callers asking on their own stay quiet then.
 */
export async function checkForUpdate(): Promise<AvailableUpdate | null> {
  return await invoke<AvailableUpdate | null>("plugin:updater|check", { timeout: 30_000 });
}

/** An update found outside the bar (Help → Check for updates…), and whether it should be installed now. */
export interface UpdateOffer {
  update: AvailableUpdate;
  install: boolean;
}

const offerListeners = new Set<(offer: UpdateOffer) => void>();

/** The update bar (../pages/DesktopUpdate.tsx) listens here while it is shown. */
export function onUpdateOffered(listener: (offer: UpdateOffer) => void): () => void {
  offerListeners.add(listener);
  return () => { offerListeners.delete(listener); };
}

/** Hands an update to the bar, which shows it (and installs it, with its progress). Answers false when no bar is shown. */
export function offerUpdate(offer: UpdateOffer): boolean {
  for (const listener of offerListeners) listener(offer);
  return offerListeners.size > 0;
}

/**
 * Download the update, check its signature and install it. Windows: the installer closes the app and opens the new
 * version. Linux (the AppImage is replaced in place): the app restarts itself. `progress` gets 0 to 1 when the size
 * is known.
 */
export async function installUpdate(update: AvailableUpdate, progress: (fraction: number | null) => void): Promise<void> {
  let total: number | null = null;
  let done = 0;
  const onEvent = new Channel<DownloadEvent>();
  onEvent.onmessage = (e) => {
    if (e.event === "Started") { total = e.data.contentLength ?? null; progress(total ? 0 : null); }
    else if (e.event === "Progress") { done += e.data.chunkLength; progress(total ? Math.min(1, done / total) : null); }
  };
  await invoke("plugin:updater|download_and_install", { rid: update.rid, onEvent, timeout: 30 * 60_000 });
  // Windows never gets here (the installer has closed the app). Linux: start the new AppImage.
  await invoke("restart_app");
}
