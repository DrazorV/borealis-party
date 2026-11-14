import { useSyncExternalStore } from "react";

/**
 * A combat link is waiting to be followed (0.3.3): the Live chip and the "Your turn" banner set it from anywhere; the
 * Play page then opens its Play sub-tab (playTabsCore.tsx) and scrolls its live combat section into view
 * (../../play/PlayTop.tsx), which clears it. The names keep the 0.3.0 Combat sub-tab's, which this replaced.
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
