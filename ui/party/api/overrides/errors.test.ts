import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { createHttpApi, serverFetch } from "../http";
import type { ServerSession } from "../session";
import { DEFAULT_LIMITS, describeFailure, encodedSize, forgetLimitsForTests, isHomeNetwork, learnLimits, requestLimit } from "./errors";

const MiB = 1024 * 1024;
const PUBLIC = "https://dnd.example.com";
const HOME = "http://192.168.1.20:8787";

const user = { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false };
const at = (baseUrl: string): ServerSession => ({ baseUrl, token: "t", user });

let fetched: string[];
let reply: () => Response | Promise<Response>;

beforeAll(() => {
  // Installs the module's hooks and its file check, as the app does.
  createHttpApi(at(PUBLIC));
});

beforeEach(() => {
  forgetLimitsForTests();
  fetched = [];
  reply = () => new Response("{}", { status: 200 });
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    fetched.push(url);
    if (url.endsWith("/api/server")) return new Response(JSON.stringify({ uploads: { maxUploadMb: 4, publicUploadMb: 2 } }), { status: 200 });
    return reply();
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

/** A request body of about `mb` megabytes. */
const body = (mb: number) => ({ contentBase64: "A".repeat(Math.round(mb * MiB)) });
const html = (status: number) => () => new Response("<!DOCTYPE html><html><body>Error</body></html>", { status });
const sent = () => fetched.filter((u) => !u.endsWith("/api/server"));

describe("the addresses", () => {
  it("tells the home network from the public address", () => {
    for (const home of ["http://localhost:8787", "http://192.168.1.20:8787", "http://10.0.0.5", "http://172.20.20.247", "http://nas.local",
      "http://party", "http://[::1]:8787", "http://100.101.102.103"]) expect(isHomeNetwork(home), home).toBe(true);
    for (const away of ["https://dnd.example.com", "http://8.8.8.8", "http://172.32.0.1", "not a url"]) expect(isHomeNetwork(away), away).toBe(false);
  });

  it("uses the server's limits once it has answered, and the defaults before", async () => {
    expect(requestLimit(PUBLIC)).toBe(DEFAULT_LIMITS.publicUploadMb * MiB);
    expect(requestLimit(HOME)).toBe(DEFAULT_LIMITS.maxUploadMb * MiB);
    await learnLimits(PUBLIC);
    await learnLimits(HOME);
    expect(requestLimit(PUBLIC)).toBe(2 * MiB);
    expect(requestLimit(HOME)).toBe(4 * MiB);
  });

  it("keeps the defaults for an older server that reports no limits", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ app: "borealis-party" }), { status: 200 })));
    await learnLimits(PUBLIC);
    expect(requestLimit(PUBLIC)).toBe(95 * MiB);
  });

  it("counts a file a third larger, as it travels in base64", () => {
    expect(encodedSize(3 * MiB)).toBe(4 * MiB + 64 * 1024);
  });
});

describe("a request over the limit", () => {
  it("is refused before it is sent, with the limit and the home-network address", async () => {
    await learnLimits(PUBLIC);
    const refused = serverFetch(at(PUBLIC), "POST", "/api/rpc/stage_5etools_zip", body(3));
    await expect(refused).rejects.toMatchObject({ code: "too_large" });
    const message = String(((await refused.catch((e: unknown) => e)) as { message: string }).message);
    expect(message).toContain("Too large to send through dnd.example.com");
    expect(message).toContain("at most 1 MB of files at a time");
    expect(message).toContain("home-network address, which takes up to 2 MB");
    expect(sent()).toEqual([]);
  });

  it("on the home network names the server's own setting", async () => {
    await learnLimits(HOME);
    await expect(serverFetch(at(HOME), "POST", "/api/rpc/stage_bundle", body(5))).rejects.toMatchObject({
      code: "too_large", message: expect.stringContaining("server.max_upload_mb"),
    });
    expect(sent()).toEqual([]);
  });

  it("under the limit goes out", async () => {
    await learnLimits(PUBLIC);
    await serverFetch(at(PUBLIC), "POST", "/api/rpc/stage_bundle", body(1));
    expect(sent()).toEqual([`${PUBLIC}/api/rpc/stage_bundle`]);
  });
});

describe("failed replies in words", () => {
  const failing = () => serverFetch(at(PUBLIC), "POST", "/api/rpc/import_5etools", { x: 1 });

  it("413 from a proxy with a lower cap than the server reports: says so, never 'The server answered 413.'", async () => {
    reply = html(413);
    const error = await failing().catch((e: unknown) => e) as { code: string; message: string };
    expect(error.code).toBe("too_large");
    expect(error.message).toContain("Too large to send through dnd.example.com");
    expect(error.message).toContain("server.public_upload_mb");
    expect(error.message).not.toContain("answered 413");
  });

  it("413 for a request over the limits: the limit and the alternative", () => {
    const error = describeFailure({ baseUrl: PUBLIC, method: "POST", path: "/x", bytes: 200 * MiB, status: 413, text: "", error: { code: "internal", message: "The server answered 413." } as never });
    expect(error?.message).toMatch(/at most 71 MB of files at a time.*home-network address, which takes up to 383 MB/);
  });

  it("524 (Cloudflare's timeout) and 504: the server may still be finishing; no page shown", async () => {
    for (const status of [524, 504]) {
      reply = html(status);
      const error = await failing().catch((e: unknown) => e) as { code: string; message: string };
      expect(error.code).toBe("timeout");
      expect(error.message).toContain("may still be finishing");
      expect(error.message).not.toContain("<");
    }
  });

  it("502 and 503: can't be reached right now", async () => {
    for (const status of [502, 503]) {
      reply = html(status);
      await expect(failing()).rejects.toMatchObject({ code: "unavailable", message: expect.stringContaining(`(${status})`) });
    }
  });

  it("a 500 page: a problem on the server, without the page", async () => {
    reply = html(500);
    await expect(failing()).rejects.toMatchObject({ message: "The server ran into a problem (500). Try again; if it keeps happening, tell the server's admin." });
  });

  it("the server's own error message is kept", async () => {
    reply = () => new Response(JSON.stringify({ code: "internal", message: "The 5etools data could not be read." }), { status: 500 });
    await expect(failing()).rejects.toMatchObject({ message: "The 5etools data could not be read." });
  });

  it("another address's web page (a 403 challenge): check the address", async () => {
    reply = html(403);
    await expect(failing()).rejects.toMatchObject({ message: expect.stringContaining("answered with a web page instead of the server (403)") });
  });

  it("the connection lost while sending a file: said so, with the home network as the way round", async () => {
    reply = () => { throw new TypeError("Failed to fetch"); };
    await expect(serverFetch(at(PUBLIC), "POST", "/api/rpc/stage_bundle", body(1.5))).rejects.toMatchObject({
      code: "network", message: expect.stringMatching(/connection dropped while sending 2 MB.*home-network address/),
    });
  });

  it("the connection lost on a small request: the usual message", async () => {
    reply = () => { throw new TypeError("Failed to fetch"); };
    await expect(serverFetch(at(PUBLIC), "GET", "/api/me")).rejects.toMatchObject({
      code: "network", message: "The server can't be reached. Check your connection.",
    });
  });
});

/** A file of `mb` megabytes without its bytes. */
function bigFile(name: string, mb: number): File {
  const file = new File(["x"], name);
  Object.defineProperty(file, "size", { value: Math.round(mb * MiB) });
  return file;
}

/** A file chooser holding `files`; `cleared()` tells whether the check emptied it. */
function chooser(files: File[]): { input: HTMLInputElement; cleared: () => boolean; seen: () => number | null } {
  const input = document.createElement("input");
  input.type = "file";
  let held = files;
  let seen: number | null = null;
  Object.defineProperty(input, "files", { get: () => Object.assign([...held], { item: (i: number) => held[i] ?? null }) });
  Object.defineProperty(input, "value", { get: () => (held.length ? held[0]!.name : ""), set: (v: string) => { if (v === "") held = []; } });
  // The page's own handler, as upstream's choosers have: it reads what is still chosen.
  input.addEventListener("change", () => { seen = input.files!.length; });
  document.body.appendChild(input);
  return { input, cleared: () => held.length === 0, seen: () => seen };
}

describe("files checked before they are read", () => {
  it("a chooser asks for the limits when it opens", () => {
    const { input } = chooser([]);
    input.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(fetched).toEqual([`${PUBLIC}/api/server`]);
  });

  it("files over the limit are taken back from the chooser, with the app's message box", async () => {
    await learnLimits(PUBLIC);
    const { input, cleared, seen } = chooser([bigFile("data.zip", 0.75), bigFile("more.zip", 0.75)]);
    input.dispatchEvent(new Event("change", { bubbles: true }));
    expect(cleared()).toBe(true);
    expect(seen()).toBe(0);
    await vi.waitFor(() => expect(document.querySelector('[role="dialog"]')).not.toBeNull());
    expect(document.body.textContent).toContain("File too large");
    expect(document.body.textContent).toContain("Too large to send through dnd.example.com: 2 MB, and it takes at most 1 MB");
    expect(sent()).toEqual([]);
  });

  it("files under the limit reach the page untouched", async () => {
    await learnLimits(PUBLIC);
    const { input, cleared, seen } = chooser([bigFile("small.zip", 0.5)]);
    input.dispatchEvent(new Event("change", { bubbles: true }));
    expect(cleared()).toBe(false);
    expect(seen()).toBe(1);
    expect(document.querySelector('[role="dialog"]')).toBeNull();
  });

  it("a drop over the limit goes no further", async () => {
    await learnLimits(PUBLIC);
    const reached = vi.fn();
    window.addEventListener("drop", reached);
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(drop, "dataTransfer", { value: { files: [bigFile("data.zip", 3)], types: ["Files"] } });
    document.body.dispatchEvent(drop);
    window.removeEventListener("drop", reached);
    expect(drop.defaultPrevented).toBe(true);
    expect(reached).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(document.body.textContent).toContain("File too large"));
  });

  it("a drop under the limit goes on to the page", async () => {
    await learnLimits(PUBLIC);
    const reached = vi.fn();
    window.addEventListener("drop", reached);
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(drop, "dataTransfer", { value: { files: [bigFile("hero.dnd5e", 0.1)], types: ["Files"] } });
    document.body.dispatchEvent(drop);
    window.removeEventListener("drop", reached);
    expect(reached).toHaveBeenCalledOnce();
  });
});
