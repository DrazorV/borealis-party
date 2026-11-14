import { useEffect, useRef, useState } from "react";
import { Button } from "../../../src/components/Button";
import { tryParseNotation } from "../../../src/lib/dice";
import { DAMAGE_TYPES, type DamageType, type DiceInput } from "./common";
import { isCharacter } from "./derive";
import { playerName, promptsFor, type SentPrompt, type SentStatus } from "./privacy";
import type { Combatant, Command, CombatState, DmCombat } from "./types";

/** How a sent prompt reads: "7 slashing damage", "5 healing". */
export function sentText(p: Pick<SentPrompt, "kind" | "amount" | "parts" | "criticalHit">): string {
  if (p.kind === "heal") return `${p.amount} healing`;
  const types = [...new Set((p.parts ?? []).map((x) => x.damage_type).filter(Boolean))];
  return `${p.amount}${types.length ? ` ${types.join(" and ")}` : ""} damage${p.criticalHit ? " (critical)" : ""}`;
}

const STATUS_WORD: Record<SentStatus | "answered", string> = {
  delivered: "Delivered · waiting for the player",
  accepted: "Accepted",
  declined: "Declined",
  answered: "Answered",
};

/**
 * "Send damage to Aria" (user, 2026-11-14): the DM never sees a player's HP, so damage and healing to a PC, or to a
 * creature its player controls, go to the player as a prompt (D7 a) that they accept on their Play tab. Typed first
 * (a table roll); dice notation such as `2d6+3` is rolled by the server. Each sent prompt shows as delivered, then
 * accepted or declined, never the HP it left.
 */
export function SendDamage({ view, state, c, send, disabled }: {
  view: Pick<DmCombat, "prompts">; state: CombatState; c: Combatant; send: (cmd: Command) => void; disabled: boolean;
}) {
  const [amount, setAmount] = useState("");
  const [type, setType] = useState<DamageType | "">("");
  const [crit, setCrit] = useState(false);
  const who = playerName(state, c);
  const target = isCharacter(c) ? who : `${c.label} (${who}'s)`;
  const t = amount.trim();
  const n = Number(t);
  const input: DiceInput | null = !t ? null
    : Number.isInteger(n) && n > 0 ? { kind: "typed", total: n }
      : tryParseNotation(t) ? { kind: "app", notation: t, ...(crit ? { critical: true } : {}) } : null;
  const sent = useSent(view, c.id);
  const damage = () => {
    if (!input) return;
    send({ command: "damage", target: c.id, parts: [{ amount: input, ...(type ? { damage_type: type } : {}) }], ...(crit ? { critical_hit: true } : {}) });
    setAmount("");
  };
  const heal = () => {
    if (!input) return;
    send({ command: "heal", target: c.id, amount: input });
    setAmount("");
  };
  return (
    <section className="cb-send" aria-label={`Send damage to ${target}`}>
      <h4 className="cb-group-title">Send damage to {target}</h4>
      <div className="cb-hpc-row">
        <input className="input cb-amount" aria-label={`Amount for ${target}: your table roll, or dice to roll`} placeholder="7 or 2d6+3" value={amount}
          inputMode="text" enterKeyHint="send" disabled={disabled} onChange={(e) => setAmount(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") damage(); }} />
        <select className="input cb-type-select" aria-label="Damage type" value={type} disabled={disabled} onChange={(e) => setType(e.target.value as DamageType | "")}>
          <option value="">Untyped</option>
          {DAMAGE_TYPES.map((d) => <option key={d} value={d}>{d}</option>)}
        </select>
        <label className="cb-check"><input type="checkbox" checked={crit} onChange={(e) => setCrit(e.target.checked)} />Critical hit</label>
      </div>
      <div className="cb-hpc-row">
        <Button small variant="danger" disabled={disabled || !input} onClick={damage}>Send damage</Button>
        <Button small variant="success" disabled={disabled || !input} onClick={heal}>Send healing</Button>
      </div>
      {sent.length > 0 && (
        <ul className="cb-sent" aria-label={`Sent to ${target}`}>
          {sent.map((p) => (
            <li key={p.id} className={`cb-sent-${p.shown}`} data-status={p.shown}>
              <span>{sentText(p)}</span> <span className={`chip${p.shown === "accepted" ? " gold" : p.shown === "declined" ? " warning" : ""}`}>{STATUS_WORD[p.shown]}</span>
            </li>
          ))}
        </ul>
      )}
      <p className="cb-small muted">{who}'s player applies it on their Play tab. You see whether they took it, never their HP.</p>
    </section>
  );
}

type Shown = SentPrompt & { shown: SentStatus | "answered" };

/**
 * The last few prompts sent to this combatant with how they stand. A server that keeps answered prompts says
 * accepted or declined; one that drops them on an answer shows "Answered" here once one goes away.
 */
function useSent(view: Pick<DmCombat, "prompts">, id: string): Shown[] {
  const seen = useRef(new Map<string, SentPrompt>());
  const [, tick] = useState(0);
  const now = promptsFor(view, id);
  const ids = now.map((p) => p.id).join(",");
  useEffect(() => {
    let changed = false;
    for (const p of now) if (!seen.current.has(p.id)) { seen.current.set(p.id, p); changed = true; }
    if (changed) tick((x) => x + 1);
    // `ids` stands for the prompts' list.
  }, [ids]);
  const live = new Map(now.map((p) => [p.id, p]));
  const all = [...new Map([...seen.current, ...live]).values()].sort((a, b) => (b.at ?? 0) - (a.at ?? 0)).slice(0, 4);
  return all.map((p) => {
    const cur = live.get(p.id);
    if (!cur) return { ...p, shown: "answered" };
    const s = cur.status;
    return { ...cur, shown: s === "accepted" || s === "declined" ? s : "delivered" };
  });
}
