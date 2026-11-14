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
        {/* The tracker's blocks are monster-shaped; one that leaves out its size or type (a thin or hand-made block)
            must still read as a monster, not an Aurora companion (which has no `creature_type` key). */}
        <EncounterStatBlock title={title} block={{ ...block, size: block.size ?? [], creature_type: block.creature_type ?? "" } as unknown as EncounterBlock} />
      </div>
    </Dialog>
  );
}
