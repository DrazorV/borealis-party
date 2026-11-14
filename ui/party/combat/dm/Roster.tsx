import { useState } from "react";
import { Button, IconButton } from "../../../src/components/Button";
import { HpBar } from "./HpBar";
import { conditionWord } from "./common";
import { conditionsOf, effectiveAc, effectiveInitiative, effectsOn, followDepth, isCharacter, isDefeated, leader, poolOf, rosterOrder, byId } from "./derive";
import { BandChip } from "./BandChip";
import { playerSide } from "./privacy";
import type { Combatant, Command, CombatState } from "./types";

/**
 * The roster (C1–C7): everyone sorted by initiative, PCs in the same list. A PC with no initiative is not in this
 * fight yet (blank means absent, C1). A follower shows its leader's initiative (C3). A creature's row has the quick
 * −5/+5, its HP, its economy, and reveal/hide (D20 b). A player's row (a PC, or a creature its player controls) shows
 * only its name, initiative, AC, conditions, effects and concentration: the DM never sees players' HP (./privacy.ts).
 */
export function Roster({ state, selected, onSelect, send, disabled, onAdd, joinable = 0, onAddMember }: {
  state: CombatState;
  selected: string | null;
  onSelect: (id: string) => void;
  send: (c: Command) => void;
  disabled: boolean;
  onAdd: () => void;
  /** 0.3.5 (A2): how many party characters aren't fighting; "Party member" adds one. */
  joinable?: number;
  onAddMember?: () => void;
}) {
  const list = rosterOrder(state);
  const alive = state.combatants.filter((c) => !c.removed && c.kind === "creature" && !isDefeated(c)).length;
  const total = state.combatants.filter((c) => !c.removed && c.kind === "creature").length;
  return (
    <section className="cb-panel cb-roster" aria-label="Combatants">
      <div className="cb-panel-head">
        <strong>Combatants</strong>
        <span className="muted cb-small">{alive}/{total} creatures up</span>
        <Button small icon="plus" onClick={onAdd} disabled={disabled}>Creature</Button>
        {joinable > 0 && onAddMember && <Button small icon="plus" onClick={onAddMember} disabled={disabled} title="A party member who isn't in this fight">Party member</Button>}
      </div>
      <ol className="cb-rows">
        {list.map((c) => <Row key={c.id} c={c} state={state} selected={selected === c.id} onSelect={onSelect} send={send} disabled={disabled} />)}
      </ol>
      <p className="muted cb-small cb-pad">Sorted by initiative. A player's character with no initiative is not in this fight.</p>
    </section>
  );
}

/** "Bless, from Aria": who an effect came from, on its chip. */
function fxTitle(state: CombatState, e: { name: string; source?: string }): string {
  const from = byId(state, e.source);
  return from ? `${e.name}, from ${from.label}` : e.name;
}

function InitBox({ c, state, send, disabled }: { c: Combatant; state: CombatState; send: (c: Command) => void; disabled: boolean }) {
  const [text, setText] = useState<string | null>(null);
  const lead = leader(state, c);
  if (lead) {
    return <span className="cb-init-linked" title={`Acts right after ${lead.label}`}>{effectiveInitiative(state, c) ?? "–"} ↳</span>;
  }
  const value = text ?? (c.initiative == null ? "" : String(c.initiative));
  const commit = () => {
    if (text === null) return;
    const t = text.trim();
    setText(null);
    if (t === "" && c.initiative != null) send({ command: "clear_initiative", id: c.id });
    else if (t !== "" && Number.isInteger(Number(t)) && Number(t) !== c.initiative) send({ command: "set_initiative", id: c.id, roll: { kind: "typed", total: Number(t) } });
  };
  return (
    <input className="input cb-init" inputMode="numeric" aria-label={`${c.label} initiative`} value={value} placeholder="–" disabled={disabled}
      title={isCharacter(c) ? "The player's initiative; blank = not in this fight" : `Initiative (modifier ${(c.initiative_bonus ?? 0) >= 0 ? "+" : ""}${c.initiative_bonus ?? 0})`}
      onClick={(e) => e.stopPropagation()} onChange={(e) => setText(e.target.value)} onBlur={commit}
      onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} />
  );
}

function Row({ c, state, selected, onSelect, send, disabled }: {
  c: Combatant; state: CombatState; selected: boolean; onSelect: (id: string) => void; send: (c: Command) => void; disabled: boolean;
}) {
  const pc = isCharacter(c);
  const mine = playerSide(c);
  const current = state.current === c.id;
  const out = pc && c.initiative == null;
  const effects = effectsOn(state, c.id);
  const conds = conditionsOf(state, c.id);
  const e = c.economy ?? { action: false, bonus_action: false, reaction: false };
  const dead = isDefeated(c);
  const depth = Math.min(followDepth(state, c), 3);
  const owner = byId(state, c.owner_id);
  const pool = pc ? null : poolOf(state, c.id)?.[1];
  const cls = [`cb-row cb-depth-${depth}`, c.owner_id && "pet", pc && "pc", mine && "player-side", selected && "selected", current && "active", dead && "dead", out && "out", c.absent && "out"].filter(Boolean).join(" ");
  return (
    <li className={cls} data-combatant={c.id} aria-current={current ? "true" : undefined}>
      <button type="button" className="cb-row-main" onClick={() => onSelect(c.id)} aria-label={`${c.label}${current ? ", current turn" : ""}`}>
        <span className="cb-row-name">
          <span className="cb-name-text">{c.label}</span>
          {c.concentration && <span className="cb-conc" title={`Concentrating on ${c.concentration.name}`}>CONC</span>}
          {effects.length > 0 && <span className="cb-fx-count" title={effects.map((x) => x.name).join(", ")}>✦{effects.length}</span>}
          {!pc && !c.visible && <span className="cb-hidden-mark" title="Hidden from players">Hidden</span>}
          {owner && <span className="cb-pet-mark" title={`Controlled by ${c.controller?.kind === "player" ? `${owner.label}'s player` : "the DM"}`}>{c.controller?.kind === "player" ? "Player's" : "Summon"}</span>}
        </span>
        <span className="cb-row-meta">
          {pc && <span>Player · {c.absent ? "absent" : out ? "no initiative" : "PC"}</span>}
          {mine && <BandChip c={c} />}
          <span>AC <b>{effectiveAc(state, c).total ?? "—"}</b></span>
          {owner && <span>↳ {owner.label}</span>}
          {pool && (
            <span className="cb-la-mini" aria-label={`${c.label} legendary actions: ${pool.remaining} of ${pool.max} left`}
              title={pool.members.length > 1 ? "Legendary actions (linked pool)" : "Legendary actions"}>
              LA {Array.from({ length: pool.max }, (_, i) => (i < pool.remaining ? "●" : "○")).join("")}
            </span>
          )}
          {conds.length > 0 && <span className="cb-row-conds">{conds.map(conditionWord).join(", ")}</span>}
        </span>
        {effects.length > 0 && (
          <span className="cb-row-fx" aria-label={`${c.label} effects`}>
            {[...new Set(effects.map((x) => x.name))].slice(0, 4).map((name) => <span key={name} className="cb-fx-chip" title={fxTitle(state, effects.find((x) => x.name === name)!)}>{name}</span>)}
          </span>
        )}
      </button>
      <div className="cb-row-side">
        <InitBox c={c} state={state} send={send} disabled={disabled} />
        {!pc && !(c.controller?.kind === "player") && (
          <IconButton icon={c.visible ? "globe" : "lock"} label={c.visible ? `Hide ${c.label} from players` : `Reveal ${c.label} to players`}
            disabled={disabled || dead} onClick={() => send({ command: c.visible ? "hide" : "reveal", id: c.id })} />
        )}
      </div>
      {!mine && <div className="cb-row-hp">
        <button type="button" className="cb-quick" disabled={disabled || dead} aria-label={`5 damage to ${c.label}`}
          onClick={() => send({ command: "damage", target: c.id, parts: [{ amount: { kind: "typed", total: 5 } }] })}>−5</button>
        <HpBar current={c.hp.current} max={c.hp.max} temp={c.hp.temp} label={`${c.label} hit points`} />
        <button type="button" className="cb-quick" disabled={disabled || dead} aria-label={`5 healing to ${c.label}`}
          onClick={() => send({ command: "heal", target: c.id, amount: { kind: "typed", total: 5 } })}>+5</button>
      </div>}
      {state.status === "active" && !mine && (
        <div className="cb-econ-mini" aria-label={`${c.label} economy`}>
          <span className={e.action ? "spent" : ""} title={e.action ? "Action spent" : "Action ready"}>A</span>
          <span className={e.bonus_action ? "spent" : ""} title={e.bonus_action ? "Bonus action spent" : "Bonus action ready"}>B</span>
          <span className={e.reaction ? "spent" : ""} title={e.reaction ? "Reaction spent" : "Reaction ready"}>R</span>
        </div>
      )}
    </li>
  );
}
