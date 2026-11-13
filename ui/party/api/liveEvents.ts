import { onBack, serverReached, suspectServer, watchConnection } from "./connectivity";
import type { ServerEvent } from "./http";
import type { ServerSession } from "./session";

/**
 * The server's event stream (`GET /api/events`) for a phone that loses its signal: like http.ts's `eventStream`, but
 * a drop never hammers the server or the battery. A dropped stream reopens at once (a proxy's idle timeout or a server
 * restart); one that can't reopen tells connectivity.ts the server may be gone and waits 1, 2, 4… up to 30 seconds,
 * and nothing at all while the OS says there is no network. It reopens at once when the network or the server comes
 * back. `onOpen(again)` runs on every open; `again` is true after a drop, when events may have been missed.
 *
 * One stream per session is shared by everyone who listens (the API's events, the sync runner, the Parties page), so
 * a browser over plain HTTP keeps its few connections to the server for requests (plan 3.7). It reopens with
 * `?after=<the last event id>`, so the server can tell the client whether it missed anything (party events carry ids).
 *
 * A stream that stays open but goes silent (a proxy or a phone network that keeps the socket and drops the data) is
 * caught too: once the server has sent its first `party://ping` (every 15 s, combat-api.md), {@link SILENT_MS} with
 * nothing at all aborts that stream and reopens it like a drop. A server that doesn't ping is never cut off this way.
 */

const FIRST_MS = 1000;
const CAP_MS = 30 * 1000;
/** A stream that ends sooner than this after opening counts as a failed open (no tight reopen loop). */
const STEADY_MS = 5000;
/** The server's heartbeat (every 15 s): after the first one, three missed and the stream is taken for dead. */
export const PING_EVENT = "party://ping";
export const SILENT_MS = 45_000;

export interface LiveOptions {
  onOpen?: (again: boolean) => void;
  /** Anything arrived on the open stream: an event, the server's `party://ping`, or a keep-alive comment. */
  onBeat?: () => void;
  /** The open stream ended or failed; it reopens by itself (onOpen with `again`). */
  onDrop?: () => void;
  /** Tests: a stream of its own over this fetch, not the shared one. */
  fetch?: typeof fetch;
}

interface Listener {
  onEvent: (e: ServerEvent) => void;
  onOpen?: (again: boolean) => void;
  onBeat?: () => void;
  onDrop?: () => void;
}

/** The shared streams, by session. */
const shared = new Map<string, { listeners: Set<Listener>; stop: () => void }>();

export function liveEvents(session: ServerSession, onEvent: (e: ServerEvent) => void, options: LiveOptions = {}): () => void {
  if (options.fetch) return openStream(session, onEvent, options);
  const key = `${session.baseUrl}\n${session.token}`;
  let entry = shared.get(key);
  if (!entry) {
    const listeners = new Set<Listener>();
    const created = {
      listeners,
      stop: openStream(session, (e) => {
        for (const l of [...listeners]) {
          try { l.onEvent(e); } catch { /* one listener's bad event never reaches the others */ }
        }
      }, {
        onOpen: (again) => { for (const l of [...listeners]) l.onOpen?.(again); },
        onBeat: () => { for (const l of [...listeners]) l.onBeat?.(); },
        onDrop: () => { for (const l of [...listeners]) l.onDrop?.(); },
      }),
    };
    shared.set(key, created);
    entry = created;
  }
  const listener: Listener = { onEvent, onOpen: options.onOpen, onBeat: options.onBeat, onDrop: options.onDrop };
  const mine = entry;
  mine.listeners.add(listener);
  return () => {
    if (!mine.listeners.delete(listener) || mine.listeners.size > 0) return;
    mine.stop();
    if (shared.get(key) === mine) shared.delete(key);
  };
}

function openStream(session: ServerSession, onEvent: (e: ServerEvent) => void, options: LiveOptions): () => void {
  const get = options.fetch ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
  const abort = new AbortController();
  let wake: (() => void) | null = null;
  const nudge = () => wake?.();
  const offBack = onBack(nudge);
  window.addEventListener("online", nudge);
  watchConnection();
  /** The id of the last event that had one: a reopen asks for what came after it. */
  let lastId: string | null = null;

  /** Wait `ms`, or less when the network or the server comes back; while the OS says offline, until it doesn't. */
  const pause = (ms: number) => new Promise<void>((resolve) => {
    const done = () => { clearTimeout(timer); wake = null; resolve(); };
    const timer = navigator.onLine === false ? undefined : setTimeout(done, ms);
    wake = done;
    abort.signal.addEventListener("abort", done, { once: true });
  });

  void (async () => {
    let opened = false;
    let fails = 0;
    while (!abort.signal.aborted) {
      let reached = false;
      let answered = false;
      let openedAt = 0;
      // This attempt's own abort: the caller's stop, or the silence watchdog.
      const attempt = new AbortController();
      const onStop = () => attempt.abort();
      abort.signal.addEventListener("abort", onStop, { once: true });
      let pings = false;
      let dog: ReturnType<typeof setTimeout> | undefined;
      const watch = () => {
        clearTimeout(dog);
        if (pings) dog = setTimeout(() => attempt.abort(), SILENT_MS);
      };
      try {
        const after = lastId !== null ? `?after=${encodeURIComponent(lastId)}` : "";
        const response = await get(`${session.baseUrl}/api/events${after}`, { headers: { authorization: `Bearer ${session.token}` }, signal: attempt.signal });
        answered = true;
        if (!response.ok || !response.body) {
          // A proxy in front of a server that is down; a refused token is the requests' business (http.ts).
          if (response.status === 502 || response.status === 503 || response.status === 504) suspectServer();
          throw new Error("events refused");
        }
        reached = true;
        openedAt = Date.now();
        serverReached();
        options.onOpen?.(opened);
        opened = true;
        const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
        let buffer = "";
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          options.onBeat?.();
          watch();
          buffer += value;
          let end = buffer.indexOf("\n\n");
          while (end >= 0) {
            const lines = buffer.slice(0, end).split("\n");
            const data = lines.filter((l) => l.startsWith("data:")).map((l) => l.slice(5).trim()).join("\n");
            const id = lines.find((l) => l.startsWith("id:"))?.slice(3).trim();
            buffer = buffer.slice(end + 2);
            if (id) lastId = id;
            if (data) {
              try {
                const event = JSON.parse(data) as ServerEvent;
                // The heartbeat is the stream's own business: it arms the watchdog and goes no further.
                if (event.event === PING_EVENT) {
                  if (!pings) { pings = true; watch(); }
                } else onEvent(event);
              } catch { /* one bad event never ends the stream */ }
            }
            end = buffer.indexOf("\n\n");
          }
        }
      } catch {
        if (abort.signal.aborted) break;
        // No answer at all: the network or the server.
        if (!answered) suspectServer();
      } finally {
        clearTimeout(dog);
        abort.signal.removeEventListener("abort", onStop);
      }
      if (abort.signal.aborted) break;
      if (reached) options.onDrop?.();
      // It was open a while and dropped: reopen at once. It could not open, or ended at once: back off.
      if (reached && Date.now() - openedAt >= STEADY_MS) {
        fails = 0;
        continue;
      }
      fails += 1;
      await pause(Math.min(FIRST_MS * 2 ** (fails - 1), CAP_MS));
    }
  })();

  return () => {
    abort.abort();
    offBack();
    window.removeEventListener("online", nudge);
  };
}
