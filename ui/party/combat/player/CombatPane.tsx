import { useEffect, useState } from "react";
import type { CharacterSnapshot } from "../../../src/api/contract";
import { Button } from "../../../src/components/Button";
import { Alert, Chip } from "../../../src/components/Card";
import { Modal } from "../../../src/components/Modal";
import { useConnectivity } from "../../api/connectivity";
import {
  actingId, deathSave, myCombatant, resolveConcentration, resolvePrompt, setInitiative,
  type CombatPrompt, type D20Input, type PlayerCombat, type PlayerCombatant,
} from "./api";
import { usePlayMutate } from "../../../src/pages/play/playQueue";
import { ConditionChips } from "../shared/ConditionChips";
import { HealthBand } from "../shared/HealthBand";
import { RollOrType } from "../shared/RollOrType";
import { StatBlockView } from "../shared/StatBlockView";
import { adopt, contextFor, deviceDeps, useCombatStore } from "./store";
import "../shared/combat.css";

/** How many revealed events the log shows. */
export const LOG_LINES = 8;

const errorText = (e: unknown) => (e as { message?: string } | null)?.message ?? "That didn't go through.";

/** Who is acting, in words. */
function nowText(c: PlayerCombat, me: PlayerCombatant | null): string {
  switch (c.pointer.kind) {
    case "combatant": {
      const id = c.pointer.id;
      if (me && id === me.id) return "Your turn";
      return c.combatants.find((x) => x.id === id)?.name ?? "Someone";
    }
    case "enemy_turn": return "Enemy turn";
    case "waiting": return "Waiting for the DM";
    case "not_started": return "Rolling initiative";
    default: return "Combat over";
  }
}

function Offline() {
  const { offline, offlineSince } = useCombatStore();
  const link = useConnectivity().link;
  const away = offline || link === "down" || link === "checking";
  const [seen] = useState(() => deviceDeps.now());
  const since = offlineSince ?? (away ? seen : null);
  const [, tick] = useState(0);
  const stale = since != null && deviceDeps.now() - since >= deviceDeps.staleMs;
  // Say "may be old" once a minute has gone by with no answer (D44).
  useEffect(() => {
    if (!away || stale || since == null) return undefined;
    const timer = setTimeout(() => tick((n) => n + 1), Math.max(0, deviceDeps.staleMs - (deviceDeps.now() - since)) + 20);
    return () => clearTimeout(timer);
  }, [away, stale, since]);
  if (!away) return null;
  return (
    <span className="combat-offline" role="status" title={stale ? "This may be out of date" : "Showing the last state seen"}>
      <Chip tone="danger">{stale ? "Offline · may be old" : "Offline"}</Chip>
    </span>
  );
}

/** "Take 7 slashing? Apply" (D7 a): applied to the player's OWN character, by the player's own command. */
function PromptCard({ combat, prompt, me, modifier }: { combat: PlayerCombat; prompt: CombatPrompt; me: PlayerCombatant; modifier: number }) {
  const run = usePlayMutate();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const go = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try { await work(); } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  const ctx = contextFor(combat.partyId);
  const answer = (applied: boolean) => go(async () => {
    let hp: { current: number; max: number; temp: number } | undefined;
    if (applied && prompt.kind === "damage") {
      const done = await run((api, id) => api.takeDamage({ characterId: id, amount: prompt.amount }));
      if (!done) throw new Error("Your character didn't take the damage. Try again.");
      hp = done.play.hitPoints;
    }
    if (applied && prompt.kind === "heal") {
      const done = await run((api, id) => api.regainHitPoints({ characterId: id, amount: prompt.amount }));
      if (!done) throw new Error("Your character didn't regain the hit points. Try again.");
      hp = done.play.hitPoints;
    }
    // The sheet's HP after it, so the server needn't wait for the roster card (combat-api.md "resolve_prompt").
    adopt(await resolvePrompt(combat.id, prompt.id, applied, hp && { current: hp.current, max: hp.max, temp: hp.temp }, ctx));
  });
  const roll = (input: D20Input) => go(async () => adopt(await resolveConcentration(combat.id, me.id, input, ctx)));
  if (prompt.kind === "damage" || prompt.kind === "heal") {
    const what = prompt.kind === "damage"
      ? `Take ${prompt.amount}${prompt.damageType ? ` ${prompt.damageType}` : ""} damage${prompt.critical ? " (critical hit)" : ""}?`
      : `Regain ${prompt.amount} hit points?`;
    return (
      <section className="combat-prompt" role="alertdialog" aria-label={what}>
        <p className="combat-prompt-q">{what}</p>
        <div className="combat-prompt-actions">
          <Button variant="primary" disabled={busy} onClick={() => void answer(true)}>Apply</Button>
          <Button disabled={busy} onClick={() => void answer(false)}>Not now</Button>
        </div>
        {error && <Alert tone="danger">{error}</Alert>}
      </section>
    );
  }
  if (prompt.kind === "concentration") {
    return (
      <section className="combat-prompt" role="alertdialog" aria-label={`Concentration save, DC ${prompt.dc}`}>
        <p className="combat-prompt-q">Concentration save, DC {prompt.dc}</p>
        <RollOrType label="Concentration save" modifier={modifier} busy={busy} onRoll={roll} />
        {error && <Alert tone="danger">{error}</Alert>}
      </section>
    );
  }
  return null;
}

function Pips({ label, filled, tone }: { label: string; filled: number; tone: "success" | "danger" }) {
  return (
    <span className="combat-pips" role="img" aria-label={`${label}: ${filled} of 3`}>
      <span className="combat-pips-label">{label}</span>
      {[0, 1, 2].map((i) => <span key={i} className={`combat-pip ${tone}${i < filled ? " on" : ""}`} />)}
    </span>
  );
}

/** The player's own character: HP and temp HP (their own, D7), concentration, death saves. */
function Me({ combat, me, s }: { combat: PlayerCombat; me: PlayerCombatant; s: CharacterSnapshot }) {
  const hp = s.play.hitPoints;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const saves = me.deathSaves ?? { successes: 0, failures: 0 };
  const dying = me.life === "dying" || (hp.current <= 0 && me.life !== "stable" && me.life !== "dead");
  const save = async (input: D20Input) => {
    setBusy(true);
    setError(null);
    try { adopt(await deathSave(combat.id, me.id, input, contextFor(combat.partyId))); } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  const share = hp.max > 0 ? Math.max(0, Math.min(1, hp.current / hp.max)) : 0;
  const tone = hp.current <= 0 ? "down" : hp.current * 2 <= hp.max ? "bloodied" : "";
  return (
    <section className="card combat-me" aria-label="Your character">
      <div className="combat-me-head">
        <strong className="combat-me-name">{s.name || me.name}</strong>
        <span className="combat-me-hp" role="status" aria-label={`Hit points: ${hp.current} of ${hp.max}${hp.temp ? `, ${hp.temp} temporary` : ""}`}>
          <span className="combat-me-hp-now">{hp.current}</span><span className="muted">/{hp.max}</span>
          {hp.temp > 0 && <span className="combat-me-temp">+{hp.temp} temp</span>}
        </span>
      </div>
      <div className="combat-hp-bar" aria-hidden="true"><span className={tone} style={{ width: `${share * 100}%` }} /></div>
      {s.play.concentration && <p className="combat-me-conc"><Chip tone="info">Concentrating: {s.play.concentration.name}</Chip></p>}
      <ConditionChips conditions={me.conditions} label="Your conditions" />
      {(dying || me.life === "stable" || me.life === "dead" || saves.successes + saves.failures > 0) && (
        <div className="combat-death" aria-label="Death saves" role="group">
          <div className="combat-death-pips">
            <Pips label="Successes" filled={saves.successes} tone="success" />
            <Pips label="Failures" filled={saves.failures} tone="danger" />
          </div>
          {me.life === "stable" && <p className="combat-death-state">Stable</p>}
          {me.life === "dead" && <p className="combat-death-state">Dead</p>}
          {dying && <RollOrType label="Death save" busy={busy} onRoll={save} />}
          {error && <Alert tone="danger">{error}</Alert>}
        </div>
      )}
    </section>
  );
}

function Initiative({ combat, me, modifier }: { combat: PlayerCombat; me: PlayerCombatant; modifier: number }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (me.initiative != null) return null;
  const roll = async (input: D20Input) => {
    setBusy(true);
    setError(null);
    try { adopt(await setInitiative(combat.id, me.id, input, contextFor(combat.partyId))); } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  return (
    <section className="combat-prompt" role="alertdialog" aria-label="Roll initiative">
      <p className="combat-prompt-q">Roll initiative <span className="muted">({modifier >= 0 ? `+${modifier}` : modifier})</span></p>
      <RollOrType label="Initiative" modifier={modifier} busy={busy} onRoll={roll} />
      {error && <Alert tone="danger">{error}</Alert>}
    </section>
  );
}

function Order({ combat, onStatBlock }: { combat: PlayerCombat; onStatBlock: (c: PlayerCombatant) => void }) {
  const acting = actingId(combat);
  return (
    <ol className="combat-order" aria-label="Turn order">
      {combat.combatants.map((c) => {
        const now = c.id === acting;
        return (
          <li key={c.id} className={`combat-row${now ? " acting" : ""}${c.mine ? " mine" : ""}${c.life === "dead" ? " dead" : ""}`}
            aria-current={now ? "true" : undefined}>
            {/* Creatures' initiative isn't sent to players (combat-api.md): their place in the order says it. */}
            {c.kind === "creature" ? <span className="combat-init" aria-hidden="true" />
              : <span className="combat-init" aria-label={c.initiative == null ? "No initiative yet" : `Initiative ${c.initiative}`}>{c.initiative ?? "–"}</span>}
            <span className="combat-row-main">
              <span className="combat-row-name">
                {c.name}{c.mine && <span className="muted"> (you)</span>}
                {c.concentration && !c.mine && <span className="muted combat-row-owner"> · concentrating</span>}
                {now && <span className="sr-only"> is acting now</span>}
              </span>
              <ConditionChips conditions={c.conditions} label={`${c.name} conditions`} />
            </span>
            <span className="combat-row-side">
              {c.band && <HealthBand band={c.band} />}
              {c.kind === "creature" && c.statBlock && (
                <Button small onClick={() => onStatBlock(c)} aria-label={`${c.name} stat block`}>Stats</Button>
              )}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

function Log({ combat }: { combat: PlayerCombat }) {
  const lines = combat.log.slice(-LOG_LINES).reverse();
  if (lines.length === 0) return null;
  return (
    <section className="card combat-log" aria-label="What happened">
      <h2 className="combat-h">What happened</h2>
      <ul>
        {lines.map((l, i) => <li key={`${l.at}-${i}`}><span className="muted">R{l.round}</span> {l.text}</li>)}
      </ul>
    </section>
  );
}

/** The Play tab's Combat sub-tab (plan 4.4 "Player combat view"): phone first, the player's projection only. */
export function CombatPane({ combat, s }: { combat: PlayerCombat; s: CharacterSnapshot }) {
  const me = myCombatant(combat, { id: s.id, name: s.name });
  const [sheet, setSheet] = useState<PlayerCombatant | null>(null);
  const yours = me != null && actingId(combat) === me.id;
  const round = combat.pointer.kind === "combatant" || combat.pointer.kind === "enemy_turn" || combat.pointer.kind === "waiting" ? combat.pointer.round : combat.round;
  const conSave = s.abilities.save.con?.bonus ?? 0;
  return (
    <div className="combat-pane" data-testid="combat-pane">
      <div className="combat-head">
        <span className="combat-round" aria-label={`Round ${round}`}>{round > 0 ? `Round ${round}` : "Setup"}</span>
        {(combat.title || combat.partyName) && <span className="muted combat-party">{combat.title ?? combat.partyName}</span>}
        <span className="spacer" />
        <Offline />
      </div>
      <div className={`combat-now${yours ? " yours" : ""}`} role="status" aria-live="polite">
        <span className="combat-now-label">{yours ? "" : "Now"}</span>
        <strong className="combat-now-name">{nowText(combat, me)}</strong>
      </div>
      {me && <Initiative combat={combat} me={me} modifier={s.stats.initiative} />}
      {me && combat.prompts.filter((p) => p.combatant === me.id).map((p) => (
        <PromptCard key={p.id} combat={combat} prompt={p} me={me} modifier={conSave} />
      ))}
      {me && <Me combat={combat} me={me} s={s} />}
      <section className="card combat-order-card" aria-label="Initiative">
        <h2 className="combat-h">Turn order</h2>
        <Order combat={combat} onStatBlock={setSheet} />
      </section>
      <Log combat={combat} />
      {sheet?.statBlock && (
        <Modal title={sheet.name} onClose={() => setSheet(null)}>
          {(sheet.hp || sheet.ac != null) && (
            <p className="combat-sb-now">
              {sheet.hp && <Chip tone="info">HP {sheet.hp.current} / {sheet.hp.max}</Chip>} {sheet.ac != null && <Chip>AC {sheet.ac}</Chip>}
            </p>
          )}
          <StatBlockView name={sheet.name} block={sheet.statBlock} />
        </Modal>
      )}
    </div>
  );
}
