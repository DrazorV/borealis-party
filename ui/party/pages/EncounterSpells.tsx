import { type FormEvent, useEffect, useState } from "react";
import { Button, IconButton } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { Modal } from "../../src/components/Modal";
import { errorMessage } from "../../src/state/useAsync";
import { COVERAGE_HELP, COVERAGE_WORD } from "../combat/dm/spellTypes";
import { searchSpells, type Precast, type PrecastFields, type SpellEntry } from "../combat/dm/spellApi";
import type { Creature, Effect, OverrideSpell, SpellFrequency } from "../api/overrides/encounters";
import "../combat/dm/spells.css";

/** "dnd2024" → "2024": the spell index's edition words. */
export const editionOf = (ruleset: string): string => ruleset.replace(/^dnd/, "");

export const FREQUENCIES: { id: string; label: string; value: SpellFrequency }[] = [
  { id: "at_will", label: "At will", value: "at_will" },
  { id: "1", label: "1/day", value: { per_day: 1 } },
  { id: "2", label: "2/day", value: { per_day: 2 } },
  { id: "3", label: "3/day", value: { per_day: 3 } },
  { id: "slot", label: "Slot", value: "slot" },
];
const freqId = (f: SpellFrequency): string => (f === "at_will" || f === "slot" ? f : String(f.per_day));

/**
 * [Add spell…]: the installed spells, ONE copy per spell per edition (the user's rule: no duplicates; the other
 * copies are named under "also in"), the creature's edition first.
 */
export function SpellPicker({ ruleset, taken, onPick, onClose }: {
  ruleset: string; taken: string[]; onPick: (s: SpellEntry) => void; onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [items, setItems] = useState<SpellEntry[] | null>(null);
  const [error, setError] = useState("");
  const edition = editionOf(ruleset);
  useEffect(() => {
    let live = true;
    const t = setTimeout(() => {
      searchSpells({ text: text.trim() || undefined, limit: 60 })
        .then((list) => {
          if (!live) return;
          // The creature's edition first; within it by level and name.
          setItems([...list].sort((a, b) => Number(b.ruleset === edition) - Number(a.ruleset === edition) || a.level - b.level || a.name.localeCompare(b.name)));
          setError("");
        })
        .catch((e: unknown) => { if (live) { setItems([]); setError(errorMessage(e)); } });
    }, 150);
    return () => { live = false; clearTimeout(t); };
  }, [text, edition]);
  return (
    <Modal title="Add a spell" onClose={onClose} footer={<Button onClick={onClose}>Close</Button>}>
      <div className="party-form">
        <input className="input" autoFocus aria-label="Search spells" placeholder="Spell name" value={text} onChange={(e) => setText(e.target.value)} />
        {error && <Alert tone="danger">{error}</Alert>}
        <ul className="cb-pick-list" aria-label="Spells">
          {(items ?? []).map((s) => {
            const has = taken.includes(`${s.key}:${s.ruleset}`);
            return (
              <li key={`${s.key}:${s.ruleset}`}>
                <button type="button" className="cb-pick" disabled={has} onClick={() => onPick(s)}>
                  <b>{s.name}</b> <span className="muted cb-small">{s.level === 0 ? "Cantrip" : `L${s.level}`} · {s.ruleset}{s.concentration ? " · concentration" : ""}</span>{" "}
                  <span className={`cb-cov cb-cov-${s.coverage}`} title={COVERAGE_HELP[s.coverage]}>{COVERAGE_WORD[s.coverage]}</span>
                  {s.alsoIn?.length ? <span className="muted cb-small"> · also in {s.alsoIn.join(", ")}</span> : null}
                  {has ? <span className="muted cb-small"> · added</span> : null}
                </button>
              </li>
            );
          })}
          {items && items.length === 0 && <li className="muted cb-small">No spell in your content matches.</li>}
          {!items && <li className="muted cb-small">Searching…</li>}
        </ul>
      </div>
    </Modal>
  );
}

export interface SpellsValue {
  spells: OverrideSpell[];
  slots: Record<string, string>;
  dc: string;
  attack: string;
}

/**
 * The override dialog's Spells section (0.3.2): real spells from the DM's content, how often (At will, N/day, Slot),
 * the slots per level and the save DC and spell attack.
 */
export function SpellsSection({ ruleset, value, onChange }: { ruleset: string; value: SpellsValue; onChange: (v: SpellsValue) => void }) {
  const [picking, setPicking] = useState(false);
  const set = (patch: Partial<SpellsValue>) => onChange({ ...value, ...patch });
  return (
    <section className="enc-spells" aria-label="Spells">
      <h4 className="party-sub">Spells</h4>
      {value.spells.length === 0 ? <p className="muted party-note">Spells from your content; the creature casts them in the tracker.</p> : (
        <ul className="cb-spell-rows">
          {value.spells.map((s, i) => (
            <li key={`${s.key}:${s.ruleset}`} className="cb-spell-row">
              <span className="cb-spell-name">{s.name}</span>
              <span className="cb-small muted">{s.level === 0 ? "Cantrip" : `L${s.level}`}</span>
              <span className="cb-chips" role="radiogroup" aria-label={`${s.name}: how often`}>
                {FREQUENCIES.filter((f) => s.level > 0 || f.id === "at_will").map((f) => (
                  <button key={f.id} type="button" role="radio" aria-checked={freqId(s.frequency) === f.id} className={`cb-chip cb-numchip${freqId(s.frequency) === f.id ? " on" : ""}`}
                    onClick={() => set({ spells: value.spells.map((x, j) => (j === i ? { ...x, frequency: f.value } : x)) })}>{f.label}</button>
                ))}
              </span>
              <IconButton icon="trash" label={`Remove ${s.name}`} onClick={() => set({ spells: value.spells.filter((_, j) => j !== i) })} />
            </li>
          ))}
        </ul>
      )}
      <Button small icon="plus" onClick={() => setPicking(true)}>Add spell…</Button>
      {value.spells.some((s) => s.frequency === "slot") && (
        <fieldset className="enc-saves" aria-label="Spell slots">
          <legend>Slots per level</legend>
          {[1, 2, 3, 4, 5, 6, 7, 8, 9].map((l) => (
            <label key={l} className="enc-save">
              <span>L{l}</span>
              <input className="input" inputMode="numeric" aria-label={`Level ${l} slots`} value={value.slots[String(l)] ?? ""} onChange={(e) => set({ slots: { ...value.slots, [String(l)]: e.target.value } })} />
            </label>
          ))}
        </fieldset>
      )}
      {value.spells.length > 0 && (
        <div className="enc-grid2">
          <label className="party-field">Spell save DC<input className="input" inputMode="numeric" aria-label="Spell save DC" value={value.dc} onChange={(e) => set({ dc: e.target.value })} /></label>
          <label className="party-field">Spell attack<input className="input" inputMode="numeric" aria-label="Spell attack bonus" placeholder="+6" value={value.attack} onChange={(e) => set({ attack: e.target.value })} /></label>
        </div>
      )}
      {picking && (
        <SpellPicker ruleset={ruleset} taken={value.spells.map((s) => `${s.key}:${s.ruleset}`)} onClose={() => setPicking(false)}
          onPick={(p) => { setPicking(false); set({ spells: [...value.spells, { key: p.key, elementId: p.elementId, name: p.name, level: p.level, ruleset: p.ruleset, frequency: p.level === 0 ? "at_will" : "slot", concentration: p.concentration }] }); }} />
      )}
    </section>
  );
}

/** The spells a creature can precast: its override spells, else nothing (the picker covers the rest). */
const castable = (c: Creature | undefined): OverrideSpell[] => c?.overrides.spells ?? [];

/**
 * [Precast spell] (0.3.2, replaces "Starting effect"): a spell cast before the fight, by a caster, at a slot, on
 * targets. It works exactly like a cast in the fight and its slot is spent at the start. Warns when the caster would
 * hold two concentration spells (only the last one lasts).
 */
export function PrecastDialog({ creatures, precasts, precast, onSave, onClose }: {
  creatures: Creature[]; precasts: Precast[]; precast?: Precast; onSave: (f: PrecastFields) => Promise<unknown>; onClose: () => void;
}) {
  const [casterId, setCasterId] = useState(precast?.casterCreatureId ?? creatures.find((c) => castable(c).length)?.id ?? creatures[0]?.id ?? "");
  const caster = creatures.find((c) => c.id === casterId);
  const [spell, setSpell] = useState<OverrideSpell | null>(() => {
    if (!precast) return null;
    return castable(caster).find((s) => s.key === precast.spellKey) ?? { key: precast.spellKey, name: precast.spellName, level: precast.level, ruleset: editionOf(caster?.ruleset ?? ""), frequency: "slot", concentration: precast.concentration };
  });
  const [slot, setSlot] = useState<string>(precast?.slotLevel != null ? String(precast.slotLevel) : "");
  const [targets, setTargets] = useState<string[]>(precast?.targets ?? []);
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const slotLevels = spell && spell.level > 0 ? [1, 2, 3, 4, 5, 6, 7, 8, 9].filter((l) => l >= spell.level) : [];
  const otherConc = spell?.concentration ? precasts.filter((p) => p.casterCreatureId === casterId && p.concentration && p.id !== precast?.id) : [];
  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!spell || !caster) return;
    setBusy(true);
    setError("");
    try {
      await onSave({
        casterCreatureId: caster.id, spellKey: spell.key, spellName: spell.name, level: spell.level, concentration: Boolean(spell.concentration),
        slotLevel: slot ? Number(slot) : spell.level > 0 && spell.frequency === "slot" ? spell.level : null, targets,
      });
      onClose();
    } catch (err) { setError(errorMessage(err)); setBusy(false); }
  };
  return (
    <Modal title={precast ? `Edit ${precast.spellName}` : "Precast spell"} onClose={onClose}
      footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" disabled={busy || !spell || !caster} onClick={() => void submit()}>Save</Button></>}>
      <form className="party-form enc-form" onSubmit={(e) => void submit(e)}>
        <p className="muted party-note">Cast before the fight: it applies at the start, exactly like a cast in the fight, and spends its slot.</p>
        <label className="party-field">Caster
          <select className="input" aria-label="Caster" value={casterId} onChange={(e) => { setCasterId(e.target.value); setSpell(null); }}>
            {creatures.map((c) => <option key={c.id} value={c.id}>{c.dmLabel}</option>)}
          </select>
        </label>
        <div className="party-field">Spell
          <div className="cb-chips" role="radiogroup" aria-label="Spell">
            {castable(caster).map((s) => (
              <button key={`${s.key}:${s.ruleset}`} type="button" role="radio" aria-checked={spell?.key === s.key} className={`cb-chip${spell?.key === s.key ? " on" : ""}`}
                onClick={() => { setSpell(s); setSlot(""); }}>{s.name}</button>
            ))}
            <Button small icon="plus" onClick={() => setPicking(true)}>Another spell…</Button>
          </div>
          {spell && !castable(caster).some((s) => s.key === spell.key) && <span className="muted party-note">{spell.name} (from your content)</span>}
        </div>
        {slotLevels.length > 0 && (
          <div className="party-field">Slot
            <div className="cb-chips" role="radiogroup" aria-label="Slot">
              {slotLevels.map((l) => {
                const on = (slot || String(spell!.level)) === String(l);
                return <button key={l} type="button" role="radio" aria-checked={on} className={`cb-chip${on ? " on" : ""}`} onClick={() => setSlot(String(l))}>L{l}</button>;
              })}
            </div>
          </div>
        )}
        <div className="party-field">On
          <div className="cb-chips" role="group" aria-label="Targets">
            {creatures.map((c) => {
              const on = targets.includes(c.id);
              return <button key={c.id} type="button" aria-pressed={on} className={`cb-chip${on ? " on" : ""}`} onClick={() => setTargets(on ? targets.filter((t) => t !== c.id) : [...targets, c.id])}>{c.dmLabel}</button>;
            })}
          </div>
          <span className="muted party-note">None: on the caster itself.</span>
        </div>
        {otherConc.length > 0 && (
          <Alert tone="warning">{caster?.dmLabel} already concentrates on {otherConc.map((p) => p.spellName).join(", ")}. Only one concentration spell lasts: the last one cast.</Alert>
        )}
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
      {picking && caster && (
        <SpellPicker ruleset={caster.ruleset} taken={[]} onClose={() => setPicking(false)}
          onPick={(p) => { setPicking(false); setSpell({ key: p.key, elementId: p.elementId, name: p.name, level: p.level, ruleset: p.ruleset, frequency: "slot", concentration: p.concentration }); setSlot(""); }} />
      )}
    </Modal>
  );
}

/**
 * "Before the fight" (replaces "Starting effects"): precast spells first, then the older custom effects, which stay
 * as "Custom effect" and now spend their slot too.
 */
export function BeforeFightCard({ creatures, precasts, effects, named, onPrecast, onEditPrecast, onRemovePrecast, onCustom, onEditEffect, onRemoveEffect, canAddEffect }: {
  creatures: Creature[]; precasts: Precast[]; effects: Effect[]; named: (id: string | null) => string;
  onPrecast: () => void; onEditPrecast: (p: Precast) => void; onRemovePrecast: (p: Precast) => void;
  onCustom: () => void; onEditEffect: (e: Effect) => void; onRemoveEffect: (e: Effect) => void; canAddEffect: boolean;
}) {
  return (
    <Card title="Before the fight" aria-label="Before the fight" className="enc-card"
      actions={creatures.length > 0 ? <>
        <Button small variant="primary" icon="plus" onClick={onPrecast}>Precast spell</Button>
        {canAddEffect && <Button small variant="ghost" onClick={onCustom}>Custom effect</Button>}
      </> : undefined}>
      {precasts.length === 0 && effects.length === 0
        ? <p className="muted party-note">Spells already up when the fight starts, like Mage Armor cast beforehand.</p>
        : (
          <ul className="party-list">
            {precasts.map((p) => (
              <li key={p.id} className="party-line enc-item">
                <span className="grow">
                  <strong>{p.spellName}</strong> <span className="muted">by {named(p.casterCreatureId)}</span>
                  <span className="muted party-note">{[p.slotLevel ? `level ${p.slotLevel} slot` : p.level === 0 ? "cantrip" : null, p.concentration ? "concentration" : null,
                    p.targets.length ? `on ${p.targets.map((t) => named(t)).join(", ")}` : "on itself"].filter(Boolean).join(" · ")}</span>
                </span>
                <Button small onClick={() => onEditPrecast(p)}>Edit</Button>
                <IconButton icon="trash" label={`Remove ${p.spellName}`} onClick={() => onRemovePrecast(p)} />
              </li>
            ))}
            {effects.map((e) => (
              <li key={e.id} className="party-line enc-item">
                <span className="grow">
                  <strong>{e.name}</strong> <span className="muted">custom effect on {named(e.creatureId)}</span>
                  <span className="muted party-note">{[e.sourceCreatureId ? `by ${named(e.sourceCreatureId)}` : null, e.durationTurns ? `${e.durationTurns} turns` : null,
                    e.charges ? `${e.charges} charges` : null, e.slotLevel ? `level ${e.slotLevel} slot` : null, e.concentration ? "concentration" : null].filter(Boolean).join(" · ")}</span>
                </span>
                <Button small onClick={() => onEditEffect(e)}>Edit</Button>
                <IconButton icon="trash" label={`Remove ${e.name}`} onClick={() => onRemoveEffect(e)} />
              </li>
            ))}
          </ul>
        )}
    </Card>
  );
}
