import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { loadSession, rememberedServer, saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { moveToAddress } from "./AccountPage";

const user = { id: "u1", username: "alice", email: null, admin: false, mustChangePassword: false };
const OLD = "http://192.168.1.20:8787";
const NEW = "https://party.example.com";

let reload: ReturnType<typeof vi.fn>;
let meAnswer: { status: number; body: unknown };
let requests: { url: string; auth: string | null }[];

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: OLD, token: "secret", user });
  requests = [];
  meAnswer = { status: 200, body: user };
  reload = vi.fn();
  vi.stubGlobal("location", { ...window.location, reload });
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    requests.push({ url, auth: new Headers(init?.headers).get("authorization") });
    if (url.endsWith("/api/server")) return new Response(JSON.stringify({ app: "borealis-party", version: "1" }), { status: 200 });
    return new Response(JSON.stringify(meAnswer.body), { status: meAnswer.status });
  }));
});

afterEach(() => { vi.unstubAllGlobals(); });

it("keeps the sign-in on the new address when the server there knows this account", async () => {
  expect(await moveToAddress("party.example.com")).toBeNull();
  expect(requests).toEqual([
    { url: `${NEW}/api/server`, auth: null },
    { url: `${NEW}/api/me`, auth: "Bearer secret" },
  ]);
  expect(loadSession()).toEqual({ baseUrl: NEW, token: "secret", user });
  expect(rememberedServer()).toBe(NEW);
  expect(reload).toHaveBeenCalledOnce();
});

it("refuses another account's server and leaves this device signed in where it was", async () => {
  meAnswer = { status: 200, body: { ...user, id: "someone-else" } };
  expect(await moveToAddress(NEW)).toMatch(/doesn't know this account/);
  expect(loadSession()?.baseUrl).toBe(OLD);
  expect(reload).not.toHaveBeenCalled();
});

it("refuses a server that rejects the sign-in, without signing this device out", async () => {
  meAnswer = { status: 401, body: { code: "unauthorized", message: "Sign in again." } };
  expect(await moveToAddress(NEW)).toMatch(/doesn't know this account/);
  expect(loadSession()).toEqual({ baseUrl: OLD, token: "secret", user });
  expect(reload).not.toHaveBeenCalled();
});

it("refuses an address that isn't a Borealis Party server", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ app: "something" }), { status: 200 })));
  await expect(moveToAddress(NEW)).rejects.toThrow(/isn't a Borealis Party server/);
  expect(loadSession()?.baseUrl).toBe(OLD);
});

it("says so when the address is the current one, in any case", async () => {
  expect(await moveToAddress(`${OLD}/`)).toMatch(/already/);
  expect(await moveToAddress("HTTP://192.168.1.20:8787")).toMatch(/already/);
  expect(requests).toEqual([]);
});

it("never sends the sign-in unencrypted to a domain name", async () => {
  expect(await moveToAddress("http://party.example.com")).toMatch(/https/);
  expect(requests).toEqual([]);
  expect(loadSession()?.baseUrl).toBe(OLD);
});

it("allows plain http on the home network", async () => {
  expect(await moveToAddress("http://raspberrypi:8787")).toBeNull();
  expect(loadSession()?.baseUrl).toBe("http://raspberrypi:8787");
});
