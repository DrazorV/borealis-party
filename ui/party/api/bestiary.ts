import { withEnabledSources } from "./contentRules";
import { serverFetch } from "./http";
import { loadSession } from "./session";

/** The bestiary on the server (party/crates/borealis-server/src/bestiary.rs, party/docs/bestiary-api.md). */

export interface Count { value: string; count: number }

export type CoverageStatus = "complete" | "partial" | "companions_only" | "unknown" | "installed";

export interface PackCoverage {
  packId: string;
  name: string;
  kind: "bundled" | "5etools" | "index" | "custom";
  enabled: boolean;
  packRev: string;
  status: CoverageStatus;
  monsters: number;
  companions: number;
  inData: number | null;
  skipped: number;
  partial: number;
  skippedReasons: Count[];
  partialNotes: Count[];
  sources: { name: string; monsters: number }[];
  byCr: Count[];
  byType: Count[];
  rulesets: Count[];
  extras: Record<"environment" | "page" | "mythic" | "covenCr" | "altSpeeds" | "variants" | "reactionIntro" | "fluff" | "tokens", number>;
  captured: boolean;
}

export interface Coverage {
  totals: { packs: number; monsters: number; companions: number };
  packs: PackCoverage[];
}

export type CreatureKind = "monster" | "companion";

export interface MonsterSummary {
  id: string; name: string; kind: CreatureKind; cr: string | null; crValue: number | null;
  type: string | null; tags: string[]; size: string[]; ac: number | null; hp: number | null; hpFormula: string | null; source: string; packId: string;
  packRev: string; ruleset: string; legendary: boolean; mythic: boolean; lair: boolean; environments: string[];
  page: number | null;
}

export interface MonsterQuery {
  text?: string; crMin?: string; crMax?: string; types?: string[]; sizes?: string[]; environments?: string[];
  sources?: string[]; ruleset?: "dnd2014" | "dnd2024"; legendary?: boolean; kinds?: CreatureKind[];
  sort?: "name" | "cr"; limit?: number; offset?: number;
}

export interface MonsterPage { total: number; offset: number; limit: number; items: MonsterSummary[] }

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject(new Error("Not signed in."));
  return serverFetch(session, method, path, body) as Promise<T>;
}

/** What the account's content holds in creatures, per pack. */
export const bestiaryCoverage = () => call<Coverage>("GET", "/api/bestiary/coverage");

/** A page of the account's creatures matching `query`. */
/** Creatures of the account's content, of its enabled sources only (contentRules.ts). */
export const searchMonsters = async (query: MonsterQuery) =>
  call<MonsterPage>("POST", "/api/bestiary/search", await withEnabledSources((m, p, b) => call(m, p, b), query));
