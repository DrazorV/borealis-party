import { type FormEvent, type ReactNode, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert } from "../../src/components/Card";
import { Modal } from "../../src/components/Modal";
import { errorMessage } from "../../src/state/useAsync";
import {
  ABILITIES, ABILITY_WORD, INIT_WORD, LIMITS, rollFormula, signed, validFormula, type Ability, type Conflict, type Creature,
  type Effect, type EffectFields, type Encounter, type InitMode, type OverridesForm, type Rule, type RuleFields, type Warning,
} from "../api/overrides/encounters";
import { EncounterStatBlock } from "./EncounterStatBlock";

/** Run `work`, closing on success and showing the error in the dialog otherwise. */
function useSave(onClose: () => void) {
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const save = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try {
      await work();
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };
  return { error, busy, save, setError };
}

function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <label className="party-field">
      {label}
      {children}
      {hint && <span className="muted party-note">{hint}</span>}
    </label>
  );
}

/** A whole number typed in (empty is none). */
const whole = (s: string): number | null => (s.trim() === "" || !/^-?\d+$/.test(s.trim()) ? null : Number(s.trim()));

/** The encounter's two names. */
export function DetailsDialog({ enc, onSave, onClose }: {
  enc: Encounter; onSave: (fields: { name: string; publicName: string }) => Promise<unknown>; onClose: () => void;
}) {
  const [name, setName] = useState(enc.name);
  const [publicName, setPublicName] = useState(enc.publicName);
  const { error, busy, save } = useSave(onClose);
  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    void save(() => onSave({ name: name.trim(), publicName: publicName.trim() }));
  };
  return (
    <Modal title="Encounter names" onClose={onClose}
      footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" disabled={busy || !name.trim() || !publicName.trim()} onClick={() => submit()}>Save</Button></>}>
      <form className="party-form" onSubmit={submit}>
        <Field label="Name (only you see it)"><input className="input" aria-label="Encounter name" maxLength={LIMITS.name} value={name} onChange={(e) => setName(e.target.value)} /></Field>
        <Field label="Name players see"><input className="input" aria-label="Name players see" maxLength={LIMITS.publicName} value={publicName} onChange={(e) => setPublicName(e.target.value)} /></Field>
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
    </Modal>
  );
}

/**
 * One creature: the DM's label and the name players see, its starting hit points (rolled in the app or typed from
 * real dice, D21), for a controlled creature when it acts, and the encounter's changes to its stat block (D41: HP,
 * AC, saves, added traits, a note). Changed numbers show the printed ones beside them.
 */
export function CreatureDialog({ creature, owner, onSave, onClose }: {
  creature: Creature;
  /** The DM's label of the creature that controls it, if any. */
  owner?: string;
  onSave: (fields: { dmLabel: string; publicName: string; hpMode: "encounter" | "fixed"; hpMax: number | null; initMode?: InitMode; overrides: OverridesForm }) => Promise<unknown>;
  onClose: () => void;
}) {
  const o = creature.overrides;
  const [dmLabel, setDmLabel] = useState(creature.dmLabel);
  const [publicName, setPublicName] = useState(creature.publicName);
  const [fixed, setFixed] = useState(creature.hpMode === "fixed");
  const [hpMax, setHpMax] = useState(creature.hpMax != null ? String(creature.hpMax) : "");
  const [hp, setHp] = useState(o.hp ? String(o.hp.value.average) : "");
  const [formula, setFormula] = useState(o.hp?.value.formula ?? "");
  const [ac, setAc] = useState(o.ac ? String(o.ac.value.value) : "");
  const [acSource, setAcSource] = useState(o.ac?.value.source ?? "");
  const [saves, setSaves] = useState<Record<Ability, string>>(() =>
    Object.fromEntries(ABILITIES.map((a) => [a, o.saves?.[a] ? String(o.saves[a]!.value) : ""])) as Record<Ability, string>);
  const [traits, setTraits] = useState(o.traits ?? []);
  const [notes, setNotes] = useState(o.notes ?? "");
  const [initMode, setInitMode] = useState<InitMode>(creature.initMode);
  const [showBlock, setShowBlock] = useState(false);
  const [rolled, setRolled] = useState("");
  const { error, busy, save, setError } = useSave(onClose);
  const printed = creature.printed;
  const dice = (formula.trim() && validFormula(formula) ? formula.trim() : null) ?? creature.hp?.formula ?? printed.hp?.formula ?? null;

  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    const hpValue = whole(hp);
    if (hp.trim() && (hpValue == null || hpValue < 1)) return setError("Hit points are a whole number from 1.");
    if (formula.trim() && !validFormula(formula)) return setError("Hit dice look like 4d10+8.");
    const acValue = whole(ac);
    if (ac.trim() && acValue == null) return setError("Armor class is a whole number.");
    const fixedHp = whole(hpMax);
    if (fixed && (fixedHp == null || fixedHp < 1)) return setError("Type the hit points it starts with.");
    const saveForm: Partial<Record<Ability, number | null>> = {};
    for (const a of ABILITIES) {
      const v = whole(saves[a]);
      if (saves[a].trim() && v == null) return setError(`${ABILITY_WORD[a]} save is a whole number like 4 or -1.`);
      const had = o.saves?.[a]?.value ?? null;
      if (v !== had) saveForm[a] = v;
    }
    const overrides: OverridesForm = {
      hp: hpValue != null ? { average: hpValue, ...(formula.trim() ? { formula: formula.trim() } : {}) } : null,
      ac: acValue != null ? { value: acValue, ...(acSource.trim() ? { source: acSource.trim() } : {}) } : null,
      saves: saveForm,
      traits: traits.filter((t) => t.name.trim()).map((t) => ({ name: t.name.trim(), text: t.text.trim() })),
      notes: notes.trim() ? notes.trim() : null,
    };
    void save(() => onSave({
      dmLabel: dmLabel.trim(), publicName: publicName.trim(), hpMode: fixed ? "fixed" : "encounter", hpMax: fixed ? fixedHp : null,
      ...(owner ? { initMode } : {}), overrides,
    }));
  };

  return (
    <Modal title={creature.dmLabel} full onClose={onClose} className="enc-creature-dialog"
      footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" disabled={busy || !dmLabel.trim() || !publicName.trim()} onClick={() => submit()}>Save</Button></>}>
      <form className="party-form enc-form" onSubmit={submit}>
        <p className="muted party-note">{creature.name} · {creature.source}{creature.cr ? ` · CR ${creature.cr}` : ""}{creature.kind === "companion" ? " · Companion" : ""}</p>
        <Field label="Your label (only you see it)"><input className="input" aria-label="Your label" maxLength={LIMITS.label} value={dmLabel} onChange={(e) => setDmLabel(e.target.value)} /></Field>
        <Field label="Name players see" hint="Shown only once you reveal it. A number here would tell players how many are hiding.">
          <input className="input" aria-label="Name players see" maxLength={LIMITS.publicName} value={publicName} onChange={(e) => setPublicName(e.target.value)} />
        </Field>

        <h4 className="party-sub">Starting hit points</h4>
        <div className="segmented enc-segmented" role="radiogroup" aria-label="Starting hit points">
          <button type="button" role="radio" aria-checked={!fixed} onClick={() => setFixed(false)}>As the encounter says</button>
          <button type="button" role="radio" aria-checked={fixed} onClick={() => setFixed(true)}>A number I type</button>
        </div>
        {fixed && (
          <>
            <Field label="Hit points it starts with" hint="Rolled with real dice? Type the total.">
              <input className="input" inputMode="numeric" aria-label="Hit points it starts with" value={hpMax} onChange={(e) => setHpMax(e.target.value)} />
            </Field>
            {dice && (
              <div className="party-line">
                <Button small icon="dice" onClick={() => {
                  const total = rollFormula(dice);
                  if (total != null) { setHpMax(String(total)); setRolled(`Rolled ${dice}: ${total}`); }
                }}>Roll {dice}</Button>
                {rolled && <span className="muted party-note" aria-live="polite">{rolled}</span>}
              </div>
            )}
          </>
        )}
        {owner && (
          <>
            <h4 className="party-sub">Turn (controlled by {owner})</h4>
            <div className="segmented enc-segmented" role="radiogroup" aria-label="When it acts">
              {(["after_owner", "own"] as InitMode[]).map((m) => (
                <button key={m} type="button" role="radio" aria-checked={initMode === m} onClick={() => setInitMode(m)}>{INIT_WORD[m]}</button>
              ))}
            </div>
          </>
        )}

        <h4 className="party-sub">Changes for this encounter</h4>
        <p className="muted party-note">Leave a field empty to keep the printed value.</p>
        <div className="enc-grid2">
          <Field label={`Hit points (printed ${printed.hp ? printed.hp.average : "none"})`}>
            <input className="input" inputMode="numeric" aria-label="Hit points" value={hp} onChange={(e) => setHp(e.target.value)} />
          </Field>
          <Field label={`Hit dice (printed ${printed.hp?.formula ?? "none"})`}>
            <input className="input" aria-label="Hit dice" placeholder="4d10+8" value={formula} onChange={(e) => setFormula(e.target.value)} />
          </Field>
          <Field label={`Armor class (printed ${printed.ac?.value ?? "none"})`}>
            <input className="input" inputMode="numeric" aria-label="Armor class" value={ac} onChange={(e) => setAc(e.target.value)} />
          </Field>
          <Field label="What gives it">
            <input className="input" aria-label="What gives the armor class" placeholder="natural armor" value={acSource} onChange={(e) => setAcSource(e.target.value)} />
          </Field>
        </div>
        <fieldset className="enc-saves">
          <legend>Saving throws</legend>
          {ABILITIES.map((a) => (
            <label key={a} className="enc-save">
              <span>{ABILITY_WORD[a]} <span className="muted">({printed.saves[a] != null ? signed(printed.saves[a]!) : "none"})</span></span>
              <input className="input" inputMode="numeric" aria-label={`${ABILITY_WORD[a]} save`} value={saves[a]} onChange={(e) => setSaves({ ...saves, [a]: e.target.value })} />
            </label>
          ))}
        </fieldset>
        <h4 className="party-sub">Added traits</h4>
        {traits.map((t, i) => (
          <div key={i} className="enc-trait">
            <input className="input" aria-label={`Trait ${i + 1} name`} maxLength={LIMITS.traitName} value={t.name} onChange={(e) => setTraits(traits.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))} />
            <textarea className="input" aria-label={`Trait ${i + 1} text`} rows={2} maxLength={LIMITS.notes} value={t.text} onChange={(e) => setTraits(traits.map((x, j) => (j === i ? { ...x, text: e.target.value } : x)))} />
            <Button small variant="danger" onClick={() => setTraits(traits.filter((_, j) => j !== i))}>Remove trait</Button>
          </div>
        ))}
        {traits.length < LIMITS.traits && <Button small icon="plus" onClick={() => setTraits([...traits, { name: "", text: "" }])}>Add a trait</Button>}
        <Field label="Your note on this creature (only you see it)">
          <textarea className="input" aria-label="Your note" rows={3} maxLength={LIMITS.notes} value={notes} onChange={(e) => setNotes(e.target.value)} />
        </Field>
        <Button small onClick={() => setShowBlock(!showBlock)} aria-expanded={showBlock}>{showBlock ? "Hide the stat block" : "Show the stat block"}</Button>
        {showBlock && <EncounterStatBlock title={creature.name} block={creature.statBlock} />}
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
    </Modal>
  );
}

const shown = (v: unknown): string => {
  if (v == null) return "none";
  if (typeof v === "object") {
    const o = v as Record<string, unknown>;
    if ("average" in o) return `${String(o.average)}${o.formula ? ` (${String(o.formula)})` : ""}`;
    if ("value" in o) return `${String(o.value)}${o.source ? ` (${String(o.source)})` : ""}`;
    return JSON.stringify(v);
  }
  return String(v);
};
const FIELD_WORD = (f: string) => (f === "hp" ? "Hit points" : f === "ac" ? "Armor class" : f.startsWith("saves.") ? `${f.slice(6).toUpperCase()} save` : f);

/** "Refresh stat block" found fields both the DM and the source changed: the DM picks, per field (D41). */
export function RefreshDialog({ creature, conflicts, onApply, onClose }: {
  creature: Creature; conflicts: Conflict[]; onApply: (choices: Record<string, "keep" | "source">) => Promise<unknown>; onClose: () => void;
}) {
  const [choices, setChoices] = useState<Record<string, "keep" | "source">>({});
  const { error, busy, save } = useSave(onClose);
  const all = conflicts.every((c) => choices[c.field]);
  return (
    <Modal title={`Refresh ${creature.dmLabel}`} onClose={onClose}
      footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" disabled={busy || !all} onClick={() => void save(() => onApply(choices))}>Refresh</Button></>}>
      <p>The source's stat block changed where you also changed it. Choose which value to use.</p>
      <ul className="party-list">
        {conflicts.map((c) => (
          <li key={c.field} className="party-member">
            <strong>{FIELD_WORD(c.field)}</strong>
            <p className="muted party-note">Printed when you changed it: {shown(c.was)} · printed now: {shown(c.source)} · yours: {shown(c.value)}</p>
            <div className="segmented enc-segmented" role="radiogroup" aria-label={FIELD_WORD(c.field)}>
              <button type="button" role="radio" aria-checked={choices[c.field] === "keep"} onClick={() => setChoices({ ...choices, [c.field]: "keep" })}>Keep mine ({shown(c.value)})</button>
              <button type="button" role="radio" aria-checked={choices[c.field] === "source"} onClick={() => setChoices({ ...choices, [c.field]: "source" })}>Use the source ({shown(c.source)})</button>
            </div>
          </li>
        ))}
      </ul>
      {error && <Alert tone="danger">{error}</Alert>}
    </Modal>
  );
}

const creatureOptions = (list: Creature[]) => list.map((c) => <option key={c.id} value={c.id}>{c.dmLabel}</option>);

/** A starting effect (C19): a buff on a creature before the fight, with what it costs and how long it lasts. */
export function EffectDialog({ creatures, effect, onSave, onClose }: {
  creatures: Creature[]; effect?: Effect; onSave: (fields: EffectFields) => Promise<unknown>; onClose: () => void;
}) {
  const [creatureId, setCreatureId] = useState(effect?.creatureId ?? creatures[0]?.id ?? "");
  const [name, setName] = useState(effect?.name ?? "");
  const [detail, setDetail] = useState(effect?.detail ?? "");
  const [source, setSource] = useState(effect?.sourceCreatureId ?? "");
  const [duration, setDuration] = useState(effect?.durationTurns != null ? String(effect.durationTurns) : "");
  const [charges, setCharges] = useState(effect?.charges != null ? String(effect.charges) : "");
  const [slot, setSlot] = useState(effect?.slotLevel != null ? String(effect.slotLevel) : "");
  const [concentration, setConcentration] = useState(effect?.concentration ?? false);
  const [acMod, setAcMod] = useState(effect?.modifiers.ac != null ? String(effect.modifiers.ac) : "");
  const [speedAdd, setSpeedAdd] = useState(effect?.modifiers.speedAdd != null ? String(effect.modifiers.speedAdd) : "");
  const { error, busy, save, setError } = useSave(onClose);
  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    const nums = { durationTurns: whole(duration), charges: whole(charges), slotLevel: whole(slot), ac: whole(acMod), speedAdd: whole(speedAdd) };
    for (const [text, v] of [[duration, nums.durationTurns], [charges, nums.charges], [slot, nums.slotLevel], [acMod, nums.ac], [speedAdd, nums.speedAdd]] as const) {
      if (text.trim() && v == null) return setError("Numbers are whole numbers.");
    }
    const modifiers = { ...(nums.ac != null ? { ac: nums.ac } : {}), ...(nums.speedAdd != null ? { speedAdd: nums.speedAdd } : {}) };
    const fields: EffectFields = {
      ...(effect ? {} : { creatureId }),
      name: name.trim(), detail: detail.trim(), sourceCreatureId: source || null,
      durationTurns: nums.durationTurns, charges: nums.charges, slotLevel: nums.slotLevel, concentration, modifiers,
    };
    void save(() => onSave(fields));
  };
  return (
    <Modal title={effect ? `Edit ${effect.name}` : "Starting effect"} onClose={onClose}
      footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" disabled={busy || !name.trim() || !creatureId} onClick={() => submit()}>Save</Button></>}>
      <form className="party-form enc-form" onSubmit={submit}>
        <p className="muted party-note">Applied when the fight starts.</p>
        <Field label="On"><select className="input" aria-label="On" value={creatureId} disabled={Boolean(effect)} onChange={(e) => setCreatureId(e.target.value)}>{creatureOptions(creatures)}</select></Field>
        <Field label="Effect"><input className="input" aria-label="Effect" maxLength={LIMITS.effectName} placeholder="Mage Armor" value={name} onChange={(e) => setName(e.target.value)} /></Field>
        <Field label="Details"><textarea className="input" aria-label="Details" rows={2} maxLength={LIMITS.detail} value={detail} onChange={(e) => setDetail(e.target.value)} /></Field>
        <Field label="Cast by"><select className="input" aria-label="Cast by" value={source} onChange={(e) => setSource(e.target.value)}><option value="">Nobody here</option>{creatureOptions(creatures)}</select></Field>
        <div className="enc-grid2">
          <Field label="Lasts (caster's turns)"><input className="input" inputMode="numeric" aria-label="Lasts" value={duration} onChange={(e) => setDuration(e.target.value)} /></Field>
          <Field label="Charges"><input className="input" inputMode="numeric" aria-label="Charges" value={charges} onChange={(e) => setCharges(e.target.value)} /></Field>
          <Field label="Spell slot spent"><input className="input" inputMode="numeric" aria-label="Spell slot spent" value={slot} onChange={(e) => setSlot(e.target.value)} /></Field>
          <Field label="AC change"><input className="input" inputMode="numeric" aria-label="AC change" value={acMod} onChange={(e) => setAcMod(e.target.value)} /></Field>
          <Field label="Speed change (ft.)"><input className="input" inputMode="numeric" aria-label="Speed change" value={speedAdd} onChange={(e) => setSpeedAdd(e.target.value)} /></Field>
        </div>
        <label className="party-line enc-check"><input type="checkbox" checked={concentration} onChange={(e) => setConcentration(e.target.checked)} /> Needs concentration</label>
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
    </Modal>
  );
}

/**
 * An encounter rule (C20, D42): a banner for the DM, tied to a creature, ending when it drops or by hand, with
 * abilities it switches on or off while it lasts. A note and flags, no scripting.
 */
export function RuleDialog({ creatures, rule, onSave, onClose }: {
  creatures: Creature[]; rule?: Rule; onSave: (fields: RuleFields) => Promise<unknown>; onClose: () => void;
}) {
  const [name, setName] = useState(rule?.name ?? "");
  const [text, setText] = useState(rule?.text ?? "");
  const [creatureId, setCreatureId] = useState(rule?.creatureId ?? creatures[0]?.id ?? "");
  const [endsWhen, setEndsWhen] = useState<Rule["endsWhen"]>(rule?.endsWhen ?? (creatures.length ? "creature_down" : "manual"));
  const [flags, setFlags] = useState(rule?.flags ?? []);
  const { error, busy, save } = useSave(onClose);
  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    void save(() => onSave({
      name: name.trim(), text: text.trim(), creatureId: creatureId || null, endsWhen,
      flags: flags.filter((f) => f.ability.trim()).map((f) => ({ ...f, ability: f.ability.trim() })),
    }));
  };
  return (
    <Modal title={rule ? `Edit ${rule.name}` : "Encounter rule"} onClose={onClose}
      footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" disabled={busy || !name.trim() || (endsWhen === "creature_down" && !creatureId)} onClick={() => submit()}>Save</Button></>}>
      <form className="party-form enc-form" onSubmit={submit}>
        <Field label="Rule"><input className="input" aria-label="Rule" maxLength={LIMITS.name} placeholder="While the leader stands" value={name} onChange={(e) => setName(e.target.value)} /></Field>
        <Field label="What it does (your banner)"><textarea className="input" aria-label="What it does" rows={3} maxLength={LIMITS.ruleText} value={text} onChange={(e) => setText(e.target.value)} /></Field>
        <Field label="Tied to"><select className="input" aria-label="Tied to" value={creatureId} onChange={(e) => setCreatureId(e.target.value)}><option value="">No creature</option>{creatureOptions(creatures)}</select></Field>
        <div className="segmented enc-segmented" role="radiogroup" aria-label="Ends">
          <button type="button" role="radio" aria-checked={endsWhen === "creature_down"} disabled={!creatureId} onClick={() => setEndsWhen("creature_down")}>When it drops to 0 HP</button>
          <button type="button" role="radio" aria-checked={endsWhen === "manual"} onClick={() => setEndsWhen("manual")}>When I end it</button>
        </div>
        <h4 className="party-sub">While it lasts</h4>
        {flags.map((f, i) => (
          <div key={i} className="enc-flag">
            <select className="input" aria-label={`Switch ${i + 1}: creature`} value={f.creatureId} onChange={(e) => setFlags(flags.map((x, j) => (j === i ? { ...x, creatureId: e.target.value } : x)))}>{creatureOptions(creatures)}</select>
            <input className="input" aria-label={`Switch ${i + 1}: ability`} maxLength={LIMITS.traitName} placeholder="Ability, as printed" value={f.ability} onChange={(e) => setFlags(flags.map((x, j) => (j === i ? { ...x, ability: e.target.value } : x)))} />
            <select className="input" aria-label={`Switch ${i + 1}: on or off`} value={f.whileActive} onChange={(e) => setFlags(flags.map((x, j) => (j === i ? { ...x, whileActive: e.target.value as "on" | "off" } : x)))}>
              <option value="on">can be used</option>
              <option value="off">can't be used</option>
            </select>
            <Button small variant="danger" onClick={() => setFlags(flags.filter((_, j) => j !== i))}>Remove</Button>
          </div>
        ))}
        {creatures.length > 0 && flags.length < LIMITS.flags && (
          <Button small icon="plus" onClick={() => setFlags([...flags, { creatureId: creatures[0]!.id, ability: "", whileActive: "on" }])}>Switch an ability</Button>
        )}
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
    </Modal>
  );
}

/** The edition notes (D23 b): creatures and characters of the other edition. */
export function WarningsDialog({ warnings, onClose }: { warnings: Warning[]; onClose: () => void }) {
  return (
    <Modal title="Edition notes" onClose={onClose} footer={<Button onClick={onClose}>Close</Button>}>
      {warnings.length === 0 ? <p className="muted">Everything uses the party's edition.</p> : (
        <ul className="party-list">
          {warnings.map((w, i) => <li key={`${w.kind}-${i}`} className="party-line"><span className="grow">{w.text}</span></li>)}
        </ul>
      )}
    </Modal>
  );
}
