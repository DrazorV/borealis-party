import { Chip } from "../../../src/components/Card";
import { BAND_LABEL, type HealthBand as Band } from "../../api/overrides/parties";

/** The tone each band shows in (plan D6): Unhurt, Bloodied (at or under half), Down. */
export const BAND_TONE: Record<Band, "success" | "warning" | "danger"> = { unhurt: "success", bloodied: "warning", down: "danger" };

/** A combatant's health band, as a chip. Shared by the player's Combat sub-tab and the DM tracker. */
export function HealthBand({ band }: { band: Band }) {
  return (
    <span className={`combat-band combat-band-${band}`} aria-label={`Health: ${BAND_LABEL[band]}`}>
      <Chip tone={BAND_TONE[band]}>{BAND_LABEL[band]}</Chip>
    </span>
  );
}
