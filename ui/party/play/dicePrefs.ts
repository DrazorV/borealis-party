import { useSyncExternalStore } from "react";

/**
 * "Hide Roll for me" (0.3.5, roadmap P4/P22/D43): a table with real dice can hide the app's own dice on this device, so
 * every roll is typed. Kept on the device, not the account: one player may use the app's dice on a laptop and only real
 * dice on the phone at the table.
 */
export const HIDE_DICE_KEY = "party.hideDigitalDice";

let hidden: boolean | null = null;
const listeners = new Set<() => void>();

export function digitalDiceHidden(): boolean {
  if (hidden === null) {
    try { hidden = localStorage.getItem(HIDE_DICE_KEY) === "true"; } catch { hidden = false; }
  }
  return hidden;
}

export function setDigitalDiceHidden(value: boolean): void {
  hidden = value;
  try { localStorage.setItem(HIDE_DICE_KEY, String(value)); } catch { /* a private window: this session only */ }
  listeners.forEach((l) => l());
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => { listeners.delete(l); };
};

export function useDigitalDiceHidden(): boolean {
  return useSyncExternalStore(subscribe, digitalDiceHidden, digitalDiceHidden);
}

/** Tests: read the device's value again. */
export function resetDicePrefs(): void {
  hidden = null;
  listeners.forEach((l) => l());
}
