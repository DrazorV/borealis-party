import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { pushOverlay } from "../../src/components/controls/overlayStack";
import { HOMEBREW_BUTTON, ZIP_BUTTON } from "../../src/pages/content/FiveToolsTab";
import { doorCount, resetDoor, sendFiles, setImportNavigate, takeFocus, takeInbox } from "../import/door";
import { DROP_LIMIT_TITLE, takeNativeDrop, unifyImports } from "./unifyImports";

const asked = vi.hoisted(() => [] as string[]);
vi.mock("../api/ask", () => ({ ask: async (title: string, message: string) => { asked.push(`${title}: ${message}`); return "ok"; } }));

let went: string[];
let upstream: string[];
let chooserClicks: number;
let stop: () => void;

/** Upstream's markup for an entry point, with upstream's own handler, which must not run. */
function mount(html: string): HTMLElement {
  const holder = document.createElement("div");
  holder.innerHTML = html;
  document.body.appendChild(holder);
  holder.querySelectorAll<HTMLElement>("button, a, form").forEach((el) => {
    el.addEventListener("click", () => upstream.push(`click ${el.textContent?.trim() || el.getAttribute("aria-label")}`));
    el.addEventListener("submit", (e) => { e.preventDefault(); upstream.push("submit"); });
  });
  return holder;
}

beforeEach(() => {
  went = [];
  upstream = [];
  chooserClicks = 0;
  setImportNavigate((to) => went.push(to));
  vi.spyOn(HTMLInputElement.prototype, "click").mockImplementation(function (this: HTMLInputElement) {
    if (this.hasAttribute("data-party-import-chooser")) chooserClicks += 1;
  });
  stop = unifyImports();
});

afterEach(() => {
  stop();
  resetDoor();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
  asked.length = 0;
});

const click = (el: Element | null) => (el as HTMLElement).click();

describe("file picks open the Import screen with its chooser", () => {
  const cases: [string, string, string][] = [
    ["Start > Import characters", `<button class="v4-newbtn"><span class="t">Import characters</span><span class="d">Load one or more .dnd5e files</span></button>`, "button"],
    ["File > Import Characters (desktop bar)", `<div class="aur-menu-panel"><button class="aur-menu-item"><span class="aur-menu-icon"></span><span class="aur-menu-label">Import Characters</span></button></div>`, "button"],
    ["File > Load Character (phone Menu)", `<nav class="menu-tree"><button class="menu-tree-item"><span class="aur-menu-icon"></span>Load Character</button></nav>`, "button"],
    ["File > Open bundle...", `<div class="aur-menu-panel"><button class="aur-menu-item"><span class="aur-menu-label">Open bundle...</span></button></div>`, "button"],
    ["the floating +", `<button class="fab-import" aria-label="Import content" title="Import content"><svg></svg></button>`, "button"],
    ["Additional Content > Import files", `<div class="v4-content"><button class="btn"><svg></svg>Import files</button></div>`, "button"],
    ["5etools > .zip", `<div class="v4-content"><button class="btn">${ZIP_BUTTON}</button></div>`, "button"],
    ["5etools > homebrew .json", `<div class="v4-content"><button class="btn">${HOMEBREW_BUTTON}</button></div>`, "button"],
  ];
  it.each(cases)("%s", (_name, html, selector) => {
    const holder = mount(html);
    click(holder.querySelector(selector));
    expect(upstream).toEqual([]);
    expect(went).toEqual(["/import-files"]);
    expect(chooserClicks).toBe(1);
  });

  it("files chosen there arrive on the Import screen", () => {
    mount(`<button class="v4-newbtn"><span class="t">Import characters</span></button>`);
    click(document.querySelector(".v4-newbtn"));
    const input = document.querySelector<HTMLInputElement>("[data-party-import-chooser]")!;
    const file = new File(["{}"], "Hero.dnd5e");
    Object.defineProperty(input, "files", { value: [file], configurable: true });
    input.dispatchEvent(new Event("change"));
    expect(takeInbox().files).toEqual([file]);
  });

  it("closes the phone Menu sheet the item was in", () => {
    const close = vi.fn();
    const off = pushOverlay(close);
    mount(`<nav class="menu-tree"><button class="menu-tree-item">Import Characters</button></nav>`);
    click(document.querySelector(".menu-tree-item"));
    expect(close).toHaveBeenCalledOnce();
    off();
  });

  it("closes the desktop menu the item was in (Escape, as upstream's menu bar reads it)", () => {
    const holder = mount(`<div class="aur-menubar"><div class="aur-menu-panel"><button class="aur-menu-item">Open bundle...</button></div></div>`);
    const keys: string[] = [];
    holder.querySelector(".aur-menubar")!.addEventListener("keydown", (e) => keys.push((e as KeyboardEvent).key));
    click(holder.querySelector(".aur-menu-item"));
    expect(keys).toEqual(["Escape"]);
  });

  it("leaves every other button alone", () => {
    const holder = mount(`<button class="v4-newbtn"><span class="t">New character</span></button>
      <div class="v4-content"><button class="btn">Update</button><button class="btn">Remove</button><button class="btn">Import my content folder</button></div>
      <div class="aur-menu-panel"><button class="aur-menu-item">Export to bundle...</button></div>
      <button class="btn">Import files</button>`);
    holder.querySelectorAll("button").forEach((b) => b.click());
    expect(went).toEqual([]);
    expect(upstream).toHaveLength(6);
  });

  it("a disabled button does nothing", () => {
    const holder = mount(`<div class="v4-content"><button class="btn" disabled>Import files</button></div>`);
    holder.querySelector("button")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(went).toEqual([]);
  });
});

describe("link forms take the address to the Import screen", () => {
  const form = (id: string, value: string) =>
    mount(`<form class="v4-urlrow"><input id="${id}" type="url" value="${value}"><button type="submit" class="btn">Fetch</button></form>`);
  const submit = (holder: HTMLElement) => holder.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));

  it("Index files > Fetch", () => {
    submit(form("index-url", "https://example.com/srd.index"));
    expect(upstream).toEqual([]);
    expect(went).toEqual(["/import-files"]);
    expect(takeInbox().links).toEqual([{ url: "https://example.com/srd.index", kind: "index" }]);
    expect(chooserClicks).toBe(0);
  });

  it("5etools > Import from an address", () => {
    submit(form("fivetools-url", " https://github.com/example/data "));
    expect(takeInbox().links).toEqual([{ url: "https://github.com/example/data", kind: "fivetools" }]);
  });

  it("without an address, shows where to paste one", () => {
    submit(form("index-url", ""));
    expect(takeFocus()).toBe("link");
    submit(form("fivetools-url", ""));
    expect(takeFocus()).toBe("fivetools");
    expect(takeInbox().links).toEqual([]);
  });

  it("other forms submit as before", () => {
    submit(mount(`<form><input id="search"></form>`));
    expect(upstream).toEqual(["submit"]);
    expect(went).toEqual([]);
  });
});

describe("drops", () => {
  const drop = (files: File[], types = ["Files"]) => {
    const e = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(e, "dataTransfer", { value: { types, files } });
    document.body.dispatchEvent(e);
    return e;
  };

  it("files dropped anywhere go to the Import screen, not to upstream's drop handler", () => {
    const globalImport = vi.fn();
    window.addEventListener("drop", globalImport);
    const file = new File(["x"], "Party.borealis");
    expect(drop([file]).defaultPrevented).toBe(true);
    expect(globalImport).not.toHaveBeenCalled();
    expect(takeInbox().files).toEqual([file]);
    expect(went).toEqual(["/import-files"]);
    window.removeEventListener("drop", globalImport);
  });

  it("a drop of text (no files) is left alone", () => {
    expect(drop([], ["text/plain"]).defaultPrevented).toBe(false);
    expect(went).toEqual([]);
  });
});

describe("the desktop app's native drop", () => {
  const read = async (tokens: string[]) => tokens.map((t) => ({ fileName: `${t}.dnd5e`, contentBase64: "e30=" }));

  it("reads the dropped files from the device and opens the Import screen", async () => {
    await takeNativeDrop({ files: [{ token: "hero", name: "hero.dnd5e" }], folders: [], bundles: [], rejected: [] }, read);
    expect(takeInbox().files.map((f) => f.name)).toEqual(["hero.dnd5e"]);
    expect(went).toEqual(["/import-files"]);
    expect(asked).toEqual([]);
  });

  it("says a bundle or folder must be chosen instead, and names files nothing reads", async () => {
    await takeNativeDrop({ files: [], folders: [{ token: "f", name: "content" }], bundles: [{ token: "b", name: "Party.borealis" }], rejected: ["notes.pdf"] }, read);
    expect(asked).toHaveLength(1);
    expect(asked[0]).toContain(DROP_LIMIT_TITLE);
    expect(asked[0]).toContain("Party.borealis, content: a bundle or folder can't be dropped here");
    expect(asked[0]).toContain("notes.pdf: nothing here reads this file");
  });
});

describe("the door", () => {
  it("counts the doors opened and keeps files sent while the screen is busy", () => {
    sendFiles([new File(["a"], "a.dnd5e")]);
    sendFiles([new File(["b"], "b.dnd5e")]);
    expect(takeInbox().files.map((f) => f.name)).toEqual(["a.dnd5e", "b.dnd5e"]);
    expect(takeInbox().files).toEqual([]);
    expect(doorCount()).toBe(0);
  });
});
