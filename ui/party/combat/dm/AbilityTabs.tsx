import { useState } from "react";
import { Button } from "../../../src/components/Button";
import { Segmented } from "../../../src/components/Tabs";
import { RollInput } from "./RollInput";
import { abilityStatus } from "./derive";
import { EffectDialog, draftSpecs, emptyDraft } from "./dialogs";
import { featureFor, KIND_WORD, spellLists } from "./statRef";
import type { Ability, Combatant, Command, CombatState, StatBlock } from "./types";

type Tab = "combat" | "spells" | "traits";

const GROUPS: { title: string; kinds: Ability["kind"][] }[] = [
  { title: "Actions", kinds: ["action"] },
  { title: "Bonus actions", kinds: ["bonus_action"] },
  { title: "Reactions", kinds: ["reaction"] },
  { title: "Legendary actions", kinds: ["legendary"] },
  { title: "Free / lair", kinds: ["free"] },
];

/**
 * The creature card's tabs (C14, C15, C18): Combat lists its abilities grouped by what they spend, each with a Use
 * button that spends the economy, recharge, uses, group limit or legendary actions, or says why it can't. Spells has
 * the slots and per-day uses and Cast, which records the effect and who failed the save. Traits is the reference.
 * Nothing rolls an attack: the DM rolls (or the app's dice) and types damage (D43).
 */
export function AbilityTabs({ state, c, block, send, disabled }: {
  state: CombatState; c: Combatant; block: StatBlock | undefined; send: (cmd: Command) => void; disabled: boolean;
}) {
  const [tab, setTab] = useState<Tab>("combat");
  const lists = spellLists(block);
  const spellNames = new Set(lists.flatMap((l) => l.lists.flatMap((x) => x.spells.map((s) => s.toLowerCase()))));
  const isSpell = (a: Ability) => spellNames.has(a.name.toLowerCase());
  const abilities = (c.abilities ?? []).filter((a) => a.kind !== "passive");
  const hasSpells = lists.length > 0 || Object.keys(c.spell_slots ?? {}).length > 0;
  return (
    <div className="cb-tabs">
      <Segmented<Tab> label="Creature sections" value={tab} onChange={setTab}
        options={[{ id: "combat", label: "Combat" }, { id: "spells", label: hasSpells ? "Spells" : "Spells (none)" }, { id: "traits", label: "Traits" }]} />
      {tab === "combat" && (
        <div className="cb-tab-body">
          <Economy c={c} send={send} disabled={disabled} />
          {GROUPS.map((g) => {
            const items = abilities.filter((a) => g.kinds.includes(a.kind) && !isSpell(a));
            if (!items.length) return null;
            return (
              <section key={g.title}>
                <h4 className="cb-group-title">{g.title}</h4>
                <ul className="cb-abilities">{items.map((a) => <AbilityCard key={a.key} a={a} state={state} c={c} block={block} send={send} disabled={disabled} />)}</ul>
              </section>
            );
          })}
          {abilities.length === 0 && <p className="muted cb-small">No usable abilities on this combatant; see Traits for its stat block.</p>}
        </div>
      )}
      {tab === "spells" && <Spells state={state} c={c} block={block} send={send} disabled={disabled} spells={abilities.filter(isSpell)} />}
      {tab === "traits" && <Traits c={c} block={block} />}
    </div>
  );
}

/** C5: action, bonus action and reaction, spent or ready; a tap flips one. */
export function Economy({ c, send, disabled }: { c: Combatant; send: (cmd: Command) => void; disabled: boolean }) {
  const e = c.economy ?? { action: false, bonus_action: false, reaction: false };
  const chip = (key: "action" | "bonus_action" | "reaction", word: string) => (
    <button type="button" className={`cb-econ${e[key] ? " spent" : ""}`} aria-pressed={e[key]} disabled={disabled}
      onClick={() => send({ command: "set_economy", id: c.id, [key]: !e[key] })} title={e[key] ? `${word} spent: tap to mark ready` : `${word} ready: tap to mark spent`}>
      {word}: {e[key] ? "spent" : "ready"}
    </button>
  );
  return <div className="cb-econ-row" role="group" aria-label="Action economy">{chip("action", "Action")}{chip("bonus_action", "Bonus")}{chip("reaction", "Reaction")}</div>;
}

function AbilityCard({ a, state, c, block, send, disabled, slot }: {
  a: Ability; state: CombatState; c: Combatant; block: StatBlock | undefined; send: (cmd: Command) => void; disabled: boolean; slot?: number;
}) {
  const why = abilityStatus(state, c, a, slot);
  const text = a.text || featureFor(block, a)?.text || "";
  const res = a.uses ? c.resources?.[a.uses] : undefined;
  const meta = [
    KIND_WORD[a.kind],
    a.kind === "legendary" ? `${a.legendary_cost ?? 1} LA` : null,
    a.recharge ? `Recharge ${a.recharge.min === 6 ? "6" : `${a.recharge.min}–6`}: ${a.recharge.ready ? "ready" : "spent"}` : null,
    res ? `${res.label}: ${res.pool.remaining}/${res.pool.max}` : null,
    a.group_limit ? `group: ${a.group_limit}` : null,
    a.requires_effect ? `needs ${a.requires_effect}` : null,
  ].filter(Boolean).join(" · ");
  return (
    <li className="cb-ability">
      <div className="cb-ability-top">
        <div>
          <div className="cb-ability-name">{a.name}</div>
          <div className="cb-small muted">{meta}</div>
        </div>
        <Button small variant={why ? "default" : "primary"} disabled={disabled || Boolean(why)} title={why ?? `Use ${a.name}`}
          onClick={() => send({ command: "use_ability", id: c.id, ability: a.key, ...(slot !== undefined ? { slot } : {}) })}>
          {why ? "Unavailable" : "Use"}
        </Button>
      </div>
      {text && <p className="cb-ability-text">{text}</p>}
      {why && why !== "Passive" && <p className="cb-small cb-why">{why}</p>}
      {a.recharge && !a.recharge.ready && (
        <div className="cb-recharge">
          <RollInput kind="dice" label={`Recharge ${a.name} (d6)`} notation="1d6" min={1} max={6} disabled={disabled}
            onSubmit={(roll) => send({ command: "roll_recharge", id: c.id, ability: a.key, roll })} />
          <Button small variant="ghost" disabled={disabled} onClick={() => send({ command: "set_recharge", id: c.id, ability: a.key, ready: true })}>Mark ready</Button>
        </div>
      )}
    </li>
  );
}

function Spells({ state, c, block, send, disabled, spells }: {
  state: CombatState; c: Combatant; block: StatBlock | undefined; send: (cmd: Command) => void; disabled: boolean; spells: Ability[];
}) {
  const [casting, setCasting] = useState<string | null>(null);
  const lists = spellLists(block);
  const slots = Object.entries(c.spell_slots ?? {}).sort(([a], [b]) => Number(a) - Number(b));
  const resources = Object.entries(c.resources ?? {});
  const levels = slots.filter(([, p]) => p.remaining > 0).map(([l]) => Number(l));
  const cast = (name: string, slot: number | undefined, specs: ReturnType<typeof draftSpecs>, concentration: boolean) => {
    const ability = (c.abilities ?? []).find((a) => a.name.toLowerCase() === name.toLowerCase());
    if (ability) send({ command: "use_ability", id: c.id, ability: ability.key, ...(slot !== undefined ? { slot } : {}) });
    else if (slot !== undefined) send({ command: "adjust_slot", id: c.id, level: slot, delta: -1 });
    if (concentration && specs.length === 0) send({ command: "set_concentration", id: c.id, name });
    for (const effect of specs) send({ command: "add_effect", effect });
  };
  return (
    <div className="cb-tab-body">
      {slots.length > 0 && (
        <section>
          <h4 className="cb-group-title">Spell slots</h4>
          <div className="cb-slots">
            {slots.map(([level, p]) => (
              <span key={level} className="cb-slot">
                Lv {level}: <b>{p.remaining}</b>/{p.max}
                <button type="button" aria-label={`Spend a level ${level} slot`} disabled={disabled || p.remaining <= 0} onClick={() => send({ command: "adjust_slot", id: c.id, level: Number(level), delta: -1 })}>−</button>
                <button type="button" aria-label={`Restore a level ${level} slot`} disabled={disabled || p.remaining >= p.max} onClick={() => send({ command: "adjust_slot", id: c.id, level: Number(level), delta: 1 })}>+</button>
              </span>
            ))}
          </div>
        </section>
      )}
      {resources.length > 0 && (
        <section>
          <h4 className="cb-group-title">Uses</h4>
          <div className="cb-slots">
            {resources.map(([key, r]) => (
              <span key={key} className="cb-slot">
                {r.label}: <b>{r.pool.remaining}</b>/{r.pool.max}
                <button type="button" aria-label={`Spend ${r.label}`} disabled={disabled || r.pool.remaining <= 0} onClick={() => send({ command: "adjust_resource", id: c.id, key, delta: -1 })}>−</button>
                <button type="button" aria-label={`Restore ${r.label}`} disabled={disabled || r.pool.remaining >= r.pool.max} onClick={() => send({ command: "adjust_resource", id: c.id, key, delta: 1 })}>+</button>
              </span>
            ))}
          </div>
        </section>
      )}
      {lists.map((l) => (
        <section key={l.feature}>
          <h4 className="cb-group-title">{l.feature}{l.dc != null ? ` · DC ${l.dc}` : ""}{l.attack != null ? ` · ${l.attack >= 0 ? "+" : ""}${l.attack} to hit` : ""}</h4>
          {l.lists.map((x) => (
            <div key={x.label} className="cb-spell-list">
              <span className="cb-small muted">{x.label}</span>
              {x.spells.map((s) => <Button key={s} small disabled={disabled} onClick={() => setCasting(s)}>{s}</Button>)}
            </div>
          ))}
        </section>
      ))}
      {spells.length > 0 && (
        <ul className="cb-abilities">{spells.map((a) => (
          <li key={a.key} className="cb-ability">
            <div className="cb-ability-top">
              <div><div className="cb-ability-name">{a.name}</div><div className="cb-small muted">{KIND_WORD[a.kind]}</div></div>
              <Button small variant="primary" disabled={disabled || Boolean(abilityStatus(state, c, a))} title={abilityStatus(state, c, a) ?? `Cast ${a.name}`} onClick={() => setCasting(a.name)}>Cast</Button>
            </div>
            {a.text && <p className="cb-ability-text">{a.text}</p>}
            {abilityStatus(state, c, a) && <p className="cb-small cb-why">{abilityStatus(state, c, a)}</p>}
          </li>
        ))}</ul>
      )}
      <Button small icon="plus" disabled={disabled} onClick={() => setCasting("")}>Cast another spell…</Button>
      {lists.length === 0 && slots.length === 0 && <p className="muted cb-small">No spellcasting in this stat block.</p>}
      {casting !== null && (
        <EffectDialog title={casting ? `Cast ${casting}` : "Cast a spell"} castMode combatants={state.combatants} slotLevels={levels}
          initial={{ ...emptyDraft(c.id, ""), name: casting, targets: [], concentration: /concentration/i.test(spells.find((a) => a.name === casting)?.text ?? "") }}
          onDone={(r) => {
            setCasting(null);
            if (!r) return;
            cast(r.draft.name.trim(), r.slot, r.draft.targets.length ? draftSpecs(r.draft) : [], r.draft.concentration);
          }} />
      )}
    </div>
  );
}

function Traits({ c, block }: { c: Combatant; block: StatBlock | undefined }) {
  const traits = block?.traits ?? [];
  const passive = (c.abilities ?? []).filter((a) => a.kind === "passive");
  const extra = [...(block?.actions ?? []), ...(block?.bonus_actions ?? []), ...(block?.reactions ?? [])]
    .filter((f) => !(c.abilities ?? []).some((a) => a.name.toLowerCase() === f.name.toLowerCase()));
  return (
    <div className="cb-tab-body">
      {traits.length === 0 && passive.length === 0 && extra.length === 0 && <p className="muted cb-small">No traits in this stat block.</p>}
      <ul className="cb-abilities">
        {traits.map((t) => <li key={`t-${t.name}`} className="cb-ability"><div className="cb-ability-name">{t.name}</div><p className="cb-ability-text">{t.text}</p></li>)}
        {passive.map((a) => <li key={`p-${a.key}`} className="cb-ability"><div className="cb-ability-name">{a.name}</div>{a.text && <p className="cb-ability-text">{a.text}</p>}</li>)}
        {extra.map((t) => <li key={`x-${t.name}`} className="cb-ability"><div className="cb-ability-name">{t.name}</div><p className="cb-ability-text">{t.text}</p></li>)}
      </ul>
      {block?.legendary?.preamble && <p className="cb-small muted">{block.legendary.preamble}</p>}
    </div>
  );
}
