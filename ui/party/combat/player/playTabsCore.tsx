import type { ComponentProps } from "react";
import { TabPanel as UpstreamTabPanel, Tabs as UpstreamTabs } from "../../../src/components/Tabs";
import { useCharacter } from "../../../src/state/CharacterContext";
import { CombatPane } from "./CombatPane";
import { useCombatFor } from "./store";
import { leaveCombatTab, selectCombatTab, useCombatTabSelected } from "./tabState";

/**
 * The Play page's sub-tab strip and panel with the Combat sub-tab added while the open character is in a live combat
 * of its party (plan 4.4, 0.3.0 part F). Only the Play page's own strip (`idPrefix="play"`) changes. It imports
 * upstream's Tabs file itself, never the components index, which playTabs.tsx stands in for.
 */

export const COMBAT_TAB = "combat";
const PLAY = "play";

function useLive() {
  const { snapshot } = useCharacter();
  const combat = useCombatFor(snapshot?.id);
  const selected = useCombatTabSelected();
  return { snapshot, combat, showing: Boolean(combat && selected) };
}

export function Tabs(props: ComponentProps<typeof UpstreamTabs>) {
  if (props.idPrefix !== PLAY) return <UpstreamTabs {...props} />;
  return <PlayTabs {...props} />;
}

function PlayTabs(props: ComponentProps<typeof UpstreamTabs>) {
  const { combat, showing } = useLive();
  if (!combat) return <UpstreamTabs {...props} />;
  const tabs = [{ id: COMBAT_TAB, label: "Combat" }, ...props.tabs];
  const onChange = (id: string) => {
    if (id === COMBAT_TAB) { selectCombatTab(); return; }
    leaveCombatTab();
    props.onChange(id);
  };
  return <UpstreamTabs {...props} tabs={tabs} active={showing ? COMBAT_TAB : props.active} onChange={onChange} />;
}

export function TabPanel(props: ComponentProps<typeof UpstreamTabPanel>) {
  if (props.idPrefix !== PLAY) return <UpstreamTabPanel {...props} />;
  return <PlayPanel {...props} />;
}

function PlayPanel(props: ComponentProps<typeof UpstreamTabPanel>) {
  const { snapshot, combat, showing } = useLive();
  if (!combat || !snapshot || !showing) return <UpstreamTabPanel {...props} />;
  return <UpstreamTabPanel active={COMBAT_TAB} idPrefix={PLAY}><CombatPane combat={combat} s={snapshot} /></UpstreamTabPanel>;
}
