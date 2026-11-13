import { useId, useState } from "react";
import { Button } from "../../../src/components/Button";
import type { D20Input } from "./types";

/** The highest d20 total anyone types (a high bonus plus a 20); the server checks again. */
export const MAX_TOTAL = 99;
export const MIN_TOTAL = -20;

/**
 * Every roll can be made in the app OR typed from the table's dice (D21, D43). "Roll in app" asks the server's roller
 * (`{kind: "roll"}`); "Type my table roll" opens a number field and sends `{kind: "typed", total}`. A typed result is
 * the player's own dice and always wins. Shared by the player's Combat sub-tab and the DM tracker.
 */
export function RollOrType({ label, modifier = 0, onRoll, busy, rollLabel = "Roll in app", typeLabel = "Type my table roll" }: {
  /** What is being rolled, e.g. "Initiative" or "Death save". */
  label: string;
  modifier?: number;
  onRoll: (input: D20Input) => void | Promise<unknown>;
  busy?: boolean;
  rollLabel?: string;
  typeLabel?: string;
}) {
  const [typing, setTyping] = useState(false);
  const [text, setText] = useState("");
  const id = useId();
  const total = Number(text);
  const valid = text.trim() !== "" && Number.isInteger(total) && total >= MIN_TOTAL && total <= MAX_TOTAL;
  const send = () => {
    if (!valid) return;
    void onRoll({ kind: "typed", total });
    setTyping(false);
    setText("");
  };
  return (
    <div className="combat-roll" role="group" aria-label={label}>
      {!typing ? (
        <div className="combat-roll-buttons">
          <Button variant="primary" disabled={busy} onClick={() => void onRoll({ kind: "roll", modifier, mode: "normal" })}>{rollLabel}</Button>
          <Button disabled={busy} onClick={() => setTyping(true)}>{typeLabel}</Button>
        </div>
      ) : (
        <form className="combat-roll-typed" onSubmit={(e) => { e.preventDefault(); send(); }}>
          <label className="field-label" htmlFor={id}>{label} total</label>
          <input id={id} className="input" type="number" inputMode="numeric" min={MIN_TOTAL} max={MAX_TOTAL} autoFocus
            value={text} onChange={(e) => setText(e.target.value)} />
          <Button type="submit" variant="primary" disabled={busy || !valid}>Send</Button>
          <Button onClick={() => { setTyping(false); setText(""); }}>Cancel</Button>
        </form>
      )}
    </div>
  );
}
