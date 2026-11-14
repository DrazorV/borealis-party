import { DM_BAND_LABEL, dmBand } from "./privacy";
import type { Combatant } from "./types";

const TONE = { healthy: "success", bloodied: "warning", down: "danger", stable: "", dead: "danger" } as const;

/** A player's combatant's health band on the DM screen (Healthy / Bloodied / Down / Stable / Dead), never a number. */
export function BandChip({ c }: { c: Combatant }) {
  const band = dmBand(c);
  if (!band) return null;
  return <span className={`chip cb-band ${TONE[band]}`} data-band={band} aria-label={`Health: ${DM_BAND_LABEL[band]}`}>{DM_BAND_LABEL[band]}</span>;
}
