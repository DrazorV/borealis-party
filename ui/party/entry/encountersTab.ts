import type { MainTabDef } from "../../src/components/shell/sections";

/**
 * The DM's two main tabs (0.3.1), after Play, shown only to accounts with the Dungeon Master switch on (plan D2):
 * **Encounters** builds them (session prep on a big screen) and **Combat** runs them (plan 4.5b: no menu in a menu, so a
 * DM finds both at once). On phones they are secondary tabs: they live under More, like Manage and the Character
 * Sheet, and More shows their rows (pages/AccountBar.tsx). Players run their side from the Play tab's Combat sub-tab.
 */
export const ENCOUNTERS_ROUTE = "/encounters";
export const COMBAT_ROUTE = "/combat";

export const ENCOUNTERS_TAB: MainTabDef = {
  id: "encounters", label: "Encounters", short: "Encounters", to: ENCOUNTERS_ROUTE, icon: "cards", needsCharacter: false,
  match: [ENCOUNTERS_ROUTE], secondary: true,
};

export const COMBAT_TAB: MainTabDef = {
  id: "combat", label: "Combat", short: "Combat", to: COMBAT_ROUTE, icon: "swords", needsCharacter: false,
  match: [COMBAT_ROUTE], secondary: true,
};

/** The main tabs for an account: upstream's, then Encounters and Combat for a DM. Unknown (`null`) is not a DM. */
export function withEncounters(upstream: readonly MainTabDef[], dm: boolean | null): readonly MainTabDef[] {
  return dm ? [...upstream, ENCOUNTERS_TAB, COMBAT_TAB] : upstream;
}

/** Upstream's `activeTab`, over the given tabs. */
export function activeIn(tabs: readonly MainTabDef[], pathname: string): MainTabDef {
  return tabs.find((t) => t.match.some((m) => pathname === m || pathname.startsWith(`${m}/`))) ?? tabs[0]!;
}
