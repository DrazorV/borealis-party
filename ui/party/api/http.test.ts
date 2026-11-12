import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setPhoneOsForTests } from "../../src/lib/phone";
import { createHttpApi } from "./http";
import type { ServerSession } from "./session";

const session: ServerSession = {
  baseUrl: "http://192.168.1.20:8787", token: "t",
  user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
};

let paths: string[];

beforeEach(() => {
  paths = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    paths.push(url.replace(session.baseUrl, ""));
    return new Response(JSON.stringify({ fileName: "Hero.pdf", mime: "application/pdf", path: null, dataBase64: "JVBERi0=" }), { status: 200 });
  }));
});

afterEach(() => {
  setPhoneOsForTests(null);
  vi.unstubAllGlobals();
});

describe("shareSheet on the phones", () => {
  it("iOS: the server's PDF comes back for the web view's share sheet", async () => {
    setPhoneOsForTests("ios");
    const out = await createHttpApi(session).shareSheet({ characterId: "hero-1", options: { format: "pdf" } } as never);
    expect(paths).toEqual(["/api/rpc/export_sheet"]);
    expect(out).toEqual({ fileName: "Hero.pdf", mime: "application/pdf", shared: false, dataBase64: "JVBERi0=" });
  });

  it("Android: not offered, since its web view has no share sheet", async () => {
    setPhoneOsForTests("android");
    await expect(createHttpApi(session).shareSheet({ characterId: "hero-1" } as never)).rejects.toMatchObject({ code: "not_available" });
    expect(paths).toEqual([]);
  });
});
