/**
 * Upstream's shell sections (`src/components/shell/sections.ts`) as the party's top bar and phone bottom bar see them
 * (`Chrome.tsx` and `MobileChrome.tsx`, through party/vite.config.ts): all of it, with the main tabs gaining
 * Encounters and Combat for a DM (entry/encountersTab.ts).
 *
 * `MAIN_TABS` is a live binding that follows the DM switch (../api/dmRole.ts). The shell redraws when the switch
 * changes because its `useLocation` (entry/shellRouter.tsx) listens to it and refreshes this first.
 */
import { dmRole, onDmRoleChanged } from "../api/dmRole";
import { MAIN_TABS as UPSTREAM_TABS, type MainTabDef } from "../../src/components/shell/sections";
import { activeIn, withEncounters } from "./encountersTab";

export * from "../../src/components/shell/sections";

export let MAIN_TABS: readonly MainTabDef[] = withEncounters(UPSTREAM_TABS, dmRole());

/** Follow the switch and the signed-in account. */
export function refreshMainTabs(): void {
  const dm = dmRole();
  if ((MAIN_TABS !== UPSTREAM_TABS) !== Boolean(dm)) MAIN_TABS = withEncounters(UPSTREAM_TABS, dm);
}
onDmRoleChanged(refreshMainTabs);

export function activeTab(pathname: string): MainTabDef {
  return activeIn(MAIN_TABS, pathname);
}
