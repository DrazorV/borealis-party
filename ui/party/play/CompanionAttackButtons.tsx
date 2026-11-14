import { Button } from "../../src/components/Button";
import { type CompanionAttack, signed } from "./companionAttacks";
import { useRoller } from "./rolls/RollContext";

/**
 * A companion's attacks as buttons (0.3.6): each asks for the table's total first, "Roll for me" second, then its
 * damage, exactly as the character's own attacks on the Play tab (./rolls/RollContext.tsx). The roll lands in the
 * session's history; nothing is sent to anyone unless the player shares it during a live combat.
 */
export function AttackButtons({ owner, attacks }: { owner: string; attacks: readonly CompanionAttack[] }) {
  const roller = useRoller();
  return (
    <div className="bp-companion-attacks" role="group" aria-label={`${owner} attacks`}>
      {attacks.map((a) => (
        <Button key={a.name} small icon="dice" title={a.damageText ? `Hit: ${a.damageText}` : undefined}
          aria-label={`${owner}: ${a.name}, ${signed(a.toHit)} to hit`}
          onClick={() => roller.d20({
            label: `${owner} · ${a.name}`, modifier: a.toHit, mode: "normal",
            damage: a.damage ? { label: `${owner} · ${a.name} damage`, expr: a.damage } : null,
          })}>
          {a.name} <span className="bp-companion-hit">{signed(a.toHit)}</span>
        </Button>
      ))}
    </div>
  );
}
