import { useSyncExternalStore } from "react";
import type { PlayConcentrationSpell } from "../../src/api/contract";

/**
 * Concentration saves after damage (0.3.5, roadmap P2): every damage the Play tab's Hit Points card takes while the
 * character concentrates asks for a Constitution save, DC 10 or half the damage, whichever is higher. Each hit is its
 * own save, asked in order. At 0 hit points concentration ends without a save (./HitPointsCard.tsx).
 */
export interface ConcentrationCheck {
  id: number;
  characterId: string;
  spell: PlayConcentrationSpell;
  damage: number;
  dc: number;
}

export const concentrationDc = (damage: number): number => Math.max(10, Math.floor(damage / 2));

let checks: readonly ConcentrationCheck[] = [];
let seq = 0;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

export function askConcentration(characterId: string, spell: PlayConcentrationSpell, damage: number): void {
  checks = [...checks, { id: ++seq, characterId, spell, damage, dc: concentrationDc(damage) }];
  emit();
}

export function answerConcentration(id: number): void {
  checks = checks.filter((c) => c.id !== id);
  emit();
}

/** Concentration ended (a failed save, 0 hit points, the End button): no save is left to ask for it. */
export function dropConcentrationChecks(characterId: string): void {
  checks = checks.filter((c) => c.characterId !== characterId);
  emit();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => { listeners.delete(l); };
};
const snapshot = () => checks;

/** The first save waiting for this character, or null. */
export function useConcentrationCheck(characterId: string | undefined): ConcentrationCheck | null {
  const all = useSyncExternalStore(subscribe, snapshot, snapshot);
  return all.find((c) => c.characterId === characterId) ?? null;
}

/** Tests: none waiting. */
export function resetConcentrationChecks(): void {
  checks = [];
  emit();
}
