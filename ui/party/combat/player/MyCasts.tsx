import { useState } from "react";
import { Button } from "../../../src/components/Button";
import { Alert } from "../../../src/components/Card";
import { RollOrType } from "../shared/RollOrType";
import type { D20Input } from "../shared/types";
import type { MyCast, PlayerCombat } from "./api";
import { playerWrite } from "./store";

const errorText = (e: unknown) => (e as { message?: string } | null)?.message ?? "That didn't go through.";
const OUTCOME: Record<string, string> = { lands: "Lands", hit: "Hit", crit: "Critical hit", miss: "Miss", saved: "Saved", failed: "Failed" };
const AMOUNT: Record<MyCast["amounts"][number]["kind"], string> = { damage: "Damage", heal: "Healing", temp_hp: "Temporary hit points" };

/**
 * The player's own open casts (party/docs/role-views.md `myCasts`), on the Play tab's live combat section: their
 * attack rolls (typed from the table first, "Roll for me" second), the DM's creatures' saves shown as waiting for the
 * DM, then the damage or healing to roll and apply, and Cancel. A cast the server resolved at once never shows here.
 */
export function MyCasts({ combat }: { combat: PlayerCombat }) {
  if (combat.myCasts.length === 0) return null;
  return (
    <section className="card combat-my-casts" aria-label="Your spells in play">
      <h2 className="combat-h">Your spells in play</h2>
      {combat.myCasts.map((k) => <CastCard key={k.cast} combat={combat} cast={k} />)}
    </section>
  );
}

function CastCard({ combat, cast }: { combat: PlayerCombat; cast: MyCast }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [typed, setTyped] = useState<string[]>(() => cast.amounts.map(() => ""));
  const name = (id: string) => combat.combatants.find((c) => c.id === id)?.name || "you";
  const go = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try { await work(); } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  const resolve = (row: MyCast["rows"][number], roll: D20Input) => go(() => playerWrite(combat, "resolve_cast_row", {
    cast: cast.cast, target: row.target, ...(row.ray != null ? { ray: row.ray } : {}),
    roll: roll.kind === "typed" ? { kind: "typed", total: roll.total } : { kind: "app" },
  }));
  const totals = typed.map((t) => (/^\d+$/.test(t.trim()) ? Number(t.trim()) : null));
  const allTyped = totals.every((t) => t !== null);
  const apply = (app: boolean) => go(() => playerWrite(combat, "apply_cast", {
    cast: cast.cast, amounts: totals.map((t) => (app || t === null ? { kind: "app" } : { kind: "typed", total: t })),
  }, (c) => ({ ...c, myCasts: c.myCasts.filter((x) => x.cast !== cast.cast) })));
  const cancel = () => go(() => playerWrite(combat, "cancel_cast", { cast: cast.cast },
    (c) => ({ ...c, myCasts: c.myCasts.filter((x) => x.cast !== cast.cast) })));
  return (
    <article className="combat-my-cast" aria-label={cast.spell}>
      <div className="combat-my-cast-head">
        <strong>{cast.spell}</strong>{cast.slot ? <span className="muted"> · level {cast.slot}</span> : null}
        <span className="spacer" />
        <Button small variant="ghost" disabled={busy} onClick={() => void cancel()}>Cancel</Button>
      </div>
      <ul className="combat-my-cast-rows">
        {cast.rows.map((r, i) => (
          <li key={`${r.target}-${r.ray ?? 0}-${i}`}>
            <span className="combat-my-cast-target">{name(r.target)}{r.ray != null && r.ray > 0 ? ` (ray ${r.ray + 1})` : ""}</span>
            {r.outcome
              ? <span className="chip">{OUTCOME[r.outcome] ?? r.outcome}</span>
              : cast.delivery === "attack"
                ? <RollOrType label={`${cast.spell} attack on ${name(r.target)}`} busy={busy} onRoll={(roll) => void resolve(r, roll)} />
                : <span className="muted">Waiting for the DM's roll</span>}
          </li>
        ))}
      </ul>
      {cast.needs_apply && cast.amounts.length > 0 && (
        <form className="combat-my-cast-apply" onSubmit={(e) => { e.preventDefault(); if (allTyped) void apply(false); }}>
          {cast.amounts.map((a, i) => (
            <label key={i} className="combat-my-cast-amount">
              <span>{AMOUNT[a.kind]} {a.dice}{a.damage_type ? ` ${a.damage_type}` : ""}</span>
              <input className="input" inputMode="numeric" placeholder="Your roll" aria-label={`${AMOUNT[a.kind]} ${a.dice}: my table roll`}
                value={typed[i] ?? ""} disabled={busy} onChange={(e) => setTyped((t) => t.map((x, j) => (j === i ? e.target.value : x)))} />
            </label>
          ))}
          <div className="combat-prompt-actions">
            <Button type="submit" variant="primary" disabled={busy || !allTyped}>Apply</Button>
            <Button variant="ghost" icon="dice" disabled={busy} onClick={() => void apply(true)}>Roll for me</Button>
          </div>
        </form>
      )}
      {error && <Alert tone="danger">{error}</Alert>}
    </article>
  );
}
