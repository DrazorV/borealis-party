import { useEffect, useSyncExternalStore } from "react";
import type { CharacterSnapshot } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { useToast } from "../../src/components/Toast";
import { rollD20 } from "../../src/lib/dice";
import { usePlayMutate } from "../../src/pages/play/playQueue";
import { addRoll, currentDie } from "../../src/pages/play/rolls/rollStore";
import { RollOrType } from "../combat/shared/RollOrType";
import type { D20Input } from "../combat/shared/types";
import "../combat/shared/combat.css";
import "./quick-wins.css";

/**
 * Death saves on the Play sub-tab outside a tracked combat (0.3.5, roadmap P10): at 0 hit points the player rolls at
 * the table and types the d20 (or "Roll for me"). 10 or more is a success, 9 or less a failure, a 1 two failures, a 20
 * brings them back with 1 hit point. Three successes: stable; three failures: dead. Damage at 0 hit points is a
 * failure (two on a critical hit). Inside a live combat the combat section's own death saves are the ones (the server
 * keeps them), so this card steps aside (./PlayTop.tsx).
 *
 * The count is kept on this device for the character (it is about a few minutes of one fight); it clears once the
 * character has hit points again.
 */
export interface DeathSaves { successes: number; failures: number }

const NONE: DeathSaves = { successes: 0, failures: 0 };
const key = (characterId: string) => `party.deathSaves.${characterId}`;
const cache = new Map<string, DeathSaves>();
const listeners = new Set<() => void>();

export function deathSavesOf(characterId: string): DeathSaves {
  const hit = cache.get(characterId);
  if (hit) return hit;
  let value = NONE;
  try {
    const raw = JSON.parse(localStorage.getItem(key(characterId)) ?? "null") as Partial<DeathSaves> | null;
    if (raw) value = { successes: clamp(raw.successes), failures: clamp(raw.failures) };
  } catch { /* none kept */ }
  cache.set(characterId, value);
  return value;
}

function clamp(n: unknown): number {
  return typeof n === "number" && Number.isFinite(n) ? Math.max(0, Math.min(3, Math.floor(n))) : 0;
}

export function setDeathSaves(characterId: string, next: DeathSaves): void {
  const value = { successes: clamp(next.successes), failures: clamp(next.failures) };
  cache.set(characterId, value);
  try {
    if (value.successes === 0 && value.failures === 0) localStorage.removeItem(key(characterId));
    else localStorage.setItem(key(characterId), JSON.stringify(value));
  } catch { /* a private window: this session only */ }
  listeners.forEach((l) => l());
}

/** One death save's d20 (the total typed or rolled; a death save adds nothing unless a feature says so). */
export function applyDeathSave(s: DeathSaves, total: number): { saves: DeathSaves; revived: boolean } {
  if (total >= 20) return { saves: NONE, revived: true };
  if (total >= 10) return { saves: { ...s, successes: Math.min(3, s.successes + 1) }, revived: false };
  return { saves: { ...s, failures: Math.min(3, s.failures + (total <= 1 ? 2 : 1)) }, revived: false };
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => { listeners.delete(l); };
};

export function useDeathSaves(characterId: string): DeathSaves {
  return useSyncExternalStore(subscribe, () => deathSavesOf(characterId), () => deathSavesOf(characterId));
}

/** Tests: read the device again. */
export function resetDeathSaves(): void {
  cache.clear();
  listeners.forEach((l) => l());
}

function Pips({ label, filled, tone }: { label: string; filled: number; tone: "success" | "danger" }) {
  return (
    <span className="combat-pips" role="img" aria-label={`${label}: ${filled} of 3`}>
      <span className="combat-pips-label">{label}</span>
      {[0, 1, 2].map((i) => <span key={i} className={`combat-pip ${tone}${i < filled ? " on" : ""}`} />)}
    </span>
  );
}

/** The dying / stable / dead card, while the character is at 0 hit points (or a count is still kept). */
export function DeathSavesCard({ s }: { s: CharacterSnapshot }) {
  const saves = useDeathSaves(s.id);
  const run = usePlayMutate();
  const toast = useToast();
  const hp = s.play.hitPoints;
  const up = hp.current > 0;
  // Back on their feet (healed, a long rest): the count starts over.
  useEffect(() => { if (up && (saves.successes > 0 || saves.failures > 0)) setDeathSaves(s.id, NONE); }, [up, saves, s.id]);
  if (up) return null;
  const stable = saves.successes >= 3;
  const dead = saves.failures >= 3;
  const state = dead ? "Dead" : stable ? "Stable" : "Dying";
  const roll = async (input: D20Input) => {
    let total: number;
    if (input.kind === "typed") {
      total = input.total;
      addRoll({ characterId: s.id, label: "Death save", total, breakdown: "rolled at the table" });
    } else {
      const r = rollD20(0, "normal", currentDie());
      total = r.total;
      addRoll({ characterId: s.id, label: "Death save", total, breakdown: r.breakdown });
    }
    const { saves: next, revived } = applyDeathSave(saves, total);
    setDeathSaves(s.id, next);
    if (revived) {
      await run((api, id) => api.regainHitPoints({ characterId: id, amount: 1 }));
      toast.push("A 20: you are back up with 1 hit point.");
    } else if (next.successes >= 3) toast.push("Three successes: you are stable.");
    else if (next.failures >= 3) toast.push("Three failures: your character dies.", "warning");
  };
  const hurt = (n: number) => setDeathSaves(s.id, { successes: stable ? 0 : saves.successes, failures: (stable ? 0 : saves.failures) + n });
  return (
    <section className="card combat-me bp-death-card" aria-label="Death saves">
      <div className="combat-me-head">
        <strong className="combat-me-name">{state}</strong>
        <span className="muted">0 hit points</span>
      </div>
      <div className="combat-death" role="group" aria-label="Death save count">
        <div className="combat-death-pips">
          <Pips label="Successes" filled={saves.successes} tone="success" />
          <Pips label="Failures" filled={saves.failures} tone="danger" />
        </div>
        {!stable && !dead && <RollOrType label="Death save" onRoll={(i) => void roll(i)} />}
        {stable && <p className="v5-note">Stable: no more death saves. You regain 1 hit point after 1d4 hours, or when healed.</p>}
        {!dead && (
          <div className="bp-death-hurt" role="group" aria-label="Damage at 0 hit points">
            <Button small onClick={() => hurt(1)}>Took damage</Button>
            <Button small onClick={() => hurt(2)}>Critical hit</Button>
          </div>
        )}
        {(saves.successes > 0 || saves.failures > 0) && <Button small variant="ghost" onClick={() => setDeathSaves(s.id, NONE)}>Clear</Button>}
      </div>
    </section>
  );
}
