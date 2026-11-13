import type { ReactNode } from "react";
import type { StatBlock } from "./types";

const ABILITIES = ["str", "dex", "con", "int", "wis", "cha"] as const;
const mod = (score: number) => Math.floor((score - 10) / 2);
const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

function Line({ label, children }: { label: string; children: ReactNode }) {
  return <p className="combat-sb-line"><strong>{label}</strong> {children}</p>;
}

function Section({ title, items }: { title: string; items?: { name: string; text: string }[] }) {
  if (!items || items.length === 0) return null;
  return (
    <section className="combat-sb-section" aria-label={title}>
      <h3>{title}</h3>
      {items.map((f, i) => <p key={`${f.name}-${i}`}><strong><em>{f.name}.</em></strong> {f.text}</p>)}
    </section>
  );
}

const clauses = (list?: { types: string[]; text?: string }[]) => (list ?? []).map((c) => c.text ?? c.types.join(", ")).join("; ");

/**
 * A creature's printed stat block (upstream's `MonsterData`, docs/monster-model.md) as plain text sections. Players see
 * it once the DM reveals the creature (D20 b); the DM tracker can show the same. Only fields that are present show.
 */
export function StatBlockView({ name, block }: { name: string; block: StatBlock }) {
  const kind = [block.size?.map(cap).join(" or "), block.creature_type, block.tags?.length ? `(${block.tags.join(", ")})` : ""]
    .filter(Boolean).join(" ");
  const ac = block.ac?.map((a) => `${a.value}${a.source ? ` (${a.source})` : ""}${a.condition ? ` ${a.condition}` : ""}`).join(", ") ?? block.ac_note;
  const hp = block.hp ? `${block.hp.average}${block.hp.formula ? ` (${block.hp.formula})` : ""}` : block.hp_note;
  const speed = block.speed
    ? [block.speed.walk != null ? `${block.speed.walk} ft.` : "", ...(["burrow", "climb", "fly", "swim"] as const)
      .filter((k) => block.speed?.[k] != null).map((k) => `${k} ${block.speed![k]} ft.${k === "fly" && block.speed?.hover ? " (hover)" : ""}`), block.speed.note ?? ""]
      .filter(Boolean).join(", ")
    : "";
  const senses = block.senses
    ? [...Object.entries(block.senses).filter(([k, v]) => typeof v === "number" && k !== "passive_perception").map(([k, v]) => `${k.replace(/_/g, " ")} ${v} ft.`),
      block.senses.passive_perception != null ? `passive Perception ${block.senses.passive_perception}` : "", block.senses.notes ?? ""].filter(Boolean).join(", ")
    : "";
  const saves = Object.entries(block.saves ?? {}).map(([k, v]) => `${cap(k)} ${signed(Number(v))}`).join(", ");
  const skills = Object.entries(block.skills ?? {}).map(([k, v]) => `${cap(k)} ${signed(Number(v))}`).join(", ");
  return (
    <article className="combat-sb" aria-label={`${name} stat block`}>
      {(kind || block.alignment) && <p className="combat-sb-kind"><em>{[kind, block.alignment].filter(Boolean).join(", ")}</em></p>}
      {ac && <Line label="Armor Class">{ac}</Line>}
      {hp && <Line label="Hit Points">{hp}</Line>}
      {speed && <Line label="Speed">{speed}</Line>}
      {block.abilities && (
        <dl className="combat-sb-abilities">
          {ABILITIES.map((a) => {
            const score = block.abilities?.[a];
            return score == null ? null : (
              <div key={a}><dt>{a.toUpperCase()}</dt><dd>{score} ({signed(mod(score))})</dd></div>
            );
          })}
        </dl>
      )}
      {saves && <Line label="Saving Throws">{saves}</Line>}
      {skills && <Line label="Skills">{skills}</Line>}
      {clauses(block.damage_vulnerabilities) && <Line label="Damage Vulnerabilities">{clauses(block.damage_vulnerabilities)}</Line>}
      {clauses(block.damage_resistances) && <Line label="Damage Resistances">{clauses(block.damage_resistances)}</Line>}
      {clauses(block.damage_immunities) && <Line label="Damage Immunities">{clauses(block.damage_immunities)}</Line>}
      {block.condition_immunities?.length ? <Line label="Condition Immunities">{block.condition_immunities.join(", ")}</Line> : null}
      {senses && <Line label="Senses">{senses}</Line>}
      {block.languages && <Line label="Languages">{block.languages.length ? block.languages.join(", ") : "—"}</Line>}
      {(block.cr || block.cr_note) && <Line label="Challenge">{block.cr ?? block.cr_note}</Line>}
      <Section title="Traits" items={block.traits?.filter((t) => !/^legendary resistance/i.test(t.name))} />
      <Section title="Actions" items={block.actions} />
      <Section title="Bonus Actions" items={block.bonus_actions} />
      <Section title="Reactions" items={block.reactions} />
      {block.legendary && (
        <section className="combat-sb-section" aria-label="Legendary Actions">
          <h3>Legendary Actions</h3>
          {/* The actions' text only: how many the creature has, and Legendary Resistance, are the DM's (user, 2026-11-13). */}
          {block.legendary.actions.map((f, i) => <p key={`${f.name}-${i}`}><strong><em>{f.name}.</em></strong> {f.text}</p>)}
        </section>
      )}
    </article>
  );
}
