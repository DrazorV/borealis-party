import type { PlayHitPoints } from "../../src/api/contract";
import { IconButton } from "../../src/components";
import { useToast } from "../../src/components/Toast";
import { Expander } from "../../src/components/controls";
import { AmountControl } from "../../src/pages/play/AmountControl";
import { MAX_HP_CHANGE } from "../../src/pages/play/HitPointsCard";
import { usePlayMutate } from "../../src/pages/play/playQueue";
import { askConcentration, dropConcentrationChecks } from "./concentration";
import "../../src/pages/play/hit-points.css";

export { MAX_HP_CHANGE };

/**
 * Upstream's Hit Points card (`src/pages/play/HitPointsCard.tsx`, the same controls), with what damage does to
 * concentration (0.3.5, roadmap P2), in or out of combat: while the character concentrates, each damage asks for a
 * concentration save (./concentration.ts, answered in the concentration banner, ./SpellSlotsCard.tsx); damage that
 * drops them to 0 hit points ends concentration at once.
 */
export function HitPointsCard({ hp }: { hp: PlayHitPoints }) {
  const run = usePlayMutate();
  const toast = useToast();
  const damage = async (amount: number) => {
    const next = await run((api, id) => api.takeDamage({ characterId: id, amount }));
    const spell = next?.play.concentration;
    if (!next || !spell || amount <= 0) return;
    if (next.play.hitPoints.current <= 0) {
      dropConcentrationChecks(next.id);
      await run((api, id) => api.setConcentration({ characterId: id, spellId: null }));
      toast.push(`Concentration on ${spell.name} ended: you dropped to 0 hit points.`);
      return;
    }
    askConcentration(next.id, spell, amount);
  };
  const heal = (amount: number) => void run((api, id) => api.regainHitPoints({ characterId: id, amount }));
  const setTemp = (amount: number) => void run((api, id) => api.setTempHitPoints({ characterId: id, amount }));
  const untouchable = hp.current <= 0 && hp.temp <= 0;
  return (
    <Expander title="Hit Points">
      <div className="v5-inline">
        <div className="v5-foot hp-foot">
          <span className="field-label">Current</span>
          <IconButton icon="minus" label="Take 1 damage" variant="default" disabled={untouchable} onClick={() => void damage(1)} />
          <span className="v5-exh" role="status" aria-label={`Hit points: ${hp.current} of ${hp.max}`}>{hp.current}</span>
          <IconButton icon="plus" label="Regain 1 hit point" variant="default" disabled={hp.current >= hp.max} onClick={() => heal(1)} />
          <span className="v5-note">of {hp.max}</span>
        </div>
        <div className="v5-foot hp-foot">
          <span className="field-label">Temporary</span>
          <span className="v5-exh" role="status" aria-label={`Temporary hit points: ${hp.temp}`}>{hp.temp}</span>
          {hp.temp > 0 && <IconButton icon="x" label="End temporary hit points" variant="default" onClick={() => setTemp(0)} />}
        </div>
        <div className="v5-foot">
          <AmountControl label="Hit point amount" max={MAX_HP_CHANGE} actions={[
            { label: "Damage", name: "Take damage", run: (n) => void damage(n), disabled: untouchable },
            { label: "Heal", name: "Regain hit points", run: heal, disabled: hp.current >= hp.max },
            { label: "Temp", name: "Set temporary hit points", run: setTemp, allowZero: true },
          ]} />
        </div>
      </div>
    </Expander>
  );
}
