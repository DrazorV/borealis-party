import { useLocation, useNavigate } from "react-router";
import "../shared/combat.css";
import { Button } from "../../../src/components/Button";
import { useCharacter } from "../../../src/state/CharacterContext";
import { myCombatant, type PlayerCombat } from "./api";
import { myTurn, useCombatStore } from "./store";
import { selectCombatTab, useCombatTabSelected } from "./tabState";

export const PLAY_ROUTE = "/play";

/** Open the Play tab's Combat sub-tab for the player's character in `combat` (opening that character if need be). */
export function useOpenCombat(): (combat: PlayerCombat, characterId?: string | null) => Promise<void> {
  const nav = useNavigate();
  const { snapshot, open } = useCharacter();
  return async (combat, characterId) => {
    const id = characterId ?? (snapshot && myCombatant(combat, snapshot.id) ? snapshot.id : myCombatant(combat)?.characterId);
    if (id && snapshot?.id !== id) await open(id);
    selectCombatTab();
    void nav(PLAY_ROUTE);
  };
}

/**
 * The Live chip in the top chrome (plan D10, 4.4): shown while one of the player's parties has a live combat, from
 * any page, and it leads to the Combat sub-tab. On the player's turn it reads "Your turn".
 */
export function LiveChip() {
  const store = useCombatStore();
  const openCombat = useOpenCombat();
  const first = store.combats[0];
  if (!first) return null;
  const turn = myTurn(store);
  return (
    <Button small variant={turn ? "warning" : "danger"} className="combat-live-chip" onClick={() => void openCombat(turn?.combat ?? first, turn?.characterId)}
      title={turn ? "It's your turn: open the combat" : "A combat is live: open it"}>
      <span className="combat-live-dot" aria-hidden="true" />{turn ? "Your turn" : "Live"}
    </Button>
  );
}

/**
 * "Your turn" across the top of every page but the Combat sub-tab itself, which says so in its own header. A tap opens
 * the Combat sub-tab.
 */
export function YourTurnBanner() {
  const store = useCombatStore();
  const openCombat = useOpenCombat();
  const { pathname } = useLocation();
  const onCombat = useCombatTabSelected() && pathname === PLAY_ROUTE;
  const turn = myTurn(store);
  if (!turn || onCombat) return null;
  const me = turn.combat.combatants.find((x) => x.mine && x.characterId === turn.characterId);
  return (
    <button type="button" className="combat-your-turn" onClick={() => void openCombat(turn.combat, turn.characterId)}>
      <strong>Your turn</strong>{me && <span> · {me.name}</span>}<span className="combat-your-turn-go">Open combat</span>
    </button>
  );
}
