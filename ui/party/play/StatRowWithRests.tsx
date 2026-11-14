import { useRef, useState, type ReactNode } from "react";
import type { PlayState } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { useConfirm } from "../../src/components/Confirm";
import { StatRow as UpstreamStatRow } from "../../src/components/StatBlock";
import { useToast } from "../../src/components/Toast";
import { haptic } from "../../src/lib/haptics";
import { LONG_REST_AT_ZERO_HP } from "../../src/pages/play/RestBar";
import { usePlayMutate } from "../../src/pages/play/playQueue";
import { restSummary } from "../../src/pages/play/restSummary";
import { offersHitDice, ShortRestDialog } from "../../src/pages/play/ShortRestDialog";
import { useCharacter } from "../../src/state/CharacterContext";
import { usePlaySubTab } from "./subTab";
import "./rests.css";

/**
 * The Play page's stat tiles (AC, Initiative, Speed, Max HP, Proficiency) with Short Rest and Long Rest, bigger, on the
 * same row (user, 2026-11-14: "make short rest and long rest bigger and in the same line as ac, hp"). Only on the
 * Play sub-tab; the Traits sub-tab shows the same tiles without them. A narrow window wraps the rests under the
 * tiles, on the right; a phone gives them a full row under the tiles, side by side.
 */
export function StatRow({ children }: { children: ReactNode }) {
  const tab = usePlaySubTab();
  const { snapshot } = useCharacter();
  if (tab !== "play" || !snapshot) return <UpstreamStatRow>{children}</UpstreamStatRow>;
  return (
    <div className="stat-row bp-stat-row">
      {children}
      <Rests play={snapshot.play} />
    </div>
  );
}

/**
 * Short Rest and Long Rest as upstream's rest bar does them (src/pages/play/RestBar.tsx): a rest applies at once and
 * says what came back; a Short Rest then offers hit dice; a Long Rest at 0 hit points asks first.
 */
function Rests({ play }: { play: PlayState }) {
  const run = usePlayMutate();
  const confirm = useConfirm();
  const toast = useToast();
  const latest = useRef<PlayState>(play);
  latest.current = play;
  const [offer, setOffer] = useState(false);
  const rest = async (long: boolean) => {
    if (long && play.hitPoints.current <= 0) {
      const answer = await confirm({ title: "Long Rest", body: LONG_REST_AT_ZERO_HP, confirmLabel: "Rest anyway" });
      if (answer !== "confirm") return;
    }
    haptic("rest");
    let before: PlayState | null = null;
    const next = await run((api, id) => {
      before = latest.current;
      return long ? api.longRest({ characterId: id }) : api.shortRest({ characterId: id });
    });
    if (!next) return;
    if (before) toast.push(restSummary(long ? "Long Rest" : "Short Rest", before, next.play));
    if (!long && offersHitDice(next.play)) setOffer(true);
  };
  return (
    <div className="bp-rests" role="group" aria-label="Rests">
      <Button icon="sparkles" onClick={() => void rest(false)}>Short Rest</Button>
      <Button variant="primary" icon="star" onClick={() => void rest(true)}>Long Rest</Button>
      {offer && <ShortRestDialog onClose={() => setOffer(false)} />}
    </div>
  );
}
