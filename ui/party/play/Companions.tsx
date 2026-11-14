import { useState, useSyncExternalStore } from "react";
import type { CharacterSnapshot, CompanionState } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { Chip } from "../../src/components/Card";
import { Expander } from "../../src/components/controls";
import { CONDITIONS, CreatureCard } from "../combat/player/Creatures";
import { myCombatant, type PlayerCombat, type PlayerCombatant } from "../combat/player/api";
import { useCombatFor } from "../combat/player/store";
import { ConditionChips } from "../combat/shared/ConditionChips";
import "../combat/shared/combat.css";
import { AttackButtons } from "./CompanionAttackButtons";
import { companionAttacks } from "./companionAttacks";
import "./companions.css";

/**
 * The Companions card on the Play tab (0.3.6, roadmap "Companion and summon stat-block card on Play"): one card per
 * companion, familiar or summon the open character controls, with its name, AC, hit points with Damage / Heal / Temp,
 * its attacks as buttons (typed total first, as the character's own) and its conditions.
 *
 * - Outside a live combat (or a companion that isn't in it) the hit points and conditions are kept on this device,
 *   per character and companion: nothing is sent to anyone, as with the death saves card (./DeathSaves.tsx).
 * - In a live combat, a creature of mine in the fight is the combat's own card (combat/player/Creatures.tsx): its HP
 *   lives in the combat, the DM's damage is a prompt answered with Apply / Not now, and it follows the same privacy
 *   rules as the player's own card. A summon (no sheet companion) shows up only then. The live combat section above
 *   keeps the "Bring them into the fight?" prompt and leaves these cards to this one (./PlayTop.tsx).
 *
 * The cards sit in a grid: equal height in a row on wide screens, one column on a phone (./companions.css).
 */

export interface LocalCompanion { damage: number; temp: number; conditions: string[] }

const NONE: LocalCompanion = { damage: 0, temp: 0, conditions: [] };
const MAX_AMOUNT = 9999;
const key = (characterId: string, companionId: string) => `party.companion.${characterId}.${companionId}`;
const cache = new Map<string, LocalCompanion>();
const listeners = new Set<() => void>();

const count = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? Math.max(0, Math.min(MAX_AMOUNT, Math.floor(v))) : 0);

export function localCompanion(characterId: string, companionId: string): LocalCompanion {
  const k = key(characterId, companionId);
  const hit = cache.get(k);
  if (hit) return hit;
  let value = NONE;
  try {
    const raw = JSON.parse(localStorage.getItem(k) ?? "null") as Partial<LocalCompanion> | null;
    if (raw) {
      const known = new Set<string>(CONDITIONS);
      value = {
        damage: count(raw.damage), temp: count(raw.temp),
        conditions: Array.isArray(raw.conditions) ? raw.conditions.filter((c): c is string => typeof c === "string" && known.has(c)) : [],
      };
    }
  } catch { /* unreadable or blocked storage: start fresh */ }
  cache.set(k, value);
  return value;
}

export function setLocalCompanion(characterId: string, companionId: string, value: LocalCompanion) {
  const k = key(characterId, companionId);
  cache.set(k, value);
  try {
    if (value.damage === 0 && value.temp === 0 && value.conditions.length === 0) localStorage.removeItem(k);
    else localStorage.setItem(k, JSON.stringify(value));
  } catch { /* storage blocked: kept for this session */ }
  listeners.forEach((l) => l());
}

function useLocalCompanion(characterId: string, companionId: string): LocalCompanion {
  return useSyncExternalStore(
    (l) => { listeners.add(l); return () => { listeners.delete(l); }; },
    () => localCompanion(characterId, companionId),
  );
}

/** Damage the way the rules take it: temporary hit points first, never below 0. */
export function damageLocal(v: LocalCompanion, maxHp: number, n: number): LocalCompanion {
  const fromTemp = Math.min(v.temp, n);
  return { ...v, temp: v.temp - fromTemp, damage: Math.min(maxHp, v.damage + n - fromTemp) };
}
export const healLocal = (v: LocalCompanion, n: number): LocalCompanion => ({ ...v, damage: Math.max(0, v.damage - n) });
export const tempLocal = (v: LocalCompanion, n: number): LocalCompanion => ({ ...v, temp: Math.max(v.temp, n) });

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** The AC and HP a sheet companion prints: the number, or its formula note when the stat block has one. */
const acText = (k: CompanionState) => (k.armorClassNote?.trim() ? k.armorClassNote : String(k.armorClass));

function LocalCompanionCard({ s, k, note }: { s: CharacterSnapshot; k: CompanionState; note?: string }) {
  const v = useLocalCompanion(s.id, k.id);
  const [amount, setAmount] = useState("");
  const [condition, setCondition] = useState("");
  const n = Number(amount);
  const valid = amount.trim() !== "" && Number.isInteger(n) && n > 0 && n <= MAX_AMOUNT;
  const max = Math.max(0, k.hp);
  const current = Math.max(0, max - v.damage);
  const save = (next: LocalCompanion) => { setLocalCompanion(s.id, k.id, next); setAmount(""); };
  const share = max > 0 ? current / max : 0;
  const tone = current <= 0 ? "down" : current * 2 <= max ? "bloodied" : "";
  const attacks = companionAttacks(k.actions);
  const id = `cmp-${k.id}`;
  return (
    <li className="card combat-creature bp-companion" aria-label={k.name}>
      <div className="combat-me-head">
        <strong className="combat-me-name">{k.name}</strong>
        <span className="combat-me-hp" role="status" aria-label={`${k.name} hit points: ${current} of ${max}${v.temp ? `, ${v.temp} temporary` : ""}`}>
          <span className="combat-creature-hp-now">{current}</span><span className="muted">/{max}</span>
          {v.temp > 0 && <span className="combat-me-temp">+{v.temp} temp</span>}
          <Chip>AC {acText(k)}</Chip>
        </span>
      </div>
      {k.build && <p className="muted bp-companion-build">{k.build}</p>}
      {note && <p className="muted bp-companion-note">{note}</p>}
      {max > 0 && <div className="combat-hp-bar" aria-hidden="true"><span className={tone} style={{ width: `${share * 100}%` }} /></div>}
      <ConditionChips conditions={v.conditions.map((name) => ({ name }))} label={`${k.name} conditions`}
        onRemove={(name) => save({ ...v, conditions: v.conditions.filter((c) => c !== name) })} />
      <div className="combat-creature-row">
        <label className="field-label" htmlFor={`amt-${id}`}>Amount</label>
        <input id={`amt-${id}`} className="input combat-amount" type="number" inputMode="numeric" min={1} max={MAX_AMOUNT}
          value={amount} onChange={(e) => setAmount(e.target.value)} aria-label={`${k.name} amount`} />
        <Button disabled={!valid} onClick={() => save(damageLocal(v, max, n))} aria-label={`Damage ${k.name}`}>Damage</Button>
        <Button disabled={!valid} onClick={() => save(healLocal(v, n))} aria-label={`Heal ${k.name}`}>Heal</Button>
        <Button disabled={!valid} onClick={() => save(tempLocal(v, n))} aria-label={`Temporary hit points for ${k.name}`}>Temp</Button>
      </div>
      <div className="combat-creature-row">
        <select className="input" aria-label={`Add a condition to ${k.name}`} value={condition}
          onChange={(e) => { const c = e.target.value; setCondition(""); if (c) save({ ...v, conditions: [...v.conditions, c] }); }}>
          <option value="">Add a condition…</option>
          {CONDITIONS.filter((c) => !v.conditions.includes(c)).map((c) => <option key={c} value={c}>{cap(c)}</option>)}
        </select>
        {(v.damage > 0 || v.temp > 0) && <Button onClick={() => save({ ...v, damage: 0, temp: 0 })} aria-label={`${k.name}: full hit points`}>Full HP</Button>}
      </div>
      {attacks.length > 0 && <AttackButtons owner={k.name} attacks={attacks} />}
    </li>
  );
}

/** What the card lists: each sheet companion (in the fight or not), then my other creatures in the fight (summons). */
export function companionCards(s: CharacterSnapshot, combat: PlayerCombat | null, me: PlayerCombatant | null) {
  const fighting = combat && me
    ? combat.combatants.filter((c) => c.mine && c.kind === "creature" && c.ownerId === me.id)
    : [];
  const used = new Set<string>();
  const sheet = (s.companions ?? []).map((k) => {
    const live = fighting.find((c) => !used.has(c.id) && c.name === k.name) ?? null;
    if (live) used.add(live.id);
    return { k, live };
  });
  const summons = fighting.filter((c) => !used.has(c.id));
  return { sheet, summons };
}

export function CompanionsCard({ s }: { s: CharacterSnapshot }) {
  const combat = useCombatFor({ id: s.id, name: s.name });
  const me = combat ? myCombatant(combat, { id: s.id, name: s.name }) : null;
  const inFight = Boolean(combat && me && me.inInitiative !== false);
  const { sheet, summons } = companionCards(s, inFight ? combat : null, inFight ? me : null);
  if (sheet.length === 0 && summons.length === 0) return null;
  return (
    <Expander title="Companions">
      <ul className="bp-companions" aria-label="Companions">
        {sheet.map(({ k, live }) => (live && combat
          ? <CreatureCard key={k.id} combat={combat} c={live} attacks={companionAttacks(k.actions)} />
          : <LocalCompanionCard key={k.id} s={s} k={k} note={inFight ? "Not in this fight: kept on this device." : undefined} />))}
        {combat && summons.map((c) => <CreatureCard key={c.id} combat={combat} c={c} />)}
      </ul>
    </Expander>
  );
}
