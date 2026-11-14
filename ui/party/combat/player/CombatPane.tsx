import { useEffect, useState } from "react";
import type { CharacterSnapshot } from "../../../src/api/contract";
import { Button } from "../../../src/components/Button";
import { Alert, Chip } from "../../../src/components/Card";
import { Modal } from "../../../src/components/Modal";
import { useConnectivity } from "../../api/connectivity";
import {
  actingId, myCombatant,
  type CombatPrompt, type D20Input, type PlayerCombat, type PlayerCombatant,
} from "./api";
import { usePlayMutate } from "../../../src/pages/play/playQueue";
import { ConditionChips } from "../shared/ConditionChips";
import { HealthBand } from "../shared/HealthBand";
import { RollOrType } from "../shared/RollOrType";
import { StatBlockView } from "../shared/StatBlockView";
import { Creatures } from "./Creatures";
import { deviceDeps, playerWrite, useCombatStore } from "./store";
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
      const who = c.combatants.find((x) => x.id === id);
      // One of my summons or pets: it's mine to play too.
      if (who?.mine) return `Your turn: ${who.name}`;
      return who?.name ?? "Someone";
    }
    case "enemy_turn": return "Enemy turn";
    case "waiting": return "Waiting for the DM";
    case "not_started": return "Rolling initiative";
    default: return "Combat over";
  }
}

/**
 * The link to the server, as the sub-tab says it (D44): nothing for a blip; "Reconnecting…" once about 3 s have gone
 * by without an answer (a request failed, or the stream is being reopened); "Offline · may be old" after a minute.
 * The last state stays on screen throughout.
 */
function Offline() {
  const { offline, offlineSince } = useCombatStore();
  const link = useConnectivity().link;
  const away = offline || link === "down" || link === "checking";
  const [awaySince, setAwaySince] = useState<number | null>(null);
  const [, tick] = useState(0);
  useEffect(() => { setAwaySince(away ? (offlineSince ?? deviceDeps.now()) : null); }, [away, offlineSince]);
  const gone = awaySince == null ? 0 : deviceDeps.now() - awaySince;
  const stale = gone >= deviceDeps.staleMs;
  const shown = gone >= deviceDeps.reconnectingMs;
  // Look again when the next threshold comes.
  useEffect(() => {
    if (awaySince == null || stale) return undefined;
    const next = shown ? deviceDeps.staleMs : deviceDeps.reconnectingMs;
    const timer = setTimeout(() => tick((n) => n + 1), Math.max(0, next - (deviceDeps.now() - awaySince)) + 20);
    return () => clearTimeout(timer);
  }, [awaySince, stale, shown]);
  if (awaySince == null || !shown) return null;
  return (
    <span className="combat-offline" role="status" title={stale ? "This may be out of date" : "Showing the last state seen"}>
      <Chip tone="danger">{stale ? "Offline · may be old" : "Reconnecting…"}</Chip>
    </span>
  );
}

/** "Take 7 slashing? Apply" (D7 a): applied to the player's OWN character, by the player's own command. */
function PromptCard({ combat, prompt, me, modifier, saveOf }: { combat: PlayerCombat; prompt: CombatPrompt; me: PlayerCombatant; modifier: number; saveOf?: (ability: string) => number }) {
  const run = usePlayMutate();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const go = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try { await work(); } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
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
    await playerWrite(combat, "resolve_prompt", { promptId: prompt.id, applied, ...(hp ? { hp: { current: hp.current, max: hp.max, temp: hp.temp } } : {}) },
      (c) => ({ ...c, prompts: c.prompts.filter((p) => p.id !== prompt.id) }));
  });
  const roll = (input: D20Input) => go(() => playerWrite(combat, "resolve_concentration", { id: me.id, roll: input },
    (c) => ({ ...c, prompts: c.prompts.filter((p) => p.id !== prompt.id) })));
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
  if (prompt.kind === "cast_save") {
    const what = `${prompt.ability.toUpperCase()} save${prompt.dc != null ? ` DC ${prompt.dc}` : ""} against ${prompt.name}`;
    const send = (input: D20Input) => go(() => playerWrite(combat, "resolve_cast_row", { cast: prompt.cast, target: me.id, roll: input },
      (c) => ({ ...c, prompts: c.prompts.filter((p) => p.id !== prompt.id) })));
    return (
      <section className="combat-prompt" role="alertdialog" aria-label={what}>
        <p className="combat-prompt-q">{what}</p>
        <p className="muted combat-prompt-from">{prompt.from ? `From ${prompt.from}` : "From an unseen creature"}</p>
        <TableRoll label={what} modifier={saveOf?.(prompt.ability) ?? 0} busy={busy} onRoll={send} />
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

/**
 * A roll at the table first (0.3.2, the user's "promote actual rolls on table"): the typed total is the main action;
 * "Roll for me" is for players without dice.
 */
function TableRoll({ label, modifier, busy, onRoll }: { label: string; modifier: number; busy: boolean; onRoll: (input: D20Input) => void }) {
  const [text, setText] = useState("");
  const total = /^-?\d+$/.test(text.trim()) ? Number(text.trim()) : null;
  return (
    <form className="combat-table-roll" onSubmit={(e) => { e.preventDefault(); if (total !== null) onRoll({ kind: "typed", total }); }}>
      <input className="input" inputMode="numeric" enterKeyHint="send" autoFocus placeholder="Your roll" aria-label={`${label}: my table roll`}
        value={text} onChange={(e) => setText(e.target.value)} disabled={busy} />
      <Button type="submit" variant="primary" disabled={busy || total === null}>Send</Button>
      <Button variant="ghost" icon="dice" disabled={busy} onClick={() => onRoll({ kind: "app", modifier })}>Roll for me ({modifier >= 0 ? `+${modifier}` : modifier})</Button>
    </form>
  );
}

function Pips({ label, filled, tone }: { label: string; filled: number; tone: "success" | "danger" }) {
  return (
    <span className="combat-pips" role="img" aria-label={`${label}: ${filled} of 3`}>
      <span className="combat-pips-label">{label}</span>
      {[0, 1, 2].map((i) => <span key={i} className={`combat-pip ${tone}${i < filled ? " on" : ""}`} />)}
    </span>
  );
}

/** What a typed death save does (combat-engine.md), shown before the server answers. */
function guessSave(c: PlayerCombat, id: string, total: number): PlayerCombat {
  return {
    ...c, combatants: c.combatants.map((x) => {
      if (x.id !== id) return x;
      const s = { ...(x.deathSaves ?? { successes: 0, failures: 0 }) };
      if (total >= 20) return { ...x, life: "alive" as const, deathSaves: { successes: 0, failures: 0 } };
      if (total >= 10) s.successes += 1; else s.failures += total <= 1 ? 2 : 1;
      return { ...x, deathSaves: s, life: s.successes >= 3 ? "stable" as const : s.failures >= 3 ? "dead" as const : x.life };
    }),
  };
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
    try { await playerWrite(combat, "death_save", { id: me.id, roll: input }, input.kind === "typed" ? (c) => guessSave(c, me.id, input.total) : undefined); } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
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
    try {
      await playerWrite(combat, "set_initiative", { id: me.id, roll: input },
        input.kind === "typed" ? (c) => ({ ...c, combatants: c.combatants.map((x) => (x.id === me.id ? { ...x, initiative: input.total } : x)) }) : undefined);
    } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
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
  const names = new Map(combat.combatants.map((c) => [c.id, c.name || "you"]));
  return (
    <ol className="combat-order" aria-label="Turn order">
      {combat.combatants.filter((c) => c.inOrder).map((c) => {
        const now = c.id === acting;
        const follows = c.ownerId && names.has(c.ownerId) && c.initiative == null;
        return (
          <li key={c.id} className={`combat-row${now ? " acting" : ""}${c.mine ? " mine" : ""}${c.life === "dead" ? " dead" : ""}${follows ? " follows" : ""}`}
            aria-current={now ? "true" : undefined}>
            {/* Creatures' initiative isn't sent to players (combat-api.md): their place in the order says it. */}
            {c.kind === "creature" ? <span className="combat-init" aria-hidden="true" />
              : <span className="combat-init" aria-label={c.initiative == null ? "No initiative yet" : `Initiative ${c.initiative}`}>{c.initiative ?? "–"}</span>}
            <span className="combat-row-main">
              <span className="combat-row-name">
                {c.name}{c.mine && <span className="muted">{c.kind === "character" ? " (you)" : " (yours)"}</span>}
                {c.concentration && !c.mine && <span className="muted combat-row-owner"> · concentrating</span>}
                {now && <span className="sr-only"> is acting now</span>}
              </span>
              {c.ownerId && names.has(c.ownerId) && <span className="muted combat-row-owner">with {names.get(c.ownerId)}</span>}
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
  const acting = actingId(combat);
  const yours = acting != null && combat.combatants.some((x) => x.mine && x.id === acting);
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
        <PromptCard key={p.id} combat={combat} prompt={p} me={me} modifier={conSave}
          saveOf={(a) => (s.abilities.save as Record<string, { bonus?: number } | undefined>)[a]?.bonus ?? 0} />
      ))}
      {me && <Me combat={combat} me={me} s={s} />}
      {me && <Creatures combat={combat} me={me} s={s} />}
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
