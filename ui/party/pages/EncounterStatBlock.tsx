import type { ReactNode } from "react";
import { ABILITIES, ABILITY_WORD, modifier, signed, xpText, type Feature, type MonsterBlock } from "../api/overrides/encounters";

/**
 * A creature's stat block as the encounter uses it (overrides applied), read from the server's copy. Everything is
 * plain text: no HTML from content reaches the page (plan 3.0, rich text only through SafeHtml, and none is used here).
 */

const cap = (s: string) => (s ? s[0]!.toUpperCase() + s.slice(1) : s);

function speedText(speed: MonsterBlock["speed"]): string {
  if (!speed) return "";
  return Object.entries(speed)
    .filter(([, v]) => typeof v === "number")
    .map(([k, v]) => (k === "walk" ? `${v} ft.` : `${cap(k)} ${v} ft.`))
    .join(", ");
}

function clauses(list: { text?: string; types?: string[] }[] | undefined): string {
  return (list ?? []).map((c) => c.text ?? (c.types ?? []).join(", ")).filter(Boolean).join("; ");
}

function Line({ label, children }: { label: string; children: ReactNode }) {
  if (children === "" || children == null) return null;
  return <p className="enc-sb-line"><strong>{label}</strong> {children}</p>;
}

function Features({ title, list }: { title: string; list?: Feature[] }) {
  if (!list?.length) return null;
  return (
    <section className="enc-sb-section" aria-label={title}>
      <h4>{title}</h4>
      {list.map((f, i) => (
        <p key={`${f.name}-${i}`} className="enc-sb-feature"><strong>{f.name}{f.cost && f.cost > 1 ? ` (Costs ${f.cost} Actions)` : ""}.</strong> {f.text}</p>
      ))}
    </section>
  );
}

/** `title` is the name shown on top; `showXp` is false wherever a player could see it (XP never reaches players). */
export function EncounterStatBlock({ title, block, showXp = true, hp }: {
  title: string; block: MonsterBlock; showXp?: boolean; hp?: { current: number; max: number };
}) {
  const ac = block.ac?.length ? block.ac.map((a) => `${a.value}${a.source ? ` (${a.source})` : ""}`).join(", ") : block.ac_note ?? "";
  const hpText = hp ? `${hp.current} / ${hp.max}` : block.hp ? `${block.hp.average}${block.hp.formula ? ` (${block.hp.formula})` : ""}` : block.hp_note ?? "";
  const kind = [block.size.map(cap).join(" or "), block.creature_type, block.tags?.length ? `(${block.tags.join(", ")})` : "", block.alignment ? `, ${block.alignment}` : ""]
    .filter(Boolean).join(" ").replace(" ,", ",");
  return (
    <article className="enc-statblock" aria-label={title}>
      <h3 className="enc-sb-name">{title}</h3>
      <p className="enc-sb-kind muted">{kind}</p>
      <Line label="Armor Class">{ac}</Line>
      <Line label="Hit Points">{hpText}</Line>
      <Line label="Speed">{speedText(block.speed)}</Line>
      {block.abilities && (
        <div className="enc-sb-abilities" role="list" aria-label="Ability scores">
          {ABILITIES.map((a) => {
            const score = block.abilities![a];
            const save = block.saves?.[a];
            return (
              <span key={a} role="listitem" className="enc-sb-ability" aria-label={`${ABILITY_WORD[a]} ${score}`}>
                <span className="enc-sb-ab-name">{ABILITY_WORD[a]}</span>
                <span className="enc-sb-ab-score">{score}</span>
                <span className="enc-sb-ab-mod">{signed(modifier(score))}{save != null ? ` / save ${signed(save)}` : ""}</span>
              </span>
            );
          })}
        </div>
      )}
      <Line label="Skills">{Object.entries(block.skills ?? {}).map(([k, v]) => `${cap(k)} ${signed(v)}`).join(", ")}</Line>
      <Line label="Vulnerabilities">{clauses(block.damage_vulnerabilities)}</Line>
      <Line label="Resistances">{clauses(block.damage_resistances)}</Line>
      <Line label="Immunities">{[clauses(block.damage_immunities), (block.condition_immunities ?? []).join(", ")].filter(Boolean).join("; ")}</Line>
      <Line label="Languages">{(block.languages ?? []).join(", ")}</Line>
      <Line label="Challenge">{block.cr ? `${block.cr}${showXp && block.xp != null ? ` (${xpText(block.xp)})` : ""}` : ""}</Line>
      <Features title="Traits" list={block.traits} />
      <Features title="Actions" list={block.actions} />
      <Features title="Bonus actions" list={block.bonus_actions} />
      <Features title="Reactions" list={block.reactions} />
      <Features title="Legendary actions" list={block.legendary?.actions} />
    </article>
  );
}
