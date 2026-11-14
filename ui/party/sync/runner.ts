import { useSyncExternalStore } from "react";
import type { ApiError } from "../../src/api/contract";
import { createTauriApi } from "../../src/api/tauri";
import { isTauri } from "../../src/lib/tauri";
import { connectivity, holdReload, onBack } from "../api/connectivity";
import { serverFetch } from "../api/http";
import { liveEvents } from "../api/liveEvents";
import { loadSession } from "../api/session";
import { conflictsChanged } from "./conflicts";
import { hasCopy } from "./mirror";
import { rereadIfReplaced } from "./replaced";
import { syncOnce, type LocalApi, type ServerCall, type SyncReport } from "./sync";

/** What the account bar shows. */
export type SyncStatus =
  | { state: "idle" }
  | { state: "syncing"; first: boolean; done: number; total: number }
  /** `waiting`: characters edited offline here and not sent yet. */
  | { state: "synced"; atMs: number; waiting: number }
  | { state: "problem"; message: string };

let status: SyncStatus = { state: "idle" };
const watchers = new Set<() => void>();

function publish(next: SyncStatus): void {
  status = next;
  watchers.forEach((w) => w());
}

export function useSyncStatus(): SyncStatus {
  return useSyncExternalStore((w) => { watchers.add(w); return () => watchers.delete(w); }, () => status);
}

/** Server events after which the copy is behind. */
const CHANGED = ["party://characters-changed", "borealis://content-reloaded"];
const DEBOUNCE_MS = 2000;
const EVERY_MS = 5 * 60 * 1000;
const RETRY_MS = 30 * 1000;

interface Runner {
  local: LocalApi;
  server: ServerCall;
  userId: string;
}

let runner: Runner | null = null;
let running: Promise<void> | null = null;
let again = false;
/** Signing out: no new sync starts (see `lastSync`). */
let paused = false;
let timer: ReturnType<typeof setTimeout> | null = null;

function problemText(report: SyncReport): string | null {
  const problems = Object.values(report.problems);
  return problems.length > 0 ? problems.join(" ") : null;
}

/** The server can't be reached right now (connectivity.ts is probing, or says "Reconnecting…"). */
const unreachable = () => ["checking", "down"].includes(connectivity().link);

async function run(r: Runner): Promise<void> {
  const first = !hasCopy(r.userId);
  const before = status;
  publish({ state: "syncing", first, done: 0, total: 0 });
  // A reload in the middle of copying files would leave the copy half-written (connectivity.ts waits).
  const release = holdReload("sync");
  try {
    const report = await syncOnce({ ...r, onProgress: (done, total) => publish({ state: "syncing", first, done, total }) });
    // Characters changed in two places: the dialog asks (pages/SyncConflicts.tsx).
    conflictsChanged();
    const problem = problemText(report);
    if (problem && unreachable()) {
      // Lost the signal part-way: the banner says so, and the sync runs again once the server answers.
      publish(before.state === "syncing" ? { state: "idle" } : before);
    } else {
      publish(problem ? { state: "problem", message: problem } : { state: "synced", atMs: Date.now(), waiting: report.waiting.length });
    }
  } catch (e) {
    // No signal is no sync problem: the chip keeps what it said, the banner says "Reconnecting…".
    if (String((e as Partial<ApiError>)?.code) === "network" || unreachable()) publish(before.state === "syncing" ? { state: "idle" } : before);
    else publish({ state: "problem", message: (e as Partial<ApiError>)?.message ?? String(e) });
    later(RETRY_MS);
  } finally {
    release();
  }
}

/** Sync now, or once more after the sync that is running. */
export function requestSync(): Promise<void> {
  const r = runner;
  if (!r || paused) return Promise.resolve();
  if (running) {
    again = true;
    return running;
  }
  running = (async () => {
    do {
      again = false;
      await run(r);
    } while (again);
    running = null;
  })();
  return running;
}

/**
 * Signing out: one last sync (after the one running, if any) sends what this device has, then no other starts until
 * `resumeSync` (the sign-out was cancelled), so the copy can be removed.
 */
export async function lastSync(): Promise<void> {
  if (!runner || paused) return;
  if (running) await running;
  await requestSync();
  paused = true;
  if (timer) clearTimeout(timer);
  timer = null;
}

export function resumeSync(): void {
  paused = false;
}

function later(ms: number): void {
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => { timer = null; void requestSync(); }, ms);
}

/**
 * Keep the app's copy of the account up to date: at start, after server changes, when the app comes back to the
 * front and every 5 minutes. Apps only: a browser keeps no copy. Returns how to stop (tests).
 */
export function startSync(deps?: Partial<Runner>): () => void {
  const session = loadSession();
  if (runner || !session || session.user.mustChangePassword || (!deps && !isTauri())) return () => {};
  runner = {
    local: deps?.local ?? createTauriApi(),
    server: deps?.server ?? ((command, args) => serverFetch(session, "POST", `/api/rpc/${command}`, args ?? {})),
    userId: deps?.userId ?? session.user.id,
  };
  void requestSync();
  // Events missed while the stream was down: sync once it is back.
  const stopEvents = deps ? () => {} : liveEvents(session, (e) => {
    rereadIfReplaced(e);
    if (CHANGED.includes(e.event)) later(DEBOUNCE_MS);
  }, { onOpen: (again) => { if (again) later(DEBOUNCE_MS); } });
  const stopBack = onBack(() => void requestSync());
  const every = setInterval(() => void requestSync(), EVERY_MS);
  const front = () => { if (document.visibilityState === "visible") void requestSync(); };
  document.addEventListener("visibilitychange", front);
  return () => {
    stopEvents();
    stopBack();
    clearInterval(every);
    document.removeEventListener("visibilitychange", front);
    if (timer) clearTimeout(timer);
    timer = null;
    runner = null;
    paused = false;
    publish({ state: "idle" });
  };
}
