/**
 * The DM's live connection to one combat: the server's last state, the write queue, the stream's health and the
 * offline state (D44). It keeps the screen current without stale states:
 *
 * - **Server state is the truth.** `view` is what the server last sent. `shown` is that state with the writes still on
 *   their way applied on top (optimistic: a tap shows at once). When the server answers, its state replaces the
 *   guess; a refused write simply drops out of `shown` (the rollback) and its reason is reported.
 * - **Events in seq order.** `party://combat` events are invalidations: one with a `seq` not newer than the state shown
 *   is ignored; a newer one (the next, or a gap) re-fetches `GET /api/combats/{id}` once, coalescing a burst into a
 *   single reload. A reopened stream (events may have been missed) and a tab coming back from the background also
 *   re-fetch once.
 * - **Writes.** One at a time, each with its own `requestId` and no `expectSeq` (an intent like "7 slashing to X"
 *   applies to the newest state). After no answer the same `requestId` is sent again, so the server does it once.
 *   A 409 re-fetches and says "Someone else changed this; updated."
 * - **No signal.** Writes stay queued and are retried with a backoff while the last state stays on screen. After
 *   {@link OFFLINE_AFTER_MS} without an answer the tracker is "offline": no new writes, and when the server answers
 *   again the DM chooses to send or drop what waited.
 * - **The stream.** It reconnects by itself (../../api/liveEvents.ts). `streamDown` turns on only {@link STREAM_GRACE_MS}
 *   after a drop that didn't reopen, or when nothing (not even the server's ping) arrived for
 *   {@link SILENT_AFTER_MS}.
 */
import { isNetworkFailure, loadCombat, needsForce, newRequestId, sendCommand, undoCombat, watchCombats, type StreamStatus } from "../../api/overrides/combat";
import { predict } from "./mockEngine";
import type { Command, DmCombat } from "./types";

/** D44: about a minute of queued writes before the tracker says "offline". */
export const OFFLINE_AFTER_MS = 60_000;
/** The "Reconnecting…" chip waits this long after a drop, so a quick reopen never flashes it. */
export const STREAM_GRACE_MS = 3000;
/** The server pings at least every 20 s; this long with nothing at all means the stream is gone. */
export const SILENT_AFTER_MS = 20_000 + STREAM_GRACE_MS;
const RETRY_MS = [1000, 2000, 4000, 8000, 10_000];

export type Connection = "live" | "sending" | "reconnecting" | "offline";

export interface Queued {
  /** The write's `requestId`: a server that saw it once does it at most once. */
  op: string;
  command: Command;
  at: number;
}

export interface ClientState {
  /** The server's last state. */
  view: DmCombat | null;
  /** What the screen shows: `view` with the queued writes applied (the optimistic state). */
  shown: DmCombat | null;
  connection: Connection;
  queue: Queued[];
  /** Offline writes waiting for the DM's choice after the server answered again. */
  held: Queued[];
  /** The event stream has been down for a while: the screen may miss others' changes. */
  streamDown: boolean;
  /** A load error before any state arrived. */
  error: string | null;
}

export interface ClientDeps {
  now: () => number;
  setTimeout: (fn: () => void, ms: number) => unknown;
  clearTimeout: (handle: unknown) => void;
  load: (id: string) => Promise<DmCombat>;
  send: (id: string, command: Command, op: string) => Promise<DmCombat>;
  undo: (id: string, seq: number, force: boolean) => Promise<DmCombat>;
  watch: (listener: (e: { combat: string; seq: number }) => void, status: StreamStatus & { onOpen?: (again: boolean) => void }) => () => void;
  /** Tab visibility (a phone app coming back from the background): answers a stop. */
  onVisible: (fn: () => void) => () => void;
  /** A refused write or undo, in words for a toast. */
  report: (message: string, tone: "error" | "warning" | "info") => void;
}

export const defaultClientDeps = (report: ClientDeps["report"]): ClientDeps => ({
  now: () => Date.now(),
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (h) => globalThis.clearTimeout(h as ReturnType<typeof setTimeout>),
  load: loadCombat,
  send: (id, command, op) => sendCommand(id, command, op),
  undo: undoCombat,
  watch: watchCombats,
  onVisible: (fn) => {
    if (typeof document === "undefined") return () => {};
    const on = () => { if (document.visibilityState === "visible") fn(); };
    document.addEventListener("visibilitychange", on);
    window.addEventListener("pageshow", on);
    return () => { document.removeEventListener("visibilitychange", on); window.removeEventListener("pageshow", on); };
  },
  report,
});

const messageOf = (e: unknown) => String((e as { message?: string } | null)?.message ?? e);
const codeOf = (e: unknown) => (e as { code?: string } | null)?.code;
const isConflict = (e: unknown) => codeOf(e) === "conflict" || codeOf(e) === "stale" || (e as { status?: number } | null)?.status === 409;
export const CHANGED_ELSEWHERE = "Someone else changed this; updated.";

export class CombatClient {
  private state: ClientState = { view: null, shown: null, connection: "live", queue: [], held: [], streamDown: false, error: null };
  private listeners = new Set<(s: ClientState) => void>();
  private timer: unknown = null;
  private attempt = 0;
  private flushing = false;
  private stops: (() => void)[] = [];
  private failingSince: number | null = null;
  private closed = false;
  /** A load in flight, and whether another was asked for meanwhile (coalescing). */
  private loading = false;
  private loadAgain = false;
  /** The highest seq an event announced. */
  private announced = -1;
  private dropTimer: unknown = null;
  private silentTimer: unknown = null;

  constructor(readonly id: string, private readonly deps: ClientDeps) {}

  get snapshot(): ClientState {
    return this.state;
  }

  subscribe(listener: (s: ClientState) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  private set(next: Partial<ClientState>) {
    const merged = { ...this.state, ...next };
    // The optimistic state: the server's, with every write still on its way applied in order.
    let shown = merged.view;
    if (shown) {
      let state = shown.state;
      for (const q of [...merged.queue, ...merged.held]) state = predict(state, q.command) ?? state;
      shown = state === shown.state ? shown : { ...shown, state };
    }
    this.state = { ...merged, shown };
    for (const l of [...this.listeners]) l(this.state);
  }

  /** Loads the combat and follows its changes. */
  open(): void {
    this.closed = false;
    this.stop();
    this.stops.push(this.deps.watch((e) => {
      if (e.combat !== this.id && e.combat !== "*") return;
      if (e.combat === this.id) {
        // Not newer than what is shown, or already asked for: nothing to do.
        if (e.seq <= Math.max(this.state.view?.seq ?? -1, this.announced)) return;
        this.announced = e.seq;
      }
      void this.refresh();
    }, {
      onOpen: () => this.beat(),
      onBeat: () => this.beat(),
      onDrop: () => this.dropped(),
    }));
    this.stops.push(this.deps.onVisible(() => void this.refresh()));
    this.beat();
    void this.refresh();
  }

  private stop() {
    for (const s of this.stops.splice(0)) s();
  }

  close(): void {
    this.closed = true;
    this.stop();
    for (const t of [this.timer, this.dropTimer, this.silentTimer]) if (t !== null) this.deps.clearTimeout(t);
    this.timer = this.dropTimer = this.silentTimer = null;
  }

  // ----- the stream's health -----

  private beat() {
    if (this.dropTimer !== null) { this.deps.clearTimeout(this.dropTimer); this.dropTimer = null; }
    if (this.silentTimer !== null) this.deps.clearTimeout(this.silentTimer);
    this.silentTimer = this.deps.setTimeout(() => { this.silentTimer = null; this.set({ streamDown: true }); }, SILENT_AFTER_MS);
    if (this.state.streamDown) {
      this.set({ streamDown: false });
      // Back after a while away: whatever was missed comes with one reload.
      void this.refresh();
    }
  }

  private dropped() {
    if (this.dropTimer !== null) return;
    this.dropTimer = this.deps.setTimeout(() => { this.dropTimer = null; this.set({ streamDown: true }); }, STREAM_GRACE_MS);
  }

  // ----- reads -----

  /** Fetches the state once (a newer event, a reopened stream, the tab back). A burst becomes one reload. */
  async refresh(): Promise<void> {
    if (this.closed) return;
    if (this.state.queue.length > 0) { void this.flush(); return; }
    if (this.loading) { this.loadAgain = true; return; }
    this.loading = true;
    try {
      do {
        this.loadAgain = false;
        try {
          const view = await this.deps.load(this.id);
          if (this.closed) return;
          this.answered();
          this.adopt(view);
          if (this.state.error) this.set({ error: null });
        } catch (e) {
          if (this.closed) return;
          if (isNetworkFailure(e)) { this.noAnswer(() => void this.refresh()); return; }
          if (!this.state.view) this.set({ error: messageOf(e) });
          else this.deps.report(messageOf(e), "error");
          return;
        }
      } while (this.loadAgain && !this.closed);
    } finally {
      this.loading = false;
    }
  }

  /** A view from the server: shown unless it is older than the one shown. */
  adopt(view: DmCombat): void {
    if (!this.state.view || view.seq >= this.state.view.seq || view.id !== this.state.view.id) this.set({ view });
  }

  // ----- writes -----

  /** Whether a write is taken now (not while offline, D44). */
  get accepting(): boolean {
    return this.state.connection !== "offline" && this.state.view !== null && this.state.held.length === 0;
  }

  /** Queues a write and shows it at once. Answers false when it can't be taken (offline). */
  send(command: Command): boolean {
    if (!this.accepting) {
      this.deps.report("Offline: changes can't be made until the server answers again.", "warning");
      return false;
    }
    this.set({ queue: [...this.state.queue, { op: newRequestId(), command, at: this.deps.now() }] });
    void this.flush();
    return true;
  }

  /** C8. Undo goes straight to the server and isn't queued: it reverts what the server has (`expectSeq`). */
  async undo(force = false): Promise<"done" | "needs_force" | "failed"> {
    const view = this.state.view;
    if (!view || this.state.queue.length > 0 || this.state.connection === "offline") {
      this.deps.report("Wait until the changes on their way are sent, then undo.", "warning");
      return "failed";
    }
    try {
      this.adopt(await this.deps.undo(this.id, view.seq, force));
      return "done";
    } catch (e) {
      if (needsForce(e) && !force) return "needs_force";
      if (isConflict(e)) {
        await this.refresh();
        this.deps.report(CHANGED_ELSEWHERE, "warning");
      } else this.deps.report(messageOf(e), "error");
      return "failed";
    }
  }

  /** After the DM's choice: send the writes held while offline, or drop them. */
  resolveHeld(send: boolean): void {
    const held = this.state.held;
    this.set({ held: [], queue: send ? held.map((q) => ({ ...q, at: this.deps.now() })) : [] });
    if (send) void this.flush();
    else void this.refresh();
  }

  private async flush(): Promise<void> {
    if (this.flushing || this.closed) return;
    this.flushing = true;
    try {
      while (this.state.queue.length > 0 && !this.closed && this.state.connection !== "offline") {
        const [head] = this.state.queue as [Queued, ...Queued[]];
        if (!this.state.view) break;
        if (this.state.connection !== "sending") this.set({ connection: "sending" });
        try {
          const next = await this.deps.send(this.id, head.command, head.op);
          this.answered();
          const rest = this.state.queue.filter((q) => q !== head);
          if (!this.state.view || next.seq >= this.state.view.seq) this.set({ view: next, queue: rest });
          else this.set({ queue: rest });
        } catch (e) {
          if (isNetworkFailure(e)) {
            // The same requestId goes again: the server does the write at most once.
            this.noAnswer(() => void this.flush());
            return;
          }
          this.answered();
          // The rollback: the write leaves the queue, so the optimistic state shows the server's again.
          this.set({ queue: this.state.queue.filter((q) => q !== head) });
          if (isConflict(e)) {
            this.flushing = false;
            await this.refreshNow();
            this.flushing = true;
            this.deps.report(CHANGED_ELSEWHERE, "warning");
          } else this.deps.report(messageOf(e), "error");
        }
      }
      if (this.state.connection === "sending") this.set({ connection: "live" });
    } finally {
      this.flushing = false;
    }
  }

  /** A reload even while writes are queued (after a 409). */
  private async refreshNow(): Promise<void> {
    try {
      this.adopt(await this.deps.load(this.id));
    } catch (e) {
      if (!isNetworkFailure(e)) this.deps.report(messageOf(e), "error");
    }
  }

  /** The server answered: back to live, and writes kept while offline wait for the DM's choice. */
  private answered() {
    this.failingSince = null;
    this.attempt = 0;
    if (this.timer !== null) { this.deps.clearTimeout(this.timer); this.timer = null; }
    if (this.state.connection === "offline") {
      this.set({ connection: "live", held: [...this.state.held, ...this.state.queue], queue: [] });
    } else if (this.state.connection === "reconnecting") {
      this.set({ connection: "live" });
    }
  }

  /** No answer: retry later; after a minute, offline. */
  private noAnswer(retry: () => void) {
    const now = this.deps.now();
    this.failingSince ??= now;
    const since = Math.min(this.state.queue[0]?.at ?? this.failingSince, this.failingSince);
    const offline = now - since >= OFFLINE_AFTER_MS;
    this.set({ connection: offline ? "offline" : "reconnecting" });
    // The last retry before "offline" falls on the minute, so the state changes on time.
    const backoff = RETRY_MS[Math.min(this.attempt, RETRY_MS.length - 1)]!;
    const wait = offline ? backoff : Math.max(0, Math.min(backoff, since + OFFLINE_AFTER_MS - now));
    this.attempt++;
    if (this.timer !== null) this.deps.clearTimeout(this.timer);
    this.timer = this.deps.setTimeout(() => {
      this.timer = null;
      if (this.state.connection === "offline") void this.probe();
      else retry();
    }, wait);
  }

  /** Offline: only reads, until the server answers. */
  private async probe(): Promise<void> {
    try {
      const view = await this.deps.load(this.id);
      if (this.closed) return;
      this.answered();
      this.adopt(view);
    } catch (e) {
      if (this.closed) return;
      if (isNetworkFailure(e)) this.noAnswer(() => void this.probe());
      else this.deps.report(messageOf(e), "error");
    }
  }
}
