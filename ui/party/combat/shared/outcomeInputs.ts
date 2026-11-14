import type { Outcome } from "./OutcomeButtons";
import type { D20Input } from "./types";

/**
 * The outcome buttons of a pass/fail d20 (0.3.5) as the engine's typed d20: a pass is the DC, a fail one under it.
 * The engine compares the total with the DC, so the outcome lands exactly as pressed.
 */
export function passFail(dc: number, pass = "Saved", fail = "Failed"): Outcome<D20Input>[] {
  return [
    { label: pass, value: { kind: "typed", total: dc }, variant: "success" },
    { label: fail, value: { kind: "typed", total: dc - 1 }, variant: "danger" },
  ];
}

/** A death save: a success (10+), a failure, and the naturals that change the count (a 20 wakes, a 1 is two failures). */
export const DEATH_SAVE_OUTCOMES: Outcome<D20Input>[] = [
  { label: "Success", value: { kind: "typed", total: 10 }, variant: "success" },
  { label: "Failure", value: { kind: "typed", total: 9 }, variant: "danger" },
  { label: "Natural 20", value: { kind: "typed", total: 20, natural: 20 } },
  { label: "Natural 1", value: { kind: "typed", total: 1, natural: 1 } },
];
