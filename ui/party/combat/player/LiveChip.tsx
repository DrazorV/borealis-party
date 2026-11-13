import { useLocation, useNavigate } from "react-router";
import "../shared/combat.css";
import { Button } from "../../../src/components/Button";
import { useCharacter } from "../../../src/state/CharacterContext";
import { actingId, myCombatant, type PlayerCombat } from "./api";
import { contextFor, dmCombatPath, myTurn, useCombatStore } from "./store";
import { selectCombatTab, useCombatTabSelected } from "./tabState";

export const PLAY_ROUTE = "/play";

/** Open the Play tab's Combat sub-tab for the player's character in `combat` (opening that character if need be). */
export function useOpenCombat(): (combat: PlayerCombat, characterId?: string | null) => Promise<void> {
  const nav = useNavigate();
  const { snapshot, open } = useCharacter();
  return async (combat, characterId) => {
    // The open character when it's in this fight; else the PC's character (or the party's only one of this account's).
    const id = characterId ?? (snapshot && myCombatant(combat, { id: snapshot.id, name: snapshot.name }) ? snapshot.id
      : myCombatant(combat)?.characterId ?? contextFor(combat.partyId).myCharacters?.[0]);
    if (id && snapshot?.id !== id) await open(id);
    selectCombatTab();
    void nav(PLAY_ROUTE);
  };
}

/**
 * The Live chip in the top chrome (plan D10, 4.4): shown while one of the player's parties has a live combat, from
 * any page, and it leads to the Combat sub-tab. On the player's turn it reads "Your turn", and away from the Combat
 * sub-tab (which says it in its own header) a "Your turn" banner leads there too.
 * - `auto` (the desktop account bar): the chip, widened into the banner on the player's turn;
 * - `chip` (a phone's title bar, which has no row to spare): the chip only;
 * - `banner` (a phone's account bar): the banner only, on the player's turn (hidden on a phone held sideways, party.css).
 */
export function LiveChip({ mode = "auto" }: { mode?: "auto" | "chip" | "banner" }) {
  const store = useCombatStore();
  const openCombat = useOpenCombat();
  const nav = useNavigate();
  const { pathname } = useLocation();
  const onCombat = useCombatTabSelected() && pathname === PLAY_ROUTE;
  const first = store.combats[0];
  const dm = store.dmCombats[0];
  // The DM's own live combat (registered by the DM tracker) leads to the tracker.
  if (!first && dm && mode !== "banner") {
    const to = dmCombatPath(dm.id);
    return to ? (
      <Button small variant="danger" className="combat-live-chip" onClick={() => void nav(to)} title="Your combat is live: open the tracker">
        <span className="combat-live-dot" aria-hidden="true" />Live
      </Button>
    ) : null;
  }
  if (!first) return null;
  const turn = myTurn(store);
  if (turn && !onCombat && mode !== "chip") {
    const me = turn.combat.combatants.find((x) => x.mine && x.id === actingId(turn.combat));
    return (
      <button type="button" className="btn btn-warning combat-your-turn" onClick={() => void openCombat(turn.combat, turn.characterId)}>
        <span className="combat-live-dot" aria-hidden="true" /><strong>Your turn</strong>{me && <span className="combat-your-turn-name"> · {me.name}</span>}
        <span className="combat-your-turn-go">Open combat</span>
      </button>
    );
  }
  if (mode === "banner") return null;
  return (
    <Button small variant={turn ? "warning" : "danger"} className="combat-live-chip" onClick={() => void openCombat(turn?.combat ?? first, turn?.characterId)}
      title={turn ? "It's your turn" : "A combat is live: open it"}>
      <span className="combat-live-dot" aria-hidden="true" />{turn ? "Your turn" : "Live"}
    </Button>
  );
}
