import "./spells.css";
import { useState } from "react";
import { Button } from "../../../src/components/Button";
import { EffectDialog, draftSpecs, emptyDraft } from "./dialogs";
import { CastSheet, type CastDone } from "./CastSheet";
import { asCommand, COVERAGE_HELP, COVERAGE_WORD, type SpellMech } from "./spellTypes";
import { SummonPanel, summonOptions, type AddPreset } from "./SummonPanel";
import { GROUP_WORD, reactionButtons, turnButtons, type TurnButton, type TurnGroup } from "./turn";
import type { Combatant, Command, CombatState, StatBlock } from "./types";

const GROUPS: TurnGroup[] = ["action", "bonus_action", "reaction", "legendary", "free"];

/** Action, bonus action and reaction as pips: filled = ready; a tap flips one (the DM's correction). */
export function EconomyPips({ c, send, disabled }: { c: Combatant; send: (cmd: Command) => void; disabled: boolean }) {
  const e = c.economy ?? { action: false, bonus_action: false, reaction: false };
  const pip = (key: "action" | "bonus_action" | "reaction", word: string) => (
    <button type="button" className={`cb-pip${e[key] ? " spent" : ""}`} aria-pressed={!e[key]} disabled={disabled}
      aria-label={`${word}: ${e[key] ? "spent" : "ready"}`} title={e[key] ? `${word} spent: tap to mark ready` : `${word} ready: tap to mark spent`}
      onClick={() => send({ command: "set_economy", id: c.id, [key]: !e[key] })}>
      <i aria-hidden="true" />{word}
    </button>
  );
  return <div className="cb-pips" role="group" aria-label="Action economy">{pip("action", "Action")}{pip("bonus_action", "Bonus")}{pip("reaction", "Reaction")}</div>;
}

/** Slot pips per level: ● left, ○ spent. */
export function SlotPips({ c }: { c: Combatant }) {
  const slots = Object.entries(c.spell_slots ?? {}).sort(([a], [b]) => Number(a) - Number(b));
  if (!slots.length) return null;
  return (
    <div className="cb-slotpips" aria-label="Spell slots">
      {slots.map(([l, p]) => (
        <span key={l} className="cb-slotpip" aria-label={`Level ${l}: ${p.remaining} of ${p.max} left`}>
          <b>L{l}</b>{Array.from({ length: p.max }, (_, i) => <i key={i} className={i < p.remaining ? "on" : ""} aria-hidden="true" />)}
        </span>
      ))}
    </div>
  );
}

function TurnBtn({ b, onPress, disabled }: { b: TurnButton; onPress: () => void; disabled: boolean }) {
  return (
    <li className="cb-tb-item">
      <button type="button" className={`cb-tb cb-tb-${b.kind}`} disabled={disabled || Boolean(b.reason)} onClick={onPress}
        title={b.reason ?? (b.coverage ? COVERAGE_HELP[b.coverage] : undefined)} data-turn-button={b.key}
        aria-describedby={b.reason ? `why-${b.key}` : undefined}>
        <span className="cb-tb-name">{b.label}</span>
        {b.detail && <span className="cb-tb-detail">{b.detail}</span>}
        {b.coverage && b.coverage !== "auto" && <span className={`cb-cov cb-cov-${b.coverage}`}>{COVERAGE_WORD[b.coverage]}</span>}
      </button>
      {b.reason && b.reason !== "Passive" && <span id={`why-${b.key}`} className="cb-tb-why">{b.reason}</span>}
    </li>
  );
}

/**
 * The turn panel (0.3.2, design 4 and the user's "less free text, more mechanics in buttons"): the active creature's
 * economy pips, its concentration with [End] and its slot pips, then a button for EVERY action it has, grouped
 * Action / Bonus / Reaction: statblock attacks ("Scimitar +4 · 1d6+2"), saves and breath weapons ("Fire Breath 5–6"),
 * spells, and Dodge, Dash, Disengage and Help. Each spends what it costs, so the DM sees what is left. A disabled
 * button says why. [More…] keeps the old custom effect. While the reaction is unspent, a sticky strip offers the
 * reactions (Shield, Counterspell, an opportunity attack).
 */
export function TurnPanel({ state, c, block, blockOf, send, disabled, onAddCreature }: {
  state: CombatState; c: Combatant; block: StatBlock | undefined; blockOf: (id: string) => StatBlock | undefined;
  send: (cmd: Command) => void; disabled: boolean; onAddCreature?: (p: AddPreset) => void;
}) {
  const [sheet, setSheet] = useState<TurnButton | null>(null);
  const [more, setMore] = useState(false);
  const [summon, setSummon] = useState<{ mech: SpellMech; slot?: number } | null>(null);
  const buttons = turnButtons(state, c, block);
  const reactions = reactionButtons(buttons);
  const e = c.economy ?? { action: false, bonus_action: false, reaction: false };
  const current = state.current === c.id;
  const press = (b: TurnButton) => {
    if (b.kind === "standard" && b.standard !== "help") { send(asCommand({ command: "standard_action", id: c.id, action: b.standard! })); return; }
    if (b.kind === "use") { send({ command: "use_ability", id: c.id, ability: b.ability!.key }); return; }
    if (b.kind === "spell" || b.kind === "attack" || b.kind === "save" || b.standard === "help") setSheet(b);
  };
  const done = (r: CastDone) => {
    setSheet(null);
    if (r.sent && r.mech && summonOptions(r.mech, r.slot)) setSummon({ mech: r.mech, slot: r.slot });
  };
  return (
    <div className="cb-turn" data-turn-panel={c.id}>
      <div className="cb-turn-head">
        <span className="cb-overline">{current ? "Its turn" : "Not its turn: reactions and legendary actions"}</span>
        <EconomyPips c={c} send={send} disabled={disabled} />
        {c.concentration && (
          <span className="cb-conc-chip">● {c.concentration.name}
            <Button small variant="danger" disabled={disabled} onClick={() => send({ command: "end_concentration", id: c.id })} aria-label={`End concentration on ${c.concentration.name}`}>End</Button>
          </span>
        )}
        <SlotPips c={c} />
      </div>
      {summon && <SummonPanel mech={summon.mech} slot={summon.slot} caster={c} onClose={() => setSummon(null)} onAdd={(p) => { onAddCreature?.(p); }} />}
      {!e.reaction && reactions.length > 0 && (
        <div className="cb-react-strip" role="toolbar" aria-label="Reactions ready">
          <span className="cb-overline">Reaction ready</span>
          {reactions.map((b) => <Button key={b.key} small disabled={disabled} onClick={() => press(b)}>{b.label}</Button>)}
        </div>
      )}
      {GROUPS.map((g) => {
        const list = buttons.filter((b) => b.group === g);
        if (!list.length) return null;
        return (
          <section key={g} className="cb-tb-group" aria-label={GROUP_WORD[g]}>
            <h4 className="cb-group-title">{GROUP_WORD[g]}{g === "action" && e.action ? " · spent" : g === "bonus_action" && e.bonus_action ? " · spent" : g === "reaction" && e.reaction ? " · spent" : ""}</h4>
            <ul className="cb-tb-list">{list.map((b) => <TurnBtn key={b.key} b={b} disabled={disabled} onPress={() => press(b)} />)}</ul>
          </section>
        );
      })}
      <Button small variant="ghost" icon="plus" disabled={disabled} onClick={() => setMore(true)}>More… (custom effect)</Button>
      {sheet && <CastSheet state={state} caster={c} block={block} button={sheet} blockOf={blockOf} send={send} onDone={done} />}
      {more && (
        <EffectDialog title={`Custom effect from ${c.label}`} combatants={state.combatants} initial={emptyDraft(c.id, "")}
          onDone={(r) => { setMore(false); if (r) for (const effect of draftSpecs(r.draft)) send({ command: "add_effect", effect }); }} />
      )}
    </div>
  );
}
