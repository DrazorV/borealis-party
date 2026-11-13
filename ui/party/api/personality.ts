import type { BuilderTab, CharacterSnapshot, SelectionSlot } from "../../src/api/contract";
import type { Method } from "./overrides/registry";

/**
 * "Skip Personality Choices" (account setting, default on): the selection flow leaves out a background's Personality
 * Trait, Ideal, Bond and Flaw lists, which many players never pick. Next selection never goes to them and the open-choice
 * counts (the Build and Magic badges, the status line) don't count them. They stay where they are: the Background section
 * still lists them, a pick can be made or cleared there as before, and picks already made are kept and shown.
 *
 * Every content route spells them the same way: Aurora XML (`<select type="List" name="Personality Trait">`), the 5etools
 * converter and the import assistant all write `List` selects with those four names, owned by the background (or, in
 * older Aurora files, by a `Background Characteristics` element granted by it). No type or id marks them otherwise, so a
 * slot is one when it selects from a list (`elementType` "list"), its name ends in one of the four words ("Personality
 * Trait", "Lorehold Personality Trait", "Ideals"), and it is a background's (the Background section, or an owner id that
 * names a background).
 *
 * The rewrite is of each snapshot the API answers (./overrides/personality.ts online, ./offline.ts offline): those
 * slots read as optional (`required: false`, so the section's "open the next pending picker" passes them by too) and
 * `progress` drops them. Upstream's own code is unchanged.
 */

/** The account setting, kept by the server beside upstream's settings (party/crates/borealis-server/src/settings.rs). */
export const SKIP_KEY = "skipPersonalityChoices";

let skip = true;

/** Whether the selection flow skips the personality lists now: the account's setting, on until it says otherwise. */
export function personalitySkipped(): boolean {
  return skip;
}

/** Follow the settings the API answered with (absent = the default, on: an older server, or the app offline). */
export function followSettings(settings: unknown): void {
  if (settings && typeof settings === "object") skip = (settings as Record<string, unknown>)[SKIP_KEY] !== false;
}

const NAME = /(?:^|\s)(?:personality traits?|ideals?|bonds?|flaws?)$/i;

/** A background's Personality Trait, Ideal, Bond or Flaw list. */
export function isPersonalitySlot(slot: SelectionSlot): boolean {
  if (slot.elementType !== "list" || !NAME.test(slot.title.trim())) return false;
  const owner = slot.id.split(":")[0] ?? "";
  return slot.tab === "background" || /BACKGROUND/i.test(owner);
}

/** Selection Flow group order and type priority: the host's (`snapshot_map::FLOW_TABS`, `ids::flow_priority`). */
const FLOW_TABS: BuilderTab[] = ["race", "class", "background", "abilities", "languages", "proficiencies", "feats", "companion", "spells"];
const FLOW_PRIORITY: Record<string, number> = {
  race: 0, subrace: 1, class: 2, archetype: 3, multiclass: 4, background: 5, asi: 6, language: 7, proficiency: 8, skill: 8,
  feat: 9, companion: 10, spell: 11,
};
const UNRANKED = 12;
const tabRank = (s: SelectionSlot) => { const i = FLOW_TABS.indexOf(s.tab); return i < 0 ? FLOW_TABS.length : i; };
// The host ranks a feat's own choice of an unranked type as a feat; on the Feats section that is nearly always the owner.
const priority = (s: SelectionSlot) => FLOW_PRIORITY[s.elementType] ?? (s.tab === "feats" ? FLOW_PRIORITY.feat! : UNRANKED);

/**
 * The open pick Next selection goes to once the host's own one is skipped. The host's pick ranked first, so every other
 * candidate ranks after it: the first of them in the host's order (group, type priority, slot order) is the next.
 */
function nextOpen(slots: SelectionSlot[]): string | null {
  const open = slots.map((s, i) => ({ s, i })).filter(({ s }) => s.required && !s.chosen && !s.locked);
  open.sort((a, b) => tabRank(a.s) - tabRank(b.s) || priority(a.s) - priority(b.s) || a.i - b.i);
  return open[0]?.s.id ?? null;
}

/** The snapshot with the personality lists out of the selection flow (the same snapshot when it has none). */
export function withoutPersonality(snapshot: CharacterSnapshot): CharacterSnapshot {
  const skipped = new Set(snapshot.slots.filter(isPersonalitySlot).map((s) => s.id));
  if (skipped.size === 0) return snapshot;
  const byTab = { ...snapshot.progress.byTab };
  let remaining = snapshot.progress.remaining;
  for (const s of snapshot.slots) {
    if (!skipped.has(s.id) || !s.required || s.chosen) continue;
    byTab[s.tab] = Math.max(0, (byTab[s.tab] ?? 0) - 1);
    remaining -= 1;
  }
  const slots = snapshot.slots.map((s) => (skipped.has(s.id) && s.required ? { ...s, required: false } : s));
  const host = snapshot.progress.nextSlotId;
  const nextSlotId = host && skipped.has(host) ? nextOpen(slots) : host;
  return { ...snapshot, slots, progress: { ...snapshot.progress, byTab, remaining: Math.max(0, remaining), nextSlotId } };
}

const isSnapshot = (v: unknown): v is CharacterSnapshot =>
  !!v && typeof v === "object" && Array.isArray((v as CharacterSnapshot).slots) && !!(v as CharacterSnapshot).progress?.byTab;

/** An answer with its snapshot rewritten: a snapshot itself, or one under `snapshot` (switch version, convert to 2024). */
export function rewrite(result: unknown): unknown {
  if (!skip || !result || typeof result !== "object") return result;
  if (isSnapshot(result)) return withoutPersonality(result);
  const inner = (result as { snapshot?: unknown }).snapshot;
  if (isSnapshot(inner)) return { ...result, snapshot: withoutPersonality(inner) };
  return result;
}

const SETTINGS: ReadonlySet<string> = new Set(["getSettings", "saveSettings", "restoreDefaultSettings"]);

/**
 * Wrap every command: snapshots are rewritten, and the server's settings answers set the switch. The app's own backend
 * (./offline.ts, `fromSettings` false) has no such setting, so offline the account's last known choice stays.
 */
export function wrapPersonality(methods: Record<string, Method>, fromSettings = true): void {
  for (const name of Object.keys(methods)) {
    const call = methods[name]!;
    methods[name] = async (args?: Record<string, unknown>) => {
      const result = await call(args);
      if (fromSettings && SETTINGS.has(name)) followSettings(result);
      return rewrite(result);
    };
  }
}

