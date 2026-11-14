/**
 * Which of the session's rolls can be shared with the DM (0.3.5), by roll id: d20 tests (checks, saves, skills,
 * attacks, initiative, concentration) as `d20`, attack damage as `dice`. Hit dice and death saves are never shared:
 * they tell the DM about hit points and resources (the DM-privacy rule). Ids the set has never seen are not shareable.
 */
const kinds = new Map<number, "d20" | "dice">();
const shared = new Set<number>();
const listeners = new Set<() => void>();
let version = 0;

export function markShareable(id: number, kind: "d20" | "dice"): void {
  kinds.set(id, kind);
}

export const shareKind = (id: number): "d20" | "dice" | null => kinds.get(id) ?? null;

export const wasShared = (id: number): boolean => shared.has(id);

export function markShared(id: number): void {
  shared.add(id);
  version += 1;
  listeners.forEach((l) => l());
}

export const sharedVersion = (): number => version;
export const subscribeShared = (l: () => void) => {
  listeners.add(l);
  return () => { listeners.delete(l); };
};

/** Tests: forget every roll. */
export function resetShareable(): void {
  kinds.clear();
  shared.clear();
  version += 1;
}
