import { useEffect } from "react";
import type { PlayConcentrationSpell } from "../../src/api/contract";
import { addRoll, currentDie } from "../../src/pages/play/rolls/rollStore";
import { ConcentrationBanner as UpstreamBanner } from "../../src/pages/play/SpellSlotsCard";
import { rollD20 } from "../../src/lib/dice";
import { usePlayMutate } from "../../src/pages/play/playQueue";
import { useCharacter } from "../../src/state/CharacterContext";
import { useToast } from "../../src/components/Toast";
import { Button } from "../../src/components/Button";
import { OutcomeButtons } from "../combat/shared/OutcomeButtons";
import { passFail } from "../combat/shared/outcomeInputs";
import type { D20Input } from "../combat/shared/types";
import { answerConcentration, dropConcentrationChecks, useConcentrationCheck, type ConcentrationCheck } from "./concentration";
import { useDigitalDiceHidden } from "./dicePrefs";
import { markShareable } from "./rolls/shareable";
import "../combat/shared/combat.css";
import "./quick-wins.css";

export * from "../../src/pages/play/SpellSlotsCard";

/**
 * Upstream's concentration banner, pinned at the top of the Play sub-tab, with the concentration save that damage asks
 * for under it (0.3.5, ./concentration.ts): "Concentration save, DC 12 (24 damage)". The table rolls and presses Saved
 * or Failed (no number box: a pass/fail roll, as in the combat section), or "Roll for me". Saved keeps the spell;
 * Failed ends concentration.
 */
export function ConcentrationBanner({ concentration }: { concentration: PlayConcentrationSpell | null }) {
  const { snapshot } = useCharacter();
  const check = useConcentrationCheck(snapshot?.id);
  // Ended some other way (End, a long rest, another spell): its saves are moot.
  const stale = Boolean(check && check.spell.id !== concentration?.id);
  useEffect(() => { if (stale && check) dropConcentrationChecks(check.characterId); }, [stale, check]);
  return (<>
    <UpstreamBanner concentration={concentration} />
    {concentration && check && check.spell.id === concentration.id && <SavePrompt check={check} modifier={snapshot?.abilities.save.con?.bonus ?? 0} />}
  </>);
}

function SavePrompt({ check, modifier }: { check: ConcentrationCheck; modifier: number }) {
  const run = usePlayMutate();
  const toast = useToast();
  const hideApp = useDigitalDiceHidden();
  const label = `Concentration save, DC ${check.dc}`;
  const roll = async (input: D20Input) => {
    let saved: boolean;
    if (input.kind === "typed") {
      // An outcome button: the DC for Saved, one under it for Failed. Only the outcome means anything here.
      saved = input.total >= check.dc;
      addRoll({ characterId: check.characterId, label, total: input.total, breakdown: `${saved ? "Saved" : "Failed"} at the table` });
    } else {
      const r = rollD20(modifier, "normal", currentDie());
      saved = r.total >= check.dc;
      markShareable(addRoll({ characterId: check.characterId, label, total: r.total, breakdown: r.breakdown }).id, "d20");
    }
    answerConcentration(check.id);
    if (saved) {
      toast.push(`Saved: you keep concentrating on ${check.spell.name}.`);
      return;
    }
    dropConcentrationChecks(check.characterId);
    await run((api, id) => api.setConcentration({ characterId: id, spellId: null }));
    toast.push(`Failed: concentration on ${check.spell.name} ends.`);
  };
  const mod = modifier >= 0 ? `+${modifier}` : `${modifier}`;
  return (
    <section className="combat-prompt bp-conc-save" role="alertdialog" aria-label={label}>
      <p className="combat-prompt-q">{label} <span className="muted">({check.damage} damage)</span></p>
      <OutcomeButtons label="Concentration save" outcomes={passFail(check.dc)} onPick={(i) => void roll(i)}
        rollLabel={`Roll for me (${mod})`} onRoll={hideApp ? undefined : () => void roll({ kind: "app", modifier, mode: "normal" })} />
      <Button small variant="ghost" onClick={() => answerConcentration(check.id)}>Not now</Button>
    </section>
  );
}
