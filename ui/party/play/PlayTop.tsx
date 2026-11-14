import { useEffect, useRef } from "react";
import type { PlayHitPoints } from "../../src/api/contract";
import { useCharacter } from "../../src/state/CharacterContext";
import { CombatPane } from "../combat/player/CombatPane";
import { myCombatant } from "../combat/player/api";
import { useCombatFor } from "../combat/player/store";
import { leaveCombatTab, useCombatTabSelected } from "../combat/player/tabState";

/**
 * What the Play page's Play sub-tab starts with, in place of upstream's rest bar (the rests moved onto the stat
 * tiles' row, ./StatRowWithRests.tsx): during a live combat of the open character's party, the player's live combat
 * section (user, 2026-11-14: the Play tab is the player's screen; the separate Combat sub-tab is retired into it).
 * It appears when the DM starts a combat with the open character in it, and goes when it ends. The Live chip and the
 * "Your turn" banner lead here and scroll it into view.
 */
// PlayPage passes the hit points upstream's rest bar used; the rests now read them themselves.
export function RestBar(props: { hp: PlayHitPoints }) {
  void props;
  const { snapshot } = useCharacter();
  const combat = useCombatFor(snapshot ? { id: snapshot.id, name: snapshot.name } : null);
  const asked = useCombatTabSelected();
  const ref = useRef<HTMLDivElement>(null);
  const live = Boolean(combat && snapshot);
  useEffect(() => {
    if (!live || !asked) return;
    ref.current?.scrollIntoView?.({ behavior: "smooth", block: "start" });
    leaveCombatTab();
  }, [live, asked]);
  if (!combat || !snapshot) return null;
  // Only for a character in the fight's initiative (user, 2026-11-14): the server says when one isn't.
  if (myCombatant(combat, { id: snapshot.id, name: snapshot.name })?.inInitiative === false) return null;
  return <div ref={ref} className="bp-play-combat"><CombatPane combat={combat} s={snapshot} /></div>;
}
