import type { ContentPack } from "../../src/api/contract";
import { serverFetch } from "./http";
import { loadSession } from "./session";

/** Bulk pack removal on the server (party/crates/borealis-server/src/pack_removal.rs). */

export interface PackSource { id: string; indexUrl: string | null; fiveToolsUrl: string | null }
export interface Removed { removed: number; skipped: string[]; packs: ContentPack[] }

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject(new Error("Not signed in."));
  return serverFetch(session, method, path, body) as Promise<T>;
}

/** Where each installed pack came from (an index or a 5etools address); packs from neither are left out. */
export async function packSources(): Promise<PackSource[]> {
  const out = await call<{ packs?: PackSource[] } | null>("GET", "/api/content/pack-sources");
  return Array.isArray(out?.packs) ? out.packs : [];
}

/** Removes the account's packs `ids` in one go; the content reloads once. Built-in packs are never removed. */
export const removePacks = (ids: string[]) => call<Removed>("POST", "/api/content/remove-packs", { packIds: ids });

const trimmed = (url: string) => url.trim().replace(/\/+$/, "");

/** The packs that came from the source `url` (an index or a 5etools address). */
export function packsFrom(sources: PackSource[], url: string): string[] {
  const want = trimmed(url);
  return sources
    .filter((s) => [s.indexUrl, s.fiveToolsUrl].some((u) => u !== null && trimmed(u) === want))
    .map((s) => s.id);
}
