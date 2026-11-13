import { onBack, serverReached, suspectServer, watchConnection } from "./connectivity";
import type { ServerEvent } from "./http";
import type { ServerSession } from "./session";

/**
 * The server's event stream (`GET /api/events`) for a phone that loses its signal: like http.ts's `eventStream`, but
 * a drop never hammers the server or the battery. A dropped stream reopens at once (a proxy's idle timeout or a server
 * restart); one that can't reopen tells connectivity.ts the server may be gone and waits 1, 2, 4… up to 30 seconds,
 * and nothing at all while the OS says there is no network. It reopens at once when the network or the server comes
 * back. `onOpen(again)` runs on every open; `again` is true after a drop, when events may have been missed.
 */

const FIRST_MS = 1000;
const CAP_MS = 30 * 1000;
/** A stream that ends sooner than this after opening counts as a failed open (no tight reopen loop). */
const STEADY_MS = 5000;

export interface LiveOptions {
  onOpen?: (again: boolean) => void;
  /** Tests. */
  fetch?: typeof fetch;
}

export function liveEvents(session: ServerSession, onEvent: (e: ServerEvent) => void, options: LiveOptions = {}): () => void {
  const get = options.fetch ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
  const abort = new AbortController();
  let wake: (() => void) | null = null;
  const nudge = () => wake?.();
  const offBack = onBack(nudge);
  window.addEventListener("online", nudge);
  watchConnection();

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
      try {
        const response = await get(`${session.baseUrl}/api/events`, { headers: { authorization: `Bearer ${session.token}` }, signal: abort.signal });
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
          buffer += value;
          let end = buffer.indexOf("\n\n");
          while (end >= 0) {
            const data = buffer.slice(0, end).split("\n").filter((l) => l.startsWith("data:")).map((l) => l.slice(5).trim()).join("\n");
            buffer = buffer.slice(end + 2);
            if (data) {
              try { onEvent(JSON.parse(data) as ServerEvent); } catch { /* one bad event never ends the stream */ }
            }
            end = buffer.indexOf("\n\n");
          }
        }
      } catch {
        if (abort.signal.aborted) break;
        // No answer at all: the network or the server.
        if (!answered) suspectServer();
      }
      if (abort.signal.aborted) break;
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
