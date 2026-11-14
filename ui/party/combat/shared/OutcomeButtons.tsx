import { Button, type ButtonVariant } from "../../../src/components/Button";
import "./outcomes.css";

export interface Outcome<T> {
  label: string;
  value: T;
  variant?: ButtonVariant;
}

/**
 * A roll whose only result is yes or no (user, 0.3.5: "dm and player can roll the die and just press the button they
 * do not need to enter the number"): a recharge, a save against a known DC, a death save, a concentration save. The
 * table rolls and presses the outcome; "Roll for me" lets the app roll. No number box. Side by side, the same height.
 * Numbers stay where the number itself matters (initiative, attack totals against AC, damage).
 */
export function OutcomeButtons<T>({ label, outcomes, onPick, onRoll, rollLabel = "Roll for me", disabled }: {
  label: string;
  outcomes: Outcome<T>[];
  onPick: (value: T) => void;
  /** The app's roll; absent when there is none. */
  onRoll?: () => void;
  rollLabel?: string;
  disabled?: boolean;
}) {
  return (
    <div className="roll-outcomes" role="group" aria-label={label}>
      <span className="roll-outcomes-label">{label}</span>
      <div className="roll-outcomes-row">
        {onRoll && <Button icon="dice" disabled={disabled} onClick={onRoll}>{rollLabel}</Button>}
        {outcomes.map((o) => (
          <Button key={o.label} variant={o.variant} disabled={disabled} onClick={() => onPick(o.value)}>{o.label}</Button>
        ))}
      </div>
    </div>
  );
}
