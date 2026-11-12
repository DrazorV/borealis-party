import { createElement } from "react";
import { createRoot } from "react-dom/client";
import type { ApiError } from "../../../src/api/contract";
import { MessageBox } from "../../../src/components/controls/Dialog";
import { serverHooks, type ServerFailure, type ServerRequest } from "../http";
import { override } from "./registry";

/**
 * Uploads that are too large, and failures that would otherwise read "The server answered 413." or show a proxy's page.
 *
 * Files go up as base64 inside one JSON request, a third larger than the file. The public address passes through
 * Cloudflare, whose free plan stops requests over 100 MB; on the home network only the server's own limit applies.
 * The server reports both limits (`/api/server` `uploads`, `server.max_upload_mb` and `server.public_upload_mb`).
 * - A file chosen or dropped is checked here before upstream reads it, so a phone never encodes 300 MB for nothing.
 * - Every request is checked again before it is sent (files the OS opened with the app, for example).
 * - Failed replies are put in words (serverHooks.fail in ../http.ts). The pages show them in their own dialogs.
 */

const MiB = 1024 * 1024;
/** Room in a request for everything but the files' bytes: names and JSON. */
const OVERHEAD = 64 * 1024;

export interface UploadLimits {
  maxUploadMb: number;
  publicUploadMb: number;
}

/** The server's defaults, until it has answered (or when it is older than these limits). */
export const DEFAULT_LIMITS: UploadLimits = { maxUploadMb: 512, publicUploadMb: 95 };

const known = new Map<string, UploadLimits>();
const asking = new Map<string, Promise<void>>();

/** Asks the server for its limits once per address. Never fails: the defaults stand in. */
export function learnLimits(baseUrl: string): Promise<void> {
  const already = asking.get(baseUrl);
  if (already) return already;
  const ask = fetch(`${baseUrl}/api/server`)
    .then((r) => (r.ok ? r.json() : null))
    .then((info: { uploads?: Partial<UploadLimits> } | null) => {
      const u = info?.uploads;
      if (u && Number(u.maxUploadMb) > 0 && Number(u.publicUploadMb) > 0) {
        known.set(baseUrl, { maxUploadMb: Number(u.maxUploadMb), publicUploadMb: Number(u.publicUploadMb) });
      }
    })
    .catch(() => { asking.delete(baseUrl); });
  asking.set(baseUrl, ask);
  return ask;
}

export function limitsFor(baseUrl: string): UploadLimits {
  return known.get(baseUrl) ?? DEFAULT_LIMITS;
}

/** Tests start from nothing known. */
export function forgetLimitsForTests(): void {
  known.clear();
  asking.clear();
}

/** An address on the home network (or a VPN into it), where no public proxy stands in front of the server. */
export function isHomeNetwork(baseUrl: string): boolean {
  let host: string;
  try { host = new URL(baseUrl).hostname.toLowerCase().replace(/^\[|\]$/g, ""); } catch { return false; }
  if (host === "localhost" || (!host.includes(".") && !host.includes(":"))) return true;
  if (/\.(localhost|local|lan|home|internal|home\.arpa)$/.test(host)) return true;
  if (host === "::1" || /^f[cd][0-9a-f]{0,2}:/.test(host) || host.startsWith("fe80:")) return true;
  const ip = host.split(".").map(Number);
  if (ip.length !== 4 || ip.some((n) => !Number.isInteger(n) || n < 0 || n > 255)) return false;
  const [a, b] = ip as [number, number, number, number];
  return a === 10 || a === 127 || (a === 192 && b === 168) || (a === 172 && b >= 16 && b <= 31)
    || (a === 169 && b === 254) || (a === 100 && b >= 64 && b <= 127);
}

/** The largest request this address takes, in bytes. */
export function requestLimit(baseUrl: string): number {
  const l = limitsFor(baseUrl);
  return (isHomeNetwork(baseUrl) ? l.maxUploadMb : Math.min(l.publicUploadMb, l.maxUploadMb)) * MiB;
}

/** The most file bytes that fit in a request of `limit` bytes, in whole MB. */
const filesMb = (limit: number) => Math.max(0, Math.floor(((limit - OVERHEAD) * 3) / 4 / MiB));
/** How many bytes `fileBytes` of files take once encoded (base64 and JSON). */
export const encodedSize = (fileBytes: number) => Math.ceil(fileBytes / 3) * 4 + OVERHEAD;
const mb = (bytes: number) => Math.max(1, Math.round(bytes / MiB));
const hostOf = (baseUrl: string) => { try { return new URL(baseUrl).host; } catch { return baseUrl; } };

function apiError(code: string, message: string): ApiError {
  return { code, message } as ApiError;
}

/** What to tell someone whose files (`fileBytes`, or a request of `requestBytes`) are over this address's limit. */
export function tooLargeMessage(baseUrl: string, size: { fileBytes: number } | { requestBytes: number }): string {
  const most = filesMb(requestLimit(baseUrl));
  const chosen = "fileBytes" in size ? mb(size.fileBytes) : mb((Math.max(0, size.requestBytes - OVERHEAD) * 3) / 4);
  if (isHomeNetwork(baseUrl)) {
    return `Too large for this server: ${chosen} MB, and it takes at most ${most} MB of files at a time. `
      + "Use a smaller file, or ask the server's admin to raise server.max_upload_mb.";
  }
  const home = filesMb(limitsFor(baseUrl).maxUploadMb * MiB);
  return `Too large to send through ${hostOf(baseUrl)}: ${chosen} MB, and it takes at most ${most} MB of files at a time. `
    + `Use a smaller file, or connect to the server's home-network address, which takes up to ${home} MB.`;
}

/** Refuses a request over this address's limit before it is sent. */
export function checkRequest(request: ServerRequest): void {
  if (request.bytes > requestLimit(request.baseUrl)) {
    throw apiError("too_large", tooLargeMessage(request.baseUrl, { requestBytes: request.bytes }));
  }
}

/** The server's own JSON error, as opposed to a proxy's page or an empty reply. */
function fromServer(text: string): boolean {
  try {
    const parsed = JSON.parse(text) as { message?: unknown } | null;
    return typeof parsed?.message === "string";
  } catch { return false; }
}

/** A readable error for a failed request, or nothing to keep the one serverFetch made. */
export function describeFailure(f: ServerFailure): ApiError | undefined {
  const sending = f.bytes > MiB;
  if (f.status === 0) {
    if (!sending) return undefined;
    const home = isHomeNetwork(f.baseUrl) ? "" : " A large file goes through more reliably on the server's home-network address.";
    return apiError("network", `The connection dropped while sending ${mb(f.bytes)} MB. Check your connection and try again.${home}`);
  }
  if (f.status === 413) {
    if (f.bytes > requestLimit(f.baseUrl)) return apiError("too_large", tooLargeMessage(f.baseUrl, { requestBytes: f.bytes }));
    // Under the limits the server reports: something in front of it has a lower one.
    return apiError("too_large", `Too large to send through ${hostOf(f.baseUrl)}: it refused ${mb(f.bytes)} MB, though the server `
      + `expects up to ${filesMb(requestLimit(f.baseUrl))} MB. Use a smaller file, and tell the server's admin to check the `
      + "upload limit of the proxy in front of it (and server.public_upload_mb).");
  }
  if (fromServer(f.text)) return undefined;
  if (f.status === 504 || f.status === 524) {
    return apiError("timeout", "The server took too long to answer, but it may still be finishing. Wait a minute and "
      + "reload the page to see whether it worked before you try again, so it isn't done twice.");
  }
  if (f.status === 502 || f.status === 503 || (f.status >= 520 && f.status <= 523) || f.status === 530) {
    return apiError("unavailable", `The server can't be reached right now (${f.status}); it may be restarting. Try again in a minute.`);
  }
  if (f.status >= 500) {
    return apiError("internal", `The server ran into a problem (${f.status}). Try again; if it keeps happening, tell the server's admin.`);
  }
  if (f.text.trim().startsWith("<")) {
    return apiError("internal", `${hostOf(f.baseUrl)} answered with a web page instead of the server (${f.status}). Check the server's address.`);
  }
  return undefined;
}

/** The app's own message box, outside React's tree (the check runs before any page sees the files). */
export function showMessage(title: string, message: string): void {
  const host = document.createElement("div");
  (document.querySelector(".aur-window") ?? document.body).appendChild(host);
  const root = createRoot(host);
  const close = () => { root.unmount(); host.remove(); };
  root.render(createElement(MessageBox, { title, message, buttons: [{ id: "ok", label: "OK" }], onResult: close }));
}

const total = (files: Iterable<File>) => [...files].reduce((sum, f) => sum + f.size, 0);
const isFileInput = (t: EventTarget | null): t is HTMLInputElement => t instanceof HTMLInputElement && t.type === "file";
const draggingFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");

/** Whether these files fit in one request, telling the user when they don't. */
function fits(baseUrl: string, fileBytes: number): boolean {
  if (encodedSize(fileBytes) <= requestLimit(baseUrl)) return true;
  showMessage("File too large", tooLargeMessage(baseUrl, { fileBytes }));
  return false;
}

/**
 * Checks files as they are chosen or dropped, before upstream's handlers read them (window, capturing phase). Files
 * over the limit are taken back: a chooser is emptied, so its page sees nothing chosen, and a drop goes no further.
 * The limits are asked for as soon as a chooser opens or files are dragged in, so they are known by then.
 */
export function guardFiles(baseUrl: () => string): () => void {
  const learn = () => { void learnLimits(baseUrl()); };
  const click = (e: Event) => { if (isFileInput(e.target)) learn(); };
  const enter = (e: DragEvent) => { if (draggingFiles(e)) learn(); };
  const change = (e: Event) => {
    const input = e.target;
    if (!isFileInput(input) || !input.files?.length) return;
    if (!fits(baseUrl(), total(input.files))) input.value = "";
  };
  const drop = (e: DragEvent) => {
    const files = e.dataTransfer?.files;
    if (!files?.length || fits(baseUrl(), total(files))) return;
    e.preventDefault();
    e.stopPropagation();
  };
  window.addEventListener("click", click, true);
  window.addEventListener("dragenter", enter, true);
  window.addEventListener("change", change, true);
  window.addEventListener("drop", drop, true);
  return () => {
    window.removeEventListener("click", click, true);
    window.removeEventListener("dragenter", enter, true);
    window.removeEventListener("change", change, true);
    window.removeEventListener("drop", drop, true);
  };
}

let current = "";
let installed = false;

override("errors", (_methods, session) => {
  current = session.baseUrl;
  if (installed) return;
  installed = true;
  serverHooks.send.push(checkRequest);
  serverHooks.fail.push(describeFailure);
  guardFiles(() => current);
});
