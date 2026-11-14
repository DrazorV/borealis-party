import type { PlayHitPoints } from "../../src/api/contract";
import { useCharacter } from "../../src/state/CharacterContext";
import { CompanionsCard } from "./Companions";
import { HitPointsCard as CharacterHitPoints } from "./HitPointsCard";

export * from "./HitPointsCard";

/**
 * What the Play page draws for "./HitPointsCard" (party/vite.config.ts): the character's own Hit Points card
 * (./HitPointsCard.tsx), then the Companions card right under it (./Companions.tsx, 0.3.6).
 */
export function HitPointsCard({ hp }: { hp: PlayHitPoints }) {
  const { snapshot } = useCharacter();
  return (<>
    <CharacterHitPoints hp={hp} />
    {snapshot && <CompanionsCard s={snapshot} />}
  </>);
}
