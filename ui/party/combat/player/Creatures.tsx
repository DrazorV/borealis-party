import { useState } from "react";
import type { CharacterSnapshot } from "../../../src/api/contract";
import { Button } from "../../../src/components/Button";
import { Alert, Chip } from "../../../src/components/Card";
import { useConfirm } from "../../../src/components/Confirm";
import { ConditionChips } from "../shared/ConditionChips";
import { RollOrType } from "../shared/RollOrType";
import type { D20Input } from "../shared/types";
import type { PlayerCombat, PlayerCombatant } from "./api";
import { playerWrite } from "./store";

/**
 * The player's own controlled creatures (summons, pets, companions; plan 4.4 "Controlled creatures", 0.3.0): each acts
 * right after the player's PC unless given its own initiative, and the player acts for it. Its HP lives in the combat
 * (it is no character file), so changes go straight to the server, with no Apply prompt like the PC's.
 */

/** The SRD conditions the engine knows (party-combat `Condition`). */
export const CONDITIONS = [
  "blinded", "charmed", "deafened", "frightened", "grappled", "incapacitated", "invisible",
  "paralyzed", "petrified", "poisoned", "prone", "restrained", "stunned", "unconscious",
] as const;

const errorText = (e: unknown) => (e as { message?: string } | null)?.message ?? "That didn't go through.";
/** "Grip", "Grip and Hoot", "Grip, Hoot and Ash". */
const words = (names: string[]) => (names.length <= 1 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`);
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
const typed = (total: number) => ({ kind: "typed" as const, total });
const MAX_AMOUNT = 9999;

function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try { await work(); } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  return { busy, error, run };
}

/** HP after damage the way the engine takes it: temporary HP first, never below 0. */
function hurt(hp: { current: number; max: number; temp?: number }, n: number) {
  const temp = hp.temp ?? 0;
  const fromTemp = Math.min(temp, n);
  return { ...hp, temp: temp - fromTemp, current: Math.max(0, hp.current - (n - fromTemp)) };
}

function patch(c: PlayerCombat, id: string, f: (x: PlayerCombatant) => PlayerCombatant): PlayerCombat {
  return { ...c, combatants: c.combatants.map((x) => (x.id === id ? f(x) : x)) };
}

function CreatureCard({ combat, c }: { combat: PlayerCombat; c: PlayerCombatant }) {
  const { busy, error, run } = useAction();
  const confirm = useConfirm();
  const [amount, setAmount] = useState("");
  const [condition, setCondition] = useState("");
  const n = Number(amount);
  const valid = amount.trim() !== "" && Number.isInteger(n) && n > 0 && n <= MAX_AMOUNT;
  const hp = c.hp ?? { current: 0, max: 0, temp: 0 };
  const write = (action: string, args: Record<string, unknown>, guess?: (x: PlayerCombat) => PlayerCombat) =>
    run(async () => { await playerWrite(combat, action, args, guess); setAmount(""); });
  const damage = () => write("damage", { target: c.id, parts: [{ amount: typed(n), damage_type: null }], magical: false, apply_qualified: false, critical_hit: false },
    (x) => patch(x, c.id, (y) => ({ ...y, hp: hurt(hp, n) })));
  const heal = () => write("heal", { target: c.id, amount: typed(n) },
    (x) => patch(x, c.id, (y) => ({ ...y, hp: { ...hp, current: Math.min(hp.max || hp.current + n, hp.current + n) } })));
  const temp = () => write("grant_temp_hp", { id: c.id, amount: n }, (x) => patch(x, c.id, (y) => ({ ...y, hp: { ...hp, temp: Math.max(hp.temp ?? 0, n) } })));
  const addCondition = (name: string) => run(() => playerWrite(combat, "add_condition", { effect: { name: cap(name), target: c.id, conditions: [name] } },
    (x) => patch(x, c.id, (y) => ({ ...y, conditions: [...y.conditions, { name }] }))));
  const removeCondition = (name: string) => run(() => playerWrite(combat, "remove_condition", { target: c.id, condition: name },
    (x) => patch(x, c.id, (y) => ({ ...y, conditions: y.conditions.filter((k) => k.name !== name) }))));
  const roll = (input: D20Input) => run(() => playerWrite(combat, "roll", { id: c.id, label: "d20", roll: { d20: input } }));
  const dismiss = async () => {
    const answer = await confirm({ title: `Dismiss ${c.name}?`, body: `${c.name} leaves the fight. The DM can bring it back.`, confirmLabel: "Dismiss", danger: true });
    if (answer === "confirm") await run(() => playerWrite(combat, "dismiss", { id: c.id }, (x) => ({ ...x, combatants: x.combatants.filter((y) => y.id !== c.id) })));
  };
  const share = hp.max > 0 ? Math.max(0, Math.min(1, hp.current / hp.max)) : 0;
  const tone = hp.current <= 0 ? "down" : hp.current * 2 <= hp.max ? "bloodied" : "";
  return (
    <li className="card combat-creature" aria-label={c.name}>
      <div className="combat-me-head">
        <strong className="combat-me-name">{c.name}</strong>
        <span className="combat-me-hp" role="status" aria-label={`${c.name} hit points: ${hp.current} of ${hp.max}${hp.temp ? `, ${hp.temp} temporary` : ""}`}>
          <span className="combat-creature-hp-now">{hp.current}</span><span className="muted">/{hp.max}</span>
          {(hp.temp ?? 0) > 0 && <span className="combat-me-temp">+{hp.temp} temp</span>}
          {c.ac != null && <Chip>AC {c.ac}</Chip>}
        </span>
      </div>
      {/* The DM's damage or healing on my creature is a prompt I answer (party/docs/role-views.md); the amount as sent. */}
      {combat.prompts.flatMap((p) => (p.combatant === c.id && (p.kind === "damage" || p.kind === "heal") ? [p] : [])).map((p) => {
        const what = p.kind === "damage"
          ? `${c.name}: take ${p.amount}${p.damageType ? ` ${p.damageType}` : ""} damage${p.critical ? " (critical hit)" : ""}?`
          : `${c.name}: regain ${(p as { amount: number }).amount} hit points?`;
        const answer = (applied: boolean) => run(() => playerWrite(combat, "resolve_prompt", { promptId: p.id, applied },
          (x) => ({ ...x, prompts: x.prompts.filter((y) => y.id !== p.id) })));
        return (
          <section key={p.id} className="combat-prompt" role="alertdialog" aria-label={what}>
            <p className="combat-prompt-q">{what}</p>
            <div className="combat-prompt-actions">
              <Button variant="primary" disabled={busy} onClick={() => void answer(true)}>Apply</Button>
              <Button disabled={busy} onClick={() => void answer(false)}>Not now</Button>
            </div>
          </section>
        );
      })}
      {hp.max > 0 && <div className="combat-hp-bar" aria-hidden="true"><span className={tone} style={{ width: `${share * 100}%` }} /></div>}
      <ConditionChips conditions={c.conditions} label={`${c.name} conditions`} onRemove={(name) => void removeCondition(name)} />
      <div className="combat-creature-row">
        <label className="field-label" htmlFor={`amt-${c.id}`}>Amount</label>
        <input id={`amt-${c.id}`} className="input combat-amount" type="number" inputMode="numeric" min={1} max={MAX_AMOUNT}
          value={amount} onChange={(e) => setAmount(e.target.value)} aria-label={`${c.name} amount`} />
        <Button disabled={busy || !valid} onClick={() => void damage()} aria-label={`Damage ${c.name}`}>Damage</Button>
        <Button disabled={busy || !valid} onClick={() => void heal()} aria-label={`Heal ${c.name}`}>Heal</Button>
        <Button disabled={busy || !valid} onClick={() => void temp()} aria-label={`Temporary hit points for ${c.name}`}>Temp</Button>
      </div>
      <div className="combat-creature-row">
        <select className="input" aria-label={`Add a condition to ${c.name}`} value={condition}
          onChange={(e) => { const v = e.target.value; setCondition(""); if (v) void addCondition(v); }} disabled={busy}>
          <option value="">Add a condition…</option>
          {CONDITIONS.filter((k) => !c.conditions.some((x) => x.name === k)).map((k) => <option key={k} value={k}>{cap(k)}</option>)}
        </select>
        <Button variant="danger" disabled={busy} onClick={() => void dismiss()} aria-label={`Dismiss ${c.name}`}>Dismiss</Button>
      </div>
      <RollOrType label={`${c.name} d20`} busy={busy} onRoll={roll} />
      {c.lastRoll && <p className="muted combat-last-roll">Last roll: {c.lastRoll}</p>}
      {error && <Alert tone="danger">{error}</Alert>}
    </li>
  );
}

/** A companion from the character's sheet as a quick block (name, HP, AC). */
interface QuickBlock { name: string; hp: number; ac: number | null }

function summonArgs(owner: string, b: QuickBlock, initiative?: D20Input) {
  return {
    owner,
    // The server gives it a random id whatever is sent here (combat-api.md "summon").
    combatant: { id: "new", kind: "creature", label: b.name, public_name: b.name, hp: { current: b.hp, max: b.hp, temp: 0 }, ac: b.ac },
    ...(initiative ? { initiative } : {}),
  };
}

/** The character's companions to bring: the server's offers, else the sheet's own (a quick block). */
function companionsOf(me: PlayerCombatant, s: CharacterSnapshot): { id: string; name: string; args: Record<string, unknown> }[] {
  if (me.offers) return me.offers.map((k) => ({ id: k.key, name: k.name, args: { owner: me.id, companion: k.key } }));
  return s.companions.map((k) => ({ id: k.id, name: k.name, args: summonArgs(me.id, { name: k.name, hp: k.hp, ac: k.armorClass }) }));
}

/**
 * The player's creatures, and at the start of a fight the character's companions to bring. There is no generic Summon
 * button (user, 2026-11-14): a summon comes from casting its spell or from the DM's Add creature.
 */
export function Creatures({ combat, me, s }: { combat: PlayerCombat; me: PlayerCombatant; s: CharacterSnapshot }) {
  const { busy, error, run } = useAction();
  const mine = combat.combatants.filter((c) => c.mine && c.kind === "creature" && c.ownerId === me.id);
  const offers = companionsOf(me, s);
  const missing = combat.status === "setup" ? offers.filter((k) => !mine.some((c) => c.name === k.name)) : [];
  if (missing.length === 0 && mine.length === 0) return null;
  return (
    <section className="combat-creatures" aria-label="Your creatures">
      {missing.length > 0 && (
        <div className="combat-prompt" role="group" aria-label="Bring your companions">
          <p className="combat-prompt-q">Bring {words(missing.map((k) => k.name))} into the fight?</p>
          <div className="combat-prompt-actions">
            {missing.map((k) => (
              <Button key={k.id} variant="primary" disabled={busy}
                onClick={() => void run(() => playerWrite(combat, "summon", k.args))}>
                Bring {k.name}
              </Button>
            ))}
          </div>
          {error && <Alert tone="danger">{error}</Alert>}
        </div>
      )}
      {mine.length > 0 && (
        <>
          <h2 className="combat-h">Your creatures</h2>
          <ul className="combat-creature-list">{mine.map((c) => <CreatureCard key={c.id} combat={combat} c={c} />)}</ul>
        </>
      )}
    </section>
  );
}
