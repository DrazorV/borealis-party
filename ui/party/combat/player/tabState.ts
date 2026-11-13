import { useSyncExternalStore } from "react";

/**
 * Whether the Play tab shows its Combat sub-tab. Upstream's Play page keeps its own sub-tab state and doesn't know
 * the Combat tab, so the party's Tabs and TabPanel (playTabs.tsx) keep this choice beside it. The Live chip and the
 * "Your turn" banner set it from anywhere.
 */
let selected = false;
const subscribers = new Set<() => void>();

const set = (next: boolean) => {
  if (next === selected) return;
  selected = next;
  subscribers.forEach((s) => s());
};

export const selectCombatTab = (): void => set(true);
export const leaveCombatTab = (): void => set(false);
export const combatTabSelected = (): boolean => selected;

export function useCombatTabSelected(): boolean {
  return useSyncExternalStore((s) => { subscribers.add(s); return () => subscribers.delete(s); }, () => selected);
}
