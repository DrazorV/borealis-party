import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { memoryStorage } from "../test/memoryStorage";
import { BACK_MS, connectivity, resetConnectivity } from "./connectivity";
import type { ServerEvent } from "./http";
import { liveEvents as start } from "./liveEvents";
import { saveSession, type ServerSession } from "./session";

const user = { id: "u1", username: "ana", email: null, admin: false, mustChangePassword: false };
const session: ServerSession = { baseUrl: "http://server", token: "t", user };

/** One open event stream the test controls. */
interface Stream { send: (e: ServerEvent) => void; end: () => void }

/** The server: `down` answers nothing at all; otherwise `/api/events` opens a stream and `/api/me` answers. */
let down: boolean;
let streams: Stream[];
let opens: number;
let probes: number;
let online: boolean;

function serverFetch(url: string, init?: RequestInit): Promise<Response> {
  if (down) return Promise.reject(new TypeError("Failed to fetch"));
  if (url.endsWith("/api/me")) {
    probes += 1;
    return Promise.resolve(new Response(JSON.stringify(user), { status: 200 }));
  }
  opens += 1;
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  const body = new ReadableStream<Uint8Array>({ start: (c) => { controller = c; } });
  const bytes = new TextEncoder();
  const stream: Stream = {
    send: (e) => controller.enqueue(bytes.encode(`data: ${JSON.stringify(e)}\n\n`)),
    end: () => { try { controller.close(); } catch { /* ended already */ } },
  };
  init?.signal?.addEventListener("abort", () => { try { controller.error(new DOMException("aborted", "AbortError")); } catch { /* closed */ } });
  streams.push(stream);
  return Promise.resolve(new Response(body, { status: 200 }));
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("localStorage", memoryStorage());
  vi.stubGlobal("sessionStorage", memoryStorage());
  vi.stubGlobal("fetch", vi.fn(serverFetch));
  online = true;
  vi.spyOn(navigator, "onLine", "get").mockImplementation(() => online);
  down = false;
  streams = [];
  opens = 0;
  probes = 0;
  saveSession(session);
  resetConnectivity();
});

afterEach(() => {
  resetConnectivity();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const tick = (ms = 0) => vi.advanceTimersByTimeAsync(ms);
/** Every stream a test opens is stopped after it, even when the test fails part-way. */
let stops: (() => void)[] = [];
const liveEvents = (...args: Parameters<typeof start>) => {
  const stop = start(...args);
  stops.push(stop);
  return stop;
};
afterEach(() => {
  stops.forEach((s) => s());
  stops = [];
});
const eventTries = () => vi.mocked(fetch).mock.calls.filter(([u]) => String(u).endsWith("/api/events")).length;

it("delivers the server's events and says when it opened", async () => {
  const got: ServerEvent[] = [];
  const open = vi.fn();
  const stop = liveEvents(session, (e) => got.push(e), { onOpen: open });
  await tick();
  expect(open).toHaveBeenCalledWith(false);
  streams[0]!.send({ event: "party://characters-changed", payload: { replaced: true } });
  await tick();
  expect(got).toEqual([{ event: "party://characters-changed", payload: { replaced: true } }]);
  stop();
});

it("a stream that drops after a while reopens at once, and says events may have been missed", async () => {
  const open = vi.fn();
  const stop = liveEvents(session, () => {}, { onOpen: open });
  await tick(60_000);
  streams[0]!.end();
  await tick();
  expect(opens).toBe(2);
  expect(open).toHaveBeenLastCalledWith(true);
  // A drop the stream got over by itself is no reconnecting.
  expect(connectivity().link).toBe("up");
  stop();
});

it("a stream that ends as soon as it opens backs off instead of reopening in a loop", async () => {
  const stop = liveEvents(session, () => {});
  const endEach = async (ms: number) => {
    await tick(ms);
    streams[streams.length - 1]!.end();
    await tick();
  };
  await endEach(0);
  // Ended at once: it waits 1, 2, 4, 8 seconds before the next opens.
  expect(opens).toBe(1);
  await endEach(1000);
  expect(opens).toBe(2);
  await endEach(1999);
  expect(opens).toBe(2);
  await endEach(1);
  expect(opens).toBe(3);
  await tick(3999);
  expect(opens).toBe(3);
  await tick(1);
  expect(opens).toBe(4);
  stop();
});

it("no signal: the server is suspected, the stream backs off 1, 2, 4… up to 30 seconds, and reopens when it is back", async () => {
  const open = vi.fn();
  const stop = liveEvents(session, () => {}, { onOpen: open });
  await tick(60_000);
  expect(opens).toBe(1);
  down = true;
  streams[0]!.end();
  await tick();
  // Reopening failed: connectivity.ts probes (a browser here), and the stream waits.
  expect(connectivity().link).toBe("checking");
  const tries = eventTries();
  await tick(1000 + 2000 + 4000 + 8000 + 16000);
  const after = eventTries();
  expect(after - tries).toBe(5);
  expect(connectivity().link).toBe("down");
  // Capped at 30 seconds between tries.
  await tick(29_000);
  expect(eventTries()).toBe(after);
  // The server is back: the next probe brings Back online, and the stream reopens at once, without waiting its turn.
  down = false;
  window.dispatchEvent(new Event("online"));
  await tick();
  expect(connectivity().link).toBe("back");
  expect(opens).toBe(2);
  expect(open).toHaveBeenLastCalledWith(true);
  await tick(BACK_MS);
  expect(connectivity().link).toBe("up");
  stop();
});

it("while the OS says there is no network the stream doesn't try at all, and goes again when it says there is", async () => {
  const stop = liveEvents(session, () => {});
  await tick(60_000);
  down = true;
  online = false;
  streams[0]!.end();
  await tick();
  const before = eventTries();
  await tick(10 * 60_000);
  expect(eventTries()).toBe(before);
  down = false;
  online = true;
  window.dispatchEvent(new Event("online"));
  await tick();
  expect(opens).toBe(2);
  stop();
});

it("a bad event is skipped, and stopping closes the stream for good", async () => {
  const got: ServerEvent[] = [];
  const stop = liveEvents(session, (e) => {
    if (e.event === "boom") throw new Error("handler");
    got.push(e);
  });
  await tick();
  streams[0]!.send({ event: "boom", payload: null });
  streams[0]!.send({ event: "ok", payload: 1 });
  await tick();
  expect(got.map((e) => e.event)).toEqual(["ok"]);
  stop();
  await tick(120_000);
  expect(opens).toBe(1);
  expect(probes).toBe(0);
});
