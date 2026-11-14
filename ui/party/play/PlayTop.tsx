import { useEffect, useRef } from "react";
import { useLocation } from "react-router";
import type { PlayHitPoints } from "../../src/api/contract";
import { useCharacter } from "../../src/state/CharacterContext";
import { CombatPane } from "../combat/player/CombatPane";
import { useCombatFor } from "../combat/player/store";
import { leaveCombatTab, useCombatTabSelected } from "../combat/player/tabState";

/**
 * What the Play page's Play sub-tab starts with, in place of upstream's rest bar (the rests moved onto the stat
 * tiles' row, ./StatRowWithRests.tsx): during a live combat of the open character's party, the player's live combat
 * section (user, 2026-11-14: the Play tab is the player's screen; the separate Combat sub-tab is retired into it).
 * The Live chip, the "Your turn" banner and `#/play?combat` lead here and scroll it into view.
 */
// PlayPage passes the hit points upstream's rest bar used; the rests now read them themselves.
export function RestBar(props: { hp: PlayHitPoints }) {
  void props;
  const { snapshot } = useCharacter();
  const combat = useCombatFor(snapshot ? { id: snapshot.id, name: snapshot.name } : null);
  const asked = useCombatTabSelected();
  const { search } = useLocation();
  const ref = useRef<HTMLDivElement>(null);
  const linked = asked || /(^|[?&])combat(=|&|$)/.test(search.replace(/^\?/, "&"));
  const live = Boolean(combat && snapshot);
  useEffect(() => {
    if (!live || !linked) return;
    ref.current?.scrollIntoView?.({ behavior: "smooth", block: "start" });
    leaveCombatTab();
  }, [live, linked]);
  if (!combat || !snapshot) return null;
  return <div ref={ref} className="bp-play-combat"><CombatPane combat={combat} s={snapshot} /></div>;
}
