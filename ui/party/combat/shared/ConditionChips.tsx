import { Chip } from "../../../src/components/Card";

export interface ConditionChip {
  name: string;
  /** Turns left, when it has a turn count. */
  remaining?: number | null;
  /** The rules text, shown as the chip's title. */
  text?: string | null;
}

const titleCase = (s: string) => s.replace(/(^|\s)\S/g, (m) => m.toUpperCase());

/**
 * A combatant's conditions as chips. Read-only for players; with `onRemove` each chip gets a remove button (the DM
 * tracker). Shared by both screens.
 */
export function ConditionChips({ conditions, onRemove, label = "Conditions" }: {
  conditions: ConditionChip[]; onRemove?: (name: string) => void; label?: string;
}) {
  if (conditions.length === 0) return null;
  return (
    <ul className="combat-conditions" aria-label={label}>
      {conditions.map((c) => (
        <li key={c.name}>
          <Chip tone="warning" title={c.text ?? undefined}>
            {titleCase(c.name)}{c.remaining != null && <span className="combat-condition-left"> · {c.remaining}</span>}
          </Chip>
          {onRemove && (
            <button type="button" className="combat-condition-remove" aria-label={`Remove ${c.name}`} onClick={() => onRemove(c.name)}>×</button>
          )}
        </li>
      ))}
    </ul>
  );
}
