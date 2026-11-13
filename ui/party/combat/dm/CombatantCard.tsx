import { useState } from "react";
import { Button } from "../../../src/components/Button";
import { useConfirm } from "../../../src/components/Confirm";
import { usePhone } from "../../../src/lib/phone";
import { ConditionChips } from "./ConditionToggles";
import { HpBar } from "./HpBar";
import { RollInput } from "./RollInput";
import type { Condition } from "./common";
import { AbilityTabs } from "./AbilityTabs";
import { SummonDialog, TextDialog, type SummonChoice } from "./dialogs";
import { byId, conditionsOf, effectiveAc, effectiveInitiative, effectiveSpeed, isCharacter, isDefeated, linkActive, sameSpeeds, speedText } from "./derive";
import { EffectsList } from "./EffectsList";
import { HpControls } from "./HpControls";
import { initiativeText, quickRef, saveBonus } from "./statRef";
import type { Combatant, Command, CombatState, CompanionOffer, StatBlock } from "./types";

const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);
/** A new combatant's id (the server may give it a random UUID of its own). */
const newId = () => (globalThis.crypto?.randomUUID?.() ?? `s-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`);

/**
 * The selected combatant (C6–C19): title with reveal/hide, AC and speed with effect-derived values, the quick
 * reference from its stat block, HP, concentration with the pending CON save, conditions, effects and its abilities.
 */
export function CombatantCard({ state, c, block, blockName, offers, send, disabled }: {
  state: CombatState; c: Combatant; block: StatBlock | undefined; blockName?: string; offers?: CompanionOffer[]; send: (cmd: Command) => void; disabled: boolean;
}) {
  const confirm = useConfirm();
  const phone = usePhone();
  const [dialog, setDialog] = useState<"rename" | "public" | "conc" | "summon" | null>(null);
  const pc = isCharacter(c);
  const ac = effectiveAc(state, c);
  const speed = effectiveSpeed(state, c);
  const base = c.speed ?? { walk: 30 };
  const conds = conditionsOf(state, c.id);
  const ref = quickRef(block);
  const current = state.current === c.id;
  const dead = isDefeated(c);
  const leader = byId(state, c.linked_to);
  const followers = state.combatants.filter((f) => f.linked_to === c.id && !f.removed);
  const toggleCondition = (k: Condition, on: boolean) => {
    if (on) send({ command: "add_condition", effect: { name: k.charAt(0).toUpperCase() + k.slice(1), target: c.id, conditions: [k] } });
    else send({ command: "remove_condition", target: c.id, condition: k });
  };
  const hide = async () => {
    if (c.ever_revealed) {
      const r = await confirm({ title: `Hide ${c.label}?`, body: "Players who are online have already seen it. It drops out of their turn order again; what they saw in the log stays.", confirmLabel: "Hide" });
      if (r !== "confirm") return;
    }
    send({ command: "hide", id: c.id });
  };
  const remove = async () => {
    const r = await confirm({ title: `Remove ${c.label}?`, body: "It leaves the fight with its effects and initiative. Undo brings it back.", confirmLabel: "Remove", danger: true });
    if (r === "confirm") send({ command: "remove_combatant", id: c.id });
  };
  const life = c.life ?? { state: "alive" };
  const owner = byId(state, c.owner_id);
  const pets = state.combatants.filter((p) => p.owner_id === c.id && !p.removed);
  const playerPet = c.controller?.kind === "player";
  const dismiss = async () => {
    const r = await confirm({ title: `Dismiss ${c.label}?`, body: "It leaves the fight (a summon ends, a pet goes away). Undo brings it back.", confirmLabel: "Dismiss" });
    if (r === "confirm") send({ command: "dismiss", id: c.id });
  };
  const summon = (r: SummonChoice) => {
    const initiative = r.initiative === "" ? {} : { initiative: r.initiative === "roll" ? { kind: "app" as const, modifier: r.initBonus } : { kind: "typed" as const, total: r.initiative } };
    // The server builds sheet companions and copies itself, with their stat blocks; a quick block is sent whole.
    if (r.companion) { send({ command: "summon", owner: c.id, companion: r.companion, visible: r.visible, ...initiative }); return; }
    if (r.copyOf) { send({ command: "summon", owner: c.id, copyOf: r.copyOf, visible: r.visible, ...initiative }); return; }
    const base: Combatant = {
      id: "", kind: "creature", label: r.name, public_name: r.name, visible: r.visible, hp: { current: r.hp, max: r.hp, temp: 0 },
      ac: r.ac, initiative_bonus: r.initBonus, speed: { walk: r.walk }, zero_hp: "dies", life: { state: "alive" },
    };
    const made: Combatant = {
      ...base, id: newId(), label: r.name || base.label, public_name: r.name || base.public_name, initiative: null, removed: false, absent: false,
      linked_to: undefined, owner_id: undefined, concentration: undefined, pending_concentration: [],
      hp: { current: r.hp, max: r.hp, temp: 0 }, life: { state: "alive" },
    };
    send({ command: "summon", owner: c.id, combatant: made, visible: r.visible, ...initiative });
  };
  return (
    <section className="cb-panel cb-card" aria-label={`${c.label} details`} id="cb-card">
      <div className="cb-card-lead">
        <div className="cb-title">
          <div>
            <div className="cb-overline">{pc ? "Player character" : blockName ?? "Creature"}{!pc && c.public_name !== c.label ? ` · players see "${c.public_name}"` : ""}</div>
            <h2>{c.label}{c.concentration && <span className="cb-conc">● CONC</span>}</h2>
          </div>
          <span className={`chip ${dead ? "danger" : current ? "gold" : ""}`}>{dead ? "DEFEATED" : current ? "CURRENT TURN" : life.state === "dying" ? "DYING" : life.state === "stable" ? "STABLE" : pc ? "PLAYER" : "READY"}</span>
        </div>
        <div className="cb-card-actions">
          {!pc && !playerPet && (c.visible
            ? <Button small onClick={() => void hide()} disabled={disabled}>Hide from players</Button>
            : <Button small variant="primary" onClick={() => send({ command: "reveal", id: c.id })} disabled={disabled || dead}>Reveal to players</Button>)}
          <Button small variant="ghost" onClick={() => setDialog("rename")} disabled={disabled}>Rename</Button>
          {!pc && <Button small variant="ghost" onClick={() => setDialog("public")} disabled={disabled}>Public name</Button>}
          {pc && <Button small variant="ghost" disabled={disabled} onClick={() => send({ command: "set_absent", id: c.id, absent: !c.absent })}>{c.absent ? "In this fight" : "Not in this fight"}</Button>}
          <Button small variant="ghost" onClick={() => setDialog("summon")} disabled={disabled || dead}>Summon…</Button>
          {!pc && (c.owner_id
            ? <Button small variant="ghost" onClick={() => void dismiss()} disabled={disabled}>Dismiss</Button>
            : <Button small variant="ghost" onClick={() => void remove()} disabled={disabled}>Remove</Button>)}
        </div>
        <div className="cb-statline">
          <span title={ac.total !== ac.base ? `Base ${ac.base}${ac.effects ? `, effects ${signed(ac.effects)}` : ""}${ac.cover ? `, cover +${ac.cover}` : ""}` : undefined}>
            <b>AC {ac.total}</b>{ac.total !== ac.base && <span className="muted"> (base {ac.base})</span>}
          </span>
          <span><b>{c.hp.current}/{c.hp.max} HP</b>{c.hp.temp ? ` +${c.hp.temp}` : ""}</span>
          <span>Init {effectiveInitiative(state, c) ?? "–"}{!pc && <span className="muted"> ({initiativeText(block, c.initiative_bonus)})</span>}</span>
          <span className={`cb-speed${sameSpeeds(speed, base) ? "" : " changed"}`} title={sameSpeeds(speed, base) ? undefined : `Base ${speedText(base)}`}>Speed {speedText(speed)}</span>
        </div>
        {owner && (
          <div className="cb-owner">
            <span className="cb-small">{playerPet ? `${owner.label}'s (the player controls it)` : `Controlled by the DM for ${owner.label}`}</span>
            <label className="cb-field cb-link-pick">
              <span className="cb-small muted">Initiative</span>
              <select className="input" value={c.initiative_mode ?? "after_owner"} disabled={disabled}
                onChange={(e) => send({ command: "set_initiative_mode", id: c.id, mode: e.target.value as "after_owner" | "own" })}>
                <option value="after_owner">Right after {owner.label}</option>
                <option value="own">Its own{c.initiative != null ? ` (${c.initiative})` : " (roll in the app)"}</option>
              </select>
            </label>
          </div>
        )}
        {pets.length > 0 && <p className="cb-small muted">Controls {pets.map((p) => p.label).join(", ")}.</p>}
        {leader && <p className="cb-small muted">{linkActive(state, c) ? `Acts right after ${leader.label}, at its initiative.` : `Linked to ${leader.label} (down: acts on its own).`}{" "}
          <button type="button" className="cb-link" disabled={disabled} onClick={() => send({ command: "link", id: c.id })}>Unlink</button></p>}
        {followers.length > 0 && <p className="cb-small muted">{followers.map((f) => f.label).join(", ")} act{followers.length === 1 ? "s" : ""} right after {c.label}.</p>}
        {!pc && !leader && !owner && (
          <label className="cb-field cb-link-pick">
            <span className="cb-small muted">Acts right after (a mount after its rider)</span>
            <select className="input" value="" disabled={disabled || dead} onChange={(e) => { if (e.target.value) send({ command: "link", id: c.id, to: e.target.value }); }}>
              <option value="">On its own initiative</option>
              {state.combatants.filter((o) => o.id !== c.id && !o.removed && o.linked_to !== c.id).map((o) => <option key={o.id} value={o.id}>{o.label}</option>)}
            </select>
          </label>
        )}
        {block && (
          <div className="cb-quickref" aria-label="Quick reference">
            <div className="cb-qr-grid">
              <div><div className="cb-overline">Attack bonus</div><b>{ref.attacks.join(" · ") || "—"}</b></div>
              <div><div className="cb-overline">Spell attack</div><b>{ref.spellAttack.join(" · ") || "—"}</b></div>
              <div><div className="cb-overline">Save DCs</div><b>{ref.dcs.join(" · ") || "—"}</b></div>
            </div>
            {ref.saves.length > 0 && (
              <div className="cb-saves"><span className="muted">Saves</span>{ref.saves.map((s) => <span key={s.ability} className={s.proficient ? "prof" : ""}>{s.ability.toUpperCase()} <b>{signed(s.bonus)}</b></span>)}</div>
            )}
          </div>
        )}
        <HpBar current={c.hp.current} max={c.hp.max} temp={c.hp.temp} label={`${c.label} hit points`} />
        <HpControls c={c} send={send} disabled={disabled} />
        {life.state === "dying" && (
          <div className="cb-death">
            <span>Death saves: {life.successes} ✓ · {life.failures} ✗</span>
            {!pc && <RollInput kind="d20" label="Death save" modifier={0} disabled={disabled} onSubmit={(roll) => send({ command: "death_save", id: c.id, roll })} />}
            <Button small disabled={disabled} onClick={() => send({ command: "stabilize", id: c.id })}>Stabilize</Button>
            {pc && <span className="cb-small muted">The player rolls their own death saves.</span>}
          </div>
        )}
        <div className="cb-conc-row">
          <span className="muted cb-small">Concentration:</span>
          {c.concentration
            ? <><b className="cb-small">{c.concentration.name}</b>
              <Button small variant="ghost" disabled={disabled} onClick={() => setDialog("conc")}>Change</Button>
              <Button small variant="danger" disabled={disabled} onClick={() => send({ command: "end_concentration", id: c.id })}>End</Button></>
            : <><span className="muted cb-small">None</span><Button small variant="ghost" disabled={disabled || dead} onClick={() => setDialog("conc")}>Set</Button></>}
        </div>
        {(c.pending_concentration ?? []).map((check, i) => (
          <div key={i} className="cb-conc-check" role="alert">
            <b>Concentration check: CON save DC {check.dc}</b> <span className="muted cb-small">({check.damage} damage)</span>
            {i === 0 && !pc && <RollInput kind="d20" label="CON save" modifier={saveBonus(block, "con")} disabled={disabled} onSubmit={(roll) => send({ command: "resolve_concentration", id: c.id, roll })} />}
            {i === 0 && pc && <span className="cb-small muted"> The player rolls it.</span>}
          </div>
        ))}
        <ConditionChips active={conds} onToggle={toggleCondition} disabled={disabled} immune={c.defenses?.condition_immunities} compact={phone} />
        {(c.legendary_resistance) && (
          <div className="cb-lr">
            <span>Legendary Resistance <b>{c.legendary_resistance.remaining}/{c.legendary_resistance.max}</b></span>
            <Button small variant="danger" disabled={disabled || c.legendary_resistance.remaining <= 0 || dead} onClick={() => send({ command: "adjust_legendary_resistance", id: c.id, delta: -1 })}>Use</Button>
            <Button small disabled={disabled || c.legendary_resistance.remaining >= c.legendary_resistance.max} onClick={() => send({ command: "adjust_legendary_resistance", id: c.id, delta: 1 })}>Restore</Button>
          </div>
        )}
        <EffectsList state={state} c={c} send={send} disabled={disabled} />
      </div>
      {pc
        ? <div className="cb-tab-body"><p className="cb-small muted">The player's character: HP, AC and speed come from their Play tab. Effects, conditions and concentration are tracked here.</p>
          {c.initiative == null && <RollInput kind="d20" label="Initiative (if the player can't)" modifier={c.initiative_bonus ?? 0} disabled={disabled} onSubmit={(roll) => send({ command: "set_initiative", id: c.id, roll })} />}</div>
        : <AbilityTabs state={state} c={c} block={block} send={send} disabled={disabled} />}
      {dialog === "summon" && <SummonDialog owner={c} creatures={state.combatants.filter((x) => !isCharacter(x) && !x.removed && x.hp.max > 0)}
        offers={(offers ?? []).filter((o) => o.pc === c.id)}
        onDone={(r) => { setDialog(null); if (r) summon(r); }} />}
      {dialog === "rename" && <TextDialog title={`Rename ${c.label}`} label="Your label (players don't see it)" initial={c.label} confirm="Rename"
        onDone={(v) => { setDialog(null); if (v && v !== c.label) send({ command: "rename", id: c.id, label: v }); }} />}
      {dialog === "public" && <TextDialog title="Public name" label="What players see once it's revealed" initial={c.public_name} confirm="Save"
        onDone={(v) => { setDialog(null); if (v && v !== c.public_name) send({ command: "set_public_name", id: c.id, name: v }); }} />}
      {dialog === "conc" && <TextDialog title={`Concentration: ${c.label}`} label="Concentrating on (blank ends it)" initial={c.concentration?.name ?? ""} allowEmpty confirm="Save"
        note="A new concentration ends the old one, and every effect it held."
        onDone={(v) => {
          setDialog(null);
          if (v === null) return;
          if (v === "") { if (c.concentration) send({ command: "end_concentration", id: c.id }); }
          else if (v !== c.concentration?.name) send({ command: "set_concentration", id: c.id, name: v });
        }} />}
    </section>
  );
}
