import { useMemo, useState } from "react";
import { Button } from "../../../src/components/Button";
import { Dialog } from "../../../src/components/controls/Dialog";
import { tryParseNotation } from "../../../src/lib/dice";
import { ABILITY_NAMES, DAMAGE_TYPES, type AbilityName, type DamageType, type DiceInput } from "./common";
import { isCharacter, isDefeated, rosterOrder } from "./derive";
import { saveMod } from "./ResolutionSheet";
import type { Combatant, Command, CombatState, MultiTarget, RowRoll, StatBlock } from "./types";

const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);

/** One target's save as the DM has it so far: a typed total, the DM's verdict, or "roll it in the app". */
type SaveEntry = { kind: "none" } | { kind: "typed"; text: string } | { kind: "saved" } | { kind: "failed" } | { kind: "app" };

function rowRoll(e: SaveEntry): RowRoll | null {
  switch (e.kind) {
    case "typed": return /^-?\d+$/.test(e.text.trim()) ? { kind: "typed", total: Number(e.text.trim()) } : null;
    case "saved": return { kind: "outcome", outcome: "saved" };
    case "failed": return { kind: "outcome", outcome: "failed" };
    case "app": return { kind: "app" };
    default: return null;
  }
}

/** The amount box: a typed total (D43) or dice the server rolls once (`8d6`). */
export function parseAmount(text: string): DiceInput | null {
  const t = text.trim();
  if (!t) return null;
  const n = Number(t);
  if (Number.isInteger(n) && n >= 0) return { kind: "typed", total: n };
  return tryParseNotation(t) ? { kind: "app", notation: t } : null;
}

/** What the sheet sends: `multi_damage`, or null while something it needs is missing. */
export function multiDamageCommand(f: {
  amount: string; type: DamageType | null; magical: boolean; save: boolean; ability: AbilityName; dc: string; half: boolean;
  picked: string[]; saves: Record<string, SaveEntry>;
}): Extract<Command, { command: "multi_damage" }> | null {
  const amount = parseAmount(f.amount);
  if (!amount || f.picked.length === 0) return null;
  const dc = Number(f.dc);
  if (f.save && !(Number.isInteger(dc) && dc > 0)) return null;
  const targets: MultiTarget[] = [];
  for (const id of f.picked) {
    if (!f.save) { targets.push({ id }); continue; }
    const roll = rowRoll(f.saves[id] ?? { kind: "none" });
    if (!roll) return null;
    targets.push({ id, save: roll });
  }
  return {
    command: "multi_damage", targets, amount, ...(f.type ? { damage_type: f.type } : {}), ...(f.magical ? { magical: true } : {}),
    ...(f.save ? { save: { ability: f.ability, dc, half: f.half } } : {}),
  };
}

/**
 * Multi-target damage (0.3.5, D13) from the plain Damage keypad: one roll (typed first, or dice the server rolls
 * once), several targets, and each target's save: the table roll typed, Saved or Failed, or [Roll creature saves]
 * for the DM's creatures. Half on a success (or none). A PC's save is asked at the table and typed here; its damage
 * reaches the player as a prompt (D7), so the DM never sees its HP.
 */
export function MultiDamageSheet({ state, first, blockOf, send, onClose }: {
  state: CombatState;
  /** The card the sheet was opened from: picked to start with. */
  first: string;
  blockOf: (id: string) => StatBlock | undefined;
  send: (cmd: Command) => void;
  onClose: () => void;
}) {
  // Who is in the fight: not defeated, not absent, and a PC only once its initiative is in (C1).
  const list = useMemo(() => rosterOrder(state).filter((c) => !isDefeated(c) && !c.absent && !(isCharacter(c) && c.initiative == null)), [state]);
  const [amount, setAmount] = useState("");
  const [type, setType] = useState<DamageType | null>(null);
  const [magical, setMagical] = useState(false);
  const [save, setSave] = useState(true);
  const [ability, setAbility] = useState<AbilityName>("dex");
  const [dc, setDc] = useState("");
  const [half, setHalf] = useState(true);
  const [picked, setPicked] = useState<string[]>(() => (list.some((c) => c.id === first) ? [first] : []));
  const [saves, setSaves] = useState<Record<string, SaveEntry>>({});
  const set = (id: string, e: SaveEntry) => setSaves((s) => ({ ...s, [id]: e }));
  const toggle = (id: string) => setPicked((p) => (p.includes(id) ? p.filter((x) => x !== id) : [...p, id]));
  const cmd = multiDamageCommand({ amount, type, magical, save, ability, dc, half, picked, saves });
  const creaturesOpen = picked.filter((id) => {
    const c = list.find((x) => x.id === id);
    return c && !isCharacter(c) && (saves[id]?.kind ?? "none") === "none";
  });
  const rollCreatures = () => setSaves((s) => ({ ...s, ...Object.fromEntries(creaturesOpen.map((id) => [id, { kind: "app" } as SaveEntry])) }));
  const missing = save ? picked.filter((id) => !rowRoll(saves[id] ?? { kind: "none" })).length : 0;
  const apply = () => { if (cmd) { send(cmd); onClose(); } };
  return (
    <Dialog title="Damage several targets" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="danger" disabled={!cmd} onClick={apply}>Apply to {picked.length || "no"} target{picked.length === 1 ? "" : "s"}</Button>
      </>}>
      <div className="cb-multi" data-testid="multi-damage">
        <form className="cb-multi-amount" onSubmit={(e) => { e.preventDefault(); apply(); }}>
          <label className="cb-field">
            <span>Damage: the table roll, or dice to roll once</span>
            <input className="input cb-amount" autoFocus inputMode="text" placeholder="28 or 8d6" value={amount} aria-invalid={amount.trim() !== "" && !parseAmount(amount) || undefined}
              aria-label="Damage: a total, or dice to roll once" onChange={(e) => setAmount(e.target.value)} />
          </label>
          <label className="cb-field">
            <span>Type</span>
            <select className="input" aria-label="Damage type" value={type ?? ""} onChange={(e) => setType((e.target.value || null) as DamageType | null)}>
              <option value="">Untyped damage</option>
              {DAMAGE_TYPES.map((t) => <option key={t} value={t}>{t}</option>)}
            </select>
          </label>
          <label className="cb-check"><input type="checkbox" checked={magical} onChange={(e) => setMagical(e.target.checked)} />Magical</label>
        </form>
        <div className="cb-multi-save" role="group" aria-label="Save">
          <label className="cb-check"><input type="checkbox" checked={save} onChange={(e) => setSave(e.target.checked)} />Targets make a save</label>
          {save && <>
            <select className="input cb-multi-ability" aria-label="Save ability" value={ability} onChange={(e) => setAbility(e.target.value as AbilityName)}>
              {ABILITY_NAMES.map((a) => <option key={a} value={a}>{a.toUpperCase()}</option>)}
            </select>
            <input className="input cb-multi-dc" inputMode="numeric" placeholder="DC" aria-label="Save DC" value={dc} onChange={(e) => setDc(e.target.value)} />
            <label className="cb-check"><input type="checkbox" checked={half} onChange={(e) => setHalf(e.target.checked)} />Half on a success</label>
          </>}
        </div>
        <ul className="cb-res-rows cb-multi-rows" aria-label="Targets">
          {list.map((c) => (
            <TargetRow key={c.id} c={c} on={picked.includes(c.id)} save={save} ability={ability} entry={saves[c.id] ?? { kind: "none" }}
              mod={isCharacter(c) ? null : saveMod(c, blockOf(c.id), ability)} onToggle={() => toggle(c.id)} onSave={(e) => set(c.id, e)} />
          ))}
        </ul>
        {save && creaturesOpen.length > 0 && (
          <Button small icon="dice" variant="ghost" onClick={rollCreatures}>Roll creature saves ({creaturesOpen.length})</Button>
        )}
        {save && missing > 0 && <p className="cb-small muted" role="status">{missing} save{missing === 1 ? "" : "s"} still to enter.</p>}
        <p className="cb-small muted">One roll for everyone; each target's resistances still apply. Player characters get a prompt to take it.</p>
      </div>
    </Dialog>
  );
}

function TargetRow({ c, on, save, ability, entry, mod, onToggle, onSave }: {
  c: Combatant; on: boolean; save: boolean; ability: AbilityName; entry: SaveEntry; mod: number | null;
  onToggle: () => void; onSave: (e: SaveEntry) => void;
}) {
  const pc = isCharacter(c);
  return (
    <li className={`cb-res-row cb-multi-row${on ? " on" : ""}`} data-target={c.id}>
      <label className="cb-check cb-res-name">
        <input type="checkbox" checked={on} onChange={onToggle} aria-label={`Target ${c.label}`} />
        {c.label}{pc && <span className="muted cb-small"> · asks the player</span>}
      </label>
      {on && save && (
        <span className="cb-res-entry">
          {entry.kind === "app" ? <span className="chip" role="status">Rolled in the app</span>
            : entry.kind === "saved" || entry.kind === "failed" ? <span className={`chip ${entry.kind === "saved" ? "success" : "danger"}`}>{entry.kind === "saved" ? "Saved" : "Failed"}</span>
              : <input className="input cb-roll-box" inputMode="numeric" placeholder="Table roll" aria-label={`${c.label}: ${ability.toUpperCase()} save, typed`}
                value={entry.kind === "typed" ? entry.text : ""} onChange={(e) => onSave(e.target.value ? { kind: "typed", text: e.target.value } : { kind: "none" })} />}
          <Button small onClick={() => onSave({ kind: "failed" })}>Failed</Button>
          <Button small onClick={() => onSave({ kind: "saved" })}>Saved</Button>
          {entry.kind !== "none" && entry.kind !== "typed" && <Button small variant="ghost" onClick={() => onSave({ kind: "none" })}>Clear</Button>}
          {mod != null && <span className="cb-small muted cb-res-mod">save {signed(mod)}</span>}
        </span>
      )}
    </li>
  );
}
