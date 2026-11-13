import { CONDITIONS, conditionWord, INCAPACITATING, type Condition } from "./common";

/**
 * Condition chips (C10). Read-only it lists the active ones; with `onToggle` it lists all SRD conditions as toggles,
 * and marks the incapacitating ones, which end concentration.
 */
export function ConditionChips({ active, onToggle, immune = [], disabled, label = "Conditions", compact }: {
  active: readonly Condition[];
  onToggle?: (condition: Condition, on: boolean) => void;
  immune?: readonly Condition[];
  disabled?: boolean;
  label?: string;
  /** Phones: the active ones as chips that remove, and a list to add one, so every target stays full-size. */
  compact?: boolean;
}) {
  if (onToggle && compact) {
    return (
      <div className="cb-conds" role="group" aria-label={label}>
        {active.map((c) => (
          <button key={c} type="button" className="cb-cond on" aria-pressed="true" disabled={disabled} title={`Remove ${conditionWord(c)}`}
            onClick={() => onToggle(c, false)}>{conditionWord(c)} ×</button>
        ))}
        <select className="input cb-cond-add" aria-label="Add a condition" value="" disabled={disabled}
          onChange={(e) => { if (e.target.value) onToggle(e.target.value as Condition, true); }}>
          <option value="">+ Condition…</option>
          {CONDITIONS.filter((c) => !active.includes(c) && !immune.includes(c)).map((c) => <option key={c} value={c}>{conditionWord(c)}</option>)}
        </select>
      </div>
    );
  }
  if (!onToggle) {
    if (active.length === 0) return null;
    return (
      <ul className="cb-conds" aria-label={label}>
        {active.map((c) => <li key={c} className="chip warning">{conditionWord(c)}</li>)}
      </ul>
    );
  }
  return (
    <div className="cb-conds" role="group" aria-label={label}>
      {CONDITIONS.map((c) => {
        const on = active.includes(c);
        const no = immune.includes(c);
        return (
          <button key={c} type="button" className={`cb-cond${on ? " on" : ""}`} aria-pressed={on} disabled={disabled || (no && !on)}
            title={no ? `Immune to ${c}` : INCAPACITATING.includes(c) ? `${conditionWord(c)}: ends concentration` : conditionWord(c)}
            onClick={() => onToggle(c, !on)}>
            {conditionWord(c)}
          </button>
        );
      })}
    </div>
  );
}
