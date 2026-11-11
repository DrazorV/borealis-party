/**
 * Entry points to what a server doesn't offer: device-to-device transfer and making bundle files (every device already
 * reads the same account), Notes and "Save as" (a folder on the device), the on-device search model, and closing the app.
 * Opening a bundle file is offered: "Open bundle..." stays (api/bundles.ts). Upstream's rows carry no attribute CSS could
 * match, so they are matched by their text as they appear. If upstream rewords one, the row simply comes back and fails
 * with a message.
 */
const LABELS: ReadonlySet<string> = new Set([
  "Send to device...",
  "Receive from device...",
  "Send to device",
  "Receive from device",
  "Export to bundle...",
  "Share as a file",
  "Notes",
  "Search settings",
  "Exit Application",
  "Save as…",
]);

const ROWS = ".aur-menu-item, .menu-tree-item, .v5-row, .v4-foot .btn";

function hideRows(): void {
  for (const row of document.querySelectorAll<HTMLElement>(ROWS)) {
    if (LABELS.has(row.textContent?.trim() ?? "")) (row.closest("li") ?? row).classList.add("party-hidden");
  }
  // A phone page list left with no row (the More page's "Your other devices") would still take up space.
  for (const list of document.querySelectorAll<HTMLElement>(".v5-list")) {
    const items = Array.from(list.children);
    if (items.length > 0 && items.every((li) => li.classList.contains("party-hidden"))) list.classList.add("party-hidden");
  }
}

/** Keeps the rows hidden for the life of the page; menus and pages mount them on demand. */
export function hideUnofferedRows(): void {
  hideRows();
  new MutationObserver(hideRows).observe(document.body, { childList: true, subtree: true });
}
