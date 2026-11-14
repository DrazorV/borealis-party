import { useEffect, useMemo, useState } from "react";
import type { Attack, CharacterSnapshot, SpellEntry } from "../../../src/api/contract";
import { Button } from "../../../src/components/Button";
import { Alert, Chip } from "../../../src/components/Card";
import { Modal } from "../../../src/components/Modal";
import { useToast } from "../../../src/components/Toast";
import { type CastChoice, castChoices, choiceLabel, defaultChoice } from "../../../src/pages/play/cast";
import { usePlayMutate } from "../../../src/pages/play/playQueue";
import { RollOrType } from "../shared/RollOrType";
import type { D20Input, DiceInput } from "../shared/types";
import { actingId, castArgs, type PlayerCombat, type PlayerCombatant } from "./api";
import { playerWrite } from "./store";

/**
 * The player's own turn panel (0.3.3, user 2026-11-14: the Play tab is the player's screen): their action, bonus action
 * and reaction, a button for each attack and each ready spell on their sheet (grouped by what it costs), and their
 * own slot pips. Everything is typed first: an attack or a damage roll is the table's dice, "Roll for me" second.
 *
 * - **Attack:** pick a visible target, type the attack total (the server checks it against the target's AC), then the
 *   damage; it reaches the DM's log, and the DM applies it.
 * - **Spell:** pick the slot (the sheet's own choices) and the targets, Cast. The server applies its effect (Bless on
 *   the party, a save the DM's creatures make, which the DM resolves); the slot is spent on the player's own sheet,
 *   so the DM never sees it.
 * - The action economy is this device's own reminder: it resets when the player's turn starts.
 */

type Cost = "action" | "bonus" | "reaction";
const COST_WORD: Record<Cost, string> = { action: "Action", bonus: "Bonus action", reaction: "Reaction" };
const errorText = (e: unknown) => (e as { message?: string } | null)?.message ?? "That didn't go through.";

/** What a spell costs to cast, from its casting time ("1 bonus action"); null for a minute or longer. */
export function castCost(spell: Pick<SpellEntry, "casting">): Cost | null {
  const t = (spell.casting?.castingTime ?? "1 action").toLowerCase();
  if (t.includes("bonus")) return "bonus";
  if (t.includes("reaction")) return "reaction";
  if (/\baction\b/.test(t)) return "action";
  return null;
}

/** The dice of an attack's damage text ("1d8+3 slashing" → "1d8+3"), for "Roll for me". */
export function damageDice(text: string): string | null {
  const m = text.replace(/\s+/g, "").match(/^\d*d\d+([+-]\d+)?/i);
  return m ? m[0] : null;
}

/** This device's action economy for the player's turn: spent flags, cleared when their turn starts again. */
function useEconomy(combat: PlayerCombat, me: PlayerCombatant) {
  const mine = actingId(combat) === me.id;
  const round = combat.round;
  const [spent, setSpent] = useState<Record<Cost, boolean>>({ action: false, bonus: false, reaction: false });
  useEffect(() => {
    // A new turn of mine: action and bonus action back; the reaction comes back at the start of my turn too.
    if (mine) setSpent({ action: false, bonus: false, reaction: false });
  }, [mine, round]);
  return { spent, spend: (c: Cost) => setSpent((s) => ({ ...s, [c]: true })), toggle: (c: Cost) => setSpent((s) => ({ ...s, [c]: !s[c] })) };
}

function readySpells(s: CharacterSnapshot): SpellEntry[] {
  const seen = new Set<string>();
  return s.spellcasting.classes.flatMap((c) => c.spells).filter((x) => x.ready && !seen.has(x.id) && Boolean(seen.add(x.id)));
}

function casterNumbers(s: CharacterSnapshot, spell: SpellEntry): { saveDc?: number; attackBonus?: number } {
  const block = s.spellcasting.classes.find((c) => c.spells.some((x) => x.id === spell.id));
  return block ? { saveDc: block.saveDc, attackBonus: block.attackBonus } : {};
}

export function TurnPanel({ combat, me, s }: { combat: PlayerCombat; me: PlayerCombatant; s: CharacterSnapshot }) {
  const economy = useEconomy(combat, me);
  const [attack, setAttack] = useState<Attack | null>(null);
  const [spell, setSpell] = useState<SpellEntry | null>(null);
  const mine = actingId(combat) === me.id;
  const attacks = s.attacks.filter((a) => a.showInCombat !== false);
  const spells = readySpells(s);
  const byCost = (c: Cost) => spells.filter((x) => castCost(x) === c);
  const active = combat.status === "active";
  const slots = [...s.play.spellSlots.filter((x) => x.max > 0), ...(s.play.pactSlots && s.play.pactSlots.max > 0 ? [{ ...s.play.pactSlots, pact: true }] : [])];
  return (
    <section className="card combat-turn" aria-label="Your turn panel">
      <div className="combat-turn-head">
        <h2 className="combat-h">Your actions</h2>
        <span className="combat-econ" role="group" aria-label="Action economy">
          {(Object.keys(COST_WORD) as Cost[]).map((c) => (
            <button key={c} type="button" className={`combat-econ-pip${economy.spent[c] ? " spent" : ""}`} aria-pressed={economy.spent[c]}
              title={`${COST_WORD[c]}${economy.spent[c] ? " used (tap to take back)" : " ready"}`} onClick={() => economy.toggle(c)}>
              {c === "action" ? "A" : c === "bonus" ? "B" : "R"}<span className="sr-only"> {COST_WORD[c]}{economy.spent[c] ? " used" : " ready"}</span>
            </button>
          ))}
        </span>
      </div>
      {slots.length > 0 && (
        <div className="combat-slotpips" aria-label="Your spell slots">
          {slots.map((p) => (
            <span key={`${"pact" in p ? "p" : ""}${p.level}`} className="combat-slot" aria-label={`${"pact" in p ? "Pact Magic level" : "Level"} ${p.level}: ${p.max - p.used} of ${p.max} left`}>
              <span className="combat-slot-l">{"pact" in p ? "P" : "L"}{p.level}</span>
              {Array.from({ length: p.max }, (_, i) => <span key={i} className={`combat-slot-pip${i < p.max - p.used ? " on" : ""}`} aria-hidden="true" />)}
            </span>
          ))}
        </div>
      )}
      {!active && <p className="muted combat-turn-note">The DM hasn't started the fight yet.</p>}
      {(["action", "bonus", "reaction"] as Cost[]).map((cost) => {
        const list = [
          ...(cost === "action" ? attacks.map((a) => ({ key: `a-${a.id}`, label: `${a.name}${a.attackBonus ? ` ${a.attackBonus}` : ""}${a.damage ? ` · ${a.damage}` : ""}`, run: () => setAttack(a), off: false, why: "" })) : []),
          ...byCost(cost).map((x) => {
            const choices = castChoices(x, s.play);
            const off = x.level > 0 && choices.length === 0;
            const left = x.level > 0 ? choices.filter((c) => c.kind === "slot").reduce((n, c) => n + (c.kind === "slot" ? c.left : 0), 0) : 0;
            return { key: `s-${x.id}`, label: x.level === 0 ? x.name : `${x.name} L${x.level}${left ? ` · ${left} slot${left === 1 ? "" : "s"}` : ""}`, run: () => setSpell(x), off, why: off ? "No slot left" : "" };
          }),
        ];
        if (list.length === 0) return null;
        return (
          <div key={cost} className="combat-turn-group" role="group" aria-label={COST_WORD[cost]}>
            <h3 className="combat-turn-title">{COST_WORD[cost]}{economy.spent[cost] && <span className="muted"> · used</span>}</h3>
            <div className="combat-turn-buttons">
              {list.map((b) => (
                <Button key={b.key} disabled={!active || b.off || (cost !== "reaction" && !mine) || economy.spent[cost]} title={b.why || undefined} onClick={b.run}>{b.label}</Button>
              ))}
            </div>
          </div>
        );
      })}
      {!mine && active && <p className="muted combat-turn-note">Actions open on your turn; a reaction works any time it's ready.</p>}
      {attack && <AttackSheet combat={combat} me={me} attack={attack} onClose={() => setAttack(null)} onDone={() => economy.spend("action")} />}
      {spell && <CastSheet combat={combat} me={me} s={s} spell={spell} onClose={() => setSpell(null)} onDone={() => economy.spend(castCost(spell) ?? "action")} />}
    </section>
  );
}

/** Who a player may point at: everyone they can see (the projection holds no hidden creature). */
function Targets({ combat, picked, onPick, max }: { combat: PlayerCombat; picked: string[]; onPick: (ids: string[]) => void; max: number }) {
  const list = combat.combatants.filter((c) => c.inOrder && c.life !== "dead");
  const toggle = (id: string) => {
    if (picked.includes(id)) onPick(picked.filter((x) => x !== id));
    else onPick(max === 1 ? [id] : [...picked, id].slice(-max));
  };
  return (
    <div className="combat-targets" role="group" aria-label="Targets">
      {list.map((c) => (
        <button key={c.id} type="button" className={`combat-target${picked.includes(c.id) ? " on" : ""}`} aria-pressed={picked.includes(c.id)} onClick={() => toggle(c.id)}>
          {c.name || "You"}{c.mine && c.kind === "character" ? " (you)" : ""}
        </button>
      ))}
    </div>
  );
}

function AttackSheet({ combat, me, attack, onClose, onDone }: { combat: PlayerCombat; me: PlayerCombatant; attack: Attack; onClose: () => void; onDone: () => void }) {
  const [target, setTarget] = useState<string[]>([]);
  const [step, setStep] = useState<"attack" | "damage" | "done">("attack");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dmg, setDmg] = useState("");
  const toHit = attack.toHit ?? (Number(attack.attackBonus.replace(/[^\d-]/g, "")) || 0);
  const dice = damageDice(attack.damage);
  const who = combat.combatants.find((c) => c.id === target[0])?.name ?? "the target";
  const go = async (work: () => Promise<unknown>, next: typeof step) => {
    setBusy(true);
    setError(null);
    try { await work(); setStep(next); if (next === "damage") onDone(); } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  const roll = (input: D20Input) => go(() => playerWrite(combat, "attack", { attacker: me.id, target: target[0], label: attack.name, roll: input }), "damage");
  const damage = (input: DiceInput) => go(() => playerWrite(combat, "roll", { id: me.id, label: `${attack.name} damage to ${who}`, roll: { dice: input } }), "done");
  const typedDmg = /^\d+$/.test(dmg.trim()) ? Number(dmg.trim()) : null;
  const last = me.lastRoll;
  return (
    <Modal title={attack.name} onClose={onClose}>
      <div className="combat-sheet">
        {step === "attack" && <>
          <p className="combat-sheet-lead">Who are you attacking?</p>
          <Targets combat={combat} picked={target} onPick={setTarget} max={1} />
          {target.length > 0 && <RollOrType label={`${attack.name} attack roll`} modifier={toHit} busy={busy} onRoll={roll} autoFocus />}
        </>}
        {step === "damage" && <>
          {last && <p className="combat-sheet-lead" role="status">{last}</p>}
          <p className="combat-sheet-lead">Damage{attack.damage ? ` (${attack.damage})` : ""}, if it hit:</p>
          <form className="combat-table-roll" onSubmit={(e) => { e.preventDefault(); if (typedDmg !== null) void damage({ kind: "typed", total: typedDmg }); }}>
            <input className="input" inputMode="numeric" enterKeyHint="send" placeholder="Your roll" aria-label={`${attack.name} damage: my table roll`} autoFocus
              value={dmg} onChange={(e) => setDmg(e.target.value)} disabled={busy} />
            <Button type="submit" variant="primary" disabled={busy || typedDmg === null}>Send</Button>
            {dice && <Button variant="ghost" icon="dice" disabled={busy} onClick={() => void damage({ kind: "app", notation: dice })}>Roll for me ({dice})</Button>}
          </form>
          <Button variant="ghost" onClick={onClose}>It missed</Button>
        </>}
        {step === "done" && <><p className="combat-sheet-lead" role="status">Sent to the DM{last ? `: ${last}` : ""}.</p><Button variant="primary" onClick={onClose}>Done</Button></>}
        {error && <Alert tone="danger">{error}</Alert>}
      </div>
    </Modal>
  );
}

function CastSheet({ combat, me, s, spell, onClose, onDone }: {
  combat: PlayerCombat; me: PlayerCombatant; s: CharacterSnapshot; spell: SpellEntry; onClose: () => void; onDone: () => void;
}) {
  const run = usePlayMutate();
  const toast = useToast();
  const choices = useMemo(() => (spell.level === 0 ? [] : castChoices(spell, s.play)), [spell, s.play]);
  const [choice, setChoice] = useState<CastChoice | null>(() => (spell.level === 0 ? null : defaultChoice(spell, choices) ?? choices[0] ?? null));
  const [targets, setTargets] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cost = castCost(spell) ?? "action";
  const concentrates = spell.concentration;
  const cast = async () => {
    setBusy(true);
    setError(null);
    try {
      const slot = choice?.kind === "slot" ? choice.level : undefined;
      await playerWrite(combat, "cast_spell", castArgs({
        caster: me.id, spellKey: spell.id, name: spell.name, level: spell.level, slot, targets, economy: cost, concentration: concentrates, ...casterNumbers(s, spell),
      }));
      // The slot and concentration are the sheet's own (the DM never sees them).
      if (choice?.kind === "slot") await run((api, characterId) => api.spendSpellSlot({ characterId, level: choice.level, pact: choice.pact }));
      if (concentrates && s.play.concentrationSpells.some((c) => c.id === spell.id)) await run((api, characterId) => api.setConcentration({ characterId, spellId: spell.id }));
      toast.push(`Cast ${spell.name}${slot ? ` at level ${slot}` : ""}.`);
      onDone();
      onClose();
    } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  const needsSlot = spell.level > 0 && !choice;
  return (
    <Modal title={`Cast ${spell.name}`} onClose={onClose}>
      <div className="combat-sheet">
        <p className="muted combat-sheet-lead">{COST_WORD[cost]}{spell.casting ? ` · ${spell.casting.range} · ${spell.casting.duration}` : ""}{concentrates ? " · concentration" : ""}</p>
        {spell.level > 0 && (
          <div className="combat-targets" role="group" aria-label="Slot">
            {choices.map((c, i) => (
              <button key={i} type="button" className={`combat-target${choice === c ? " on" : ""}`} aria-pressed={choice === c} onClick={() => setChoice(c)}>{choiceLabel(c)}</button>
            ))}
            {choices.length === 0 && <Chip tone="danger">No slot left</Chip>}
          </div>
        )}
        <p className="combat-sheet-lead">Targets</p>
        <Targets combat={combat} picked={targets} onPick={setTargets} max={12} />
        <div className="combat-prompt-actions">
          <Button variant="primary" disabled={busy || needsSlot} onClick={() => void cast()}>Cast {spell.name}</Button>
          <Button disabled={busy} onClick={onClose}>Cancel</Button>
        </div>
        <p className="muted combat-turn-note">What it does reaches the DM: effects on its targets, and saves the DM's creatures make. Your slots stay yours.</p>
        {error && <Alert tone="danger">{error}</Alert>}
      </div>
    </Modal>
  );
}
