import { useEffect, useRef } from "react";
import { signed } from "../../../src/components/StatBlock";
import { Dialog } from "../../../src/components/controls";
import { critical, formatNotation } from "../../../src/lib/dice";
import { RollOrType } from "../../combat/shared/RollOrType";
import type { D20Input } from "../../combat/shared/types";
import { useDigitalDiceHidden } from "../dicePrefs";
import type { Asked } from "./RollContext";
import "../../combat/shared/combat.css";

/** What to roll at the table, in words. */
function lead(a: Asked): string {
  if (a.kind === "dice") return `Roll ${formatNotation(a.request.critical ? critical(a.request.expr) : a.request.expr)} and type the total.`;
  const { modifier, mode } = a.request;
  const add = modifier === 0 ? "" : ` and add ${signed(modifier)}`;
  if (mode === "advantage") return `Roll two d20s, keep the higher${add}, then type the total.`;
  if (mode === "disadvantage") return `Roll two d20s, keep the lower${add}, then type the total.`;
  return `Roll a d20${add}, then type the total.`;
}

/** What "Roll for me" rolls: "+5, advantage" or "2d8+3". */
function hint(a: Asked): string {
  if (a.kind === "dice") return formatNotation(a.request.critical ? critical(a.request.expr) : a.request.expr);
  const mode = a.request.mode === "normal" ? "" : `, ${a.request.mode}`;
  return `${signed(a.request.modifier)}${mode}`;
}

/**
 * The table roll for one Play-tab roll button (0.3.5): the total of the real dice, typed and sent, is the main action;
 * "Roll for me" rolls the app's dice instead. An app dialog (a sheet on phones), focused on the number box.
 */
export function TypedRollSheet({ asked, onRoll, onClose }: { asked: Asked; onRoll: (input: D20Input) => void; onClose: () => void }) {
  const hidden = useDigitalDiceHidden();
  const label = asked.request.label;
  const box = useRef<HTMLDivElement>(null);
  // The dialog focuses its first control when it opens; the number box is the one to type in (a phone opens its keypad).
  useEffect(() => {
    const t = window.setTimeout(() => box.current?.querySelector("input")?.focus(), 0);
    return () => window.clearTimeout(t);
  }, []);
  return (
    <Dialog title={label} onClose={onClose} className="bp-typed-roll">
      <p className="v5-lead">{lead(asked)}</p>
      <div ref={box}><RollOrType label={label} modifier={asked.kind === "d20" ? asked.request.modifier : 0} appHint={hint(asked)} onRoll={onRoll} /></div>
      {hidden && <p className="v5-note">The app&apos;s dice are hidden in Settings (General).</p>}
    </Dialog>
  );
}
