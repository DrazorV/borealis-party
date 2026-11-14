import type { TableOutcome } from "../combat/shared/types";

/**
 * A pass/fail roll pressed at the table (0.3.5, combat/shared/OutcomeButtons.tsx) goes to the roll history with its
 * outcome in words and no number: the table rolled, nobody typed a total. NaN marks it; the history shows no count.
 */
export const NO_TOTAL = Number.NaN;

const DEATH: Record<TableOutcome, string> = { pass: "Success", fail: "Failure", nat20: "Natural 20", nat1: "Natural 1" };
/** What the death save rules count for each outcome (./DeathSaves.tsx `applyDeathSave`). */
export const DEATH_TOTAL: Record<TableOutcome, number> = { pass: 10, fail: 9, nat20: 20, nat1: 1 };

export const deathOutcomeName = (o: TableOutcome): string => DEATH[o];
