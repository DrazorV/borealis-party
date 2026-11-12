import { afterEach, describe, expect, it } from "vitest";
import { MENUS, visibleMenus } from "../../../src/components/shell/sections";
import { getStatus, setStatus } from "../../../src/state/status";
import type { AvailableUpdate } from "../desktopUpdate";
import { createHttpApi, LOCAL } from "../http";
import type { ServerSession } from "../session";
import { CHECK_ENTRY, CHECK_LINK, CHECK_TITLE, checkDeps, checkNow, placeHelpEntry, resetCheck, type CheckDeps } from "./updates";

const session: ServerSession = {
  baseUrl: "https://party.example.org", token: "t",
  user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
};

const offer: AvailableUpdate = { rid: 3, currentVersion: "0.1.4", version: "0.1.5" };

function deps(over: Partial<CheckDeps> = {}) {
  const calls: string[] = [];
  const d: CheckDeps = {
    possible: () => true,
    check: async () => { calls.push("check"); return null; },
    ask: async (title, message, buttons) => { calls.push(`ask ${title}: ${String(message)} [${buttons.map((b) => b.label).join("|")}]`); return buttons[0]!.id; },
    version: () => "0.1.4",
    offer: ({ update, install }) => { calls.push(`offer ${update.version} install=${install}`); return true; },
    install: async (update) => { calls.push(`install ${update.version}`); },
    ...over,
  };
  return { d, calls };
}

const helpIds = (mobile = false) => visibleMenus(false, true, mobile).find((m) => m.id === "help")!.entries.map((e) => e.id);

afterEach(() => {
  resetCheck();
  placeHelpEntry(false);
  setStatus("");
});

describe("Help → Check for updates…", () => {
  it("is in the Help menu after Privacy Policy when this copy can update itself, and only once", () => {
    placeHelpEntry(true);
    placeHelpEntry(true);
    const ids = helpIds();
    expect(ids.filter((id) => id === CHECK_ENTRY.id)).toHaveLength(1);
    expect(ids[ids.indexOf("privacy") + 1]).toBe(CHECK_ENTRY.id);
    expect(MENUS.find((m) => m.id === "help")!.entries.find((e) => e.id === CHECK_ENTRY.id)?.label).toBe("Check for updates...");
  });

  it("is not there in a browser or on a phone (no updater)", () => {
    placeHelpEntry(true);
    placeHelpEntry(false);
    expect(helpIds()).not.toContain(CHECK_ENTRY.id);
  });

  it("the API: placed from updatesPossible, and its link checks instead of opening a page", async () => {
    const { calls } = deps();
    let checked = 0;
    checkDeps.possible = () => false;
    createHttpApi(session);
    expect(helpIds()).not.toContain(CHECK_ENTRY.id);
    checkDeps.possible = () => true;
    checkDeps.check = async () => { checked += 1; return null; };
    checkDeps.ask = async () => "close";
    const api = createHttpApi(session);
    expect(helpIds()).toContain(CHECK_ENTRY.id);
    await api.openLink({ url: CHECK_LINK });
    expect(checked).toBe(1);
    // The offline pages answer openLink from LOCAL.
    await LOCAL.openLink!({ url: CHECK_LINK });
    expect(checked).toBe(2);
    expect(calls).toEqual([]);
  });

  it("nothing newer: says this version is up to date", async () => {
    const { d, calls } = deps();
    await checkNow(d);
    expect(calls).toEqual(["check", `ask ${CHECK_TITLE}: You're up to date (Borealis Party 0.1.4). [OK]`]);
    expect(getStatus()).toBe("");
  });

  it("an update: Install and restart hands it to the update bar to install", async () => {
    const { d, calls } = deps({ check: async () => offer });
    await checkNow(d);
    expect(calls).toEqual([
      `ask ${CHECK_TITLE}: Borealis Party 0.1.5 is available (this is 0.1.4). Your characters stay as they are. [Install and restart|Later]`,
      "offer 0.1.5 install=true",
    ]);
  });

  it("an update: Later leaves it on the update bar", async () => {
    const { d, calls } = deps({ check: async () => offer, ask: async () => "later" });
    await checkNow(d);
    expect(calls).toEqual(["offer 0.1.5 install=false"]);
  });

  it("no update bar on screen: installs with the status line", async () => {
    const { d, calls } = deps({ check: async () => offer, offer: () => false });
    await checkNow(d);
    expect(calls.at(-1)).toBe("install 0.1.5");
  });

  it("the feed can't be read, or there is no updater: says why", async () => {
    const { d, calls } = deps({ check: async () => { throw new Error("feed unreachable"); } });
    await checkNow(d);
    expect(calls).toEqual([`ask ${CHECK_TITLE}: Borealis Party couldn't check for updates: feed unreachable [OK]`]);
    expect(getStatus()).toBe("");
  });

  it("other links still open as before", async () => {
    let opened = "";
    createHttpApi(session);
    const original = window.open;
    window.open = ((url: string) => { opened = url; return null; }) as typeof window.open;
    try {
      await LOCAL.openLink!({ url: "https://example.org/" });
    } finally {
      window.open = original;
    }
    expect(opened).toBe("https://example.org/");
  });
});
