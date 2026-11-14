import type { ReactNode } from "react";
import {
  ABILITIES, ABILITY_WORD, isCompanionBlock, modifier, signed, type CompanionBlock, type Feature, type MonsterBlock,
  type StatBlock,
} from "../api/overrides/encounters";

/**
 * A creature's stat block as the encounter uses it (overrides applied), read from the server's copy: a monster's
 * `MonsterData` or an Aurora companion's printed block (D17 b; what it doesn't print is left out, never made up).
 * Everything is plain text: no HTML from content reaches the page. XP is never part of it (D16).
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

/** Senses as printed: `Darkvision 60 ft., passive Perception 9` from `{ darkvision: 60, passive_perception: 9 }`. */
function sensesText(senses: unknown): string {
  if (typeof senses === "string") return senses;
  if (!senses || typeof senses !== "object") return "";
  return Object.entries(senses as Record<string, unknown>)
    .filter(([, v]) => typeof v === "number" || typeof v === "string")
    .map(([k, v]) => (k === "passive_perception" ? `passive Perception ${v}` : `${cap(k.replace(/_/g, " "))} ${v}${typeof v === "number" ? " ft." : ""}`))
    .join(", ");
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

function Abilities({ scores, saves }: { scores: Partial<Record<string, number | null>>; saves?: Partial<Record<string, number>> }) {
  return (
    <div className="enc-sb-abilities" role="list" aria-label="Ability scores">
      {ABILITIES.map((a) => {
        const score = scores[a];
        const save = saves?.[a];
        return (
          <span key={a} role="listitem" className="enc-sb-ability" aria-label={`${ABILITY_WORD[a]} ${score ?? "not printed"}`}>
            <span className="enc-sb-ab-name">{ABILITY_WORD[a]}</span>
            <span className="enc-sb-ab-score">{score ?? "—"}</span>
            <span className="enc-sb-ab-mod">{score != null ? signed(modifier(score)) : ""}{save != null ? ` / save ${signed(save)}` : ""}</span>
          </span>
        );
      })}
    </div>
  );
}

function MonsterSheet({ block, hp }: { block: MonsterBlock; hp?: { current: number; max: number } }) {
  const ac = block.ac?.length ? block.ac.map((a) => `${a.value}${a.source ? ` (${a.source})` : ""}`).join(", ") : block.ac_note ?? "";
  const hpText = hp ? `${hp.current} / ${hp.max}` : block.hp ? `${block.hp.average}${block.hp.formula ? ` (${block.hp.formula})` : ""}` : block.hp_note ?? "";
  const kind = [block.size.map(cap).join(" or "), block.creature_type, block.tags?.length ? `(${block.tags.join(", ")})` : ""].filter(Boolean).join(" ")
    + (block.alignment ? `, ${block.alignment}` : "");
  return (
    <>
      <p className="enc-sb-kind muted">{kind}</p>
      <Line label="Armor Class">{ac}</Line>
      <Line label="Hit Points">{hpText}</Line>
      <Line label="Speed">{speedText(block.speed)}</Line>
      {block.abilities && <Abilities scores={block.abilities} saves={block.saves} />}
      <Line label="Skills">{Object.entries(block.skills ?? {}).map(([k, v]) => `${cap(k)} ${signed(v)}`).join(", ")}</Line>
      <Line label="Vulnerabilities">{clauses(block.damage_vulnerabilities)}</Line>
      <Line label="Resistances">{clauses(block.damage_resistances)}</Line>
      <Line label="Immunities">{[clauses(block.damage_immunities), (block.condition_immunities ?? []).join(", ")].filter(Boolean).join("; ")}</Line>
      <Line label="Senses">{sensesText((block as { senses?: unknown }).senses)}</Line>
      <Line label="Languages">{(block.languages ?? []).join(", ")}</Line>
      <Line label="Challenge">{block.cr ?? ""}</Line>
      <Features title="Traits" list={block.traits} />
      <Features title="Actions" list={block.actions} />
      <Features title="Bonus actions" list={block.bonus_actions} />
      <Features title="Reactions" list={block.reactions} />
      <Features title="Legendary actions" list={block.legendary?.actions} />
    </>
  );
}

function CompanionSheet({ block, hp }: { block: CompanionBlock; hp?: { current: number; max: number } }) {
  const kind = [block.size, block.type].filter(Boolean).join(" ") + (block.alignment ? `, ${block.alignment}` : "");
  const hpText = hp ? `${hp.current} / ${hp.max}` : block.hpText ?? (block.hp != null ? String(block.hp) : "");
  return (
    <>
      <p className="enc-sb-kind muted">{kind}</p>
      <Line label="Armor Class">{block.acText ?? (block.ac != null ? String(block.ac) : "")}</Line>
      <Line label="Hit Points">{hpText}</Line>
      <Line label="Speed">{block.speed ?? ""}</Line>
      <Abilities scores={block.abilities} saves={block.saves} />
      <Line label="Skills">{block.skills ?? ""}</Line>
      {block.defenses.map((d) => <Line key={d.name} label={d.name}>{d.text}</Line>)}
      <Line label="Senses">{block.senses ?? ""}</Line>
      <Line label="Languages">{block.languages ?? ""}</Line>
      <Line label="Challenge">{block.challenge ?? ""}</Line>
      <Features title="Traits" list={block.traits} />
      <Features title="Actions" list={block.actions} />
      <Features title="Reactions" list={block.reactions} />
    </>
  );
}

/** `title` is the name shown on top; `hp` the current and maximum hit points, when known (players' view). */
export function EncounterStatBlock({ title, block, hp }: { title: string; block: StatBlock; hp?: { current: number; max: number } }) {
  return (
    <article className="enc-statblock" aria-label={title}>
      <h3 className="enc-sb-name">{title}</h3>
      {isCompanionBlock(block) ? <CompanionSheet block={block} hp={hp} /> : <MonsterSheet block={block} hp={hp} />}
    </article>
  );
}
