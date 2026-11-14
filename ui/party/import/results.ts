import type { MissingContentItem } from "../../src/api/contract";

/**
 * The Import screen's results at a glance (0.3.6 import polish): one summary line of counts first, the content the
 * imported characters miss grouped once (not repeated per character), and what to do next. Each row's own details
 * open on request (../pages/ImportSection.tsx).
 */

export type Tone = "working" | "success" | "warning" | "error" | "info";
/** What a row brought in: a character (a .dnd5e file or a bundle), content (packs, 5etools data, an index), or neither. */
export type Category = "character" | "content" | "other";

export interface ResultLike {
  title: string;
  tone: Tone;
  category: Category;
  /** A character's content that is not installed. */
  missing?: MissingContentItem[];
  /** The character's own name, for "used by" (the title is the file's). */
  who?: string;
  /** Needs a choice from the user (books to pick, which kind of link). */
  waiting?: boolean;
}

export interface Summary {
  counts: Record<Tone, number>;
  /** The worst tone among the finished rows (working while any is). */
  tone: Tone;
  headline: string;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

export function summarize(rows: readonly ResultLike[]): Summary {
  const counts: Record<Tone, number> = { working: 0, success: 0, warning: 0, error: 0, info: 0 };
  for (const r of rows) counts[r.tone]++;
  const parts: string[] = [];
  if (counts.success) parts.push(`${counts.success} done`);
  if (counts.warning) parts.push(`${counts.warning} with warnings`);
  if (counts.error) parts.push(`${counts.error} failed`);
  if (counts.info) parts.push(`${counts.info} waiting for you`);
  if (counts.working) parts.push(`${counts.working} in progress`);
  const tone: Tone = counts.working ? "working" : counts.error ? "error" : counts.warning ? "warning" : counts.info ? "info" : "success";
  const lead = counts.working ? "Importing" : counts.error && !counts.success && !counts.warning ? "Nothing imported" : "Import finished";
  return { counts, tone, headline: `${lead}: ${plural(rows.length, "item")}, ${parts.join(", ")}.` };
}

export interface MissingItem { item: MissingContentItem; usedBy: string[] }
export interface MissingSource { source: string; items: MissingItem[] }

/** The missing content of every imported character, each item once with the characters that use it, by source. */
export function groupMissingAcross(rows: readonly ResultLike[]): MissingSource[] {
  const byId = new Map<string, MissingItem>();
  for (const r of rows) {
    for (const m of r.missing ?? []) {
      const known = byId.get(m.id);
      if (known) { if (!known.usedBy.includes(r.who ?? r.title)) known.usedBy.push(r.who ?? r.title); }
      else byId.set(m.id, { item: m, usedBy: [r.who ?? r.title] });
    }
  }
  const bySource = new Map<string, MissingItem[]>();
  for (const m of byId.values()) {
    const s = m.item.source || "Unknown source";
    bySource.set(s, [...(bySource.get(s) ?? []), m]);
  }
  return [...bySource.entries()]
    .sort(([a], [b]) => (a === "Unknown source" ? 1 : b === "Unknown source" ? -1 : a.localeCompare(b)))
    .map(([source, items]) => ({ source, items: items.sort((a, b) => (a.item.name || a.item.id).localeCompare(b.item.name || b.item.id)) }));
}

export type NextId = "characters" | "new-character" | "add-missing" | "parties";
export interface NextAction { id: NextId; label: string; primary: boolean }

/** What to do after the import, once nothing is in progress or waiting. */
export function nextActions(rows: readonly ResultLike[]): NextAction[] {
  if (rows.length === 0 || rows.some((r) => r.tone === "working" || r.waiting)) return [];
  const ok = (r: ResultLike) => r.tone === "success" || r.tone === "warning";
  const characters = rows.some((r) => r.category === "character" && ok(r));
  const content = rows.some((r) => r.category === "content" && ok(r));
  const missing = rows.some((r) => (r.missing?.length ?? 0) > 0);
  const out: NextAction[] = [];
  if (missing) out.push({ id: "add-missing", label: "Add the missing content", primary: true });
  if (characters) out.push({ id: "characters", label: "Open Characters", primary: !missing });
  else if (content) out.push({ id: "new-character", label: "Create a character", primary: !missing });
  if (characters) out.push({ id: "parties", label: "Bring them to a party", primary: false });
  return out;
}
