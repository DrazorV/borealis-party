import "./spells.css";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Button } from "../../../src/components/Button";
import { Dialog } from "../../../src/components/controls/Dialog";
import { useToast } from "../../../src/components/Toast";
import { usePhone } from "../../../src/lib/phone";
import { errorMessage } from "../../../src/state/useAsync";
import { ABILITY_NAMES, CONDITIONS, DAMAGE_TYPES, type AbilityName, type DamageType } from "./common";
import { preview } from "./formula";
import { draftMechanic, exportMechanics, importMechanics, listMechanics, saveMechanic, type MechDef, type MechDraft, type Preset } from "./spellApi";
import type { CastTime, Primitive, SpellMech, SummonOption } from "./spellTypes";

export const PRESETS: { id: Preset; title: string; text: string }[] = [
  { id: "buff", title: "Buff", text: "AC, dice on rolls, speed" },
  { id: "damage_save", title: "Damage + save", text: "an area or targets save; half or none" },
  { id: "spell_attack", title: "Spell attack", text: "roll to hit, then damage" },
  { id: "heal", title: "Heal", text: "dice + modifier" },
  { id: "condition", title: "Condition + repeat", text: "save or a condition, saved again later" },
  { id: "summon", title: "Summon info", text: "the creature options; you add them" },
  { id: "ruling", title: "Ruling", text: "a question and your buttons" },
  { id: "note", title: "Note", text: "spend the slot, keep a reminder" },
];

/** The builder's fields: plain strings and flags, turned into a SpellMech by {@link buildMech}. */
export interface Fields {
  preset: Preset;
  level: string; cast: CastTime; concentration: boolean; duration: string;
  target: "self" | "creatures" | "area"; max: string; filterType: string; willing: boolean; shape: string; size: string;
  saveAbility: AbilityName; half: boolean;
  dice: string; damageType: DamageType | "";
  heal: string; healMod: boolean;
  condition: string; repeatAt: "" | "start" | "end";
  acBase: string; acDex: boolean; acBonus: string; rollTerm: string; rollOn: "attack" | "save" | "both"; speedMult: boolean;
  summon: { cr: string; count: string }[]; summonType: string;
  question: string; buttons: string; note: string;
}

const base = (level: number): Fields => ({
  preset: "buff", level: String(level), cast: "action", concentration: false, duration: "",
  target: "creatures", max: "1", filterType: "", willing: false, shape: "sphere", size: "20",
  saveAbility: "dex", half: true, dice: "", damageType: "", heal: "", healMod: true,
  condition: "", repeatAt: "", acBase: "", acDex: true, acBonus: "", rollTerm: "", rollOn: "both", speedMult: false,
  summon: [{ cr: "", count: "1" }], summonType: "beast", question: "", buttons: "Yes, No", note: "",
});

export function presetFields(preset: Preset, f: Fields): Fields {
  const next = { ...f, preset };
  if (preset === "damage_save") Object.assign(next, { target: "area", half: true });
  if (preset === "spell_attack") Object.assign(next, { target: "creatures", max: "1" });
  if (preset === "heal") Object.assign(next, { target: "creatures", max: "1" });
  if (preset === "condition") Object.assign(next, { target: "creatures", half: false, saveAbility: "wis", concentration: true, repeatAt: "end" });
  if (preset === "summon") Object.assign(next, { target: "self", concentration: true });
  if (preset === "buff") Object.assign(next, { target: "creatures", willing: true });
  return next;
}

/** The fields of a saved or drafted mechanic. */
export function fieldsFrom(def: Partial<SpellMech>, preset: Preset, level: number): Fields {
  const f = presetFields(preset, base(def.level ?? level));
  if (def.cast) f.cast = def.cast;
  if (def.concentration != null) f.concentration = def.concentration;
  if (def.duration_turns) f.duration = String(def.duration_turns);
  const t = def.targeting;
  if (t?.kind === "self") f.target = "self";
  if (t?.kind === "creatures") { f.target = "creatures"; f.max = String(t.max); f.willing = t.filter?.kind === "willing"; f.filterType = t.filter?.kind === "type" ? t.filter.types.join(", ") : ""; }
  if (t?.kind === "area") { f.target = "area"; f.shape = t.shape; f.size = String(t.size_ft); }
  const d = def.delivery;
  if (d?.kind === "save") { f.saveAbility = d.ability; f.half = d.success === "half"; }
  for (const p of [...(def.on_land ?? []), ...(def.always ?? [])]) {
    if (p.kind === "damage") { f.dice = String(p.dice); f.damageType = p.type ?? ""; }
    if (p.kind === "heal") { f.heal = String(p.dice); f.healMod = Boolean(p.add_mod); }
    if (p.kind === "condition") { f.condition = p.condition; f.repeatAt = p.repeat_save?.at ?? ""; }
    if (p.kind === "ac_base") { f.acBase = String(p.base); f.acDex = p.add_dex; }
    if (p.kind === "ac_bonus") f.acBonus = String(p.value);
    if (p.kind === "roll_dice") f.rollTerm = p.term;
    if (p.kind === "speed" && p.op === "mult") f.speedMult = true;
    if (p.kind === "summon_info") { f.summon = p.options.map((o) => ({ cr: String(o.cr), count: String(o.count) })); f.summonType = p.creature_type ?? ""; }
    if (p.kind === "ruling") { f.question = p.question; f.buttons = p.buttons.join(", "); }
    if (p.kind === "note") f.note = p.text ?? p.key;
  }
  return f;
}

const crValue = (s: string): number | null => {
  const t = s.trim();
  if (/^\d+\/\d+$/.test(t)) { const [a, b] = t.split("/").map(Number); return a! / b!; }
  return /^\d+(\.\d+)?$/.test(t) ? Number(t) : null;
};

/** The SpellMech the fields describe (closed schema, numbers only). */
export function buildMech(key: string, name: string, f: Fields): SpellMech {
  const level = Math.max(0, Math.min(9, Number(f.level) || 0));
  const prim: Primitive[] = [];
  const always: Primitive[] = [];
  const p = f.preset;
  if ((p === "damage_save" || p === "spell_attack") && f.dice.trim()) prim.push({ kind: "damage", dice: f.dice.trim(), ...(f.damageType ? { type: f.damageType } : {}) });
  if (p === "heal" && f.heal.trim()) prim.push({ kind: "heal", dice: f.heal.trim(), add_mod: f.healMod });
  if ((p === "condition" || p === "damage_save") && f.condition) prim.push({ kind: "condition", condition: f.condition, ...(f.repeatAt ? { repeat_save: { ability: f.saveAbility, at: f.repeatAt } } : {}) });
  if (p === "buff") {
    if (f.acBase.trim()) always.push({ kind: "ac_base", base: Number(f.acBase), add_dex: f.acDex });
    if (f.acBonus.trim()) always.push({ kind: "ac_bonus", value: Number(f.acBonus) });
    if (f.rollTerm.trim()) {
      if (f.rollOn !== "save") always.push({ kind: "roll_dice", on: "attack", term: f.rollTerm.trim() });
      if (f.rollOn !== "attack") always.push({ kind: "roll_dice", on: "save", term: f.rollTerm.trim() });
    }
    if (f.speedMult) always.push({ kind: "speed", op: "mult", value: 2 });
  }
  if (p === "summon") {
    const options: SummonOption[] = f.summon.map((o) => ({ cr: crValue(o.cr) ?? -1, count: Number(o.count) || 1 })).filter((o) => o.cr >= 0);
    always.push({ kind: "summon_info", options, ...(f.summonType.trim() ? { creature_type: f.summonType.trim().toLowerCase() } : {}) });
  }
  if (p === "ruling") prim.push({ kind: "ruling", question: f.question.trim() || "What happens?", buttons: f.buttons.split(",").map((b) => b.trim()).filter(Boolean) });
  if (p === "note" && f.note.trim()) always.push({ kind: "note", key: "dm_note", text: f.note.trim() });
  const filterTypes = f.filterType.split(",").map((t) => t.trim().toLowerCase()).filter(Boolean);
  return {
    key, name, level, concentration: f.concentration, cast: f.cast,
    targeting: f.target === "self" ? { kind: "self" } : f.target === "area" ? { kind: "area", shape: f.shape, size_ft: Number(f.size) || 0 }
      : { kind: "creatures", max: /^\d+$/.test(f.max.trim()) ? Number(f.max) : f.max.trim() || 1, ...(filterTypes.length ? { filter: { kind: "type" as const, types: filterTypes } } : f.willing ? { filter: { kind: "willing" as const } } : {}) },
    delivery: p === "spell_attack" ? { kind: "attack" } : (p === "damage_save" || p === "condition") ? { kind: "save", ability: f.saveAbility, success: f.half ? "half" : "none" } : { kind: "auto" },
    ...(prim.length ? { on_land: prim } : {}),
    ...(always.length ? { always } : {}),
    ...(f.duration.trim() && Number(f.duration) > 0 ? { duration_turns: Number(f.duration) } : {}),
    tier: p === "ruling" ? "ruling" : p === "note" ? "utility" : "partial",
  };
}

/** Number chips from the user's own rules text: dice, distances, DCs and ability words. */
export function numberChips(text: string): { label: string; value: string; kind: "dice" | "number" | "ability" | "condition" }[] {
  const out: { label: string; value: string; kind: "dice" | "number" | "ability" | "condition" }[] = [];
  const seen = new Set<string>();
  const add = (label: string, value: string, kind: (typeof out)[number]["kind"]) => { if (!seen.has(label)) { seen.add(label); out.push({ label, value, kind }); } };
  for (const m of text.matchAll(/\b(\d+d\d+(?:\s*[+-]\s*\d+)?)\b/g)) add(m[1]!.replace(/\s+/g, ""), m[1]!.replace(/\s+/g, ""), "dice");
  for (const m of text.matchAll(/\b(\d+)[- ](?:foot|feet|ft)\b/gi)) add(`${m[1]} ft`, m[1]!, "number");
  for (const m of text.matchAll(/\bDC (\d+)\b/g)) add(`DC ${m[1]}`, m[1]!, "number");
  for (const m of text.matchAll(/\b(\d+) (?:hit points|turns|rounds|creatures|targets)\b/gi)) add(m[0]!, m[1]!, "number");
  const words: Record<string, AbilityName> = { strength: "str", dexterity: "dex", constitution: "con", intelligence: "int", wisdom: "wis", charisma: "cha" };
  for (const [w, a] of Object.entries(words)) if (new RegExp(`\\b${w}\\b`, "i").test(text)) add(w.charAt(0).toUpperCase() + w.slice(1), a, "ability");
  for (const c of CONDITIONS) if (new RegExp(`\\b${c}\\b`, "i").test(text)) add(c, c, "condition");
  return out;
}

type FieldKey = "dice" | "heal" | "max" | "size" | "duration" | "acBase" | "acBonus" | "rollTerm" | "level";

/**
 * The DM effect builder (0.3.2, design 4): define a spell's mechanics once, from preset cards, for a spell the app
 * has no rules for (a homebrew or non-SRD spell), or check a draft suggested from the user's own spell tags. Number
 * chips come from the user's own spell text; drafted fields say "check". A live preview shows what a cast does. Saved
 * to the DM's account; exported and imported as JSON. Nothing is guessed from text without the DM's look.
 */
export function EffectBuilder({ spellKey, spellName, ruleset, level = 1, text = "", suggested, onSaved, onClose }: {
  spellKey: string; spellName: string; ruleset: string; level?: number; text?: string;
  /** Fetch a tag draft first ("Suggested"). */
  suggested?: boolean;
  onSaved?: (def: MechDef) => void; onClose: () => void;
}) {
  const phone = usePhone();
  const toast = useToast();
  const [f, setF] = useState<Fields | null>(suggested ? null : null);
  const [checks, setChecks] = useState<Set<string>>(new Set());
  const [draftText, setDraftText] = useState(text);
  const [last, setLast] = useState<FieldKey>("dice");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [library, setLibrary] = useState(false);
  const [loaded, setLoaded] = useState(!suggested);

  useEffect(() => {
    let live = true;
    void (async () => {
      const saved = (await listMechanics().catch(() => [] as MechDef[])).find((d) => d.key === spellKey && d.ruleset === ruleset);
      if (!live) return;
      if (saved) { setF(fieldsFrom(saved.def, guessPreset(saved.def), level)); setLoaded(true); return; }
      if (suggested) {
        const d: MechDraft | null = await draftMechanic(spellKey, ruleset);
        if (!live) return;
        if (d) { setF(fieldsFrom(d.def, d.preset, level)); setChecks(new Set(d.checks)); if (d.text) setDraftText(d.text); }
      }
      setLoaded(true);
    })();
    return () => { live = false; };
  }, [spellKey, ruleset, suggested, level]);

  const set = <K extends keyof Fields>(k: K, v: Fields[K]) => {
    setF((x) => (x ? { ...x, [k]: v } : x));
    setChecks((c) => { const n = new Set(c); for (const g of CHECK_OF[k as string] ?? []) n.delete(g); return n; });
  };
  const chips = useMemo(() => numberChips(draftText), [draftText]);
  const mech = f ? buildMech(spellKey, spellName, f) : null;
  const chip = (c: ReturnType<typeof numberChips>[number]) => {
    if (!f) return;
    if (c.kind === "ability") set("saveAbility", c.value as AbilityName);
    else if (c.kind === "condition") set("condition", c.value);
    else if (c.kind === "dice") set(last === "heal" ? "heal" : last === "rollTerm" ? "rollTerm" : "dice", c.value);
    else set(last === "dice" || last === "heal" || last === "rollTerm" ? "size" : last, c.value);
  };
  const save = async () => {
    if (!mech) return;
    setBusy(true);
    setError("");
    try {
      const def = await saveMechanic(spellKey, ruleset, mech);
      toast.push(`${spellName}: saved to your account. The next cast uses it.`);
      onSaved?.(def);
      onClose();
    } catch (e) { setError(errorMessage(e)); setBusy(false); }
  };
  const check = (group: string) => (checks.has(group) ? <span className="cb-check-mark" title="Drafted from your spell data's tags: check it">check</span> : null);
  const num = (k: FieldKey, label: string, placeholder = "") => (
    <label className="cb-field cb-narrow"><span>{label}</span>
      <input className="input" value={f ? String(f[k]) : ""} placeholder={placeholder} aria-label={label} onFocus={() => setLast(k)} onChange={(e) => set(k, e.target.value)} />
    </label>
  );

  return (
    <Dialog title={`Define effect: ${spellName}`} onClose={onClose} full={phone} wide={!phone} className="cb-builder"
      footer={library ? <Button onClick={() => setLibrary(false)}>Back</Button> : <>
        <Button variant="ghost" onClick={() => setLibrary(true)}>My effects…</Button>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={!mech || busy} onClick={() => void save()}>Save effect</Button>
      </>}>
      {library ? <EffectLibrary /> : !loaded ? <p className="muted">Loading…</p> : !f ? (
        <div>
          <p className="cb-small muted">Pick what {spellName} mostly does. You can change every field next.</p>
          <ul className="cb-presets" aria-label="Kinds of effect">
            {PRESETS.map((p) => (
              <li key={p.id}><button type="button" className="cb-preset" onClick={() => setF(presetFields(p.id, base(level)))}><b>{p.title}</b><span className="cb-small muted">{p.text}</span></button></li>
            ))}
          </ul>
        </div>
      ) : (
        <div className="cb-form cb-builder-body">
          <div className="cb-chips" role="radiogroup" aria-label="Kind of effect">
            {PRESETS.map((p) => <button key={p.id} type="button" role="radio" aria-checked={f.preset === p.id} className={`cb-chip${f.preset === p.id ? " on" : ""}`} onClick={() => setF(presetFields(p.id, f))}>{p.title}</button>)}
          </div>
          {chips.length > 0 && (
            <section aria-label="Numbers from your spell text">
              <h4 className="cb-group-title">From your spell's text <span className="muted cb-small">(tap to fill the field you were in)</span></h4>
              <div className="cb-chips">{chips.map((c) => <button key={c.label} type="button" className="cb-chip cb-numchip" onClick={() => chip(c)}>{c.label}</button>)}</div>
            </section>
          )}
          <Group title="Casting" mark={check("cast")}>
            {num("level", "Level", "0–9")}
            <label className="cb-field cb-narrow"><span>Takes</span>
              <select className="input" value={f.cast} onChange={(e) => set("cast", e.target.value as CastTime)} aria-label="Casting time">
                <option value="action">An action</option><option value="bonus">A bonus action</option><option value="reaction">A reaction</option><option value="minute">A minute or more</option>
              </select>
            </label>
            {num("duration", "Lasts (caster's turns)", "10 = 1 min")}
            <label className="cb-check"><input type="checkbox" checked={f.concentration} onChange={(e) => set("concentration", e.target.checked)} />Concentration</label>
          </Group>
          <Group title="Targets" mark={check("targeting")}>
            <label className="cb-field cb-narrow"><span>Who</span>
              <select className="input" value={f.target} onChange={(e) => set("target", e.target.value as Fields["target"])} aria-label="Who it targets">
                <option value="self">Self</option><option value="creatures">Creatures</option><option value="area">An area</option>
              </select>
            </label>
            {f.target === "creatures" && <>{num("max", "How many", "1+slot_above")}
              <label className="cb-field"><span>Only (types)</span><input className="input" value={f.filterType} placeholder="humanoid" onChange={(e) => set("filterType", e.target.value)} aria-label="Only these creature types" /></label>
              <label className="cb-check"><input type="checkbox" checked={f.willing} onChange={(e) => set("willing", e.target.checked)} />Willing only</label></>}
            {f.target === "area" && <>
              <label className="cb-field cb-narrow"><span>Shape</span>
                <select className="input" value={f.shape} onChange={(e) => set("shape", e.target.value)} aria-label="Area shape">
                  {["sphere", "cone", "cube", "line", "cylinder", "emanation"].map((s) => <option key={s} value={s}>{s}</option>)}
                </select>
              </label>
              {num("size", "Size (ft)")}</>}
          </Group>
          {(f.preset === "damage_save" || f.preset === "condition") && (
            <Group title="Save" mark={check("delivery")}>
              <label className="cb-field cb-narrow"><span>Ability</span>
                <select className="input" value={f.saveAbility} onChange={(e) => set("saveAbility", e.target.value as AbilityName)} aria-label="Save ability">
                  {ABILITY_NAMES.map((a) => <option key={a} value={a}>{a.toUpperCase()}</option>)}
                </select>
              </label>
              <label className="cb-check"><input type="checkbox" checked={f.half} onChange={(e) => set("half", e.target.checked)} />Half damage on a success</label>
            </Group>
          )}
          {(f.preset === "damage_save" || f.preset === "spell_attack") && (
            <Group title="Damage" mark={check("on_land")}>
              {num("dice", "Dice", "8d6+(slot_above)d6")}
              <label className="cb-field cb-narrow"><span>Type</span>
                <select className="input" value={f.damageType} onChange={(e) => set("damageType", e.target.value as DamageType)} aria-label="Damage type">
                  <option value="">—</option>{DAMAGE_TYPES.map((t) => <option key={t} value={t}>{t}</option>)}
                </select>
              </label>
            </Group>
          )}
          {f.preset === "heal" && <Group title="Healing" mark={check("on_land")}>{num("heal", "Dice", "2d8")}<label className="cb-check"><input type="checkbox" checked={f.healMod} onChange={(e) => set("healMod", e.target.checked)} />+ the caster's modifier</label></Group>}
          {(f.preset === "condition" || f.preset === "damage_save") && (
            <Group title="Condition on a failed save" mark={check("condition")}>
              <label className="cb-field cb-narrow"><span>Condition</span>
                <select className="input" value={f.condition} onChange={(e) => set("condition", e.target.value)} aria-label="Condition">
                  <option value="">None</option>{CONDITIONS.map((c) => <option key={c} value={c}>{c}</option>)}
                </select>
              </label>
              <label className="cb-field cb-narrow"><span>Saves again</span>
                <select className="input" value={f.repeatAt} onChange={(e) => set("repeatAt", e.target.value as Fields["repeatAt"])} aria-label="Repeat save">
                  <option value="">Never</option><option value="end">End of its turns</option><option value="start">Start of its turns</option>
                </select>
              </label>
            </Group>
          )}
          {f.preset === "buff" && (
            <Group title="Buff" mark={check("always")}>
              {num("acBase", "AC becomes", "13")}
              <label className="cb-check"><input type="checkbox" checked={f.acDex} onChange={(e) => set("acDex", e.target.checked)} />+ DEX</label>
              {num("acBonus", "AC +", "2")}
              {num("rollTerm", "Dice on rolls", "1d4")}
              <label className="cb-field cb-narrow"><span>On</span>
                <select className="input" value={f.rollOn} onChange={(e) => set("rollOn", e.target.value as Fields["rollOn"])} aria-label="Dice on">
                  <option value="both">Attacks and saves</option><option value="attack">Attacks</option><option value="save">Saves</option>
                </select>
              </label>
              <label className="cb-check"><input type="checkbox" checked={f.speedMult} onChange={(e) => set("speedMult", e.target.checked)} />Speed doubled</label>
            </Group>
          )}
          {f.preset === "summon" && (
            <Group title="Creature options (you add them with Add creature)" mark={check("summon")}>
              <label className="cb-field cb-narrow"><span>Type</span><input className="input" value={f.summonType} onChange={(e) => set("summonType", e.target.value)} aria-label="Creature type" /></label>
              {f.summon.map((o, i) => (
                <span key={i} className="cb-frow">
                  <label className="cb-field cb-narrow"><span>CR</span><input className="input" value={o.cr} placeholder="1/2" aria-label={`Option ${i + 1} CR`} onChange={(e) => set("summon", f.summon.map((x, j) => (j === i ? { ...x, cr: e.target.value } : x)))} /></label>
                  <label className="cb-field cb-narrow"><span>How many</span><input className="input" inputMode="numeric" value={o.count} aria-label={`Option ${i + 1} count`} onChange={(e) => set("summon", f.summon.map((x, j) => (j === i ? { ...x, count: e.target.value } : x)))} /></label>
                </span>
              ))}
              <Button small icon="plus" onClick={() => set("summon", [...f.summon, { cr: "", count: "1" }])}>Another option</Button>
            </Group>
          )}
          {f.preset === "ruling" && (
            <Group title="Your ruling">
              <label className="cb-field"><span>Question</span><input className="input" value={f.question} placeholder="Was the spell countered?" onChange={(e) => set("question", e.target.value)} aria-label="Question" /></label>
              <label className="cb-field"><span>Buttons (comma separated)</span><input className="input" value={f.buttons} onChange={(e) => set("buttons", e.target.value)} aria-label="Buttons" /></label>
            </Group>
          )}
          {f.preset === "note" && <Group title="Reminder"><label className="cb-field"><span>Note</span><input className="input" value={f.note} onChange={(e) => set("note", e.target.value)} aria-label="Note" /></label></Group>}
          {mech && <p className="cb-preview" aria-live="polite" data-preview>Preview: {preview(mech, mech.level > 0 ? mech.level : undefined) || "the slot and the action are spent"}</p>}
          {error && <p className="cb-why" role="alert">{error}</p>}
          {draftText && <details className="cb-small"><summary>Your spell's text</summary><p className="cb-ability-text">{draftText}</p></details>}
        </div>
      )}
    </Dialog>
  );
}

const CHECK_OF: Record<string, string[]> = {
  level: ["cast"], cast: ["cast"], concentration: ["cast"], duration: ["cast"],
  target: ["targeting"], max: ["targeting"], filterType: ["targeting"], willing: ["targeting"], shape: ["targeting"], size: ["targeting"],
  saveAbility: ["delivery"], half: ["delivery"], dice: ["on_land"], damageType: ["on_land"], heal: ["on_land"], healMod: ["on_land"],
  condition: ["condition"], repeatAt: ["condition"], acBase: ["always"], acBonus: ["always"], rollTerm: ["always"], summon: ["summon"],
};

export function guessPreset(d: Partial<SpellMech>): Preset {
  const all = [...(d.on_land ?? []), ...(d.always ?? [])];
  if (all.some((p) => p.kind === "summon_info")) return "summon";
  if (all.some((p) => p.kind === "ruling")) return "ruling";
  if (all.some((p) => p.kind === "heal")) return "heal";
  if (d.delivery?.kind === "attack") return "spell_attack";
  if (d.delivery?.kind === "save") return all.some((p) => p.kind === "damage") ? "damage_save" : "condition";
  if (all.some((p) => p.kind === "note") && all.length === 1) return "note";
  return "buff";
}

function Group({ title, mark, children }: { title: string; mark?: ReactNode; children: ReactNode }) {
  return (
    <fieldset className="cb-fieldset cb-builder-group">
      <legend>{title} {mark}</legend>
      <div className="cb-frow">{children}</div>
    </fieldset>
  );
}

/** The DM's saved effects: export as JSON, import a file another DM exported. */
export function EffectLibrary() {
  const toast = useToast();
  const [items, setItems] = useState<MechDef[] | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const reload = () => { void listMechanics().then(setItems, (e: unknown) => setError(errorMessage(e))); };
  useEffect(reload, []);
  const doExport = async () => {
    try { setText(JSON.stringify(await exportMechanics(), null, 2)); } catch (e) { setError(errorMessage(e)); }
  };
  const doImport = async () => {
    let file: unknown;
    try { file = JSON.parse(text); } catch { setError("That isn't JSON."); return; }
    try { const r = await importMechanics(file); toast.push(`Imported ${r?.imported ?? 0} effect${r?.imported === 1 ? "" : "s"}.`); setText(""); reload(); } catch (e) { setError(errorMessage(e)); }
  };
  return (
    <div className="cb-form">
      <h4 className="cb-group-title">Effects you built</h4>
      {items === null ? <p className="muted">Loading…</p> : items.length === 0 ? <p className="muted cb-small">None yet.</p> : (
        <ul className="cb-spell-rows">{items.map((d) => <li key={`${d.key}:${d.ruleset}`} className="cb-spell-row"><span className="cb-spell-name">{d.def.name}</span><span className="cb-small muted">{d.ruleset} · L{d.def.level}{d.origin === "import" ? " · imported" : ""}</span></li>)}</ul>
      )}
      <div className="cb-tool-row">
        <Button small onClick={() => void doExport()}>Export as JSON</Button>
        <Button small disabled={!text.trim()} onClick={() => void doImport()}>Import this JSON</Button>
      </div>
      <input type="file" accept="application/json,.json" aria-label="Effects file" onChange={(e) => { const file = e.target.files?.[0]; if (file) void file.text().then(setText); }} />
      <textarea className="input cb-export" rows={8} value={text} onChange={(e) => setText(e.target.value)} placeholder="Exported effects JSON" aria-label="Effects JSON" />
      {error && <p className="cb-why" role="alert">{error}</p>}
    </div>
  );
}
