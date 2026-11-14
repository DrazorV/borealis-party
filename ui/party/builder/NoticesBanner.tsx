import { useState, type ReactNode } from "react";
import type { MissingContentItem } from "../../src/api/types/character";
import { Icon } from "../../src/components";
import { useCharacter } from "../../src/state/CharacterContext";
import "./notices.css";

/**
 * The Build page's content notices (replaces upstream's `NoticesBanner` in the party build, see `vite.config.ts`).
 * Upstream's banner is kept (one notice as it is, several as a count that expands), plus:
 *  - it lines up with the selection rows below it (same left edge and width) and keeps a gap before them;
 *  - the × dismisses the notices shown, per character and message, for this browser session; a new or changed message shows again;
 *  - an engine message naming a raw element id ("inventory item ID_... is not installed") reads as a sentence with a
 *    readable name, and the id moves into a Details toggle.
 */
export function NoticesBanner({ notices, action }: { notices: string[]; action?: ReactNode }) {
  const { snapshot } = useCharacter();
  const characterId = snapshot?.id ?? "";
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(() => readDismissed(characterId));
  const [forId, setForId] = useState(characterId);
  const [open, setOpen] = useState(false);
  if (forId !== characterId) {
    setForId(characterId);
    setDismissed(readDismissed(characterId));
  }
  const shown = notices.filter((n) => !dismissed.has(n));
  if (shown.length === 0) return null;
  const dismiss = () => {
    const next = new Set([...dismissed, ...shown]);
    setDismissed(next);
    writeDismissed(characterId, next);
  };
  const names = snapshot?.missingContent ?? [];
  const close = (
    <button type="button" className="party-notices-close" aria-label="Dismiss notices" title="Dismiss" onClick={dismiss}>
      <Icon name="x" />
    </button>
  );
  if (shown.length === 1) {
    return (
      <section className="alert info bp-notices party-notices" aria-label="Content notices">
        <div className="party-notices-body"><Notice text={shown[0]!} names={names} />{action}</div>
        {close}
      </section>
    );
  }
  return (
    <section className="alert info bp-notices party-notices" aria-label="Content notices">
      <div className="party-notices-body">
        <button type="button" className="bp-notices-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
          {shown.length} content notices<span aria-hidden="true">{open ? " ▾" : " ▸"}</span>
        </button>
        {action}
        {open && <ul className="bp-notices-list">{shown.map((n) => <li key={n}><Notice text={n} names={names} /></li>)}</ul>}
      </div>
      {close}
    </section>
  );
}

function Notice({ text, names }: { text: string; names: readonly MissingContentItem[] }) {
  const r = readableNotice(text, names);
  if (!r) return <>{text}</>;
  return (
    <>
      {r.text}
      <details className="party-notices-details">
        <summary>Details</summary>
        <code>{r.id}</code>
      </details>
    </>
  );
}

/** Engine messages that name a missing element by its raw id (`borealis-engine` inventory.rs / graph.rs). */
const MISSING: readonly { re: RegExp; what: string }[] = [
  { re: /^inventory item (\S+) is not installed$/, what: "an item" },
  { re: /^chosen element (\S+) is not installed$/, what: "a choice" },
  { re: /^option (\S+) is not installed$/, what: "an option" },
];

/** A missing-content message as a sentence with a readable name, and the raw id; `null` for any other message. */
export function readableNotice(text: string, names: readonly MissingContentItem[] = []): { text: string; id: string } | null {
  for (const { re, what } of MISSING) {
    const m = re.exec(text.trim());
    if (!m) continue;
    const id = m[1]!;
    const name = names.find((n) => n.id === id)?.name || readableId(id);
    return { text: `This character needs ${what} that isn't in your installed content: ${name}.`, id };
  }
  return null;
}

/** Aurora's proxy item ids (`borealis-engine` proxy.rs): `ID_<ABBR>_INTERNAL_ITEM_<kind infix><rest>`. */
const PROXY: readonly { re: RegExp; label: (m: RegExpExecArray) => string }[] = [
  { re: /_INTERNAL_ITEM_FEAT_PROXY_(.+)$/, label: () => "Feat" },
  { re: /_INTERNAL_ITEM_LANGUAGE_PROXY_LANGUAGE_(.+)$/, label: () => "Language" },
  { re: /_INTERNAL_ITEM_PROFICIENCY_PROXY_PROFICIENCY_(.+)$/, label: () => "Proficiency" },
  { re: /_INTERNAL_ITEM_PROXY_ASI_(.+)$/, label: () => "Ability Score Improvement" },
  { re: /_INTERNAL_ITEM_(.*?)_SPELL_PROXY_SPELL_(.+)$/, label: (m) => (m[1] ? `${words(m[1])} Spell` : "Spell") },
];

/** A readable name for an element id: "Additional Spell, Rimes Binding Ice" for a proxy, else the words after its type. */
export function readableId(id: string): string {
  for (const { re, label } of PROXY) {
    const m = re.exec(id);
    if (m) return `Additional ${label(m)}, ${words(m[m.length - 1]!)}`;
  }
  // ID_<PUBLISHER>_<SOURCE>_<TYPE>_<NAME...>: the name is what follows the last known type word.
  const parts = id.replace(/^ID_/, "").split("_").filter(Boolean);
  const types = new Set(["ITEM", "SPELL", "FEAT", "RACE", "SUBRACE", "CLASS", "ARCHETYPE", "FEATURE", "LANGUAGE", "PROFICIENCY", "BACKGROUND", "OPTION", "MAGIC"]);
  let at = -1;
  parts.forEach((p, i) => { if (types.has(p)) at = i; });
  const rest = at >= 0 && at < parts.length - 1 ? parts.slice(at + 1) : parts;
  return words(rest.join("_"));
}

function words(s: string): string {
  return s.split("_").filter(Boolean).map((w) => w.charAt(0) + w.slice(1).toLowerCase()).join(" ");
}

const KEY = "party.dismissedNotices.";

function readDismissed(characterId: string): ReadonlySet<string> {
  if (!characterId) return new Set();
  try {
    const raw = sessionStorage.getItem(KEY + characterId);
    const list: unknown = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(list) ? list.filter((x): x is string => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

function writeDismissed(characterId: string, set: ReadonlySet<string>) {
  if (!characterId) return;
  try {
    sessionStorage.setItem(KEY + characterId, JSON.stringify([...set]));
  } catch {
    // Storage blocked: the notices stay dismissed until the page reloads.
  }
}
