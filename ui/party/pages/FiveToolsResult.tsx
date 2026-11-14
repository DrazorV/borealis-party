import { useId, useState } from "react";
import type { FiveToolsImportReport } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { Expander } from "../../src/components/controls";
import { elementName, LICENCE_LINE, notesLine, type NoteGroup, viewReport } from "../import/reportNotes";

/** Entries listed per group before the rest is counted. */
const MAX_EXAMPLES = 50;

function Groups({ label, groups }: { label: string; groups: NoteGroup[] }) {
  if (groups.length === 0) return null;
  return (
    <div className="party-report-groups" role="group" aria-label={label}>
      <p className="party-report-label">{label}</p>
      {groups.map((g) => (
        <Expander key={g.title} defaultOpen={false} title={<><span className="party-report-count">{g.notes.length}</span> {g.title}</>}>
          <p className="party-report-meaning">{g.meaning}</p>
          <ul className="party-report-examples">
            {g.notes.slice(0, MAX_EXAMPLES).map((n, i) => <li key={i}>{elementName(n.element)}</li>)}
            {g.notes.length > MAX_EXAMPLES && <li className="muted">and {g.notes.length - MAX_EXAMPLES} more</li>}
          </ul>
        </Expander>
      ))}
    </div>
  );
}

/**
 * A 5etools import's result on the Import screen: the headline, one calm line about the notes and, on request, the
 * notes grouped by what they mean. Upstream's FiveToolsReport words every note as a problem; this one keeps a warning
 * for an import that brought nothing in.
 */
export function FiveToolsResult({ report, headline = true, startOpen = false }: { report: FiveToolsImportReport; headline?: boolean; startOpen?: boolean }) {
  const view = viewReport(report);
  const line = notesLine(view);
  const [open, setOpen] = useState(startOpen);
  const id = useId();
  return (
    <div className={`party-report ${view.tone}`} data-tone={view.tone}>
      {headline && <p className="party-report-headline" role={view.tone === "warning" ? "alert" : "status"}>{view.headline}</p>}
      {view.tone === "success" && <p className="muted small">{LICENCE_LINE}</p>}
      {line && (
        <>
          <p className="party-report-line">{line}</p>
          {/* Opened by its row's Details (ImportSection.tsx): no second toggle. */}
          {!startOpen && (
            <div className="party-report-toggle">
              <Button small aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}>
                {open ? "Hide details" : "Show details"}
              </Button>
            </div>
          )}
        </>
      )}
      {open && (
        <div id={id} className="party-report-details">
          <Groups label="Carried over in part" groups={view.partial} />
          <Groups label="Left out" groups={view.skipped} />
          {view.expected > 0 && <p className="muted small">{view.expected} entries the core rules already define (languages, skills, item types and similar) were not imported again.</p>}
        </div>
      )}
    </div>
  );
}
