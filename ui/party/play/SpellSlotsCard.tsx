import { useEffect } from "react";
import type { PlayConcentrationSpell } from "../../src/api/contract";
import { addRoll, currentDie } from "../../src/pages/play/rolls/rollStore";
import { ConcentrationBanner as UpstreamBanner } from "../../src/pages/play/SpellSlotsCard";
import { rollD20 } from "../../src/lib/dice";
import { usePlayMutate } from "../../src/pages/play/playQueue";
import { useCharacter } from "../../src/state/CharacterContext";
import { useToast } from "../../src/components/Toast";
import { Button } from "../../src/components/Button";
import { RollOrType } from "../combat/shared/RollOrType";
import type { D20Input } from "../combat/shared/types";
import { answerConcentration, dropConcentrationChecks, useConcentrationCheck, type ConcentrationCheck } from "./concentration";
import { markShareable } from "./rolls/shareable";
import "../combat/shared/combat.css";
import "./quick-wins.css";

export * from "../../src/pages/play/SpellSlotsCard";

/**
 * Upstream's concentration banner, pinned at the top of the Play sub-tab, with the concentration save that damage asks
 * for under it (0.3.5, ./concentration.ts): "Concentration save, DC 12 (24 damage)", the total of the table's d20 typed
 * first, "Roll for me" second. A save at or above the DC keeps the spell; below it, concentration ends.
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
  const label = "Concentration save";
  const roll = async (input: D20Input) => {
    let total: number;
    if (input.kind === "typed") {
      total = input.total;
      markShareable(addRoll({ characterId: check.characterId, label: `${label}, DC ${check.dc}`, total, breakdown: "rolled at the table" }).id, "d20");
    } else {
      const r = rollD20(modifier, "normal", currentDie());
      total = r.total;
      markShareable(addRoll({ characterId: check.characterId, label: `${label}, DC ${check.dc}`, total, breakdown: r.breakdown }).id, "d20");
    }
    answerConcentration(check.id);
    if (total >= check.dc) {
      toast.push(`${total}: you keep concentrating on ${check.spell.name}.`);
      return;
    }
    dropConcentrationChecks(check.characterId);
    await run((api, id) => api.setConcentration({ characterId: id, spellId: null }));
    toast.push(`${total}: concentration on ${check.spell.name} ends.`);
  };
  return (
    <section className="combat-prompt bp-conc-save" role="alertdialog" aria-label={`Concentration save, DC ${check.dc}`}>
      <p className="combat-prompt-q">Concentration save, DC {check.dc} <span className="muted">({check.damage} damage)</span></p>
      <RollOrType label={label} modifier={modifier} onRoll={(i) => void roll(i)} />
      <Button small variant="ghost" onClick={() => answerConcentration(check.id)}>Not now</Button>
    </section>
  );
}
