import { createElement } from "react";
import type { Api } from "../../../src/api/contract";
import { createTauriApi } from "../../../src/api/tauri";
import { EXTERNAL_LINKS } from "../../../src/lib/links";
import { isTauri } from "../../../src/lib/tauri";
import { setStatus } from "../../../src/state/status";
import { ask } from "../ask";
import { override } from "./registry";

/**
 * Web pages from the party apps (plan M5). The device's `open_link` opens only upstream's own pages
 * (src-tauri/src/links.rs `ALLOWED`) and refuses anything else, and the apps' web view opens no new window, so a
 * party page (the server's help, the app downloads, a join page) did nothing when tapped.
 * - Upstream's pages still go to the device's `open_link`.
 * - Any other web address goes to the device's `vault_open_url`, which upstream offers for web links in notes: it
 *   opens a checked http(s) address in the system browser, on the phones and the desktop alike.
 * - If the device can't open it, an app dialog shows the address with Copy link.
 * A browser opens a new tab, as before. In the apps, a tap on a web link (`<a href="https://…">` to another site) on a
 * party page takes the same way, since the web view would ignore it.
 */

type Device = Pick<Api, "openLink" | "vaultOpenUrl">;

export interface LinkDeps {
  inApp: () => boolean;
  device: () => Device;
  ask: typeof ask;
  copy: (text: string) => Promise<boolean>;
  openTab: (url: string) => void;
}

/** Copies through the clipboard API, or a selected text field where that is refused. Answers whether it worked. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const field = document.createElement("textarea");
    field.value = text;
    field.setAttribute("readonly", "");
    field.style.position = "fixed";
    field.style.opacity = "0";
    document.body.appendChild(field);
    field.select();
    let copied = false;
    try { copied = document.execCommand("copy"); } catch { /* not offered */ }
    field.remove();
    return copied;
  }
}

const DEFAULT_DEPS: LinkDeps = {
  inApp: isTauri,
  device: () => createTauriApi(),
  ask,
  copy: copyText,
  openTab: (url) => { window.open(url, "_blank", "noopener"); },
};

export const linkDeps: LinkDeps = { ...DEFAULT_DEPS };

export const COPY_TITLE = "Open in your browser";

/** Shows the address with Copy link, for a page the device could not open. */
async function offerCopy(url: string, deps: LinkDeps): Promise<void> {
  const message = createElement("div", null,
    createElement("p", null, "Borealis Party couldn't open this page. Copy the address and open it in your web browser:"),
    createElement("p", { className: "party-link-address", style: { wordBreak: "break-all", userSelect: "text" } }, url));
  const answer = await deps.ask(COPY_TITLE, message, [{ id: "copy", label: "Copy link" }, { id: "close", label: "Close" }]);
  if (answer !== "copy") return;
  const copied = await deps.copy(url);
  setStatus(copied ? "Link copied. Paste it in your web browser." : "The link could not be copied. Select it and copy it by hand.");
  if (!copied) await deps.ask(COPY_TITLE, message, [{ id: "close", label: "Close" }]);
}

/** Opens a web page from a party page or an upstream one: see the module comment. */
export async function openExternal(url: string, deps: LinkDeps = linkDeps): Promise<void> {
  if (!deps.inApp()) {
    deps.openTab(url);
    return;
  }
  const device = deps.device();
  try {
    if (EXTERNAL_LINKS.includes(url)) await device.openLink({ url });
    else await device.vaultOpenUrl({ url });
  } catch {
    await offerCopy(url, deps);
  }
}

/** A tap on a link to another web site, which the apps' web view would drop. Same-site links stay with the page. */
export function externalHref(target: EventTarget | null, here: string = window.location.href): string | null {
  const anchor = target instanceof Element ? target.closest("a[href]") : null;
  if (!(anchor instanceof HTMLAnchorElement)) return null;
  let address: URL;
  try { address = new URL(anchor.getAttribute("href") ?? "", here); } catch { return null; }
  if (address.protocol !== "http:" && address.protocol !== "https:") return null;
  if (address.origin === new URL(here).origin) return null;
  return address.href;
}

let watching = false;
function watchClicks(deps: LinkDeps): void {
  if (watching) return;
  watching = true;
  document.addEventListener("click", (e) => {
    if (e.defaultPrevented || e.button !== 0 || !deps.inApp()) return;
    const url = externalHref(e.target);
    if (!url) return;
    e.preventDefault();
    void openExternal(url, deps);
  });
}

/** Tests: start over. */
export function resetLinks(): void {
  Object.assign(linkDeps, DEFAULT_DEPS);
}

override("links", (methods) => {
  methods.openLink = (args?: Record<string, unknown>) => openExternal(String(args?.url ?? ""));
  watchClicks(linkDeps);
});
