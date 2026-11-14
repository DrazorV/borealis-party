import { useState } from "react";
import { Button } from "../../../src/components/Button";
import { Segmented } from "../../../src/components/Tabs";
import { CastSheet } from "./CastSheet";
import { RollInput } from "./RollInput";
import { isDefeated } from "./derive";
import { asCommand, COVERAGE_HELP, COVERAGE_WORD, mechOf, spellOf } from "./spellTypes";
import { spellLists } from "./statRef";
import type { AddPreset } from "./SummonPanel";
import { TurnPanel } from "./TurnPanel";
import { turnButtons, type TurnButton } from "./turn";
import type { Ability, Combatant, Command, CombatState, StatBlock } from "./types";

type Tab = "combat" | "spells" | "traits";

/**
 * The creature card's tabs (C14, C15, C18; 0.3.2): Combat is the turn panel, a button for every action it has,
 * spending what it costs. Spells lists its spells with how well the app knows each one (Auto, Partial, Ruling,
 * Unlinked, Suggested) and [Cast]. Traits is the reference.
 */
export function AbilityTabs({ state, c, block, blockOf, send, disabled, onDefine, onAddCreature }: {
  state: CombatState; c: Combatant; block: StatBlock | undefined; blockOf: (id: string) => StatBlock | undefined;
  send: (cmd: Command) => void; disabled: boolean; onDefine: (a: Ability) => void; onAddCreature?: (p: AddPreset) => void;
}) {
  const [tab, setTab] = useState<Tab>("combat");
  const hasSpells = spellLists(block).length > 0 || Object.keys(c.spell_slots ?? {}).length > 0 || turnButtons(state, c, block).some((b) => b.kind === "spell");
  return (
    <div className="cb-tabs">
      <Segmented<Tab> label="Creature sections" value={tab} onChange={setTab}
        options={[{ id: "combat", label: "Combat" }, { id: "spells", label: hasSpells ? "Spells" : "Spells (none)" }, { id: "traits", label: "Traits" }]} />
      {tab === "combat" && (
        <div className="cb-tab-body">
          <TurnPanel state={state} c={c} block={block} blockOf={blockOf} send={send} disabled={disabled} onAddCreature={onAddCreature} />
          <RechargeRows c={c} send={send} disabled={disabled} />
        </div>
      )}
      {tab === "spells" && <Spells state={state} c={c} block={block} blockOf={blockOf} send={send} disabled={disabled} onDefine={onDefine} />}
      {tab === "traits" && <Traits c={c} block={block} />}
    </div>
  );
}

/** Spent recharge abilities: the d6 typed from the table first, or rolled in the app. */
function RechargeRows({ c, send, disabled }: { c: Combatant; send: (cmd: Command) => void; disabled: boolean }) {
  const spent = (c.abilities ?? []).filter((a) => a.recharge && !a.recharge.ready);
  if (!spent.length) return null;
  return (
    <section className="cb-recharges">
      {spent.map((a) => (
        <div key={a.key} className="cb-recharge">
          <RollInput kind="dice" label={`Recharge ${a.name} (d6, ${a.recharge!.min}–6)`} notation="1d6" min={1} max={6} disabled={disabled}
            onSubmit={(roll) => send({ command: "roll_recharge", id: c.id, ability: a.key, roll })} />
          <Button small variant="ghost" disabled={disabled} onClick={() => send({ command: "set_recharge", id: c.id, ability: a.key, ready: true })}>Mark ready</Button>
        </div>
      ))}
    </section>
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

function Spells({ state, c, block, blockOf, send, disabled, onDefine }: {
  state: CombatState; c: Combatant; block: StatBlock | undefined; blockOf: (id: string) => StatBlock | undefined;
  send: (cmd: Command) => void; disabled: boolean; onDefine: (a: Ability) => void;
}) {
  const [casting, setCasting] = useState<TurnButton | null>(null);
  const lists = spellLists(block);
  const slots = Object.entries(c.spell_slots ?? {}).sort(([a], [b]) => Number(a) - Number(b));
  const resources = Object.entries(c.resources ?? {});
  const buttons = turnButtons(state, c, block).filter((b) => b.kind === "spell");
  const known = new Set(buttons.map((b) => b.ability!.name.toLowerCase()));
  // Spells the block names that aren't abilities of this combatant: cast as a ruling (the slot and action only).
  const loose = lists.flatMap((l) => l.lists.flatMap((x) => x.spells.map((s) => ({ name: s.replace(/\s*\(.*\)$/, ""), label: x.label }))))
    .filter((s) => !known.has(s.name.toLowerCase()));
  const byLevel = [...buttons].sort((x, y) => levelOf(x.ability!) - levelOf(y.ability!) || x.label.localeCompare(y.label));
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
        <p key={l.feature} className="cb-small muted">{l.feature}{l.dc != null ? ` · DC ${l.dc}` : ""}{l.attack != null ? ` · ${l.attack >= 0 ? "+" : ""}${l.attack} to hit` : ""}</p>
      ))}
      {byLevel.length > 0 && (
        <ul className="cb-spell-rows" aria-label="Spells">
          {byLevel.map((b) => {
            const a = b.ability!;
            const cov = b.coverage ?? "unlinked";
            const lvl = levelOf(a);
            return (
              <li key={a.key} className="cb-spell-row" data-spell={a.key}>
                <span className="cb-spell-name">{a.name}</span>
                <span className="cb-small muted">{lvl === 0 ? "Cantrip" : `L${lvl}`} · {b.detail}</span>
                <span className={`cb-cov cb-cov-${cov}`} title={COVERAGE_HELP[cov]}>{COVERAGE_WORD[cov]}</span>
                {(cov === "ruling" || cov === "unlinked") && <Button small variant="ghost" disabled={disabled} onClick={() => onDefine(a)}>Define effect…</Button>}
                {cov === "suggested" && <Button small variant="ghost" disabled={disabled} onClick={() => onDefine(a)}>Check and confirm…</Button>}
                <Button small variant="primary" disabled={disabled || Boolean(b.reason)} title={b.reason ?? `Cast ${a.name}`} onClick={() => setCasting(b)}>Cast</Button>
                {b.reason && <span className="cb-small cb-why">{b.reason}</span>}
              </li>
            );
          })}
        </ul>
      )}
      {loose.length > 0 && (
        <ul className="cb-spell-rows" aria-label="Other spells in the stat block">
          {loose.map((s) => (
            <li key={`${s.label}-${s.name}`} className="cb-spell-row">
              <span className="cb-spell-name">{s.name}</span>
              <span className="cb-small muted">{s.label}</span>
              <span className="cb-cov cb-cov-unlinked" title={COVERAGE_HELP.unlinked}>{COVERAGE_WORD.unlinked}</span>
              <Button small disabled={disabled || isDefeated(c)} onClick={() => send(asCommand({ command: "cast_spell", caster: c.id, targets: [],
                adhoc: { key: s.name.toLowerCase().replace(/\W+/g, "_"), name: s.name, level: 0, concentration: false, cast: "action", tier: "ruling", targeting: { kind: "self" }, delivery: { kind: "auto" } } }))}>Cast (ruling)</Button>
            </li>
          ))}
        </ul>
      )}
      {lists.length === 0 && slots.length === 0 && byLevel.length === 0 && <p className="muted cb-small">No spellcasting in this stat block.</p>}
      {casting && <CastSheet state={state} caster={c} button={casting} blockOf={blockOf} send={send} onDone={() => setCasting(null)} />}
    </div>
  );
}

const levelOf = (a: Ability): number => mechOf(a)?.level ?? spellOf(a)?.level ?? 0;

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
