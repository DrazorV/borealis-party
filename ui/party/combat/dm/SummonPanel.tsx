import { useEffect, useState } from "react";
import { Button, IconButton } from "../../../src/components/Button";
import { Dialog } from "../../../src/components/controls/Dialog";
import { Segmented } from "../../../src/components/Tabs";
import { usePhone } from "../../../src/lib/phone";
import { errorMessage } from "../../../src/state/useAsync";
import type { MonsterSummary } from "../../api/bestiary";
import { isCharacter } from "./derive";
import { searchCreatures } from "./spellApi";
import type { SpellMech, SummonOption } from "./spellTypes";
import type { Combatant, CombatState } from "./types";

export const crText = (cr: number): string => (cr === 0.125 ? "1/8" : cr === 0.25 ? "1/4" : cr === 0.5 ? "1/2" : String(cr));

/** The summon options of a spell at a slot ("one CR 2, two CR 1, …" times the slot's multiplier). */
export function summonOptions(mech: SpellMech, slot: number | undefined): { options: SummonOption[]; type?: string; note?: string } | null {
  const p = [...(mech.always ?? []), ...(mech.on_land ?? [])].find((x) => x.kind === "summon_info");
  if (!p || p.kind !== "summon_info") return null;
  const at = slot ?? mech.level;
  let times = 1;
  for (const [level, n] of Object.entries(p.per_slot ?? {})) if (at >= Number(level)) times = Math.max(times, n);
  return { options: p.options.map((o) => ({ cr: o.cr, count: o.count * times })), type: p.creature_type, note: p.note };
}

export interface AddPreset { type?: string; crMax?: number; count?: number; owner?: string; bind?: boolean; spell?: string }

/**
 * After a summon or conjure spell (0.3.2, user decision: no spawning): an info panel with the spell's options and a
 * button per option that opens Add creature pre-filtered (type, CR limit, how many), controlled by the caster.
 */
export function SummonPanel({ mech, slot, caster, onAdd, onClose }: {
  mech: SpellMech; slot?: number; caster: Combatant; onAdd: (p: AddPreset) => void; onClose: () => void;
}) {
  const info = summonOptions(mech, slot);
  if (!info) return null;
  const type = info.type ?? "creature";
  return (
    <section className="cb-summon" role="status" aria-label={`${mech.name} options`}>
      <div className="cb-summon-head">
        <strong>{mech.name}: add the creatures</strong>
        <IconButton icon="x" label="Close" onClick={onClose} />
      </div>
      <p className="cb-small">Pick one option. Controlled by {caster.label}{mech.concentration ? `; they go when its concentration ends` : ""}.</p>
      <div className="cb-chips">
        {info.options.map((o) => (
          <Button key={`${o.cr}-${o.count}`} small onClick={() => onAdd({ type: info.type, crMax: o.cr, count: o.count, owner: caster.id, bind: mech.concentration, spell: mech.name })}>
            {o.count} × CR {crText(o.cr)} {type}{o.count === 1 ? "" : "s"}…
          </Button>
        ))}
      </div>
      {info.note && <p className="cb-small muted">{info.note}</p>}
    </section>
  );
}

type Tab = "bestiary" | "copy";

/**
 * Add creature (C7, and 0.3.2's summons): from the DM's bestiary, filtered by type and CR when a summon spell opened
 * it, with how many, who controls it, and "Remove when concentration ends"; or a copy of one already in the fight.
 */
export function AddCreatureSheet({ state, preset, onCopy, onBestiary, onClose }: {
  state: CombatState;
  preset?: AddPreset;
  onCopy: (r: { from: Combatant; label: string; initiative: string; hidden: boolean }) => void;
  onBestiary: (r: { elementId: string; count: number; owner?: string; bind: boolean; hidden: boolean; initiativeMode: "after_owner" | "own" }) => void;
  onClose: () => void;
}) {
  const phone = usePhone();
  const creatures = state.combatants.filter((c) => !isCharacter(c) && !c.removed);
  const [tab, setTab] = useState<Tab>(preset || creatures.length === 0 ? "bestiary" : "bestiary");
  const [text, setText] = useState("");
  const [crMax, setCrMax] = useState(preset?.crMax != null ? crText(preset.crMax) : "");
  const [type, setType] = useState(preset?.type ?? "");
  const [results, setResults] = useState<MonsterSummary[] | null>(null);
  const [error, setError] = useState("");
  const [pick, setPick] = useState<string>("");
  const [count, setCount] = useState(preset?.count ?? 1);
  const [owner, setOwner] = useState(preset?.owner ?? "");
  const ownerC = state.combatants.find((c) => c.id === owner);
  const [bind, setBind] = useState(preset?.bind ?? true);
  const [hidden, setHidden] = useState(!preset?.owner);
  const [mode, setMode] = useState<"after_owner" | "own">("after_owner");
  // Copy
  const [fromId, setFromId] = useState(creatures[0]?.id ?? "");
  const from = creatures.find((c) => c.id === fromId);
  const [label, setLabel] = useState("");
  const [init, setInit] = useState("");

  useEffect(() => {
    if (tab !== "bestiary") return;
    let live = true;
    const t = setTimeout(() => {
      searchCreatures({ text: text.trim() || undefined, crMax: crMax || undefined, types: type ? [type] : undefined, sort: "cr", limit: 50 })
        .then((p) => { if (live) { setResults(p?.items ?? []); setError(""); } })
        .catch((e: unknown) => { if (live) { setResults([]); setError(errorMessage(e)); } });
    }, 150);
    return () => { live = false; clearTimeout(t); };
  }, [tab, text, crMax, type]);

  const chosen = results?.find((r) => r.id === pick);
  const canBind = Boolean(ownerC?.concentration);
  return (
    <Dialog title={preset?.spell ? `Add creatures: ${preset.spell}` : "Add a creature"} onClose={onClose} full={phone} className="cb-sheet cb-side cb-add"
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        {tab === "bestiary"
          ? <Button variant="primary" disabled={!chosen} onClick={() => chosen && onBestiary({ elementId: chosen.id, count, owner: owner || undefined, bind: canBind && bind, hidden, initiativeMode: mode })}>
            Add {count > 1 ? `${count} ` : ""}{chosen ? chosen.name : ""}</Button>
          : <Button variant="primary" disabled={!from} onClick={() => from && onCopy({ from, label: label.trim(), initiative: init.trim(), hidden })}>Add</Button>}
      </>}>
      <div className="cb-form">
        <Segmented<Tab> label="Add from" value={tab} onChange={setTab} options={[{ id: "bestiary", label: "Your bestiary" }, { id: "copy", label: "A copy of one here" }]} />
        {tab === "bestiary" ? <>
          <div className="cb-frow">
            <label className="cb-field"><span>Search</span><input className="input" value={text} placeholder="Name" onChange={(e) => setText(e.target.value)} aria-label="Search your bestiary" /></label>
            <label className="cb-field cb-narrow"><span>Type</span><input className="input" value={type} placeholder="any" onChange={(e) => setType(e.target.value.toLowerCase())} aria-label="Creature type" /></label>
            <label className="cb-field cb-narrow"><span>CR up to</span><input className="input" value={crMax} placeholder="any" onChange={(e) => setCrMax(e.target.value)} aria-label="CR up to" /></label>
          </div>
          {error && <p className="cb-why">{error}</p>}
          <ul className="cb-pick-list" aria-label="Creatures">
            {(results ?? []).map((m) => (
              <li key={m.id}>
                <button type="button" className={`cb-pick${pick === m.id ? " on" : ""}`} aria-pressed={pick === m.id} onClick={() => setPick(m.id)}>
                  <b>{m.name}</b> <span className="muted cb-small">CR {m.cr ?? "—"} · {m.type ?? ""} · {m.hp ?? "?"} HP · AC {m.ac ?? "?"}</span>
                </button>
              </li>
            ))}
            {results && results.length === 0 && <li className="muted cb-small">Nothing in your content matches.</li>}
            {!results && <li className="muted cb-small">Searching…</li>}
          </ul>
          <div className="cb-frow">
            <label className="cb-field cb-stepper-field"><span>How many{preset?.count ? ` (the spell: ${preset.count})` : ""}</span>
              <span className="cb-stepper">
                <Button small aria-label="One fewer" disabled={count <= 1} onClick={() => setCount(count - 1)}>−</Button>
                <b aria-live="polite" aria-label={`${count} creatures`}>{count}</b>
                <Button small aria-label="One more" disabled={count >= 20} onClick={() => setCount(count + 1)}>+</Button>
              </span>
            </label>
            <label className="cb-field"><span>Controlled by</span>
              <select className="input" value={owner} onChange={(e) => setOwner(e.target.value)} aria-label="Controlled by">
                <option value="">The DM (on its own)</option>
                {state.combatants.filter((c) => !c.removed).map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
              </select>
            </label>
          </div>
          {owner && <>
            <label className="cb-check"><input type="checkbox" checked={canBind && bind} disabled={!canBind} onChange={(e) => setBind(e.target.checked)} />
              Remove when {ownerC?.concentration ? `${ownerC.label}'s concentration on ${ownerC.concentration.name}` : "its concentration"} ends{!canBind ? " (it isn't concentrating)" : ""}</label>
            <label className="cb-field"><span>Acts</span>
              <select className="input" value={mode} onChange={(e) => setMode(e.target.value as "after_owner" | "own")} aria-label="When it acts">
                <option value="after_owner">Right after {ownerC?.label ?? "its controller"}</option>
                <option value="own">On its own initiative</option>
              </select>
            </label>
          </>}
          <label className="cb-check"><input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} />Hidden from players</label>
        </> : <>
          <label className="cb-field"><span>A copy of</span>
            <select className="input" value={fromId} onChange={(e) => setFromId(e.target.value)}>
              {creatures.map((c) => <option key={c.id} value={c.id}>{c.label} ({c.hp.max} HP, AC {c.ac ?? "—"})</option>)}
            </select>
          </label>
          <label className="cb-field"><span>Name</span><input className="input" value={label} placeholder={from ? `${from.label.replace(/\s*\d+$/, "")} (new)` : ""} onChange={(e) => setLabel(e.target.value)} /></label>
          <label className="cb-field"><span>Initiative (blank: roll in app)</span><input className="input" inputMode="numeric" value={init} onChange={(e) => setInit(e.target.value)} /></label>
          <label className="cb-check"><input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} />Hidden from players</label>
          {creatures.length === 0 && <p className="muted">There is no creature to copy.</p>}
        </>}
      </div>
    </Dialog>
  );
}
