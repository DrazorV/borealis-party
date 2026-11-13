import { useState } from "react";
import { Button } from "../../../src/components/Button";
import { tryParseNotation } from "../../../src/lib/dice";
import { usePhone } from "../../../src/lib/phone";
import { DAMAGE_TYPES, type DamageType, type DiceInput } from "./common";
import { TextDialog } from "./dialogs";
import { isCharacter, isDefeated } from "./derive";
import type { Combatant, Command, DamageClause } from "./types";

const clauseText = (list: DamageClause[] | undefined) =>
  (list ?? []).map((c) => c.text ?? c.types.join(", ")).filter(Boolean).join("; ");

/**
 * HP (C6): the amount box takes a typed total (D43) or dice the server rolls (`2d6+3`, D21), with Damage and Heal,
 * quick −10/−5/+5, damage type chips, magical and critical, Set exact and temporary HP. For a PC, Damage and Heal
 * send the player a prompt instead (D7): the PC's HP stays the player's.
 */
export function HpControls({ c, send, disabled }: { c: Combatant; send: (cmd: Command) => void; disabled: boolean }) {
  const [amount, setAmount] = useState("");
  const [type, setType] = useState<DamageType | null>(null);
  const [magical, setMagical] = useState(false);
  const [crit, setCrit] = useState(false);
  const [qualified, setQualified] = useState(false);
  const [dialog, setDialog] = useState<"set" | "temp" | null>(null);
  const phone = usePhone();
  const pc = isCharacter(c);
  const dead = isDefeated(c);
  const parse = (): DiceInput | null => {
    const t = amount.trim();
    if (!t) return null;
    const n = Number(t);
    if (Number.isInteger(n) && n >= 0) return { kind: "typed", total: n };
    if (tryParseNotation(t)) return { kind: "roll", notation: t, ...(crit ? { critical: true } : {}) };
    return null;
  };
  const input = parse();
  const damage = (value: DiceInput) => {
    send({
      command: "damage", target: c.id, parts: [{ amount: value, ...(type ? { damage_type: type } : {}) }],
      ...(magical ? { magical: true } : {}), ...(crit ? { critical_hit: true } : {}), ...(qualified ? { apply_qualified: true } : {}),
    });
  };
  const quick = (n: number) => (n < 0 ? damage({ kind: "typed", total: -n }) : send({ command: "heal", target: c.id, amount: { kind: "typed", total: n } }));
  const d = c.defenses ?? {};
  const qualifiers = [...(d.immunities ?? []), ...(d.resistances ?? []), ...(d.vulnerabilities ?? [])].filter((x) => x.text && !x.text.toLowerCase().includes("nonmagical"));
  return (
    <div className="cb-hpc">
      <div className="cb-hpc-row">
        {!pc && <Button small onClick={() => quick(-10)} disabled={disabled || dead}>−10</Button>}
        {!pc && <Button small onClick={() => quick(-5)} disabled={disabled || dead}>−5</Button>}
        <input className="input cb-amount" aria-label="Damage or healing: a total, or dice to roll" placeholder="7 or 2d6+3" value={amount}
          inputMode="text" disabled={disabled} onChange={(e) => setAmount(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && input) { damage(input); setAmount(""); } }} />
        <Button small variant="danger" disabled={disabled || !input || dead} onClick={() => { if (input) { damage(input); setAmount(""); } }}>
          {pc ? "Ask to take" : "Damage"}
        </Button>
        <Button small variant="success" disabled={disabled || !input || c.life?.state === "dead"}
          onClick={() => { if (input) { send({ command: "heal", target: c.id, amount: input }); setAmount(""); } }}>
          {pc ? "Ask to heal" : "Heal"}
        </Button>
        {!pc && <Button small onClick={() => quick(5)} disabled={disabled || dead}>+5</Button>}
      </div>
      {phone ? (
        <select className="input cb-type-select" aria-label="Damage type" value={type ?? ""} onChange={(e) => setType((e.target.value || null) as DamageType | null)}>
          <option value="">Untyped damage</option>
          {DAMAGE_TYPES.map((t) => <option key={t} value={t}>{t}{d.immunities?.some((x) => x.types.includes(t)) ? " (immune)" : d.resistances?.some((x) => x.types.includes(t)) ? " (resists)" : d.vulnerabilities?.some((x) => x.types.includes(t)) ? " (vulnerable)" : ""}</option>)}
        </select>
      ) : <div className="cb-types" role="group" aria-label="Damage type">
        {DAMAGE_TYPES.map((t) => (
          <button key={t} type="button" className={`cb-type${type === t ? " on" : ""}${d.immunities?.some((x) => x.types.includes(t)) ? " imm" : d.resistances?.some((x) => x.types.includes(t)) ? " res" : d.vulnerabilities?.some((x) => x.types.includes(t)) ? " vul" : ""}`}
            aria-pressed={type === t} onClick={() => setType(type === t ? null : t)}>{t}</button>
        ))}
      </div>}
      <div className="cb-hpc-row">
        <label className="cb-check"><input type="checkbox" checked={magical} onChange={(e) => setMagical(e.target.checked)} />Magical</label>
        <label className="cb-check"><input type="checkbox" checked={crit} onChange={(e) => setCrit(e.target.checked)} />Critical hit</label>
        {qualifiers.length > 0 && <label className="cb-check"><input type="checkbox" checked={qualified} onChange={(e) => setQualified(e.target.checked)} />Qualified clauses apply</label>}
        {!pc && <Button small variant="ghost" disabled={disabled} onClick={() => setDialog("set")}>Set exact</Button>}
        {!pc && <Button small variant="ghost" disabled={disabled || dead} onClick={() => setDialog("temp")}>Temp HP</Button>}
      </div>
      {(d.immunities?.length || d.resistances?.length || d.vulnerabilities?.length) ? (
        <p className="cb-small muted cb-defenses">
          {d.immunities?.length ? <>Immune: {clauseText(d.immunities)}. </> : null}
          {d.resistances?.length ? <>Resists: {clauseText(d.resistances)}. </> : null}
          {d.vulnerabilities?.length ? <>Vulnerable: {clauseText(d.vulnerabilities)}.</> : null}
        </p>
      ) : null}
      {pc && <p className="cb-small muted">{c.label}'s HP belongs to the player: damage and healing ask them to apply it on their Play tab.</p>}
      {dialog === "set" && (
        <TextDialog title={`Set HP: ${c.label}`} label={`Current HP (max ${c.hp.max})`} numeric initial={String(c.hp.current)} confirm="Set"
          onDone={(v) => { setDialog(null); if (v != null && v !== "") send({ command: "set_hp", id: c.id, current: Number(v) }); }} />
      )}
      {dialog === "temp" && (
        <TextDialog title={`Temporary HP: ${c.label}`} label="Temporary HP (the higher value is kept)" numeric initial="" confirm="Grant"
          onDone={(v) => { setDialog(null); if (v) send({ command: "grant_temp_hp", id: c.id, amount: Number(v) }); }} />
      )}
    </div>
  );
}
