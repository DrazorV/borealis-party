import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setPhoneOsForTests, type PhoneOs } from "../../../src/lib/phone";
import { createHttpApi } from "../http";
import type { ServerSession } from "../session";
import { MAX_WEB_SHARE_BYTES } from "./bundles";

const device = vi.hoisted(() => ({ calls: [] as unknown[][] }));
vi.mock("../../export/androidSheet", () => ({
  sheetOnDevice: vi.fn(async (...args: unknown[]) => {
    device.calls.push(args.slice(0, 3));
    return { fileName: "Hero.pdf", mime: "application/pdf", path: "content://picked/Hero.pdf", dataBase64: null };
  }),
}));
const files = vi.hoisted(() => ({ downloads: [] as string[] }));
vi.mock("../../../src/lib/files", async (actual) => ({
  ...await actual<typeof import("../../../src/lib/files")>(),
  downloadBase64: (name: string) => { files.downloads.push(name); },
}));

const session: ServerSession = {
  baseUrl: "http://192.168.1.20:8787", token: "t",
  user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
};
let paths: string[];
let bundleBytes: number;
let invoked: string[];

beforeEach(() => {
  paths = [];
  invoked = [];
  bundleBytes = 1000;
  device.calls = [];
  files.downloads = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    const path = url.replace(session.baseUrl, "");
    paths.push(path);
    const body = path.endsWith("/bundle_export")
      ? { fileName: "Hero.borealis", characters: 1, packs: 0, bytes: bundleBytes, dataBase64: "UEs=" }
      : { fileName: "Hero.pdf", mime: "application/pdf", path: null, dataBase64: "JVBERi0=" };
    return new Response(JSON.stringify(body), { status: 200 });
  }));
  // A device command the page asked the app's backend for (the apps have no bundle command: device-link is off).
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: vi.fn(async (cmd: string) => { invoked.push(cmd); throw new Error(`no such command ${cmd}`); }),
    transformCallback: () => 0,
  };
});

afterEach(() => {
  setPhoneOsForTests(null);
  vi.unstubAllGlobals();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const api = () => createHttpApi(session);

describe("E1a: the sheet PDF", () => {
  it("Android: Save and Share are made on the device, for the signed-in account, and not by the server", async () => {
    setPhoneOsForTests("android");
    const saved = await api().exportSheet({ characterId: "hero-1", options: { format: "pdf" } } as never);
    await api().shareSheet({ characterId: "hero-1", options: { format: "pdf" } } as never);
    expect(device.calls).toEqual([
      ["exportSheet", { characterId: "hero-1", options: { format: "pdf" } }, "u"],
      ["shareSheet", { characterId: "hero-1", options: { format: "pdf" } }, "u"],
    ]);
    expect(saved).toMatchObject({ path: "content://picked/Hero.pdf" });
    expect(paths).toEqual([]);
  });

  it.each<[string, PhoneOs | null]>([["iOS", "ios"], ["a browser or the desktop app", null]])("%s: the server's PDF, as before", async (_, os) => {
    setPhoneOsForTests(os);
    const shared = await api().shareSheet({ characterId: "hero-1", options: { format: "pdf" } } as never);
    const saved = await api().exportSheet({ characterId: "hero-1", options: { format: "pdf" } } as never);
    expect(paths).toEqual(["/api/rpc/export_sheet", "/api/rpc/export_sheet"]);
    expect(shared).toEqual({ fileName: "Hero.pdf", mime: "application/pdf", shared: false, dataBase64: "JVBERi0=" });
    expect(saved).toMatchObject({ dataBase64: "JVBERi0=" });
    expect(device.calls).toEqual([]);
  });
});

describe("E1b: bundles on Android", () => {
  it.each(["bundleShare", "bundleExport"] as const)("%s is refused before anything is asked of the server or the device", async (name) => {
    setPhoneOsForTests("android");
    await expect(api()[name]({ selection: { characterIds: ["hero-1"], packs: "required" } } as never)).rejects.toMatchObject({ code: "not_available" });
    expect(paths).toEqual([]);
    expect(invoked).toEqual([]);
  });
});

describe("E2: bundles on iOS and in browsers", () => {
  const share = () => api().bundleShare({ selection: { characterIds: ["hero-1"], packs: "required" } } as never);

  it("iOS: the bytes come back for the share sheet", async () => {
    setPhoneOsForTests("ios");
    await expect(share()).resolves.toMatchObject({ shared: false, dataBase64: "UEs=", bytes: 1000 });
    expect(paths).toEqual(["/api/rpc/bundle_export"]);
  });

  it("iOS: a bundle over 48 MiB is refused with a message, and the share sheet never gets it", async () => {
    setPhoneOsForTests("ios");
    bundleBytes = MAX_WEB_SHARE_BYTES;
    await expect(share()).resolves.toMatchObject({ bytes: MAX_WEB_SHARE_BYTES });
    bundleBytes = MAX_WEB_SHARE_BYTES + 1;
    await expect(share()).rejects.toMatchObject({ code: "invalid_argument", message: expect.stringContaining("at most 48.0 MB") });
  });

  it("a browser: a download of any size, and the dialog closes as done", async () => {
    setPhoneOsForTests(null);
    bundleBytes = MAX_WEB_SHARE_BYTES * 2;
    await expect(share()).resolves.toMatchObject({ shared: true, dataBase64: null });
    expect(files.downloads).toEqual(["Hero.borealis"]);
    expect(invoked).toEqual([]);
  });
});
