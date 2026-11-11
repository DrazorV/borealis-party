import { loadSession } from "../api/session";

/**
 * Where the open settings window saves to, which decides what it may change. The server enforces the same split
 * (`party/crates/borealis-server/src/settings.rs`); this only keeps the window from offering what it would refuse.
 */
export type SettingsPlace = "personal" | "library" | "group-dm" | "group-member";

export function settingsPlace(): SettingsPlace {
  const session = loadSession();
  if (!session) return "personal";
  // A DM looking at a member's character saves settings in their own view of that group.
  if (session.user.role === "dm") return session.scope === "library" && !session.view ? "library" : "group-dm";
  return session.scope.startsWith("group-") ? "group-member" : "personal";
}

/** The house rules (ability scores and the default ruleset): a group's are its DM's; outside groups a player's own. */
export type RulesAccess = "edit" | "read" | "none";

export function rulesAccess(place: SettingsPlace = settingsPlace()): RulesAccess {
  if (place === "library") return "none";
  return place === "group-member" ? "read" : "edit";
}

export const SET_BY_DM = "Your Dungeon Master sets this group's house rules.";
