import type { FiveToolsImportReport, ImportNote } from "../../src/api/contract";
import { isExpectedSkip } from "../../src/pages/content/fiveTools";

/**
 * The 5etools import report in plain words (the importer itself is unchanged). The importer notes every detail it could
 * not carry over: most are parts printed beside a stat block (variant sidebars, mythic actions) or remarks the
 * builder has no field for. They don't stop the imported content from working, so a finished import is a success with
 * notes; only an import that brought nothing in needs the user's attention.
 *
 * Reasons come as the importer words them (crates/borealis-import-5etools/src/convert, report.rs `reason_code`):
 * "creature section: variant sidebars not imported", "creature field: walk speeds not imported", and so on. Each rule
 * below gives a group a sentence-case title and what it means for the user; anything else keeps its own words.
 */

interface Rule { test: RegExp; title: string; meaning: string }

const RULES: Rule[] = [
  { test: /variant sidebars/i, title: "Variant sidebars",
    meaning: "Optional lore and variant boxes printed beside a creature. The creature's stats are complete." },
  { test: /mythic encounter/i, title: "Mythic encounters",
    meaning: "The mythic part of a legendary group isn't supported yet. Those creatures work without it." },
  { test: /mythic actions/i, title: "Mythic actions",
    meaning: "Mythic actions aren't supported yet. Those creatures work without them." },
  { test: /lair actions and regional effects/i, title: "Lair actions and regional effects",
    meaning: "Lair actions and regional effects aren't imported. The creatures' own stat blocks are complete." },
  { test: /introduction/i, title: "Action introductions",
    meaning: "The short text printed above a creature's actions or reactions. The actions themselves are imported." },
  { test: /challenge rating in a coven/i, title: "Coven challenge ratings",
    meaning: "The higher challenge rating a creature has while in a coven. Its usual challenge rating is kept." },
  { test: /speeds? not imported|walking speed only|speed change multiply/i, title: "Special speeds",
    meaning: "Speeds that depend on a condition or multiply another speed. The creature or item keeps its other speeds." },
  { test: /hit dice/i, title: "Hit dice formulas",
    meaning: "A hit dice formula that couldn't be read. The hit point total is kept." },
  { test: /spellcasting entry without text/i, title: "Empty spellcasting entries",
    meaning: "Spellcasting entries with no text in the data. Nothing is lost that you could see." },
  { test: /without a name or text/i, title: "Unnamed creature entries",
    meaning: "Traits or actions with no name or text in the data. Nothing is lost that you could see." },
  { test: /^creature field:/i, title: "Stat block remarks",
    meaning: "Printed remarks the stat block has no place for, such as \"plus PB\" or \"varies\". The numbers are kept." },
  { test: /character option.*kept as text/i, title: "Character options kept as text",
    meaning: "Options with no matching choice in the builder. Their text is there to read; pick their effects by hand." },
  { test: /sidekick class/i, title: "Sidekick classes",
    meaning: "Sidekick classes level a companion creature, not a character, so they aren't imported as classes." },
  { test: /charges|no charge count/i, title: "Rolled charges",
    meaning: "Items whose number of charges is rolled. Track their charges by hand." },
  { test: /has no core element/i, title: "Unusual proficiencies",
    meaning: "Proficiencies (firearms, vehicles and similar) the core rules don't list. The rest of each entry works." },
  { test: /weapon proficiency chosen by filter|was widened to|some tools in a tool choice/i, title: "Wider choices",
    meaning: "Some choices offer a slightly wider list than printed. Pick what the book allows." },
  { test: /matches no imported spell/i, title: "Spell choices with no spells",
    meaning: "A spell choice whose spells aren't in the imported books. Import the book with those spells to fill it." },
  { test: /prerequisite not enforced/i, title: "Prerequisites not checked",
    meaning: "Prerequisites the builder can't check (house renown and similar). Check them yourself." },
  { test: /proficiencies the item grants|doubled proficiency bonus/i, title: "Item proficiencies",
    meaning: "Proficiencies an item grants in a way the builder can't apply. Add them by hand if you use the item." },
  { test: /pack content.*is not an item/i, title: "Pack contents",
    meaning: "Equipment pack lines that aren't items. The pack's items are there." },
  { test: /creature stat block:|creature type/i, title: "Creatures that couldn't be read",
    meaning: "Stat blocks with values the builder can't use (a type that's a choice, stats printed as text). These creatures were left out." },
  { test: /cannot resolve _copy/i, title: "Copies of missing entries",
    meaning: "Entries that build on another entry that isn't in the data. They were left out." },
];

export interface NoteGroup { title: string; meaning: string; notes: ImportNote[] }

const sentenceCase = (s: string) => (s ? s[0]!.toUpperCase() + s.slice(1) : s);

/** "Goblin|MM" as "Goblin (MM)". */
export const elementName = (element: string) => {
  const [name, source] = element.split("|");
  return source ? `${name} (${source})` : element;
};

/** Notes grouped by what they mean, largest group first. */
export function explainNotes(notes: ImportNote[], fallback: string): NoteGroup[] {
  const groups = new Map<string, NoteGroup>();
  for (const n of notes) {
    const rule = RULES.find((r) => r.test.test(n.reason));
    const own = sentenceCase(n.reason.replace(/^creature (section|field|stat block): /i, "").replace(/"[^"]*"/g, "\"…\"").replace(/\d+/g, "N"));
    const title = rule?.title ?? own;
    const group = groups.get(title) ?? { title, meaning: rule?.meaning ?? fallback, notes: [] };
    group.notes.push(n);
    groups.set(title, group);
  }
  return [...groups.values()].sort((a, b) => b.notes.length - a.notes.length || a.title.localeCompare(b.title));
}

export interface ReportView {
  /** "success": imported, maybe with notes; "warning": nothing came in. */
  tone: "success" | "warning";
  headline: string;
  /** Details carried over in part (the entries themselves are in). */
  partial: NoteGroup[];
  partialCount: number;
  /** Entries left out, not counting the ones the core rules already have. */
  skipped: NoteGroup[];
  skippedCount: number;
  /** Entries the core rules already define, not imported again. */
  expected: number;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

export function viewReport(report: FiveToolsImportReport): ReportView {
  const expected = report.skipped.filter(isExpectedSkip);
  const skipped = report.skipped.filter((n) => !isExpectedSkip(n));
  const nothing = report.converted === 0 || report.packs.length === 0;
  return {
    tone: nothing ? "warning" : "success",
    headline: nothing
      ? "Nothing was imported. Check that the books you chose have content Borealis reads."
      : `Imported ${plural(report.converted, "element")} into ${plural(report.packs.length, "content pack")}.`,
    partial: explainNotes(report.partial, "A detail the builder has no place for. The rest of each entry was imported and works."),
    partialCount: report.partial.length,
    skipped: explainNotes(skipped, "These entries were left out."),
    skippedCount: skipped.length,
    expected: expected.length,
  };
}

/** The calm line under the headline; empty when there are no notes. */
export function notesLine(view: ReportView): string {
  const parts: string[] = [];
  if (view.partialCount > 0) {
    parts.push(`${view.partialCount === 1 ? "1 detail" : `${view.partialCount} details`} couldn't be carried over. They don't stop anything from working.`);
  }
  if (view.skippedCount > 0) parts.push(`${plural(view.skippedCount, "entry", "entries")} couldn't be read and ${view.skippedCount === 1 ? "was" : "were"} left out.`);
  return parts.join(" ");
}

/** What the licence marker on imported packs means (decision D14). */
export const LICENCE_LINE =
  "These packs are marked \"Local import — not redistributable\": shared content carries them as their source address, not as files.";
