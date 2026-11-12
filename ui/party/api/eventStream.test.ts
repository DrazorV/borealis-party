import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { EventName } from "../../src/api/contract";
import { memoryStorage } from "../test/memoryStorage";
import { connectivity, resetConnectivity } from "./connectivity";
import { createHttpApi } from "./http";
import { saveSession, type ServerSession } from "./session";

/**
 * The pages' events (`api.subscribe`, upstream's own listeners) go over the same stream as the sync's: a server that
 * can't be reached is tried again with a backoff, not every 3 seconds, and connectivity.ts hears about it.
 */
const user = { id: "u1", username: "ana", email: null, admin: false, mustChangePassword: false };
const session: ServerSession = { baseUrl: "http://server", token: "t", user };

let down: boolean;
let controller: ReadableStreamDefaultController<Uint8Array> | null;

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("localStorage", memoryStorage());
  vi.stubGlobal("sessionStorage", memoryStorage());
  down = true;
  controller = null;
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (down) throw new TypeError("Failed to fetch");
    if (url.endsWith("/api/me")) return new Response(JSON.stringify(user), { status: 200 });
    const body = new ReadableStream<Uint8Array>({ start: (c) => { controller = c; } });
    init?.signal?.addEventListener("abort", () => { try { controller?.error(new DOMException("aborted", "AbortError")); } catch { /* closed */ } });
    return new Response(body, { status: 200 });
  }));
  saveSession(session);
  resetConnectivity();
});

afterEach(() => {
  resetConnectivity();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const streamTries = () => vi.mocked(fetch).mock.calls.filter(([u]) => String(u).endsWith("/api/events")).length;

it("a page's subscription backs off while the server can't be reached, and gets its events once it is back", async () => {
  const api = createHttpApi(session);
  const heard: unknown[] = [];
  const stop = api.subscribe("content:reloaded" as EventName, (payload) => heard.push(payload));
  await vi.advanceTimersByTimeAsync(0);
  expect(streamTries()).toBe(1);
  // The old stream tried every 3 s: 10 tries in 30 s. Now 1, 2, 4, 8 s apart: 5 in 15 s, then none until 31 s.
  await vi.advanceTimersByTimeAsync(15_000);
  expect(streamTries()).toBe(5);
  await vi.advanceTimersByTimeAsync(15_000);
  expect(streamTries()).toBe(5);
  expect(connectivity().link).not.toBe("up");

  down = false;
  window.dispatchEvent(new Event("online"));
  await vi.advanceTimersByTimeAsync(0);
  expect(streamTries()).toBe(6);
  controller!.enqueue(new TextEncoder().encode(`data: ${JSON.stringify({ event: "borealis://content-reloaded", payload: { packs: 2 } })}\n\n`));
  await vi.advanceTimersByTimeAsync(0);
  expect(heard).toEqual([{ packs: 2 }]);

  // The last listener gone: the stream closes and is not tried again.
  stop();
  await vi.advanceTimersByTimeAsync(120_000);
  expect(streamTries()).toBe(6);
});
