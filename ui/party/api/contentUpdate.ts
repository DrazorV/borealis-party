import { serverFetch } from "./http";
import { runJob } from "./overrides/jobs";
import { loadSession } from "./session";

/**
 * "Update all content" and the automatic content update (0.3.3; server: party/crates/borealis-server/src/content_update.rs).
 * `update_all_content` runs as the account's job: the index files, then every 5etools source, one summary.
 */

export interface UpdateAll {
  indexes: number;
  sources: number;
  updated: number;
  failed: string[];
  message: string;
  /** RFC 3339. */
  checkedAt: string;
  /** Run by the automatic check. */
  auto: boolean;
}

export interface UpdateInfo {
  /** The account's "Automatic Updates" (`autoContentUpdate`, Settings, Data files). */
  auto: boolean;
  everyHours: number;
  indexes: number;
  sources: number;
  /** The last check, by hand or automatic; null before the first. */
  last: UpdateAll | null;
}

function session() {
  const s = loadSession();
  if (!s) throw new Error("Not signed in.");
  return s;
}

export const updateInfo = () => serverFetch(session(), "POST", "/api/rpc/content_update_info", {}) as Promise<UpdateInfo>;

export const updateAll = () => runJob(session(), "update_all_content", {}) as Promise<UpdateAll>;

/** "Last checked 14 Nov 2026, 11:24", or null. */
export function lastCheckedText(iso: string | null | undefined): string | null {
  if (!iso) return null;
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  return `Last checked ${d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}`;
}
