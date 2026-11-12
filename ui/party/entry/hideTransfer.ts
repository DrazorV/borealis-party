import { phoneOs } from "../../src/lib/phone";
import { isTauri } from "../../src/lib/tauri";
import { isOffline } from "../api/connectivity";
import { tweakExportDialog } from "../export/exportDialog";

/**
 * Entry points to what a server doesn't offer: device-to-device transfer (every device already reads the same account),
 * Notes and "Save as" (a folder on the device), the on-device search model, and closing the app. Bundle files are
 * offered: "Open bundle..." uploads one and "Export to bundle..." / "Share as a file" downloads or shares one
 * (api/bundles.ts). Upstream's rows carry no attribute CSS could match, so they are matched by their text as they
 * appear. If upstream rewords one, the row simply comes back and fails with a message.
 */
const LABELS: ReadonlySet<string> = new Set([
  "Send to device...",
  "Receive from device...",
  "Send to device",
  "Receive from device",
  "Notes",
  "Search settings",
  "Exit Application",
  "Save as…",
]);

/**
 * The Android app only: bundle files. The app is built without device-link, so its backend has no bundle command, and its
 * web view can't download or share the bytes (api/overrides/android.ts refuses them). Android will send bundles through
 * the server instead (share codes, 0.1.8). A phone browser on Android has the phone layer too (plan M4) and downloads
 * bundles like any browser, so it keeps them.
 */
export const ANDROID_LABELS: ReadonlySet<string> = new Set([
  "Export to bundle...",
  "Export to bundle…",
  "Export to bundle",
  "Share as a file",
]);

const ROWS = ".aur-menu-item, .menu-tree-item, .v5-row, .v4-foot .btn";

/** Offline (api/offline.ts refuses it), where upstream would show no message: Duplicate in the character's Edit dialog. */
const OFFLINE_LABELS: ReadonlySet<string> = new Set(["Duplicate"]);
const OFFLINE_ROWS = ".dialog-foot .btn, .flyout-foot .btn";

function hidden(label: string): boolean {
  return LABELS.has(label) || (phoneOs() === "android" && isTauri() && ANDROID_LABELS.has(label));
}

function hideRows(): void {
  for (const row of document.querySelectorAll<HTMLElement>(ROWS)) {
    if (hidden(row.textContent?.trim() ?? "")) (row.closest("li") ?? row).classList.add("party-hidden");
  }
  if (isOffline()) {
    for (const row of document.querySelectorAll<HTMLElement>(OFFLINE_ROWS)) {
      if (OFFLINE_LABELS.has(row.textContent?.trim() ?? "")) row.classList.add("party-hidden");
    }
  }
  // A phone page list left with no row (the More page's "Your other devices" on Android) would still take up space.
  for (const list of document.querySelectorAll<HTMLElement>(".v5-list")) {
    const items = Array.from(list.children);
    if (items.length > 0 && items.every((li) => li.classList.contains("party-hidden"))) list.classList.add("party-hidden");
  }
}

function pass(): void {
  hideRows();
  tweakExportDialog(pass);
}

/** Keeps the rows hidden (and the export dialog's party wording) for the life of the page; menus and pages mount them on demand. */
export function hideUnofferedRows(): void {
  pass();
  new MutationObserver(pass).observe(document.body, { childList: true, subtree: true, characterData: true });
  // The export dialog's pack choice changes no markup (a <select>), and decides whether the licence line shows.
  document.addEventListener("change", pass, true);
}
