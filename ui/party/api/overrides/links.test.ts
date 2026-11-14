import { afterEach, describe, expect, it, vi } from "vitest";
import { LINKS } from "../../../src/lib/links";
import { getStatus, setStatus } from "../../../src/state/status";
import { createHttpApi } from "../http";
import type { ServerSession } from "../session";
import { COPY_TITLE, externalHref, linkDeps, openExternal, resetLinks, type LinkDeps } from "./links";

const session: ServerSession = {
  baseUrl: "https://party.example.org", token: "t",
  user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
};

function deps(over: Partial<LinkDeps> = {}) {
  const calls: string[] = [];
  const d: LinkDeps = {
    inApp: () => true,
    device: () => ({
      openLink: async ({ url }) => { calls.push(`openLink ${url}`); },
      vaultOpenUrl: async ({ url }) => { calls.push(`vaultOpenUrl ${url}`); },
    }),
    ask: async (title) => { calls.push(`ask ${title}`); return "copy"; },
    copy: async (text) => { calls.push(`copy ${text}`); return true; },
    openTab: (url) => { calls.push(`tab ${url}`); },
    ...over,
  };
  return { d, calls };
}

afterEach(() => {
  resetLinks();
  setStatus("");
});

describe("web pages from the apps", () => {
  it("upstream's own pages go to the device's open_link", async () => {
    const { d, calls } = deps();
    await openExternal(LINKS.guideImportAurora, d);
    expect(calls).toEqual([`openLink ${LINKS.guideImportAurora}`]);
  });

  it("party pages go to the device's vault_open_url (the system browser)", async () => {
    const { d, calls } = deps();
    await openExternal("https://github.com/fortress-soft/meadhall/releases/latest", d);
    expect(calls).toEqual(["vaultOpenUrl https://github.com/fortress-soft/meadhall/releases/latest"]);
  });

  it("when the device can't open it, an app dialog offers to copy the address", async () => {
    const refuse = async () => { throw { code: "io_error", message: "no browser" }; };
    const { d, calls } = deps({ device: () => ({ openLink: refuse, vaultOpenUrl: refuse }) });
    await openExternal("https://party.example.org/help", d);
    expect(calls).toEqual([`ask ${COPY_TITLE}`, "copy https://party.example.org/help"]);
    expect(getStatus()).toBe("Link copied. Paste it in your web browser.");
  });

  it("a copy that fails keeps the address on screen", async () => {
    const refuse = async () => { throw new Error("no"); };
    const { d, calls } = deps({ device: () => ({ openLink: refuse, vaultOpenUrl: refuse }), copy: async () => false });
    await openExternal("https://party.example.org/help", d);
    expect(calls).toEqual([`ask ${COPY_TITLE}`, `ask ${COPY_TITLE}`]);
  });

  it("a browser opens a new tab, as before", async () => {
    const { d, calls } = deps({ inApp: () => false });
    await openExternal("https://party.example.org/help", d);
    expect(calls).toEqual(["tab https://party.example.org/help"]);
  });

  it("the page's openLink takes this way", async () => {
    const { d, calls } = deps();
    Object.assign(linkDeps, d);
    await createHttpApi(session).openLink({ url: "https://party.example.org/download" });
    expect(calls).toEqual(["vaultOpenUrl https://party.example.org/download"]);
  });
});

describe("links tapped on a page", () => {
  const here = "http://tauri.localhost/characters";
  const anchor = (href: string) => {
    const a = document.createElement("a");
    a.setAttribute("href", href);
    const inner = document.createElement("span");
    a.appendChild(inner);
    return inner;
  };

  it("a link to another web site is caught, from a child of the link too", () => {
    expect(externalHref(anchor("https://github.com/fortress-soft/meadhall/releases/latest"), here)).toBe("https://github.com/fortress-soft/meadhall/releases/latest");
  });

  it("same-site links, mail and script links are left to the page", () => {
    expect(externalHref(anchor("/content"), here)).toBeNull();
    expect(externalHref(anchor("http://tauri.localhost/sheet"), here)).toBeNull();
    expect(externalHref(anchor("javascript:void 0"), here)).toBeNull();
    expect(externalHref(anchor("mailto:dm@example.org"), here)).toBeNull();
    expect(externalHref(document.createElement("div"), here)).toBeNull();
  });

  it("is not watched in a browser", () => {
    const open = vi.fn();
    Object.assign(linkDeps, { inApp: () => false, openTab: open });
    createHttpApi(session);
    const a = document.createElement("a");
    a.href = "https://example.org/";
    document.body.appendChild(a);
    let caught: boolean | null = null;
    // Runs after the page's own listener; it stops jsdom's navigation once it has looked.
    const after = (e: Event) => { caught = e.defaultPrevented; e.preventDefault(); };
    window.addEventListener("click", after);
    a.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    window.removeEventListener("click", after);
    expect(caught).toBe(false);
    expect(open).not.toHaveBeenCalled();
    a.remove();
  });
});
