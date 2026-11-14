import { useState } from "react";
import { Button, IconButton } from "../../../src/components/Button";
import { Dialog } from "../../../src/components/controls/Dialog";
import { conditionWord } from "./common";
import { RollInput } from "./RollInput";
import { byId, durationText, effectsOn, isDefeated } from "./derive";
import { EffectDialog, draftSpecs, emptyDraft } from "./dialogs";
import { expectCast } from "./castFocus";
import { asCommand, asSpellEffect, type SpellEffect } from "./spellTypes";
import type { Combatant, Command, CombatState, Effect, Modifier } from "./types";

const modText = (m: Modifier): string => {
  const x = m as unknown as { kind: string; base?: number; add_dex?: boolean; value?: number };
  if (x.kind === "ac_base") return `AC ${x.base}${x.add_dex ? " + DEX" : ""}`;
  if (x.kind === "ac_min") return `AC at least ${x.value}`;
  switch (m.kind) {
    case "ac": return `AC ${m.value >= 0 ? "+" : ""}${m.value}`;
    case "speed_add": return `speed ${m.value >= 0 ? "+" : ""}${m.value} ft`;
    case "speed_multiply": return `speed ×${m.value}`;
    case "speed_halve": return "speed halved";
    case "speed_zero": return "speed 0";
    case "cover": return m.value === "half" ? "half cover" : "three-quarters cover";
  }
};

/**
 * Active effects on a combatant (C12): name, source, duration in the source's turns, charges, concentration, and
 * what follows when it ends. × ends it; ending a concentration ends every effect it holds (the server cascades).
 */
export function EffectsList({ state, c, send, disabled }: { state: CombatState; c: Combatant; send: (cmd: Command) => void; disabled: boolean }) {
  const [adding, setAdding] = useState(false);
  const [open, setOpen] = useState<number | null>(null);
  const list = effectsOn(state, c.id);
  return (
    <div className="cb-effects">
      <div className="cb-effects-head">
        <span>✦ Active effects {list.length > 0 && <span className="chip info">{list.length}</span>}</span>
        <Button small icon="plus" disabled={disabled} onClick={() => setAdding(true)}>Effect</Button>
      </div>
      {list.length === 0 ? <p className="muted cb-small">No effects.</p> : (
        <ul className="cb-fx-list">
          {list.map((e) => <EffectRow key={e.id} e={e} state={state} send={send} disabled={disabled} open={open === e.id} toggle={() => setOpen(open === e.id ? null : e.id)} />)}
        </ul>
      )}
      {adding && (
        <EffectDialog title={`Effect on ${c.label}`} combatants={state.combatants} initial={emptyDraft("", c.id)}
          onDone={(r) => {
            setAdding(false);
            if (!r) return;
            for (const effect of draftSpecs(r.draft)) send({ command: "add_effect", effect });
          }} />
      )}
    </div>
  );
}

function EffectRow({ e, state, send, disabled, open, toggle }: { e: Effect; state: CombatState; send: (cmd: Command) => void; disabled: boolean; open: boolean; toggle: () => void }) {
  const [saving, setSaving] = useState(false);
  const [zoning, setZoning] = useState(false);
  const fx = asSpellEffect(e);
  const source = byId(state, e.source);
  const bits = [
    e.charges != null ? `${e.charges} charge${e.charges === 1 ? "" : "s"}` : null,
    e.concentration ? "CONC" : null,
    durationText(state, e),
    source ? `from ${source.label}` : null,
    ...(e.conditions ?? []).map(conditionWord),
    ...(e.modifiers ?? []).map(modText),
    e.follow_up ? `then ${e.follow_up.name}` : null,
  ].filter(Boolean);
  // The whole spell: its source's concentration if it holds it, else every effect of that cast.
  const dismissSpell = () => {
    if (source && fx.spell && source.concentration?.name === fx.spell.name) { send({ command: "end_concentration", id: source.id }); return; }
    for (const o of state.effects.map(asSpellEffect).filter((o) => o.spell?.key === fx.spell?.key && o.source === e.source)) send({ command: "remove_effect", effect: o.id });
  };
  return (
    <li className={`cb-fx${e.concentration ? " conc" : ""}${e.charges != null ? " ward" : ""}`} data-effect={e.id}>
      <div className="cb-fx-line">
        <span className="cb-fx-name">{e.name}</span>
        <span className="cb-fx-meta">{bits.join(" · ")}</span>
        {fx.dismiss_on_end && <span className="chip info cb-small">Bound to {e.name}</span>}
        {(e.detail || e.repeat_save) && <IconButton icon="info" label={`${e.name}: rules`} onClick={toggle} aria-expanded={open} />}
      </div>
      <div className="cb-fx-actions">
        <Button small disabled={disabled} onClick={() => send({ command: "remove_effect", effect: e.id })} aria-label={`End ${e.name}`}>End</Button>
        {e.charges != null && <Button small disabled={disabled || e.charges <= 0} onClick={() => send({ command: "use_charge", effect: e.id })}>Use 1</Button>}
        {e.repeat_save && <Button small aria-expanded={saving} disabled={disabled} onClick={() => setSaving(!saving)}>Repeat save</Button>}
        {fx.zone && <Button small disabled={disabled} onClick={() => setZoning(true)}>Apply zone to…</Button>}
        {fx.spell && !fx.dismiss_on_end && <Button small variant="ghost" disabled={disabled} onClick={dismissSpell}>Dismiss spell</Button>}
        {fx.ends_on?.includes("attack") && <Button small variant="warning" disabled={disabled} onClick={() => send({ command: "remove_effect", effect: e.id })}>It attacked: end</Button>}
      </div>
      {open && e.detail && <p className="cb-small cb-fx-detail">{e.detail}</p>}
      {e.repeat_save && saving && (
        <RollInput kind="d20" label={`${e.repeat_save.ability.toUpperCase()} save DC ${e.repeat_save.dc}`} disabled={disabled}
          onSubmit={(roll) => { setSaving(false); send({ command: "resolve_effect_save", effect: e.id, roll }); }} />
      )}
      {zoning && fx.zone && <ZoneDialog state={state} e={fx} onDone={(targets) => { setZoning(false); if (targets?.length) { expectCast(e.source ?? e.target); send(asCommand({ command: "apply_zone", effect: e.id, targets })); } }} />}
    </li>
  );
}

/** "Apply zone to…": who is in the area this time (the DM taps; nothing is measured). */
function ZoneDialog({ state, e, onDone }: { state: CombatState; e: SpellEffect; onDone: (targets: string[] | null) => void }) {
  const [targets, setTargets] = useState<string[]>([]);
  const z = e.zone!;
  const live = state.combatants.filter((c) => !c.removed && !isDefeated(c) && c.id !== e.source);
  return (
    <Dialog title={`${e.name}: who is in it?`} onClose={() => onDone(null)}
      footer={<><Button onClick={() => onDone(null)}>Cancel</Button><Button variant="primary" disabled={!targets.length} onClick={() => onDone(targets)}>Ask for saves</Button></>}>
      <p className="cb-small muted">DC {z.dc} {z.ability.toUpperCase()} save{z.damage ? ` · ${z.damage.dice}${z.damage.type ? ` ${z.damage.type}` : ""}` : ""}{z.half ? ", half on a success" : ""}.</p>
      <div className="cb-chips" role="group" aria-label="In the area">
        {live.map((c) => {
          const on = targets.includes(c.id);
          return <button key={c.id} type="button" aria-pressed={on} className={`cb-chip${on ? " on" : ""}`} onClick={() => setTargets(on ? targets.filter((t) => t !== c.id) : [...targets, c.id])}>{c.label}</button>;
        })}
      </div>
    </Dialog>
  );
}
