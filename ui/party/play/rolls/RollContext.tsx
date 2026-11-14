import { type ReactNode, createContext, useCallback, useContext, useMemo, useState } from "react";
import { type DiceExpr, type DiceResult, type RollMode, critical, rollD20, rollExpr } from "../../../src/lib/dice";
import { type RollEntry, addRoll, currentDie } from "../../../src/pages/play/rolls/rollStore";
import type { D20Input } from "../../combat/shared/types";
import { markShareable } from "./shareable";
import { RollResultCard } from "./RollResult";
import { TypedRollSheet } from "./TypedRollSheet";

/**
 * The Play page's rolls, table first (0.3.5, roadmap "Type my d20 on every roll"; P4, P22, D43). Upstream's
 * `src/pages/play/rolls/RollContext.tsx` rolls the app's dice at once; this one stands in for it (party/vite.config.ts,
 * for every Play file that imports it) with the same API, so every d20 button upstream draws (checks, saves, skills,
 * initiative, attacks, spell attacks) first asks for the total of the dice rolled at the table, with "Roll for me" second
 * (hidden by Settings' "Hide Roll for me"). An attack then offers its damage the same way. `dice` stays the app's roll at
 * once: the hit die it rolls needs its face back at once (src/pages/play/DiceCards.tsx).
 * Every roll, typed or rolled, goes to the session's history (upstream's rollStore), where it can be shared with the DM.
 */

export interface DiceRequest {
  label: string;
  expr: DiceExpr;
  /** Damage dice never count below this (the attack's `dieFloor`). */
  floor?: number | null;
}

export interface D20Request {
  label: string;
  modifier: number;
  mode: RollMode;
  critRange?: number;
  damage?: DiceRequest | null;
}

export interface RollApi {
  d20: (r: D20Request) => void;
  dice: (r: DiceRequest & { critical?: boolean }) => DiceResult;
}

export interface Shown { entry: RollEntry; action: { label: string; run: () => void } | null }

/** What the typed-roll sheet asks for: a d20 test, or an attack's damage after its attack roll. */
export type Asked =
  | { kind: "d20"; request: D20Request }
  | { kind: "dice"; request: DiceRequest & { critical?: boolean } };

const Ctx = createContext<RollApi | null>(null);

const MODE_NOTE: Record<RollMode, string> = { normal: "", advantage: ", advantage", disadvantage: ", disadvantage" };

export function RollProvider({ characterId, children }: { characterId: string; children: ReactNode }) {
  const [shown, setShown] = useState<Shown | null>(null);
  const [asked, setAsked] = useState<Asked | null>(null);
  const appDice = useCallback((r: DiceRequest & { critical?: boolean }, shareable = false) => {
    const expr = r.critical ? critical(r.expr) : r.expr;
    const result = rollExpr(expr, currentDie(), { floor: r.floor, label: r.critical ? "critical" : undefined });
    const entry = addRoll({ characterId, label: r.label, total: result.total, breakdown: result.breakdown });
    if (shareable) markShareable(entry.id, "dice");
    setShown({ entry, action: null });
    return result;
  }, [characterId]);
  const dice = useCallback((r: DiceRequest & { critical?: boolean }) => appDice(r), [appDice]);
  const damageAction = useCallback((damage: DiceRequest | null | undefined, crit: boolean): Shown["action"] => {
    if (!damage) return null;
    const label = crit ? `${damage.label}, critical` : damage.label;
    return { label: crit ? "Critical damage" : "Damage", run: () => setAsked({ kind: "dice", request: { ...damage, label, critical: crit } }) };
  }, []);
  const answer = useCallback((input: D20Input) => {
    const a = asked;
    setAsked(null);
    if (!a) return;
    if (a.kind === "dice") {
      if (input.kind === "app") { appDice(a.request, true); return; }
      const entry = addRoll({ characterId, label: a.request.label, total: input.total, breakdown: "rolled at the table" });
      markShareable(entry.id, "dice");
      setShown({ entry, action: null });
      return;
    }
    const r = a.request;
    if (input.kind === "app") {
      const result = rollD20(r.modifier, r.mode, currentDie(), { critRange: r.critRange });
      const crit = Boolean(r.damage) && result.isCrit;
      const followUp = r.damage ? { label: crit ? "Roll critical damage" : "Roll damage", critical: crit } : null;
      const entry = addRoll({ characterId, label: r.label, total: result.total, breakdown: result.breakdown, followUp });
      markShareable(entry.id, "d20");
      setShown({ entry, action: damageAction(r.damage, crit) });
      return;
    }
    const entry = addRoll({ characterId, label: r.label, total: input.total, breakdown: `rolled at the table${MODE_NOTE[r.mode]}` });
    markShareable(entry.id, "d20");
    setShown({ entry, action: damageAction(r.damage, false) });
  }, [asked, appDice, characterId, damageAction]);
  const d20 = useCallback((r: D20Request) => setAsked({ kind: "d20", request: r }), []);
  const api = useMemo(() => ({ d20, dice }), [d20, dice]);
  const dismiss = useCallback(() => setShown(null), []);
  return (
    <Ctx.Provider value={api}>
      {children}
      <RollResultCard shown={shown} onDismiss={dismiss} />
      {asked && <TypedRollSheet asked={asked} onRoll={answer} onClose={() => setAsked(null)} />}
    </Ctx.Provider>
  );
}

export function useRoller(): RollApi {
  const api = useContext(Ctx);
  if (!api) throw new Error("useRoller needs a RollProvider");
  return api;
}
