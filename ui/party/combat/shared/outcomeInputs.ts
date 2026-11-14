import type { Outcome } from "./OutcomeButtons";
import type { D20Input } from "./types";

/**
 * The outcome buttons of a pass/fail d20 (0.3.5): the result itself (`{ kind: "outcome" }`), never a made-up number,
 * so the log and the other screens say "saved" or "failed", not a roll nobody made.
 */
export function passFail(pass = "Saved", fail = "Failed"): Outcome<D20Input>[] {
  return [
    { label: pass, value: { kind: "outcome", outcome: "pass" }, variant: "success" },
    { label: fail, value: { kind: "outcome", outcome: "fail" }, variant: "danger" },
  ];
}

/** A death save: a success (10+), a failure, and the naturals that change the count (a 20 wakes, a 1 is two failures). */
export const DEATH_SAVE_OUTCOMES: Outcome<D20Input>[] = [
  { label: "Success", value: { kind: "outcome", outcome: "pass" }, variant: "success" },
  { label: "Failure", value: { kind: "outcome", outcome: "fail" }, variant: "danger" },
  { label: "Natural 20", value: { kind: "outcome", outcome: "nat20" } },
  { label: "Natural 1", value: { kind: "outcome", outcome: "nat1" } },
];
