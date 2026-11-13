import { useState } from "react";
import { Button } from "../../src/components/Button";
import { Card } from "../../src/components/Card";
import type { Purpose } from "../api/overrides/assistant";
import { type AssistantContext, type AssistantFrame, AssistantPanel } from "./AssistantPanel";

/**
 * Opens the assistant panel for a purpose, with the ids of the page it is on. One line on each page that offers it:
 * the encounter list and builder, and Additional Content (part D's homebrew editor adds one too).
 */
export function AssistantButton({ purpose, context, onChanged, small = true, frame }: {
  purpose: Purpose;
  context?: AssistantContext;
  onChanged?: () => void;
  small?: boolean;
  /** Where the panel opens: an app dialog unless the page has its own tool column. */
  frame?: AssistantFrame;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button small={small} icon="sparkles" onClick={() => setOpen(true)}>Assistant</Button>
      {open && <AssistantPanel purpose={purpose} context={context} onClose={() => setOpen(false)} onChanged={onChanged} frame={frame} />}
    </>
  );
}

/** Additional Content's entry: homebrew and custom indexes with the person's own AI. */
export function AssistantContentCard() {
  return (
    <Card title="Assistant">
      <p className="muted">Draft homebrew or build a custom index with your own AI (Account, Your AI).</p>
      <div className="party-inline">
        <AssistantButton purpose="homebrew" />
      </div>
    </Card>
  );
}
