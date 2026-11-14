import "./spells.css";
import { useMemo, useState } from "react";
import { Button } from "../../../src/components/Button";
import { Dialog } from "../../../src/components/controls/Dialog";
import { usePhone } from "../../../src/lib/phone";
import { expectCast } from "./castFocus";
import type { DamageType } from "./common";
import { byId, isCharacter, isDefeated } from "./derive";
import { evalCount, preview, varsFor } from "./formula";
import { asCommand, castingOf, COVERAGE_HELP, COVERAGE_WORD, mechOf, spellOf, type SpellMech, type TargetFilter } from "./spellTypes";
import { castReason, slotsFrom, statblockSpec, type TurnButton } from "./turn";
import { featureFor } from "./statRef";
import type { Combatant, Command, CombatState, StatBlock, Targeting } from "./types";

/** A creature type out of a stat block ("humanoid"), when it prints one. */
export function creatureType(block: StatBlock | undefined): string | null {
  const t = block?.creature_type as unknown;
  if (typeof t === "string") return t.toLowerCase();
  if (t && typeof t === "object" && "type" in t && typeof (t as { type: unknown }).type === "string") return (t as { type: string }).type.toLowerCase();
  return null;
}

function filterReason(filter: TargetFilter | undefined, c: Combatant, block: StatBlock | undefined): string | null {
  if (!filter || filter.kind !== "type") return null;
  const t = c.stats?.creature_type?.toLowerCase() ?? creatureType(block);
  if (t === null) return null; // Unknown: the DM decides.
  return filter.types.includes(t) ? null : `not a ${filter.types.join(" or ")}`;
}

const tierFor = (level: number | undefined) => (level == null ? 1 : level >= 17 ? 4 : level >= 11 ? 3 : level >= 5 ? 2 : 1);

/** What the sheet did, so the panel can follow up (the summon panel). */
export interface CastDone { sent: boolean; mech?: SpellMech; slot?: number; caster: string }

/**
 * The cast sheet (0.3.2, design 4): one app dialog, full screen on a phone and a side sheet on a desktop. Slot chips
 * show only the levels it may use, with a live preview ("9d6 fire at L4"); target chips are filtered by the spell
 * and capped at its count; then the choices and [Cast]. Statblock attacks and saves ("Scimitar", "Fire Breath") and
 * Help use the same sheet for their targets. Nothing is rolled here: the resolution sheet asks for table rolls next.
 */
export function CastSheet({ state, caster, block, button, blockOf, send, onDone }: {
  state: CombatState;
  caster: Combatant;
  /** The caster's stat block (a statblock action without a definition is compiled from it). */
  block?: StatBlock;
  button: TurnButton;
  blockOf: (id: string) => StatBlock | undefined;
  send: (cmd: Command) => void;
  onDone: (r: CastDone) => void;
}) {
  const phone = usePhone();
  const a = button.ability;
  const mech = a ? mechOf(a) : undefined;
  const link = a ? spellOf(a) : undefined;
  const level = mech?.level ?? link?.level ?? 0;
  const spellMode = button.kind === "spell";
  const legal = spellMode && level > 0 && (!link?.frequency || link.frequency === "slot") && !a?.uses ? slotsFrom(caster, level) : [];
  const [slot, setSlot] = useState<number | undefined>(legal[0]);
  const spec = a?.mech;
  const [tier, setTier] = useState(spec?.cantrip_tier ?? castingOf(caster)[0]?.cantrip_tier ?? tierFor(castingOf(caster)[0]?.caster_level));
  const dmgChoice = mech?.on_land?.find((p) => p.kind === "damage" && p.choice?.length);
  const [damageType, setDamageType] = useState<DamageType | undefined>(dmgChoice && dmgChoice.kind === "damage" ? dmgChoice.choice![0] : undefined);
  const targeting: Targeting = mech?.targeting ?? (button.kind === "save" ? { kind: "area", shape: "cone", size_ft: 0 } : { kind: "creatures", max: "1" });
  const selfOnly = targeting.kind === "self";
  const max = targeting.kind === "creatures" ? evalCount(targeting.max, mech ? varsFor(mech, slot, tier, spec?.mod ?? 0) : {}) : null;
  const [targets, setTargets] = useState<string[]>(selfOnly ? [caster.id] : []);
  const filter = targeting.kind === "creatures" ? targeting.filter : undefined;
  const live = useMemo(() => state.combatants.filter((c) => !c.removed && !isDefeated(c) && !c.absent), [state.combatants]);
  const others = button.standard === "help" || filter?.kind === "willing" || filter?.kind === "ally" ? live : live.filter((c) => c.id !== caster.id || button.kind === "spell");
  const capped = max != null && targets.length >= max;
  const toggle = (id: string) => setTargets((t) => (t.includes(id) ? t.filter((x) => x !== id) : capped ? t : [...t, id]));
  const why = a && spellMode ? castReason(state, caster, a) : button.reason;
  const concOld = mech?.concentration && caster.concentration ? caster.concentration.name : null;
  const needTargets = !selfOnly && (spellMode ? Boolean(mech) : true);
  const ok = !why && (!needTargets || targets.length > 0);
  const title = button.standard === "help" ? `${caster.label}: Help` : spellMode ? `Cast ${a?.name ?? "a spell"}` : `${caster.label}: ${a?.name ?? button.label}`;
  const coverage = button.coverage;
  const verb = spellMode ? "Cast" : button.standard === "help" ? "Help" : button.kind === "attack" ? "Attack" : "Use";
  const go = () => {
    if (button.standard !== "help") expectCast(caster.id);
    if (button.standard === "help") {
      send(asCommand({ command: "standard_action", id: caster.id, action: "help", target: targets[0] }));
    } else if (spellMode) {
      send(asCommand({
        command: "cast_spell", caster: caster.id, ...(a ? { ability: a.key } : {}), ...(slot !== undefined ? { slot } : {}),
        targets: selfOnly ? [caster.id] : targets, ...(damageType ? { choices: { damage_type: damageType } } : {}),
      }));
    } else if (a) {
      const own = a.mech ? undefined : statblockSpec(state.edition, a, featureFor(block, a)) ?? undefined;
      // An opportunity attack spends the reaction (`as_reaction`, asked of the engine).
      send({ command: "perform_statblock", id: caster.id, ability: a.key, targets, ...(own ? { spec: own } : {}), ...(button.key.startsWith("oa-") ? { as_reaction: true } : {}) } as Command);
    }
    onDone({ sent: true, mech, slot, caster: caster.id });
  };
  const cantrip = mech && level === 0 && JSON.stringify(mech.on_land ?? []).includes("cantrip_tier");
  return (
    <Dialog title={title} onClose={() => onDone({ sent: false, caster: caster.id })} full={phone} className="cb-sheet cb-side"
      footer={<>
        <Button onClick={() => onDone({ sent: false, caster: caster.id })}>Cancel</Button>
        <Button variant="primary" disabled={!ok} title={why ?? undefined} onClick={go}>{verb}</Button>
      </>}>
      <div className="cb-cast">
        {spellMode && (
          <p className="cb-cast-meta">
            {level === 0 ? "Cantrip" : `Level ${level}`}{mech?.concentration ? " · Concentration" : ""}
            {coverage && <span className={`cb-cov cb-cov-${coverage}`} title={COVERAGE_HELP[coverage]}>{COVERAGE_WORD[coverage]}</span>}
          </p>
        )}
        {!spellMode && button.detail && <p className="cb-cast-meta">{button.detail}</p>}
        {why && <p className="cb-why" role="alert">{why}</p>}
        {legal.length > 0 && (
          <section aria-label="Spell slot">
            <h4 className="cb-group-title">Slot</h4>
            <div className="cb-chips" role="radiogroup" aria-label="Spell slot">
              {legal.map((l) => (
                <button key={l} type="button" role="radio" aria-checked={slot === l} className={`cb-chip${slot === l ? " on" : ""}`} onClick={() => setSlot(l)}>
                  L{l} <span className="muted">· {caster.spell_slots?.[String(l)]?.remaining ?? 0} left</span>
                </button>
              ))}
            </div>
          </section>
        )}
        {cantrip && (
          <section aria-label="Cantrip damage">
            <h4 className="cb-group-title">Cantrip dice</h4>
            <div className="cb-chips" role="radiogroup" aria-label="Cantrip dice">
              {[1, 2, 3, 4].map((t) => <button key={t} type="button" role="radio" aria-checked={tier === t} className={`cb-chip${tier === t ? " on" : ""}`} onClick={() => setTier(t)}>{t}×</button>)}
            </div>
          </section>
        )}
        {mech && <p className="cb-preview" aria-live="polite" data-preview>{preview(mech, legal.length ? slot : undefined, tier, spec?.mod ?? 0)}</p>}
        {!mech && spellMode && <p className="cb-small muted">{coverage ? COVERAGE_HELP[coverage] : "No mechanics yet."} The slot and the action are spent; you rule what happens.</p>}
        {selfOnly ? <p className="cb-small muted">On {caster.label} (self).</p> : (
          <section aria-label="Targets">
            <h4 className="cb-group-title">
              {targeting.kind === "area" ? "Who is in the area?" : "Targets"}
              {max != null && <span className="muted"> · {targets.length}/{max}</span>}
            </h4>
            <div className="cb-chips" role="group" aria-label="Targets">
              {others.map((c) => {
                const no = filterReason(filter, c, blockOf(c.id));
                const on = targets.includes(c.id);
                return (
                  <button key={c.id} type="button" aria-pressed={on} disabled={Boolean(no) || (!on && capped)} title={no ?? undefined}
                    className={`cb-chip${on ? " on" : ""}${isCharacter(c) ? " pc" : ""}`} onClick={() => toggle(c.id)}>
                    {c.label}{no ? <span className="muted"> · {no}</span> : null}
                  </button>
                );
              })}
            </div>
            {targeting.kind === "area" && <p className="cb-small muted">Tap who is in the {targeting.size_ft ? `${targeting.size_ft}-ft ` : ""}{targeting.shape}; the app doesn't measure.</p>}
          </section>
        )}
        {dmgChoice && dmgChoice.kind === "damage" && (
          <section aria-label="Damage type">
            <h4 className="cb-group-title">Damage type</h4>
            <div className="cb-chips" role="radiogroup" aria-label="Damage type">
              {dmgChoice.choice!.map((t) => <button key={t} type="button" role="radio" aria-checked={damageType === t} className={`cb-chip${damageType === t ? " on" : ""}`} onClick={() => setDamageType(t)}>{t}</button>)}
            </div>
          </section>
        )}
        {concOld && <p className="cb-warn" role="note">{caster.label} stops concentrating on {concOld}, and what it holds ends.</p>}
        {(mech?.notes ?? []).length > 0 && <ul className="cb-notes">{mech!.notes!.map((n) => <li key={n}>{n}</li>)}</ul>}
        {a?.text && <details className="cb-small"><summary>Rules text</summary><p className="cb-ability-text">{a.text}</p></details>}
        {targets.some((t) => isCharacter(byId(state, t)!)) && mech?.delivery.kind === "save" && (
          <p className="cb-small muted">Players roll their own saves at the table; you or they type the result next.</p>
        )}
      </div>
    </Dialog>
  );
}
