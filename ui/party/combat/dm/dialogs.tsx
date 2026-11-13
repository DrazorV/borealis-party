import { useState, type ReactNode } from "react";
import { Button } from "../../../src/components/Button";
import { Dialog } from "../../../src/components/controls/Dialog";
import { CONDITIONS, conditionWord, type Condition } from "./common";
import type { Combatant, Duration, EffectSpec, Modifier } from "./types";

/** A one-field app dialog (rename, set HP, concentration): never `window.prompt`. */
export function TextDialog({ title, label, initial = "", numeric, allowEmpty, onDone, confirm = "OK", note }: {
  title: string;
  label: string;
  initial?: string;
  numeric?: boolean;
  allowEmpty?: boolean;
  confirm?: string;
  note?: ReactNode;
  onDone: (value: string | null) => void;
}) {
  const [value, setValue] = useState(initial);
  const ok = allowEmpty || value.trim() !== "";
  const valid = !numeric || value.trim() === "" || Number.isInteger(Number(value));
  return (
    <Dialog title={title} onClose={() => onDone(null)}
      footer={<>
        <Button onClick={() => onDone(null)}>Cancel</Button>
        <Button variant="primary" disabled={!ok || !valid} onClick={() => onDone(value.trim())}>{confirm}</Button>
      </>}>
      <form className="cb-form" onSubmit={(e) => { e.preventDefault(); if (ok && valid) onDone(value.trim()); }}>
        <label className="cb-field">
          <span>{label}</span>
          <input className="input" autoFocus value={value} inputMode={numeric ? "numeric" : undefined} aria-invalid={!valid || undefined}
            onChange={(e) => setValue(e.target.value)} />
        </label>
        {note && <p className="muted cb-note">{note}</p>}
      </form>
    </Dialog>
  );
}

export interface EffectDraft {
  name: string;
  detail: string;
  sourceId: string;
  targets: string[];
  rounds: string;
  at: "start" | "end";
  counted: "source" | "target" | "manual";
  charges: string;
  concentration: boolean;
  conditions: Condition[];
  ac: string;
  speedAdd: string;
  speedDouble: boolean;
  speedZero: boolean;
  cover: "" | "half" | "three_quarters";
}

export const emptyDraft = (sourceId: string, target: string): EffectDraft => ({
  name: "", detail: "", sourceId, targets: [target], rounds: "", at: "start", counted: "source", charges: "",
  concentration: false, conditions: [], ac: "", speedAdd: "", speedDouble: false, speedZero: false, cover: "",
});

/** The effect specs a draft makes: one per target. */
export function draftSpecs(d: EffectDraft): EffectSpec[] {
  const rounds = Number(d.rounds);
  const counted = d.counted === "manual" || !Number.isInteger(rounds) || rounds <= 0 ? "manual" : d.counted;
  const duration: Duration = counted === "manual" ? { kind: "manual" }
    : { kind: counted === "source" ? "source_turns" : "target_turns", remaining: rounds, at: d.at };
  const modifiers: Modifier[] = [];
  if (d.ac.trim() && Number.isInteger(Number(d.ac))) modifiers.push({ kind: "ac", value: Number(d.ac) });
  if (d.speedAdd.trim() && Number.isInteger(Number(d.speedAdd))) modifiers.push({ kind: "speed_add", value: Number(d.speedAdd) });
  if (d.speedDouble) modifiers.push({ kind: "speed_multiply", value: 2 });
  if (d.speedZero) modifiers.push({ kind: "speed_zero" });
  if (d.cover) modifiers.push({ kind: "cover", value: d.cover });
  const charges = Number(d.charges);
  return d.targets.map((target) => ({
    name: d.name.trim(), target, detail: d.detail.trim(), duration,
    ...(d.sourceId ? { source: d.sourceId } : {}),
    ...(d.concentration && d.sourceId ? { concentration: true } : {}),
    ...(Number.isInteger(charges) && charges > 0 ? { charges } : {}),
    ...(d.conditions.length ? { conditions: d.conditions } : {}),
    ...(modifiers.length ? { modifiers } : {}),
  }));
}

/**
 * Adds an effect (C12, C13) or records a cast (C18): its source, targets, duration in the source's (or target's)
 * turns, charges, concentration, conditions and AC/speed modifiers. A cast also says who failed the save; those
 * targets get the effect (with its conditions), the others don't.
 */
export function EffectDialog({ title, combatants, initial, castMode, slotLevels, onDone }: {
  title: string;
  combatants: Combatant[];
  initial: EffectDraft;
  /** A cast: the targets list is "who failed the save" when `save` is checked. */
  castMode?: boolean;
  /** Slot levels the caster has left, for a cast. */
  slotLevels?: number[];
  onDone: (result: { draft: EffectDraft; slot?: number; saveTargets?: boolean } | null) => void;
}) {
  const [d, setD] = useState(initial);
  const [slot, setSlot] = useState<string>(slotLevels?.[0] != null ? String(slotLevels[0]) : "");
  const [save, setSave] = useState(false);
  const set = <K extends keyof EffectDraft>(k: K, v: EffectDraft[K]) => setD((x) => ({ ...x, [k]: v }));
  const toggleTarget = (id: string) => set("targets", d.targets.includes(id) ? d.targets.filter((t) => t !== id) : [...d.targets, id]);
  const ok = d.name.trim() !== "" && (castMode || d.targets.length > 0);
  const live = combatants.filter((c) => !c.removed);
  return (
    <Dialog title={title} onClose={() => onDone(null)} wide
      footer={<>
        <Button onClick={() => onDone(null)}>Cancel</Button>
        <Button variant="primary" disabled={!ok} onClick={() => onDone({ draft: d, ...(slot ? { slot: Number(slot) } : {}), saveTargets: save })}>
          {castMode ? "Cast" : "Add effect"}
        </Button>
      </>}>
      <div className="cb-form">
        <label className="cb-field"><span>Name</span>
          <input className="input" autoFocus={!d.name} value={d.name} onChange={(e) => set("name", e.target.value)} placeholder="Bless" />
        </label>
        {castMode && slotLevels && (
          <label className="cb-field"><span>Spell slot</span>
            <select className="input" value={slot} onChange={(e) => setSlot(e.target.value)}>
              <option value="">No slot (at will, per day, cantrip)</option>
              {slotLevels.map((l) => <option key={l} value={l}>Level {l}</option>)}
            </select>
          </label>
        )}
        <label className="cb-field"><span>From (its turns count the duration; its concentration holds it)</span>
          <select className="input" value={d.sourceId} onChange={(e) => set("sourceId", e.target.value)}>
            <option value="">Nobody (manual)</option>
            {live.map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
          </select>
        </label>
        {castMode && (
          <label className="cb-check"><input type="checkbox" checked={save} onChange={(e) => setSave(e.target.checked)} />
            It allows a save: tick only those who FAILED it</label>
        )}
        <fieldset className="cb-fieldset">
          <legend>{castMode && save ? "Who failed the save" : "On"}</legend>
          <div className="cb-checks">
            {live.map((c) => (
              <label key={c.id} className="cb-check"><input type="checkbox" checked={d.targets.includes(c.id)} onChange={() => toggleTarget(c.id)} />{c.label}</label>
            ))}
          </div>
        </fieldset>
        <div className="cb-frow">
          <label className="cb-field cb-narrow"><span>Lasts (turns)</span>
            <input className="input" inputMode="numeric" value={d.rounds} placeholder="10 = 1 min" onChange={(e) => set("rounds", e.target.value)} />
          </label>
          <label className="cb-field"><span>Counted on</span>
            <select className="input" value={`${d.counted}:${d.at}`} onChange={(e) => {
              const [counted, at] = e.target.value.split(":") as [EffectDraft["counted"], EffectDraft["at"]];
              setD((x) => ({ ...x, counted, at }));
            }}>
              <option value="source:start">Start of the source's turns</option>
              <option value="source:end">End of the source's turns</option>
              <option value="target:start">Start of the target's turns</option>
              <option value="target:end">End of the target's turns</option>
              <option value="manual:start">Until removed</option>
            </select>
          </label>
          <label className="cb-field cb-narrow"><span>Charges</span>
            <input className="input" inputMode="numeric" value={d.charges} placeholder="—" onChange={(e) => set("charges", e.target.value)} />
          </label>
        </div>
        <label className="cb-check"><input type="checkbox" checked={d.concentration} disabled={!d.sourceId} onChange={(e) => set("concentration", e.target.checked)} />
          Concentration (ends when the source's concentration ends)</label>
        <fieldset className="cb-fieldset">
          <legend>Conditions it gives</legend>
          <div className="cb-checks">
            {CONDITIONS.map((k) => (
              <label key={k} className="cb-check"><input type="checkbox" checked={d.conditions.includes(k)}
                onChange={() => set("conditions", d.conditions.includes(k) ? d.conditions.filter((x) => x !== k) : [...d.conditions, k])} />{conditionWord(k)}</label>
            ))}
          </div>
        </fieldset>
        <div className="cb-frow">
          <label className="cb-field cb-narrow"><span>AC ±</span><input className="input" inputMode="numeric" value={d.ac} placeholder="+2" onChange={(e) => set("ac", e.target.value)} /></label>
          <label className="cb-field cb-narrow"><span>Speed + ft</span><input className="input" inputMode="numeric" value={d.speedAdd} placeholder="+10" onChange={(e) => set("speedAdd", e.target.value)} /></label>
          <label className="cb-field"><span>Cover</span>
            <select className="input" value={d.cover} onChange={(e) => set("cover", e.target.value as EffectDraft["cover"])}>
              <option value="">None</option><option value="half">Half (+2)</option><option value="three_quarters">Three-quarters (+5)</option>
            </select>
          </label>
        </div>
        <div className="cb-frow">
          <label className="cb-check"><input type="checkbox" checked={d.speedDouble} onChange={(e) => set("speedDouble", e.target.checked)} />Speed doubled</label>
          <label className="cb-check"><input type="checkbox" checked={d.speedZero} onChange={(e) => set("speedZero", e.target.checked)} />Speed 0</label>
        </div>
        <label className="cb-field"><span>Rules text</span>
          <textarea className="input" rows={2} value={d.detail} onChange={(e) => set("detail", e.target.value)} />
        </label>
      </div>
    </Dialog>
  );
}

/** Adds a creature mid-fight (C7): a copy of one already in the fight, with its initiative rolled or typed. */
export function AddCreatureDialog({ creatures, onDone }: {
  creatures: Combatant[];
  onDone: (r: { from: Combatant; label: string; initiative: string; hidden: boolean } | null) => void;
}) {
  const [fromId, setFromId] = useState(creatures[0]?.id ?? "");
  const from = creatures.find((c) => c.id === fromId);
  const [label, setLabel] = useState("");
  const [init, setInit] = useState("");
  const [hidden, setHidden] = useState(true);
  return (
    <Dialog title="Add a creature" onClose={() => onDone(null)}
      footer={<>
        <Button onClick={() => onDone(null)}>Cancel</Button>
        <Button variant="primary" disabled={!from} onClick={() => from && onDone({ from, label: label.trim(), initiative: init.trim(), hidden })}>Add</Button>
      </>}>
      <div className="cb-form">
        <label className="cb-field"><span>A copy of</span>
          <select className="input" value={fromId} onChange={(e) => setFromId(e.target.value)}>
            {creatures.map((c) => <option key={c.id} value={c.id}>{c.label} ({c.hp.max} HP, AC {c.ac ?? "—"})</option>)}
          </select>
        </label>
        <label className="cb-field"><span>Name</span><input className="input" value={label} placeholder={from ? `${from.label.replace(/\s*\d+$/, "")} (new)` : ""} onChange={(e) => setLabel(e.target.value)} /></label>
        <label className="cb-field"><span>Initiative (blank: roll in app)</span><input className="input" inputMode="numeric" value={init} onChange={(e) => setInit(e.target.value)} /></label>
        <label className="cb-check"><input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} />Hidden from players</label>
        {creatures.length === 0 && <p className="muted">There is no creature to copy. Add creatures in the encounter builder.</p>}
      </div>
    </Dialog>
  );
}

