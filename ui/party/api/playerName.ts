import type { CharacterSnapshot } from "../../src/api/contract";
import type { Method } from "./overrides/registry";

/**
 * Player Name defaults to the account's username (0.2.2). Upstream's Manage tab keeps the field (`details.playerName`)
 * and upstream's create command fills it from Settings > Player Name, which ships as "Player One". In a party the
 * account already names the player, so:
 *
 *  - a NEW character whose player name is empty or still upstream's shipped default gets the username, and is saved
 *    with it straight away (create had already put it on file), so it opens clean. A Player Name the account chose in
 *    Settings is upstream's and is kept;
 *  - an OPENED character of the account's own (loadCharacter: a party view of someone else's character is another
 *    route) whose player name is empty gets the username as an edit, written with the next save (or autosave), so
 *    exported sheets and PDFs carry it. Imports, bundles and share codes keep the name they came with; an empty one
 *    is filled the same way when the character is first opened;
 *  - a non-empty player name is never changed, and the field stays editable. Clearing it reads as "the default": the
 *    username comes back the next time the character is opened.
 */

/** Upstream's shipped Settings > Player Name (src-tauri/src/settings.rs). */
export const UPSTREAM_DEFAULT_PLAYER = "Player One";

function playerOf(snapshot: unknown): string | null {
  const details = (snapshot as Partial<CharacterSnapshot> | null)?.details;
  return details && typeof details.playerName === "string" ? details.playerName : null;
}

/** Wrap createCharacter and loadCharacter of `methods` to fill an unset player name with `username()`. */
export function wrapPlayerName(methods: Record<string, Method>, username: () => string | null | undefined): void {
  const fill = async (snapshot: CharacterSnapshot, save: boolean): Promise<CharacterSnapshot> => {
    const name = username()?.trim();
    if (!name || !snapshot?.id) return snapshot;
    const characterId = snapshot.id;
    try {
      let next = (await methods.updateDetails!({ characterId, patch: { playerName: name } })) as CharacterSnapshot;
      if (save) next = (await methods.saveCharacter!({ characterId })) as CharacterSnapshot;
      return next;
    } catch {
      // The character was made or opened; a default that could not be set is no reason to fail that.
      return snapshot;
    }
  };

  const create = methods.createCharacter;
  if (create) {
    methods.createCharacter = async (args) => {
      const snapshot = (await create(args)) as CharacterSnapshot;
      const player = playerOf(snapshot);
      if (player === null || (player.trim() !== "" && player !== UPSTREAM_DEFAULT_PLAYER)) return snapshot;
      return fill(snapshot, true);
    };
  }

  const load = methods.loadCharacter;
  if (load) {
    methods.loadCharacter = async (args) => {
      const snapshot = (await load(args)) as CharacterSnapshot;
      if (playerOf(snapshot)?.trim() !== "") return snapshot;
      return fill(snapshot, false);
    };
  }
}
