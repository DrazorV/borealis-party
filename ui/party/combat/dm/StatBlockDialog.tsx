import { Dialog } from "../../../src/components/controls/Dialog";
import type { StatBlock as EncounterBlock } from "../../api/overrides/encounters";
import { EncounterStatBlock } from "../../pages/EncounterStatBlock";
import "../../pages/encounters.css";
import type { StatBlock } from "./types";

/**
 * The creature's whole stat block (0.3.5), DM only: the classic 5e layout from the data the tracker already has
 * (the encounter page's renderer). Players never get this (D6: a name and a band). The dialog body scrolls on phones.
 */
export function StatBlockDialog({ title, block, onClose }: { title: string; block: StatBlock; onClose: () => void }) {
  return (
    <Dialog title={`${title}: stat block`} onClose={onClose}>
      <div className="cb-statblock" data-testid="stat-block">
        <EncounterStatBlock title={title} block={block as unknown as EncounterBlock} />
      </div>
    </Dialog>
  );
}
