import { wrapPlayerName } from "../playerName";
import { override } from "./registry";

// A new or opened character with no player name gets the account's username (../playerName.ts).
override("playerName", (methods, session) => wrapPlayerName(methods, () => session.user.username));
