import { useCallback, useState } from "react";
import type { PackCoverage } from "../api/bestiary";

/**
 * Additional Content as summary cards that expand (0.3.1): what every card on the page shares. Long lists (the packs
 * of a 5etools source, the bestiary's sources, a pack group) are folded to a summary line; what is open, and the
 * bestiary's search and filter, are remembered on this device (localStorage, which may be missing or refuse).
 */

const PREFIX = "party.content.";

export function readPref<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    if (raw === null) return fallback;
    const value: unknown = JSON.parse(raw);
    return typeof value === typeof fallback ? (value as T) : fallback;
  } catch { return fallback; }
}

export function savePref(key: string, value: unknown): void {
  try { localStorage.setItem(PREFIX + key, JSON.stringify(value)); } catch { /* private window, blocked storage */ }
}

/** A value kept on this device under `key`. */
export function usePref<T>(key: string, fallback: T): [T, (next: T) => void] {
  const [value, setValue] = useState<T>(() => readPref(key, fallback));
  const set = useCallback((next: T) => { setValue(next); savePref(key, next); }, [key]);
  return [value, set];
}

/** "5etools-format: Book" → "Book": the format is said once, on the source, not on every pack. */
export function stripFormat(name: string): string {
  return name.replace(/^\s*5etools[- ]format\s*:\s*/i, "") || name;
}

export const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

/** Folded text matched as the user types it: case and accents left out. */
export function fold(s: string): string {
  return s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
}

export function matches(name: string, text: string): boolean {
  const q = fold(text.trim());
  return q === "" || fold(stripFormat(name)).includes(q);
}

export type SourceFilter = "all" | "complete" | "partial" | "notes";
export type SourceSort = "name" | "count";

export const FILTERS: { value: SourceFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "complete", label: "Complete" },
  { value: "partial", label: "Partly imported" },
  { value: "notes", label: "Has notes" },
];

/** The SRD bestiaries that come with the app, kept at the top of the list. */
export const isSrd = (p: PackCoverage) => p.kind === "bundled" || /^srd\b/i.test(p.name);

const creatures = (p: PackCoverage) => p.monsters + p.companions;

export function keep(p: PackCoverage, filter: SourceFilter): boolean {
  switch (filter) {
    case "complete": return p.status === "complete";
    case "partial": return p.status === "partial";
    case "notes": return p.partial > 0;
    default: return true;
  }
}

/** The bestiary's sources to show: SRD first, then the rest by name or by creature count (most first). */
export function pickSources(packs: PackCoverage[], q: { text: string; filter: SourceFilter; sort: SourceSort }): PackCoverage[] {
  const byName = (a: PackCoverage, b: PackCoverage) => stripFormat(a.name).localeCompare(stripFormat(b.name), undefined, { sensitivity: "base", numeric: true });
  const order = q.sort === "count" ? (a: PackCoverage, b: PackCoverage) => creatures(b) - creatures(a) || byName(a, b) : byName;
  return packs
    .filter((p) => keep(p, q.filter) && matches(p.name, q.text))
    .sort((a, b) => Number(isSrd(b)) - Number(isSrd(a)) || order(a, b));
}
