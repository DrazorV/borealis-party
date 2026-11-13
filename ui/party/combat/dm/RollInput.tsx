import { useId, useState, type FormEvent } from "react";
import { Button } from "../../../src/components/Button";
import { tryParseNotation } from "../../../src/lib/dice";
import type { D20Input, DiceInput, RollMode } from "./common";

/**
 * Every roll, two ways (D21, D43): type the result of the dice rolled at the table, or roll in the app (the server
 * rolls). Typed always works. For a d20 test the box takes the total; for dice it takes a total, or notation such as
 * `2d6+3`, which the server rolls.
 */
export function RollInput(props: {
  kind: "d20";
  label: string;
  modifier?: number;
  mode?: RollMode;
  onSubmit: (input: D20Input) => void;
  disabled?: boolean;
  submitLabel?: string;
} | {
  kind: "dice";
  label: string;
  /** What "Roll in app" rolls ("1d6" for a recharge). */
  notation: string;
  /** Typed totals are checked against this range. */
  min?: number;
  max?: number;
  onSubmit: (input: DiceInput) => void;
  disabled?: boolean;
  submitLabel?: string;
}) {
  const id = useId();
  const [text, setText] = useState("");
  const [bad, setBad] = useState(false);
  const typed = (): D20Input | DiceInput | null => {
    const t = text.trim();
    if (!t) return null;
    const n = Number(t);
    if (Number.isInteger(n)) {
      if (props.kind === "dice" && ((props.min != null && n < props.min) || (props.max != null && n > props.max))) return null;
      return { kind: "typed", total: n };
    }
    if (props.kind === "dice" && tryParseNotation(t)) return { kind: "roll", notation: t };
    return null;
  };
  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    const input = typed();
    if (!input) { setBad(true); return; }
    setBad(false);
    setText("");
    (props.onSubmit as (i: D20Input | DiceInput) => void)(input);
  };
  const roll = () => {
    if (props.kind === "d20") props.onSubmit({ kind: "roll", modifier: props.modifier ?? 0, mode: props.mode ?? "normal" });
    else props.onSubmit({ kind: "roll", notation: props.notation });
  };
  const mod = props.kind === "d20" && props.modifier != null ? ` (d20${props.modifier >= 0 ? "+" : ""}${props.modifier})` : props.kind === "dice" ? ` (${props.notation})` : "";
  return (
    <form className="cb-roll" onSubmit={submit} aria-label={props.label}>
      <label htmlFor={id} className="cb-roll-label">{props.label}</label>
      <input id={id} className={`input cb-roll-box${bad ? " invalid" : ""}`} inputMode={props.kind === "d20" ? "numeric" : "text"} value={text}
        placeholder={props.kind === "d20" ? "Table roll" : `Total${props.kind === "dice" && !props.notation.includes("d6") ? " or dice" : ""}`}
        aria-label={`${props.label}: typed result`} aria-invalid={bad || undefined} disabled={props.disabled}
        onChange={(e) => { setText(e.target.value); setBad(false); }} />
      <Button small type="submit" disabled={props.disabled || !text.trim()}>{props.submitLabel ?? "Enter"}</Button>
      <Button small icon="dice" onClick={roll} disabled={props.disabled} title={`Roll in app${mod}`}>Roll in app</Button>
    </form>
  );
}
