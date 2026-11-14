import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Button } from "../../../src/components/Button";
import { Dialog } from "../../../src/components/controls/Dialog";
import { usePhone } from "../../../src/lib/phone";
import { ABILITY_NAMES, type AbilityName } from "./common";
import { byId, effectiveAc, isCharacter } from "./derive";
import { castAmounts, castName, halfOnSave, rowAbility, rowKind, type CastAmount } from "./spellTypes";
import { saveBonus } from "./statRef";
import type { AmountRoll, CastRow, Combatant, Command, CombatState, PendingCast, RowOutcome, RowRoll, StatBlock } from "./types";

const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);

/** A target's save bonus: the engine's `stats.saves`, else the stat block's. */
export function saveMod(c: Combatant, block: StatBlock | undefined, ability: AbilityName): number {
  const saves = c.stats?.saves;
  const i = ABILITY_NAMES.indexOf(ability);
  if (saves && saves[i] != null) return saves[i]!;
  return saveBonus(block, ability);
}

type RowArgs = { roll: RowRoll; use_lr?: boolean };

const OUTCOME_WORD: Record<RowOutcome, string> = { failed: "Failed", saved: "Saved", hit: "Hit", miss: "Miss", crit: "Critical hit", lands: "Lands" };
const AMOUNT_WORD: Record<CastAmount["kind"], string> = { damage: "Damage", heal: "Healing", temp_hp: "Temporary hit points" };

const rowKey = (r: CastRow) => `${r.target}:${r.ray ?? 0}`;

/**
 * The resolution sheet (0.3.2, user decision "table rolls first"): one row per target, each led by a number field
 * for the roll made at the table (a number pad on phones; Enter moves to the next row), with [Failed] [Saved] (or
 * [Hit] [Miss]) to set the outcome outright. "Roll for me" is the secondary way, for people without dice, and
 * [Roll all creature saves] is an optional helper. A player character's row waits for its player, who types their
 * roll in their own Combat tab; the DM can type it instead. Then each amount (damage, healing): a typed total or
 * "Roll for me", and [Apply]: one roll, halved for those who saved.
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
  const amounts = castAmounts(cast);
  const [typed, setTyped] = useState<string[]>(() => amounts.map(() => ""));
  const [cancelling, setCancelling] = useState(false);
  const caster = byId(state, cast.caster);
  const kind = rowKind(cast);
  const ability = rowAbility(cast);
  const waiting = cast.rows.filter((r) => !r.outcome);
  const creaturesWaiting = waiting.filter((r) => kind === "save" && !isCharacter(byId(state, r.target)!));
  const head = kind === "save"
    ? `DC ${cast.spec.dc ?? "?"} ${ability?.toUpperCase()} save${halfOnSave(cast) ? " · half on a success" : ""}`
    : kind === "attack" ? `Attack ${cast.spec.attack != null ? signed(cast.spec.attack) : ""} vs AC` : "";

  useEffect(() => {
    if (focusNext === null) return;
    const i = cast.rows.findIndex((r, k) => k >= focusNext && !r.outcome);
    const el = i >= 0 ? inputs.current[i] : document.querySelector<HTMLInputElement>(".cb-res-damage input");
    el?.focus();
    setFocusNext(null);
  }, [cast.rows, focusNext]);

  const resolve = (row: CastRow, args: RowArgs, index: number) => {
    send({ command: "resolve_cast_row", cast: cast.id, target: row.target, ...(row.ray != null ? { ray: row.ray } : {}), ...args });
    setFocusNext(index + 1);
  };
  // The engine rolls with the save bonus, Bless dice and advantage it derives.
  const rollAll = () => { for (const r of creaturesWaiting) send({ command: "resolve_cast_row", cast: cast.id, target: r.target, ...(r.ray != null ? { ray: r.ray } : {}), roll: { kind: "app" } }); };
  const total = (t: string) => (/^\d+$/.test(t.trim()) ? Number(t.trim()) : null);
  const allTyped = amounts.every((_, i) => total(typed[i] ?? "") !== null);
  const apply = (rolled: "typed" | "app") => {
    const list: AmountRoll[] = amounts.map((_, i) => (rolled === "typed" ? { kind: "typed", total: total(typed[i] ?? "")! } : { kind: "app" }));
    send({ command: "apply_cast", cast: cast.id, ...(list.length ? { amounts: list } : {}) });
    onClose();
  };

  return (
    <Dialog title={`${castName(cast)}${caster ? ` · ${caster.label}` : ""}`} onClose={onClose} full={phone} className="cb-sheet cb-side cb-resolution"
      footer={cancelling ? <>
        <span className="cb-small">Cancel {castName(cast)}?</span>
        <Button onClick={() => { send({ command: "cancel_cast", cast: cast.id }); onClose(); }}>Slot stays spent</Button>
        {cast.slot !== undefined && <Button onClick={() => { send({ command: "cancel_cast", cast: cast.id, refund: true }); onClose(); }}>Give the slot back</Button>}
        <Button variant="ghost" onClick={() => setCancelling(false)}>Keep it</Button>
      </> : <>
        <Button variant="ghost" onClick={() => setCancelling(true)}>Cancel cast</Button>
        <Button onClick={onClose}>Later</Button>
      </>}>
      <div className="cb-res" data-cast={cast.id}>
        {head && <p className="cb-cast-meta">{head}</p>}
        {kind !== "auto" && <p className="cb-small muted">Type the rolls made at the table. Enter goes to the next row.</p>}
        <ul className="cb-res-rows">
          {cast.rows.map((row, i) => {
            const t = byId(state, row.target);
            if (!t) return null;
            const mod = kind === "save" ? saveMod(t, blockOf(t.id), ability ?? "dex") : cast.spec.attack ?? 0;
            return (
              <ResRow key={rowKey(row)} row={row} kind={kind} ability={ability} target={t} pc={isCharacter(t)} mod={mod} ac={effectiveAc(state, t).total}
                inputRef={(el) => { inputs.current[i] = el; }}
                onResolve={(args) => resolve(row, args, i)} />
            );
          })}
        </ul>
        {creaturesWaiting.length > 0 && (
          <Button small icon="dice" variant="ghost" onClick={rollAll}>Roll all creature saves ({creaturesWaiting.length})</Button>
        )}
        <section className="cb-res-damage" aria-label="Amounts">
          {amounts.length > 0 ? (
            <form className="cb-res-amounts" onSubmit={(e) => { e.preventDefault(); if (allTyped && waiting.length === 0) apply("typed"); }}>
              {amounts.map((am, i) => (
                <div key={i} className="cb-roll">
                  <label className="cb-roll-label" htmlFor={`amt-${cast.id}-${i}`}>{AMOUNT_WORD[am.kind]} {am.dice}{am.type ? ` ${am.type}` : ""}{am.kind === "damage" && halfOnSave(cast) ? " · half for those who saved" : ""}</label>
                  <input id={`amt-${cast.id}-${i}`} className="input cb-roll-box" inputMode="numeric" enterKeyHint="done" placeholder="Table total"
                    aria-label={`${castName(cast)} ${AMOUNT_WORD[am.kind].toLowerCase()}: typed total`}
                    value={typed[i] ?? ""} onChange={(e) => setTyped(typed.map((x, j) => (j === i ? e.target.value : x)))} disabled={waiting.length > 0} />
                </div>
              ))}
              <div className="cb-tool-row">
                <Button small variant="primary" type="submit" disabled={waiting.length > 0 || !allTyped}>Apply</Button>
                <Button small icon="dice" variant="ghost" disabled={waiting.length > 0} onClick={() => apply("app")}>Roll for me</Button>
              </div>
            </form>
          ) : (
            <Button variant="primary" disabled={waiting.length > 0} onClick={() => apply("app")}>Apply</Button>
          )}
          {waiting.length > 0 && <p className="cb-small muted" role="status">Waiting on {waiting.length} roll{waiting.length === 1 ? "" : "s"}.</p>}
        </section>
      </div>
    </Dialog>
  );
}

function ResRow({ row, kind, ability, target, pc, mod, ac, inputRef, onResolve }: {
  row: CastRow; kind: "save" | "attack" | "auto"; ability?: AbilityName; target: Combatant; pc: boolean; mod: number; ac: number | null;
  inputRef: (el: HTMLInputElement | null) => void;
  onResolve: (args: RowArgs) => void;
}) {
  const [text, setText] = useState("");
  const save = kind === "save";
  const n = text.trim() !== "" && /^-?\d+$/.test(text.trim()) ? Number(text.trim()) : null;
  const submit = () => { if (n !== null) { onResolve({ roll: { kind: "typed", total: n } }); setText(""); } };
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => { if (e.key === "Enter") { e.preventDefault(); submit(); } };
  const lr = target.legendary_resistance;
  const label = `${target.label}${row.ray != null ? ` (ray ${row.ray + 1})` : ""}: ${save ? `${ability?.toUpperCase()} save` : "attack roll"}`;
  if (row.outcome) {
    return (
      <li className={`cb-res-row done ${row.outcome}`} data-target={target.id}>
        <span className="cb-res-name">{target.label}</span>
        <span className={`chip ${row.outcome === "saved" || row.outcome === "miss" ? "success" : "danger"}`}>{OUTCOME_WORD[row.outcome]}</span>
        {row.roll && <span className="cb-small muted">{row.roll.total}{row.roll.source === "app" ? ` (rolled ${row.roll.dice.join(", ")})` : " (table)"}</span>}
        {row.used_lr && <span className="cb-small muted">Legendary Resistance</span>}
        {row.outcome === "failed" && lr && lr.remaining > 0 && <Button small onClick={() => onResolve({ roll: { kind: "outcome", outcome: "saved" }, use_lr: true })}>Use Legendary Resistance ({lr.remaining})</Button>}
      </li>
    );
  }
  const waitingForPlayer = pc && save;
  return (
    <li className="cb-res-row" data-target={target.id}>
      <span className="cb-res-name">{target.label}{!save && ac != null ? <span className="muted cb-small"> · AC {ac}</span> : null}</span>
      {waitingForPlayer ? (
        <span className="cb-res-wait">
          <span className="cb-small" role="status">Waiting for {target.label}'s player (or press their result)</span>
        </span>
      ) : (
        <span className="cb-res-entry">
          {/* 0.3.5: a save is pass or fail: roll at the table and press Failed or Saved; a number box only for attack totals (vs AC). */}
          {!save && <input ref={inputRef} className="input cb-roll-box" inputMode="numeric" enterKeyHint="next" placeholder="Table roll" aria-label={`${label}: typed result`}
            value={text} onChange={(e) => setText(e.target.value)} onKeyDown={onKey} />}
          <Button small icon="dice" variant="ghost" title={`Roll d20${signed(mod)} in the app`} onClick={() => onResolve({ roll: { kind: "app" } })}>Roll for me</Button>
        </span>
      )}
      <span className="cb-res-set">
        {save ? <>
          <Button small onClick={() => onResolve({ roll: { kind: "outcome", outcome: "failed" } })}>Failed</Button>
          <Button small onClick={() => onResolve({ roll: { kind: "outcome", outcome: "saved" } })}>Saved</Button>
        </> : <>
          <Button small onClick={() => onResolve({ roll: { kind: "outcome", outcome: "hit" } })}>Hit</Button>
          <Button small onClick={() => onResolve({ roll: { kind: "outcome", outcome: "miss" } })}>Miss</Button>
          <Button small variant="ghost" onClick={() => onResolve({ roll: { kind: "outcome", outcome: "crit" } })}>Crit</Button>
        </>}
      </span>
      {!pc && <span className="cb-small muted cb-res-mod">{save ? `save ${signed(mod)}` : `to hit ${signed(mod)}`}</span>}
    </li>
  );
}
