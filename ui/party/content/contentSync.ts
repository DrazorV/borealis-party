import type { ServerEvent } from "../api/http";
import { loadSession } from "../api/session";

/**
 * What Additional Content shows while the server is still working, and when its lists are fetched again (0.2.3).
 *
 * The server answers a removal only after it has reloaded the account's whole content, which takes seconds with a big
 * library, and adding or removing an index file reloads nothing, so upstream's lists never heard of it. Here:
 * - A removal hides its packs, index file or 5etools address at once (`begin`), shown as "Removing…" in the page's
 *   activity rows (./ContentActivity.tsx). If the server refuses, they come back with its message (`fail`).
 * - Additions and downloads get an activity row too, with the content progress the server sends.
 * - The lists are fetched again once a change is complete (`changed`): after the page's own call ends, and on the
 *   server's `party://content-changed`, which reaches every open page of the account, on any tab or device. Several
 *   of them close together make one fetch (`SETTLE_MS`). ./ContentScope.tsx does the fetching.
 */

/** Changes this close together make one fetch of the lists. */
export const SETTLE_MS = 150;
/** A finished removal stays hidden this long, in case a list fetched before it ended answers late. */
export const HIDE_AFTER_MS = 10_000;
/** A finished activity row stays this long (failed ones stay until dismissed). */
export const DONE_SHOWN_MS = 1500;
/** After an index update, bumps from it are left to upstream's Index files, which reloads its list itself. */
export const UPDATE_QUIET_MS = 3000;

export const CONTENT_CHANGED = "party://content-changed";
const RESYNC = "party://resync";

export type Area = "packs" | "indexes" | "fivetools";

export interface Activity {
  id: number;
  area: Area;
  label: string;
  state: "working" | "done" | "failed";
  message?: string;
  /** Shows the content progress the server sends while it works. */
  progress?: boolean;
}

export interface Hidden {
  packs?: string[];
  indexes?: string[];
  fiveTools?: string[];
}

interface Snapshot {
  /** Goes up each time the lists should be fetched again. */
  version: number;
  /** The last `version` that did not come during (or just after) an index update, unless forced. See `changed`. */
  loud: number;
  activity: readonly Activity[];
}

export interface ContentSyncDeps {
  now: () => number;
  setTimeout: (f: () => void, ms: number) => unknown;
  clearTimeout: (t: unknown) => void;
  /** The account's event stream. Answers a stop. */
  events: (onEvent: (e: ServerEvent) => void, onOpen: (again: boolean) => void) => () => void;
}

const DEFAULT_DEPS: ContentSyncDeps = {
  now: () => Date.now(),
  setTimeout: (f, ms) => globalThis.setTimeout(f, ms),
  clearTimeout: (t) => globalThis.clearTimeout(t as ReturnType<typeof setTimeout>),
  // Loaded when first needed: the stream's module leads to ../api/http.ts, which loads this one (an import cycle).
  events: (onEvent, onOpen) => {
    let stop: (() => void) | null = null;
    let stopped = false;
    void import("../api/liveEvents").then(({ liveEvents }) => {
      const session = loadSession();
      if (!stopped && session) stop = liveEvents(session, onEvent, { onOpen });
    });
    return () => { stopped = true; stop?.(); };
  },
};

export const contentSyncDeps: ContentSyncDeps = { ...DEFAULT_DEPS };

const norm = (url: string) => url.trim().replace(/\/+$/, "");

let snapshot: Snapshot = { version: 0, loud: 0, activity: [] };
const listeners = new Set<() => void>();
let nextId = 1;
let settle: unknown = null;
let settleForced = false;
/** Hidden ids, each with the time it may show again (Infinity while its removal runs). */
const hidden = { packs: new Map<string, number>(), indexes: new Map<string, number>(), fiveTools: new Map<string, number>() };
const hides = new Map<number, Hidden>();
/** Pack switches waiting for the server: id → enabled. */
const switches = new Map<string, { enabled: boolean; op: number }>();
/** Lists whose next fetch is answered from the last list: something was just hidden or switched in them. */
const cached = new Set<keyof typeof hidden>();
let updates = 0;
let updateEnded = -Infinity;
let watchers = 0;
let stopEvents: (() => void) | null = null;

function emit() {
  for (const l of [...listeners]) l();
}

function set(next: Partial<Snapshot>) {
  snapshot = { ...snapshot, ...next };
  emit();
}

export const contentSync = {
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  },
  get: (): Snapshot => snapshot,

  /**
   * The content changed: the lists are fetched again once, `SETTLE_MS` after the last of several close together.
   * While an index update runs, and just after, the change only moves `version`, not `loud`: upstream's Index files
   * (which follows `loud`) reloads its list itself when its update ends, and a fetch of its own then would replace the
   * update's summary line. `force`: `loud` too (the change did not come from that update).
   */
  changed(force = false): void {
    settleForced ||= force;
    if (settle !== null) return;
    settle = contentSyncDeps.setTimeout(() => {
      settle = null;
      const version = snapshot.version + 1;
      const quiet = !settleForced && (updates > 0 || contentSyncDeps.now() - updateEnded < UPDATE_QUIET_MS);
      settleForced = false;
      set(quiet ? { version } : { version, loud: version });
    }, SETTLE_MS);
  },

  /** Start an activity row; `hide` disappears from the lists until it ends. Answers its id. */
  begin(area: Area, label: string, hide: Hidden = {}, progress = false): number {
    const id = nextId++;
    if (hide.packs?.length || hide.indexes?.length || hide.fiveTools?.length) {
      hides.set(id, hide);
      for (const p of hide.packs ?? []) hidden.packs.set(p, Infinity);
      for (const u of hide.indexes ?? []) hidden.indexes.set(norm(u), Infinity);
      for (const u of hide.fiveTools ?? []) hidden.fiveTools.set(norm(u), Infinity);
      if (hide.packs?.length) cached.add("packs");
      if (hide.indexes?.length) cached.add("indexes");
      if (hide.fiveTools?.length) cached.add("fiveTools");
    }
    set({ activity: [...snapshot.activity, { id, area, label, state: "working", ...(progress ? { progress } : {}) }] });
    if (hides.has(id)) contentSync.changed();
    return id;
  },

  /** The activity ended well. Its row goes after a moment; what it hid stays hidden a little longer. */
  done(id: number, message?: string): void {
    const until = contentSyncDeps.now() + HIDE_AFTER_MS;
    const hide = hides.get(id);
    hides.delete(id);
    for (const p of hide?.packs ?? []) hidden.packs.set(p, until);
    for (const u of hide?.indexes ?? []) hidden.indexes.set(norm(u), until);
    for (const u of hide?.fiveTools ?? []) hidden.fiveTools.set(norm(u), until);
    for (const [pack, s] of switches) if (s.op === id) switches.delete(pack);
    set({ activity: snapshot.activity.map((a) => (a.id === id ? { ...a, state: "done", ...(message ? { message } : {}) } : a)) });
    contentSyncDeps.setTimeout(() => contentSync.dismiss(id), DONE_SHOWN_MS);
    contentSync.changed();
  },

  /** The activity failed: what it hid shows again, and its row keeps the message until dismissed. */
  fail(id: number, message: string): void {
    const hide = hides.get(id);
    hides.delete(id);
    for (const p of hide?.packs ?? []) hidden.packs.delete(p);
    for (const u of hide?.indexes ?? []) hidden.indexes.delete(norm(u));
    for (const u of hide?.fiveTools ?? []) hidden.fiveTools.delete(norm(u));
    for (const [pack, s] of switches) if (s.op === id) switches.delete(pack);
    set({ activity: snapshot.activity.map((a) => (a.id === id ? { ...a, state: "failed", message } : a)) });
    contentSync.changed(true);
  },

  dismiss(id: number): void {
    if (snapshot.activity.some((a) => a.id === id)) set({ activity: snapshot.activity.filter((a) => a.id !== id) });
  },

  /** Something is being added: whatever was hidden by an earlier removal may come back. */
  adding(): void {
    for (const map of Object.values(hidden)) for (const [k, until] of map) if (until !== Infinity) map.delete(k);
  },

  /** A pack switched on or off, shown so at once (`op`: the activity it belongs to). */
  switching(pack: string, enabled: boolean, op: number): void {
    switches.set(pack, { enabled, op });
    cached.add("packs");
    contentSync.changed();
  },

  /** An index update starts; answers its end. */
  updating(): () => void {
    updates += 1;
    let ended = false;
    return () => {
      if (ended) return;
      ended = true;
      updates -= 1;
      updateEnded = contentSyncDeps.now();
    };
  },

  /**
   * True once after a removal or a switch changed what `kind`'s list shows: its next fetch can be answered from the
   * last list, with no request, so the change shows at once even when the server is slow to answer. The server's own
   * list is fetched when the change ends.
   */
  takeCached(kind: keyof typeof hidden): boolean {
    return cached.delete(kind);
  },

  isHidden(kind: keyof typeof hidden, key: string): boolean {
    const map = hidden[kind];
    const k = kind === "packs" ? key : norm(key);
    const until = map.get(k);
    if (until === undefined) return false;
    if (until <= contentSyncDeps.now()) { map.delete(k); return false; }
    return true;
  },

  /** A pack list as the page should show it: hidden packs out, pending switches applied. */
  packs<T extends { id: string; enabled?: boolean }>(list: T[]): T[] {
    return list.filter((p) => !contentSync.isHidden("packs", p.id))
      .map((p) => { const s = switches.get(p.id); return s && p.enabled !== s.enabled ? { ...p, enabled: s.enabled } : p; });
  },
  indexes<T extends { url: string }>(list: T[]): T[] {
    return list.filter((i) => !contentSync.isHidden("indexes", i.url));
  },
  fiveTools<T extends { url: string }>(list: T[]): T[] {
    return list.filter((u) => !contentSync.isHidden("fiveTools", u.url));
  },

  /**
   * Follow the account's event stream while an Additional Content page is open: the server's content changes, and a
   * resync or a reopened stream (events may have been missed), fetch the lists again. Answers a stop.
   */
  watch(): () => void {
    watchers += 1;
    stopEvents ??= contentSyncDeps.events(
      (e) => {
        // An index update's own change is left to upstream's Index files when this tab runs one (see `changed`).
        const update = (e.payload as { command?: unknown } | null)?.command === "update_indexes";
        if (e.event === CONTENT_CHANGED || e.event === RESYNC) contentSync.changed(e.event === RESYNC || !update);
      },
      (again) => { if (again) contentSync.changed(true); },
    );
    let stopped = false;
    return () => {
      if (stopped) return;
      stopped = true;
      watchers -= 1;
      if (watchers === 0) { stopEvents?.(); stopEvents = null; }
    };
  },
};

/** Tests: start over. */
export function resetContentSync(): void {
  if (settle !== null) contentSyncDeps.clearTimeout(settle);
  settle = null;
  settleForced = false;
  stopEvents?.();
  stopEvents = null;
  watchers = 0;
  Object.assign(contentSyncDeps, DEFAULT_DEPS);
  snapshot = { version: 0, loud: 0, activity: [] };
  nextId = 1;
  for (const map of Object.values(hidden)) map.clear();
  hides.clear();
  switches.clear();
  cached.clear();
  updates = 0;
  updateEnded = -Infinity;
  listeners.clear();
}
