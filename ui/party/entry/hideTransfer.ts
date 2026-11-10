/**
 * Borealis's device-to-device transfer has no place on a server, where every device already reads the same characters,
 * and the server doesn't offer its commands. Upstream's menu rows carry no attribute CSS could match, so they are
 * matched by their text as they appear. If upstream rewords one, the row simply comes back and fails with a message.
 */
const LABELS: ReadonlySet<string> = new Set([
  "Send to device...",
  "Receive from device...",
  "Send to device",
  "Receive from device",
]);

const ROWS = ".aur-menu-item, .menu-tree-item, .v5-row";

function hideRows(): void {
  for (const row of document.querySelectorAll<HTMLElement>(ROWS)) {
    if (LABELS.has(row.textContent?.trim() ?? "")) (row.closest("li") ?? row).classList.add("party-hidden");
  }
}

/** Keeps the rows hidden for the life of the page; menus and pages mount them on demand. */
export function hideDeviceTransfer(): void {
  hideRows();
  new MutationObserver(hideRows).observe(document.body, { childList: true, subtree: true });
}
