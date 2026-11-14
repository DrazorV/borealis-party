import { useEffect, type ComponentProps } from "react";
import { TabPanel as UpstreamTabPanel, Tabs as UpstreamTabs } from "../../../src/components/Tabs";
import { PlaySubTab } from "../../play/subTab";
import { useCombatTabSelected } from "./tabState";

/**
 * The Play page's sub-tab strip and panel (0.3.3, user 2026-11-14: the Play tab is the player's screen). The separate
 * Combat sub-tab of 0.3.0 is retired: a live combat shows as a section at the top of the Play sub-tab
 * (../../play/PlayTop.tsx). What stays here:
 * - a combat link (the Live chip, the "Your turn" banner) opens the Play sub-tab, wherever the page was;
 * - the panel tells the party's pieces inside it which sub-tab is open (the rests sit on the Play sub-tab's stat row).
 * Only the Play page's own strip (`idPrefix="play"`) changes. It imports upstream's Tabs file itself, never the
 * components index, which playTabs.tsx stands in for.
 */

/** The Play sub-tab's id on upstream's Play page. */
export const PLAY_SUB_TAB = "play";
const PLAY = "play";

export function Tabs(props: ComponentProps<typeof UpstreamTabs>) {
  if (props.idPrefix !== PLAY) return <UpstreamTabs {...props} />;
  return <PlayTabs {...props} />;
}

function PlayTabs(props: ComponentProps<typeof UpstreamTabs>) {
  const asked = useCombatTabSelected();
  const { active, onChange } = props;
  // A combat link: the live combat section is on the Play sub-tab.
  useEffect(() => { if (asked && active !== PLAY_SUB_TAB) onChange(PLAY_SUB_TAB); }, [asked, active, onChange]);
  return <UpstreamTabs {...props} />;
}

export function TabPanel(props: ComponentProps<typeof UpstreamTabPanel>) {
  if (props.idPrefix !== PLAY) return <UpstreamTabPanel {...props} />;
  return <PlaySubTab.Provider value={props.active}><UpstreamTabPanel {...props} /></PlaySubTab.Provider>;
}
