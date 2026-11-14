import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Button } from "../../../src/components/Button";
import { Dialog } from "../../../src/components/controls/Dialog";
import { usePhone } from "../../../src/lib/phone";
import { ABILITY_NAMES, type AbilityName, type D20Input } from "./common";
import { byId, effectiveAc, isCharacter } from "./derive";
import { asCommand, type CastRow, type PendingCast, type RowOutcome, type SpellCombatant } from "./spellTypes";
import { saveBonus } from "./statRef";
import type { Combatant, Command, CombatState, StatBlock } from "./types";

const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);

/** A target's save bonus: the engine's `stats.saves`, else the stat block's. */
export function saveMod(c: Combatant, block: StatBlock | undefined, ability: AbilityName): number {
  const saves = (c as SpellCombatant).stats?.saves;
  const i = ABILITY_NAMES.indexOf(ability);
  if (saves && saves[i] != null) return saves[i]!;
  return saveBonus(block, ability);
}

type RowArgs = { roll?: D20Input; outcome?: RowOutcome; use_lr?: boolean };

const OUTCOME_WORD: Record<RowOutcome, string> = { failed: "Failed", saved: "Saved", hit: "Hit", miss: "Miss", crit: "Critical hit" };

/**
 * The resolution sheet (0.3.2, user decision "table rolls first"): one row per target, each led by a number field
 * for the roll made at the table (a number pad on phones; Enter moves to the next row), with [Failed] [Saved] (or
 * [Hit] [Miss]) to set the outcome outright. "Roll for me" is the secondary way, for people without dice, and
 * [Roll all creature saves] is an optional helper. A player character's row waits for its player, who types their
 * roll in their own Combat tab; the DM can type it instead. Then the damage: a typed total or "Roll for me", and
 * [Apply]: one roll, halved for those who saved.
 */
export function ResolutionSheet({ state, cast, blockOf, send, onClose }: {
  state: CombatState;
  cast: PendingCast;
  blockOf: (id: string) => StatBlock | undefined;
  send: (cmd: Command) => void;
  onClose: () => void;
}) {
  const phone = usePhone();
  const inputs = useRef<(HTMLInputElement | null)[]>([]);
  const [focusNext, setFocusNext] = useState<number | null>(null);
  const [damage, setDamage] = useState("");
  const [cancelling, setCancelling] = useState(false);
  const caster = byId(state, cast.caster);
  const waiting = cast.rows.filter((r) => !r.outcome);
  const creaturesWaiting = waiting.filter((r) => r.kind === "save" && !isCharacter(byId(state, r.target)!));
  const d = cast.delivery;
  const head = d.kind === "save"
    ? `DC ${cast.dc ?? "?"} ${d.ability.toUpperCase()} save${d.success === "half" ? " · half on a success" : ""}`
    : d.kind === "attack" ? `Attack ${cast.attack != null ? signed(cast.attack) : ""} vs AC` : "";

  useEffect(() => {
    if (focusNext === null) return;
    const i = cast.rows.findIndex((r, k) => k >= focusNext && !r.outcome);
    const el = i >= 0 ? inputs.current[i] : document.querySelector<HTMLInputElement>(".cb-res-damage input");
    el?.focus();
    setFocusNext(null);
  }, [cast.rows, focusNext]);

  const resolve = (row: CastRow, args: RowArgs, index: number) => {
    send(asCommand({ command: "resolve_cast_row", cast: cast.id, target: row.target, ...args }));
    setFocusNext(index + 1);
  };
  const rollAll = () => {
    for (const r of creaturesWaiting) {
      const t = byId(state, r.target)!;
      send(asCommand({ command: "resolve_cast_row", cast: cast.id, target: r.target, roll: { kind: "app", modifier: saveMod(t, blockOf(t.id), r.ability ?? "dex") } }));
    }
  };
  const apply = (typed?: number) => {
    send(asCommand({ command: "apply_cast", cast: cast.id, ...(typed !== undefined ? { damage: { kind: "typed", total: typed } } : cast.damage ? { damage: { kind: "app", notation: cast.damage.dice, ...(cast.rows.some((r) => r.outcome === "crit") ? { critical: true } : {}) } } : {}) }));
    onClose();
  };
  const typedDamage = damage.trim() !== "" && /^\d+$/.test(damage.trim()) ? Number(damage.trim()) : null;

  return (
    <Dialog title={`${cast.name}${caster ? ` · ${caster.label}` : ""}`} onClose={onClose} full={phone} className="cb-sheet cb-side cb-resolution"
      footer={cancelling ? <>
        <span className="cb-small">Cancel {cast.name}?</span>
        <Button onClick={() => { send(asCommand({ command: "cancel_cast", cast: cast.id })); onClose(); }}>Slot stays spent</Button>
        {cast.slot !== undefined && <Button onClick={() => { send(asCommand({ command: "cancel_cast", cast: cast.id, refund: true })); onClose(); }}>Give the slot back</Button>}
        <Button variant="ghost" onClick={() => setCancelling(false)}>Keep it</Button>
      </> : <>
        <Button variant="ghost" onClick={() => setCancelling(true)}>Cancel cast</Button>
        <Button onClick={onClose}>Later</Button>
      </>}>
      <div className="cb-res" data-cast={cast.id}>
        {head && <p className="cb-cast-meta">{head}</p>}
        <p className="cb-small muted">Type the rolls made at the table. Enter goes to the next row.</p>
        <ul className="cb-res-rows">
          {cast.rows.map((row, i) => {
            const t = byId(state, row.target);
            if (!t) return null;
            const pc = isCharacter(t);
            const mod = row.kind === "save" ? saveMod(t, blockOf(t.id), row.ability ?? "dex") : cast.attack ?? 0;
            return (
              <ResRow key={row.target} row={row} target={t} pc={pc} mod={mod} ac={effectiveAc(state, t).total}
                inputRef={(el) => { inputs.current[i] = el; }}
                onResolve={(args) => resolve(row, args, i)} />
            );
          })}
        </ul>
        {creaturesWaiting.length > 0 && (
          <Button small icon="dice" variant="ghost" onClick={rollAll}>Roll all creature saves ({creaturesWaiting.length})</Button>
        )}
        <section className="cb-res-damage" aria-label="Damage">
          {cast.damage ? (
            <form className="cb-roll" onSubmit={(e) => { e.preventDefault(); if (typedDamage !== null && waiting.length === 0) apply(typedDamage); }}>
              <label className="cb-roll-label" htmlFor={`dmg-${cast.id}`}>Damage {cast.damage.dice}{cast.damage.type ? ` ${cast.damage.type}` : ""}{cast.damage.half_on_save ? " · half for those who saved" : ""}</label>
              <input id={`dmg-${cast.id}`} className="input cb-roll-box" inputMode="numeric" enterKeyHint="done" placeholder="Table total" aria-label={`${cast.name} damage: typed total`}
                value={damage} onChange={(e) => setDamage(e.target.value)} disabled={waiting.length > 0} />
              <Button small variant="primary" type="submit" disabled={waiting.length > 0 || typedDamage === null}>Apply</Button>
              <Button small icon="dice" variant="ghost" disabled={waiting.length > 0} onClick={() => apply()}>Roll for me</Button>
            </form>
          ) : (
            <Button variant="primary" disabled={waiting.length > 0} onClick={() => apply()}>Apply</Button>
          )}
          {waiting.length > 0 && <p className="cb-small muted" role="status">Waiting on {waiting.length} roll{waiting.length === 1 ? "" : "s"}.</p>}
        </section>
      </div>
    </Dialog>
  );
}

function ResRow({ row, target, pc, mod, ac, inputRef, onResolve }: {
  row: CastRow; target: Combatant; pc: boolean; mod: number; ac: number | null;
  inputRef: (el: HTMLInputElement | null) => void;
  onResolve: (args: RowArgs) => void;
}) {
  const [text, setText] = useState("");
  const [override, setOverride] = useState(false);
  const save = row.kind === "save";
  const n = text.trim() !== "" && /^-?\d+$/.test(text.trim()) ? Number(text.trim()) : null;
  const submit = () => { if (n !== null) { onResolve({ roll: { kind: "typed", total: n } }); setText(""); } };
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => { if (e.key === "Enter") { e.preventDefault(); submit(); } };
  const lr = target.legendary_resistance;
  const label = `${target.label}: ${save ? `${row.ability?.toUpperCase()} save` : "attack roll"}`;
  if (row.outcome) {
    return (
      <li className={`cb-res-row done ${row.outcome}`} data-target={target.id}>
        <span className="cb-res-name">{target.label}</span>
        <span className={`chip ${row.outcome === "failed" || row.outcome === "hit" || row.outcome === "crit" ? "danger" : "success"}`}>{OUTCOME_WORD[row.outcome]}</span>
        {row.roll && <span className="cb-small muted">{row.roll.total}{row.roll.source === "app" ? ` (rolled ${row.roll.dice.join(", ")})` : " (table)"}</span>}
        {row.lr_used && <span className="cb-small muted">Legendary Resistance</span>}
        {row.outcome === "failed" && lr && lr.remaining > 0 && <Button small onClick={() => onResolve({ use_lr: true })}>Use Legendary Resistance ({lr.remaining})</Button>}
      </li>
    );
  }
  const waitingForPlayer = pc && !override;
  return (
    <li className="cb-res-row" data-target={target.id}>
      <span className="cb-res-name">{target.label}{!save && ac != null ? <span className="muted cb-small"> · AC {ac}</span> : null}</span>
      {waitingForPlayer ? (
        <span className="cb-res-wait">
          <span className="cb-small" role="status">Waiting for {target.label}'s player</span>
          <Button small variant="ghost" onClick={() => setOverride(true)}>Type it</Button>
        </span>
      ) : (
        <span className="cb-res-entry">
          <input ref={inputRef} className="input cb-roll-box" inputMode="numeric" enterKeyHint="next" placeholder="Table roll" aria-label={`${label}: typed result`}
            value={text} onChange={(e) => setText(e.target.value)} onKeyDown={onKey} />
          <Button small icon="dice" variant="ghost" title={`Roll d20${signed(mod)} in the app`} onClick={() => onResolve({ roll: { kind: "app", modifier: mod } })}>Roll for me</Button>
        </span>
      )}
      <span className="cb-res-set">
        {save ? <>
          <Button small onClick={() => onResolve({ outcome: "failed" })}>Failed</Button>
          <Button small onClick={() => onResolve({ outcome: "saved" })}>Saved</Button>
        </> : <>
          <Button small onClick={() => onResolve({ outcome: "hit" })}>Hit</Button>
          <Button small onClick={() => onResolve({ outcome: "miss" })}>Miss</Button>
          <Button small variant="ghost" onClick={() => onResolve({ outcome: "crit" })}>Crit</Button>
        </>}
      </span>
      {!pc && <span className="cb-small muted cb-res-mod">{save ? `save ${signed(mod)}` : `to hit ${signed(mod)}`}</span>}
    </li>
  );
}
