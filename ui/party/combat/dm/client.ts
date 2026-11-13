/**
 * The DM's live connection to one combat: the last state the server sent, the write queue and the offline state
 * (D44). The screen renders `view` and sends commands through {@link CombatClient.send}; it never changes the state
 * itself, so what it shows is always the server's.
 *
 * - Writes go one at a time, each with its own `requestId`, and without `expectSeq`: a queued write is an intent
 *   ("7 slashing to X") that applies to the newest state, and a retry with the same id is done at most once.
 * - Without an answer, writes stay queued and are retried with a backoff, and the last state stays on screen
 *   ("reconnecting"). After {@link OFFLINE_AFTER_MS} without an answer the tracker is "offline": no new writes are
 *   taken, and when the server answers again the DM chooses to send or drop what waited.
 * - The server's word wins: a refused write is dropped, and its reason is reported.
 */
import { isNetworkFailure, loadCombat, needsForce, newRequestId, sendCommand, undoCombat, watchCombats } from "../../api/overrides/combat";
import type { Command, DmCombat } from "./types";

/** D44: about a minute of queued writes before the tracker says "offline". */
export const OFFLINE_AFTER_MS = 60_000;
const RETRY_MS = [1000, 2000, 4000, 8000, 10_000];

export type Connection = "live" | "sending" | "reconnecting" | "offline";

export interface Queued {
  /** The write's `requestId`: a server that saw it once does it at most once. */
  op: string;
  command: Command;
  at: number;
}

export interface ClientState {
  view: DmCombat | null;
  connection: Connection;
  queue: Queued[];
  /** Offline writes waiting for the DM's choice after the server answered again. */
  held: Queued[];
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
  watch: (listener: (e: { combat: string; seq: number }) => void) => () => void;
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
  report,
});

const messageOf = (e: unknown) => String((e as { message?: string } | null)?.message ?? e);
const codeOf = (e: unknown) => (e as { code?: string } | null)?.code;

export class CombatClient {
  private state: ClientState = { view: null, connection: "live", queue: [], held: [], error: null };
  private listeners = new Set<(s: ClientState) => void>();
  private timer: unknown = null;
  private attempt = 0;
  private flushing = false;
  private stopWatch: (() => void) | null = null;
  private failingSince: number | null = null;
  private closed = false;

  constructor(readonly id: string, private readonly deps: ClientDeps) {}

  get snapshot(): ClientState {
    return this.state;
  }

  subscribe(listener: (s: ClientState) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  private set(next: Partial<ClientState>) {
    this.state = { ...this.state, ...next };
    for (const l of [...this.listeners]) l(this.state);
  }

  /** Loads the combat and follows its changes. */
  open(): void {
    this.closed = false;
    this.stopWatch?.();
    this.stopWatch = this.deps.watch((e) => {
      if (e.combat !== this.id && e.combat !== "*") return;
      if (this.state.view && e.combat === this.id && e.seq <= this.state.view.seq) return;
      void this.refresh();
    });
    void this.refresh();
  }

  close(): void {
    this.closed = true;
    this.stopWatch?.();
    this.stopWatch = null;
    if (this.timer !== null) { this.deps.clearTimeout(this.timer); this.timer = null; }
  }

  /** Fetches the state again (a change elsewhere, or the server back). Writes waiting go first. */
  async refresh(): Promise<void> {
    if (this.state.queue.length > 0) { void this.flush(); return; }
    try {
      const view = await this.deps.load(this.id);
      if (this.closed) return;
      this.answered();
      if (!this.state.view || view.seq >= this.state.view.seq) this.set({ view, error: null });
    } catch (e) {
      if (this.closed) return;
      if (isNetworkFailure(e)) this.noAnswer(() => void this.refresh());
      else if (!this.state.view) this.set({ error: messageOf(e) });
      else this.deps.report(messageOf(e), "error");
    }
  }

  /** Whether a write is taken now (not while offline, D44). */
  get accepting(): boolean {
    return this.state.connection !== "offline" && this.state.view !== null && this.state.held.length === 0;
  }

  /** Queues a write. Answers false when it can't be taken (offline). */
  send(command: Command): boolean {
    if (!this.accepting) {
      this.deps.report("Offline: changes can't be made until the server answers again.", "warning");
      return false;
    }
    this.set({ queue: [...this.state.queue, { op: newRequestId(), command, at: this.deps.now() }] });
    void this.flush();
    return true;
  }

  /** C8. Undo goes straight to the server and isn't queued: it reverts what the server has. */
  async undo(force = false): Promise<"done" | "needs_force" | "failed"> {
    const view = this.state.view;
    if (!view || this.state.queue.length > 0 || this.state.connection === "offline") {
      this.deps.report("Wait until the changes on their way are sent, then undo.", "warning");
      return "failed";
    }
    try {
      this.set({ view: await this.deps.undo(this.id, view.seq, force) });
      return "done";
    } catch (e) {
      if (needsForce(e) && !force) return "needs_force";
      if (codeOf(e) === "stale") await this.refresh();
      this.deps.report(messageOf(e), "error");
      return "failed";
    }
  }

  /** A view the page got from a call of its own (adding creatures): shown when it is not older than this one. */
  adopt(view: DmCombat): void {
    if (!this.state.view || view.seq >= this.state.view.seq) this.set({ view });
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
        const view = this.state.view;
        if (!view) break;
        this.set({ connection: "sending" });
        try {
          const next = await this.deps.send(this.id, head.command, head.op);
          this.answered();
          this.adopt(next);
          this.set({ queue: this.state.queue.slice(1) });
        } catch (e) {
          if (isNetworkFailure(e)) {
            this.noAnswer(() => void this.flush());
            return;
          }
          this.answered();
          this.set({ queue: this.state.queue.slice(1) });
          this.deps.report(messageOf(e), "error");
        }
      }
      if (this.state.connection === "sending") this.set({ connection: "live" });
    } finally {
      this.flushing = false;
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
      this.set({ view });
    } catch (e) {
      if (this.closed) return;
      if (isNetworkFailure(e)) this.noAnswer(() => void this.probe());
      else this.deps.report(messageOf(e), "error");
    }
  }
}
