import { COMMANDS, type AppSettings, type ContentSource } from "../../src/api/contract";
import { defaultRestriction, loadDefaultSources } from "../../src/pages/content/applySources";
import { withToggles } from "../../src/pages/content/sourceFilters";
import { loadSession } from "./session";

/**
 * The sources the DM tools may list (0.3.5): the ones the account's new characters start on, worked out exactly as
 * upstream's `applyDefaultSources` does (the Sources page's saved default, less the books its Restrict playtest /
 * third-party toggles leave out). The server filters by it with the engine's own source restriction
 * (party/crates/borealis-server/src/content_rules.rs), and applies the toggles itself when none is sent.
 *
 * `undefined` = send nothing: every source is on, or the sources couldn't be read (the server's toggles then apply).
 */
export type Fetch = (method: string, path: string, body?: unknown) => Promise<unknown>;

/** How long one answer is reused: a picker's searches while typing shouldn't each list the sources. */
const TTL_MS = 20_000;
let cached: { key: string; at: number; value: Promise<string[] | undefined> } | null = null;

export function forgetEnabledSources(): void {
  cached = null;
}

export function enabledSources(call: Fetch, now = Date.now()): Promise<string[] | undefined> {
  let saved: string[] | null = null;
  try { saved = loadDefaultSources(); } catch { /* storage unavailable: no saved default */ }
  const key = JSON.stringify([loadSession()?.user.id ?? null, saved]);
  if (cached && cached.key === key && now - cached.at < TTL_MS) return cached.value;
  const value = Promise.all([
    call("POST", `/api/rpc/${COMMANDS.listSources}`, {}) as Promise<ContentSource[]>,
    (call("POST", `/api/rpc/${COMMANDS.getSettings}`, {}) as Promise<Partial<AppSettings> | null>).catch(() => null),
  ]).then(([sources, settings]) => {
    if (!Array.isArray(sources)) return undefined;
    return withToggles(defaultRestriction(saved, sources.map((s) => s.id)), sources, settings ?? {}) ?? undefined;
  }, () => undefined);
  cached = { key, at: now, value };
  return value;
}

/** `query` with the enabled sources added when there is a restriction. */
export async function withEnabledSources<Q extends object>(call: Fetch, query: Q): Promise<Q & { enabledSources?: string[] }> {
  const on = await enabledSources(call);
  return on ? { ...query, enabledSources: on } : query;
}
