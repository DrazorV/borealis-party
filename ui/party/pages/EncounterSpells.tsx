import { type FormEvent, useEffect, useState } from "react";
import { Button, IconButton } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { Modal } from "../../src/components/Modal";
import { errorMessage } from "../../src/state/useAsync";
import { COVERAGE_HELP, COVERAGE_WORD, coverageWord } from "../combat/dm/spellTypes";
import { alsoInBooks, searchSpells, spellDetail, type SpellEntry } from "../combat/dm/spellApi";
import type { Creature, Effect, OverrideSpell, Precast, PrecastFields, SpellFrequency } from "../api/overrides/encounters";
import "../combat/dm/spells.css";

/** "dnd2024" → "2024". */
export const editionOf = (ruleset: string): string => ruleset.replace(/^dnd/, "");

/** A spell on the override list, with what the picker knew (level, concentration) for showing it. */
export type ListedSpell = OverrideSpell & { level?: number | null; concentration?: boolean };

const FREQ: { id: string; label: string }[] = [
  { id: "atWill", label: "At will" }, { id: "1", label: "1/day" }, { id: "2", label: "2/day" }, { id: "3", label: "3/day" }, { id: "slots", label: "Slot" },
];
const freqId = (f: SpellFrequency): string => (f.kind === "perDay" ? String(f.uses) : f.kind);
export const levelOf = (s: ListedSpell): number | null => (s.frequency.kind === "slots" ? s.frequency.level : s.level ?? null);

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
  useEffect(() => {
    let live = true;
    const t = setTimeout(() => {
      // What a new pick of a character of the creature's edition may take, from the account's enabled sources.
      searchSpells({ text: text.trim() || undefined, forRuleset: ruleset, limit: 60 })
        .then((list) => {
          if (!live) return;
          // The creature's edition first; within it by level and name.
          setItems([...list].sort((a, b) => Number(b.ruleset === ruleset) - Number(a.ruleset === ruleset) || (a.level ?? 0) - (b.level ?? 0) || a.name.localeCompare(b.name)));
          setError("");
        })
        .catch((e: unknown) => { if (live) { setItems([]); setError(errorMessage(e)); } });
    }, 150);
    return () => { live = false; clearTimeout(t); };
  }, [text, ruleset]);
  return (
    <Modal title="Add a spell" onClose={onClose} footer={<Button onClick={onClose}>Close</Button>}>
      <div className="party-form">
        <input className="input" autoFocus aria-label="Search spells" placeholder="Spell name" value={text} onChange={(e) => setText(e.target.value)} />
        {error && <Alert tone="danger">{error}</Alert>}
        <ul className="cb-pick-list" aria-label="Spells">
          {(items ?? []).map((s) => {
            const has = taken.includes(s.key);
            const cov = coverageWord(s.coverage);
            return (
              <li key={s.key}>
                <button type="button" className="cb-pick" disabled={has} onClick={() => onPick(s)}>
                  <b>{s.name}</b> <span className="muted cb-small">{s.level === 0 ? "Cantrip" : `L${s.level ?? "?"}`} · {editionOf(s.ruleset)}{s.concentration ? " · concentration" : ""} · {s.source}</span>{" "}
                  <span className={`cb-cov cb-cov-${cov}`} title={COVERAGE_HELP[cov]}>{COVERAGE_WORD[cov]}</span>
                  {alsoInBooks(s).length ? <span className="muted cb-small"> · also in {alsoInBooks(s).join(", ")}</span> : null}
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
  spells: ListedSpell[];
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
  const frequency = (s: ListedSpell, id: string): SpellFrequency => {
    if (id === "atWill") return { kind: "atWill" };
    if (id === "slots") { const level = levelOf(s) ?? 1; return { kind: "slots", level, slots: Number(value.slots[String(level)]) || 0 }; }
    return { kind: "perDay", uses: Number(id), each: true };
  };
  return (
    <section className="enc-spells" aria-label="Spells">
      <h4 className="party-sub">Spells</h4>
      {value.spells.length === 0 ? <p className="muted party-note">Spells from your content; the creature casts them in the tracker.</p> : (
        <ul className="cb-spell-rows">
          {value.spells.map((s, i) => {
            const level = levelOf(s);
            return (
              <li key={s.id} className="cb-spell-row">
                <span className="cb-spell-name">{s.name}</span>
                {level != null && <span className="cb-small muted">{level === 0 ? "Cantrip" : `L${level}`}</span>}
                <span className="cb-chips" role="radiogroup" aria-label={`${s.name}: how often`}>
                  {FREQ.filter((f) => level !== 0 || f.id === "atWill").map((f) => (
                    <button key={f.id} type="button" role="radio" aria-checked={freqId(s.frequency) === f.id} className={`cb-chip cb-numchip${freqId(s.frequency) === f.id ? " on" : ""}`}
                      onClick={() => set({ spells: value.spells.map((x, j) => (j === i ? { ...x, frequency: frequency(x, f.id) } : x)) })}>{f.label}</button>
                  ))}
                </span>
                <IconButton icon="trash" label={`Remove ${s.name}`} onClick={() => set({ spells: value.spells.filter((_, j) => j !== i) })} />
              </li>
            );
          })}
        </ul>
      )}
      <Button small icon="plus" onClick={() => setPicking(true)}>Add spell…</Button>
      {value.spells.some((s) => s.frequency.kind === "slots") && (
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
        <SpellPicker ruleset={ruleset} taken={value.spells.map((s) => s.id)} onClose={() => setPicking(false)}
          onPick={(p) => {
            setPicking(false);
            const level = p.level ?? 0;
            set({ spells: [...value.spells, { id: p.key, name: p.name, level, concentration: p.concentration,
              frequency: level === 0 ? { kind: "atWill" } : { kind: "slots", level, slots: Number(value.slots[String(level)]) || 0 } }] });
          }} />
      )}
    </section>
  );
}

/** The override list as the server takes it: id, name and frequency only. */
export const overrideSpells = (list: ListedSpell[]): OverrideSpell[] => list.map(({ id, name, frequency }) => ({ id, name, frequency }));

interface Pick { key: string; name: string; level: number | null; concentration: boolean | null }

/**
 * [Precast spell] (0.3.2, replaces "Starting effect"): a spell cast before the fight, by a caster, at a slot, on
 * targets. It works exactly like a cast in the fight and its slot is spent at the start. Warns when the caster would
 * hold two concentration spells (only the last one lasts).
 */
export function PrecastDialog({ creatures, precasts, precast, onSave, onClose }: {
  creatures: Creature[]; precasts: Precast[]; precast?: Precast; onSave: (f: PrecastFields) => Promise<unknown>; onClose: () => void;
}) {
  const castable = (c: Creature | undefined): OverrideSpell[] => c?.overrides.spells?.spells ?? [];
  const [casterId, setCasterId] = useState(precast?.casterId ?? creatures.find((c) => castable(c).length)?.id ?? creatures[0]?.id ?? "");
  const caster = creatures.find((c) => c.id === casterId);
  const [spell, setSpell] = useState<Pick | null>(precast ? { key: precast.spellKey, name: precast.name, level: null, concentration: null } : null);
  const [slot, setSlot] = useState<string>(precast?.slotLevel != null ? String(precast.slotLevel) : "");
  const [targets, setTargets] = useState<string[]>((precast?.targets ?? []).filter((t) => t !== "self"));
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  /** Which of this caster's other precasts are concentration spells (from the spells' own setters). */
  const [held, setHeld] = useState<string[]>([]);
  const others = precasts.filter((p) => p.casterId === casterId && p.id !== precast?.id);

  // A spell picked from the override list: its level and concentration come from the spell itself.
  useEffect(() => {
    if (!spell || (spell.level != null && spell.concentration != null)) return;
    let live = true;
    spellDetail(spell.key).then((d) => { if (live && d) setSpell((s) => (s && s.key === spell.key ? { ...s, level: d.spell.level ?? 0, concentration: d.spell.concentration } : s)); }, () => {});
    return () => { live = false; };
  }, [spell]);
  useEffect(() => {
    let live = true;
    Promise.all(others.map((p) => spellDetail(p.spellKey).then((d) => (d?.spell.concentration ? p.name : null), () => null)))
      .then((names) => { if (live) setHeld(names.filter((n): n is string => Boolean(n))); });
    return () => { live = false; };
    // The ids name the precasts; a new list of the same ones needn't refetch.
  }, [others.map((p) => p.id).join(",")]);

  const level = spell?.level ?? null;
  const slotLevels = level != null && level > 0 ? [1, 2, 3, 4, 5, 6, 7, 8, 9].filter((l) => l >= level) : [];
  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!spell || !caster) return;
    setBusy(true);
    setError("");
    try {
      await onSave({ casterId: caster.id, spellKey: spell.key, slotLevel: slot ? Number(slot) : level && level > 0 ? level : null, targets: targets.length ? targets : ["self"] });
      onClose();
    } catch (err) { setError(errorMessage(err)); setBusy(false); }
  };
  return (
    <Modal title={precast ? `Edit ${precast.name}` : "Precast spell"} onClose={onClose}
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
              <button key={s.id} type="button" role="radio" aria-checked={spell?.key === s.id} className={`cb-chip${spell?.key === s.id ? " on" : ""}`}
                onClick={() => { setSpell({ key: s.id, name: s.name, level: s.frequency.kind === "slots" ? s.frequency.level : null, concentration: null }); setSlot(""); }}>{s.name}</button>
            ))}
            <Button small icon="plus" onClick={() => setPicking(true)}>Another spell…</Button>
          </div>
          {spell && !castable(caster).some((s) => s.id === spell.key) && <span className="muted party-note">{spell.name} (from your content)</span>}
        </div>
        {slotLevels.length > 0 && (
          <div className="party-field">Slot
            <div className="cb-chips" role="radiogroup" aria-label="Slot">
              {slotLevels.map((l) => {
                const on = (slot || String(level)) === String(l);
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
        {spell?.concentration && held.length > 0 && (
          <Alert tone="warning">{caster?.dmLabel} already concentrates on {held.join(", ")}. Only one concentration spell lasts: the last one cast.</Alert>
        )}
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
      {picking && caster && (
        <SpellPicker ruleset={caster.ruleset} taken={[]} onClose={() => setPicking(false)}
          onPick={(p) => { setPicking(false); setSpell({ key: p.key, name: p.name, level: p.level ?? 0, concentration: p.concentration }); setSlot(""); }} />
      )}
    </Modal>
  );
}

/**
 * "Before the fight" (replaces "Starting effects"): precast spells first, then the older custom effects, which stay
 * as "Custom effect" and now spend their slot too.
 */
export function BeforeFightCard({ creatures, precasts, effects, warning, named, onPrecast, onEditPrecast, onRemovePrecast, onCustom, onEditEffect, onRemoveEffect, canAddEffect }: {
  creatures: Creature[]; precasts: Precast[]; effects: Effect[]; warning?: string; named: (id: string | null) => string;
  onPrecast: () => void; onEditPrecast: (p: Precast) => void; onRemovePrecast: (p: Precast) => void;
  onCustom: () => void; onEditEffect: (e: Effect) => void; onRemoveEffect: (e: Effect) => void; canAddEffect: boolean;
}) {
  return (
    <Card title="Before the fight" aria-label="Before the fight" className="enc-card"
      actions={creatures.length > 0 ? <>
        <Button small variant="primary" icon="plus" onClick={onPrecast}>Precast spell</Button>
        {canAddEffect && <Button small variant="ghost" onClick={onCustom}>Custom effect</Button>}
      </> : undefined}>
      {warning && <Alert tone="warning">{warning}</Alert>}
      {precasts.length === 0 && effects.length === 0
        ? <p className="muted party-note">Spells already up when the fight starts, like Mage Armor cast beforehand.</p>
        : (
          <ul className="party-list">
            {precasts.map((p) => (
              <li key={p.id} className="party-line enc-item">
                <span className="grow">
                  <strong>{p.name}</strong> <span className="muted">by {named(p.casterId)}</span>
                  <span className="muted party-note">{[p.slotLevel ? `level ${p.slotLevel} slot` : null,
                    p.targets.length && !(p.targets.length === 1 && p.targets[0] === "self") ? `on ${p.targets.filter((t) => t !== "self").map((t) => named(t)).join(", ")}` : "on itself"].filter(Boolean).join(" · ")}</span>
                </span>
                <Button small onClick={() => onEditPrecast(p)}>Edit</Button>
                <IconButton icon="trash" label={`Remove ${p.name}`} onClick={() => onRemovePrecast(p)} />
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
