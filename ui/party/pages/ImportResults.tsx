import { Button } from "../../src/components/Button";
import { Chip, type ChipTone } from "../../src/components/Card";
import { Expander } from "../../src/components/controls";
import { Icon, type IconName } from "../../src/components/Icon";
import { groupMissingAcross, type NextId, nextActions, type ResultLike, summarize, type Tone } from "../import/results";

const CHIPS: [Tone, string, ChipTone][] = [
  ["success", "done", "success"],
  ["warning", "with warnings", "warning"],
  ["error", "failed", "danger"],
  ["info", "waiting for you", "info"],
  ["working", "in progress", "default"],
];
const ICONS: Record<Tone, IconName> = { working: "update", success: "check", warning: "alert", error: "alert", info: "info" };

/**
 * The top of the Import results (0.3.6): the outcome in one line with a count per kind of outcome, the content the
 * imported characters miss grouped once by source (each item with the characters that use it), and what to do next.
 * The rows below keep their own details folded.
 */
export function ImportResults({ rows, onNext }: { rows: readonly ResultLike[]; onNext: (id: NextId) => void }) {
  const summary = summarize(rows);
  const missing = groupMissingAcross(rows);
  const missingCount = missing.reduce((n, s) => n + s.items.length, 0);
  const next = nextActions(rows);
  return (
    <div className={`party-import-summary ${summary.tone}`} data-tone={summary.tone}>
      <p className="party-import-summary-line" role="status">
        <Icon name={ICONS[summary.tone]} /> <strong>{summary.headline}</strong>
      </p>
      <div className="party-import-chips" aria-label="Results by outcome">
        {CHIPS.filter(([t]) => summary.counts[t] > 0).map(([t, label, chip]) => (
          <Chip key={t} tone={chip}>{summary.counts[t]} {label}</Chip>
        ))}
      </div>
      {missingCount > 0 && (
        <div className="party-import-missing" role="group" aria-label="Missing content">
          <Expander defaultOpen={false} title={<><span className="party-report-count">{missingCount}</span> Missing content, from {missing.length === 1 ? missing[0]!.source : `${missing.length} sources`}</>}>
            <p className="muted small">The imported characters use content that is not installed. Add it here (the sources below), then re-check on Characters: they pick it up with no re-import.</p>
            {missing.map((s) => (
              <div key={s.source} className="party-import-missing-source">
                <p className="party-report-label">{s.source} <span className="muted small">({s.items.length})</span></p>
                <ul className="party-report-examples">
                  {s.items.map(({ item, usedBy }) => (
                    <li key={item.id}>{item.name || item.id}{item.type ? <span className="muted small"> {item.type}</span> : null}
                      <span className="muted small"> (used by {usedBy.join(", ")})</span></li>
                  ))}
                </ul>
              </div>
            ))}
          </Expander>
        </div>
      )}
      {next.length > 0 && (
        <div className="party-import-actions party-import-next" aria-label="What next">
          {next.map((a) => <Button key={a.id} small variant={a.primary ? "primary" : "default"} onClick={() => onNext(a.id)}>{a.label}</Button>)}
        </div>
      )}
    </div>
  );
}
