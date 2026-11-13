import { useState } from "react";
import { Button, IconButton } from "../../../src/components/Button";
import { conditionWord } from "./common";
import { RollInput } from "./RollInput";
import { byId, durationText, effectsOn } from "./derive";
import { EffectDialog, draftSpecs, emptyDraft } from "./dialogs";
import type { Combatant, Command, CombatState, Effect, Modifier } from "./types";

const modText = (m: Modifier): string => {
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
  return (
    <li className={`cb-fx${e.concentration ? " conc" : ""}${e.charges != null ? " ward" : ""}`}>
      <div className="cb-fx-line">
        <span className="cb-fx-name">{e.name}</span>
        <span className="cb-fx-meta">{bits.join(" · ")}</span>
        {e.charges != null && <Button small disabled={disabled || e.charges <= 0} onClick={() => send({ command: "use_charge", effect: e.id })}>Use 1</Button>}
        {(e.detail || e.repeat_save) && <IconButton icon="info" label={`${e.name}: rules`} onClick={toggle} aria-expanded={open} />}
        <IconButton icon="x" label={`End ${e.name}`} disabled={disabled} onClick={() => send({ command: "remove_effect", effect: e.id })} />
      </div>
      {open && e.detail && <p className="cb-small cb-fx-detail">{e.detail}</p>}
      {e.repeat_save && (
        <RollInput kind="d20" label={`${e.repeat_save.ability.toUpperCase()} save DC ${e.repeat_save.dc}`} disabled={disabled}
          onSubmit={(roll) => send({ command: "resolve_effect_save", effect: e.id, roll })} />
      )}
    </li>
  );
}
