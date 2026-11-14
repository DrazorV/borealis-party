import { createContext, useContext } from "react";

/**
 * The Play page's open sub-tab (Play, Abilities, Attacks, Spells, Traits), for the party's pieces inside it. Upstream's
 * Play page keeps it as its own state; the party's TabPanel (../combat/player/playTabsCore.tsx) provides it here.
 */
export const PlaySubTab = createContext<string | null>(null);

export const usePlaySubTab = (): string | null => useContext(PlaySubTab);
