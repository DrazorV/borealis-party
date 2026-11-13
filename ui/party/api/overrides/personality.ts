import { wrapPersonality } from "../personality";
import { override } from "./registry";

// The selection flow skips a background's personality choices, as the account's setting says (../personality.ts).
override("personality", (methods) => wrapPersonality(methods));
