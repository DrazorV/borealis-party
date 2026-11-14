/**
 * Upstream's components index for the Play page (`src/pages/play/PlayPage.tsx`, through party/vite.config.ts): all of
 * it, with the Play page's Tabs and TabPanel (playTabsCore.tsx: combat links open the Play sub-tab) and its stat row
 * gaining Short Rest and Long Rest (../../play/StatRowWithRests.tsx).
 */
export * from "../../../src/components";
export { PLAY_SUB_TAB, TabPanel, Tabs } from "./playTabsCore";
export { StatRow } from "../../play/StatRowWithRests";
