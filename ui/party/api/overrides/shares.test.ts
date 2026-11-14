import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ServerSession } from "../session";

const session: ServerSession = {
  baseUrl: "http://server", token: "t",
  user: { id: "u", username: "bob", email: null, admin: false, mustChangePassword: false },
};
vi.mock("../session", async (importOriginal) => ({ ...(await importOriginal<typeof import("../session")>()), loadSession: () => session }));

const { createHttpApi } = await import("../http");
const shares = await import("./shares");

interface Sent { method: string; path: string; body: unknown }
let sent: Sent[];

function share(id: string) {
  return { id, from: "alice", fromId: "a", fileName: "Hero.borealis", characters: 1, packs: 0, links: [], bytes: 10, expiresAt: 1, status: "claimed" };
}

beforeEach(() => {
  sent = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
    const path = url.replace(session.baseUrl, "");
    const body = init.body ? JSON.parse(String(init.body)) : undefined;
    sent.push({ method: String(init.method), path, body });
    const reply = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
    if (path === "/api/shares/redeem") return reply({ share: share(`s-${body.code}`), bundle: { token: `tok-${body.code}`, name: "Hero.borealis" } });
    if (path === "/api/rpc/bundle_open") return reply({ bundleId: `b-${body.token}`, sourceDevice: "another Borealis Connect account", preview: { items: [] } });
    if (path === "/api/rpc/bundle_accept") return reply({ added: 1, replaced: 0, keptLocal: 0, identical: 0 });
    if (path === "/api/rpc/bundle_discard") return reply(null);
    if (path.endsWith("/accepted")) return reply({ ok: true });
    return reply({ code: "not_found", message: "no" }, 404);
  }));
});

afterEach(() => {
  shares.resetShareWatchForTests();
  vi.unstubAllGlobals();
});

describe("a share's bundle in the Open bundle window", () => {
  it("accepted: sent as another player's, then the server is told so its file can go", async () => {
    const api = createHttpApi(session);
    const heard: [string, string][] = [];
    shares.onShareOutcome((id, outcome) => heard.push([id, outcome]));
    const got = await shares.redeemShare("CODE1");
    expect(got.bundle?.token).toBe("tok-CODE1");
    await api.bundleOpen({ token: "tok-CODE1" });
    await api.bundleAccept({ bundleId: "b-tok-CODE1" });
    const accept = sent.find((s) => s.path === "/api/rpc/bundle_accept");
    expect(accept?.body).toEqual({ bundleId: "b-tok-CODE1", fromAnotherPlayer: true });
    expect(sent.at(-1)).toMatchObject({ method: "POST", path: "/api/shares/s-CODE1/accepted" });
    expect(heard).toEqual([["s-CODE1", "accepted"]]);
  });

  it("closed without a choice: the share waits, and the server is not told it was accepted", async () => {
    const api = createHttpApi(session);
    const heard: [string, string][] = [];
    shares.onShareOutcome((id, outcome) => heard.push([id, outcome]));
    await shares.redeemShare("CODE2");
    await api.bundleOpen({ token: "tok-CODE2" });
    await api.bundleDiscard({ bundleId: "b-tok-CODE2" });
    expect(sent.some((s) => s.path.endsWith("/accepted"))).toBe(false);
    expect(heard).toEqual([["s-CODE2", "cancelled"]]);
  });

  it("any other bundle goes through unchanged", async () => {
    const api = createHttpApi(session);
    const heard: unknown[] = [];
    shares.onShareOutcome((...a) => heard.push(a));
    await api.bundleOpen({ token: "uploaded" });
    await api.bundleAccept({ bundleId: "b-uploaded" });
    expect(sent.find((s) => s.path === "/api/rpc/bundle_accept")?.body).toEqual({ bundleId: "b-uploaded" });
    expect(sent.some((s) => s.path.startsWith("/api/shares"))).toBe(false);
    expect(heard).toEqual([]);
  });

  it("the share calls use the share routes, never a query string", async () => {
    await shares.declineShare("s/1", true).catch(() => undefined);
    await shares.cancelShare("s1").catch(() => undefined);
    await shares.unblockSender("a1").catch(() => undefined);
    expect(sent.map((s) => `${s.method} ${s.path}`)).toEqual([
      "POST /api/shares/s%2F1/decline", "DELETE /api/shares/s1", "DELETE /api/shares/blocks/a1",
    ]);
    expect(sent[0]?.body).toEqual({ block: true });
    expect(sent.every((s) => !s.path.includes("?"))).toBe(true);
  });
});
