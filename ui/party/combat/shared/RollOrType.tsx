import { useState } from "react";
import { Button } from "../../../src/components/Button";
import type { D20Input } from "./types";

/** The highest d20 total anyone types (a high bonus plus a 20); the server checks again. */
export const MAX_TOTAL = 99;
export const MIN_TOTAL = -20;

/**
 * Every roll, table first (user, 2026-11-13: "we want to promote actual rolls on table"; D21, D43): the box for the
 * total of the dice rolled at the table is the main action, with Send; "Roll for me" (the server's roller,
 * `{kind: "app"}`) is second, for players without dice. A typed result is the player's own dice and always wins.
 */
export function RollOrType({ label, modifier = 0, onRoll, busy, rollLabel = "Roll for me", autoFocus = false }: {
  /** What is being rolled, e.g. "Initiative" or "Death save". */
  label: string;
  modifier?: number;
  onRoll: (input: D20Input) => void | Promise<unknown>;
  busy?: boolean;
  rollLabel?: string;
  autoFocus?: boolean;
}) {
  const [text, setText] = useState("");
  const total = Number(text);
  const valid = /^-?\d+$/.test(text.trim()) && total >= MIN_TOTAL && total <= MAX_TOTAL;
  const send = () => {
    if (!valid) return;
    void onRoll({ kind: "typed", total });
    setText("");
  };
  const mod = modifier >= 0 ? `+${modifier}` : `${modifier}`;
  return (
    <form className="combat-table-roll" role="group" aria-label={label} onSubmit={(e) => { e.preventDefault(); send(); }}>
      <input className="input" inputMode="numeric" enterKeyHint="send" placeholder="Your roll" autoFocus={autoFocus}
        aria-label={`${label}: my table roll`} value={text} onChange={(e) => setText(e.target.value)} disabled={busy} />
      <Button type="submit" variant="primary" disabled={busy || !valid}>Send</Button>
      <Button variant="ghost" icon="dice" disabled={busy} onClick={() => void onRoll({ kind: "app", modifier, mode: "normal" })}>{rollLabel} ({mod})</Button>
    </form>
  );
}
