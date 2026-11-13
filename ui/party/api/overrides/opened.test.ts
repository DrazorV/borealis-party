import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedFile } from "../../../src/api/contract";
import { createHttpApi } from "../http";
import type { ServerSession } from "../session";
import { resetDoor, setImportNavigate, takeInbox } from "../../import/door";
import { ASK_TITLE, describeFiles, drainOpened, noticeOffline, OFFLINE_TITLE, openedDeps, resetOpened, takeOpened, watchOpened, type OpenedDeps } from "./opened";

const session: ServerSession = {
  baseUrl: "http://192.168.1.20:8787", token: "t",
  user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
};

const file = (fileName: string): OpenedFile => ({ fileName, contentBase64: "QQ==" });

function deps(files: OpenedFile[], answer = "import"): OpenedDeps & { asked: string[]; handed: string[][] } {
  const asked: string[] = [];
  const handed: string[][] = [];
  return {
    asked, handed,
    inApp: () => true,
    take: async () => files.splice(0),
    ask: async (title, message) => { asked.push(`${title}: ${String(message)}`); return answer; },
    toImport: (list) => { handed.push(list.map((f) => f.fileName)); },
  };
}

afterEach(() => {
  resetOpened();
  resetDoor();
  document.documentElement.classList.remove("party-offline-mode");
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  document.body.innerHTML = "";
});

describe("files opened with the app", () => {
  it("asks first, naming the files and the account, then hands them to the import", async () => {
    const d = deps([file("Hero.dnd5e"), file("srd-extra.xml")]);
    expect(await takeOpened(session, d)).toEqual([file("Hero.dnd5e"), file("srd-extra.xml")]);
    expect(d.asked).toEqual([`${ASK_TITLE}: Import Hero.dnd5e and srd-extra.xml into alice?`]);
  });

  it("Cancel drops them: nothing is imported", async () => {
    const d = deps([file("homebrew.json")], "cancel");
    expect(await takeOpened(session, d)).toEqual([]);
    expect(d.asked).toHaveLength(1);
  });

  it("asks nothing when nothing was opened, and nothing in a browser", async () => {
    const none = deps([]);
    expect(await takeOpened(session, none)).toEqual([]);
    expect(none.asked).toEqual([]);
    const browser = { ...deps([file("Hero.dnd5e")]), inApp: () => false };
    expect(await takeOpened(session, browser)).toEqual([]);
    expect(browser.asked).toEqual([]);
  });

  it("a .borealis goes to the Import screen with the other files (it opens the Open bundle window there)", async () => {
    const d = deps([file("Party.borealis"), file("Hero.dnd5e")]);
    await drainOpened(session, d);
    expect(d.handed).toEqual([["Party.borealis", "Hero.dnd5e"]]);
  });

  it("Cancel hands nothing to the Import screen", async () => {
    const d = deps([file("Hero.dnd5e")], "cancel");
    await drainOpened(session, d);
    expect(d.handed).toEqual([]);
  });

  it("files no importer reads pass through unasked, so the Import screen says it can't import them", async () => {
    const d = deps([file("notes.pdf")]);
    expect(await takeOpened(session, d)).toEqual([file("notes.pdf")]);
    expect(d.asked).toEqual([]);
  });

  it("names many files by count", () => {
    expect(describeFiles(["a.xml"])).toBe("a.xml");
    expect(describeFiles(["a.xml", "b.xml", "c.xml"])).toBe("3 files (a.xml, b.xml and c.xml)");
  });
});

describe("through the page's API", () => {
  let invoked: string[];
  let queued: OpenedFile[];

  beforeEach(() => {
    invoked = [];
    queued = [file("Hero.dnd5e")];
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 200 })));
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: vi.fn(async (cmd: string) => {
        invoked.push(cmd);
        if (cmd === "take_opened_files") return queued.splice(0);
        throw new Error(`no such command ${cmd}`);
      }),
      transformCallback: () => 0,
    };
  });

  afterEach(() => vi.unstubAllGlobals());

  it("upstream's GlobalImport gets none: the party takes the queue itself", async () => {
    expect(await createHttpApi(session).takeOpenedFiles()).toEqual([]);
    expect(invoked).toEqual([]);
  });

  it("takes the device's queue (take_opened_files), asks in an app dialog, then opens the Import screen with it", async () => {
    const went: string[] = [];
    setImportNavigate((to) => went.push(to));
    const stop = watchOpened(session);
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Import Hero.dnd5e into alice?");
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(went).toEqual(["/import-files"]));
    expect(takeInbox().files.map((f) => f.name)).toEqual(["Hero.dnd5e"]);
    expect(invoked.filter((c) => !c.startsWith("plugin:"))).toEqual(["take_opened_files"]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // Nudged again (Android's activity): asked again for the new file.
    queued.push(file("Second.dnd5e"));
    window.dispatchEvent(new Event("borealis-open-file"));
    expect((await screen.findByRole("dialog")).textContent).toContain("Second.dnd5e");
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(takeInbox().files.map((f) => f.name)).toEqual(["Second.dnd5e"]));
    stop();
  });

  it("never asks the device for bundles: take_opened_bundles is not in the apps", async () => {
    expect(await createHttpApi(session).takeOpenedBundles()).toEqual([]);
    expect(invoked).toEqual([]);
  });

  it("asks one question at a time", async () => {
    queued = [file("a.dnd5e")];
    const asked: string[] = [];
    let release: (answer: string) => void = () => undefined;
    openedDeps.ask = (_title, message) => {
      asked.push(String(message));
      return new Promise((done) => { release = done; });
    };
    const handed: string[][] = [];
    openedDeps.toImport = (list) => { handed.push(list.map((f) => f.fileName)); };
    const first = drainOpened(session);
    const second = drainOpened(session);
    await waitFor(() => expect(asked).toHaveLength(1));
    queued.push(file("b.dnd5e"));
    release("import");
    await first;
    expect(handed).toEqual([["a.dnd5e"]]);
    await waitFor(() => expect(asked).toHaveLength(2));
    release("cancel");
    await second;
    expect(handed).toEqual([["a.dnd5e"]]);
  });
});

describe("offline", () => {
  it("tells once that the file waits for the connection, and only on an offline page", async () => {
    const asked: string[] = [];
    const d = { ask: async (title: string) => { asked.push(title); return "ok"; } };
    await noticeOffline(d);
    expect(asked).toEqual([]);
    document.documentElement.classList.add("party-offline-mode");
    await noticeOffline(d);
    await noticeOffline(d);
    expect(asked).toEqual([OFFLINE_TITLE]);
  });
});
