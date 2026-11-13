import { useState } from "react";
import { Button } from "../../../src/components/Button";
import { abilityStatus, byId, isDefeated, linkActive, poolResetOwner } from "./derive";
import type { Command, CombatState, EncounterRule } from "./types";

/**
 * The sticky resource panel (C16, C17, C15, C21, C22): legendary pools, shared across creatures, with each member's
 * legendary options; Legendary Resistance; recharges; group per-round limits; mount links. On phones only the pools
 * show until "More" (C26).
 */
export function ResourcePanel({ state, send, disabled, onSelect }: { state: CombatState; send: (cmd: Command) => void; disabled: boolean; onSelect: (id: string) => void }) {
  const [more, setMore] = useState(false);
  const pools = Object.entries(state.legendary_pools ?? {});
  const resist = state.combatants.filter((c) => c.legendary_resistance && !c.removed);
  const recharges = state.combatants.flatMap((c) => (c.removed ? [] : (c.abilities ?? []).filter((a) => a.recharge).map((a) => ({ c, a }))));
  const groups = Object.entries(state.group_limits ?? {});
  const links = state.combatants.filter((c) => c.linked_to && !c.removed);
  const empty = pools.length + resist.length + recharges.length + groups.length + links.length === 0;
  return (
    <aside className={`cb-panel cb-resources${more ? " expanded" : ""}`} aria-label="Resources">
      <div className="cb-panel-head">
        <strong>Resources</strong>
        <Button small variant="ghost" className="cb-more-toggle" aria-expanded={more} onClick={() => setMore(!more)}>{more ? "▴ Less" : "▾ More"}</Button>
      </div>
      {empty && <p className="muted cb-small cb-pad">No legendary actions, recharges or group limits in this fight.</p>}
      {pools.map(([key, pool]) => {
        const owner = poolResetOwner(state, pool);
        const members = pool.members.map((m) => byId(state, m)).filter((c) => c && !c.removed);
        const shared = members.length > 1;
        return (
          <section key={key} className="cb-res-section">
            <div className="cb-res-title">
              <span>Legendary actions{shared && <span className="chip gold">SHARED</span>}</span>
              <span className="chip" aria-label={`${pool.remaining} of ${pool.max} left`}>{pool.remaining}/{pool.max}</span>
            </div>
            <div className="cb-pips" aria-hidden="true">{Array.from({ length: pool.max }, (_, i) => <span key={i} className={i < pool.remaining ? "on" : ""}>{i < pool.remaining ? "●" : "○"}</span>)}</div>
            <div className="cb-legend-opts">
              {members.flatMap((m) => (m!.abilities ?? []).filter((a) => a.kind === "legendary").map((a) => {
                const why = abilityStatus(state, m!, a);
                return (
                  <button key={`${m!.id}-${a.key}`} type="button" className="cb-legend-opt" disabled={disabled || Boolean(why)} title={why ?? a.text}
                    onClick={() => send({ command: "use_ability", id: m!.id, ability: a.key })}>
                    <strong>{shared ? `${m!.label}: ` : ""}{a.name} <span className="cb-la-cost">{a.legendary_cost ?? 1} LA</span></strong>
                    <span>{why ?? a.text}</span>
                  </button>
                );
              }))}
            </div>
            <div className="cb-res-foot">
              <Button small variant="ghost" disabled={disabled || pool.remaining <= 0} onClick={() => send({ command: "adjust_legendary", pool: key, delta: -1 })}>−1</Button>
              <Button small variant="ghost" disabled={disabled || pool.remaining >= pool.max} onClick={() => send({ command: "adjust_legendary", pool: key, delta: 1 })}>+1</Button>
              <span className="cb-small muted">Resets at the start of {owner ? `${owner.label}'s` : "its owner's"} turn. Not on a member's own turn.</span>
            </div>
          </section>
        );
      })}
      <div className="cb-res-more">
        {resist.map((c) => (
          <section key={`lr-${c.id}`} className="cb-res-section">
            <div className="cb-res-title"><span>{c.label} · Legendary Resistance</span><span className="chip">{c.legendary_resistance!.remaining}/{c.legendary_resistance!.max}</span></div>
            <div className="cb-res-foot">
              <Button small variant="danger" disabled={disabled || c.legendary_resistance!.remaining <= 0 || isDefeated(c)} onClick={() => send({ command: "adjust_legendary_resistance", id: c.id, delta: -1 })}>Use</Button>
              <Button small disabled={disabled || c.legendary_resistance!.remaining >= c.legendary_resistance!.max} onClick={() => send({ command: "adjust_legendary_resistance", id: c.id, delta: 1 })}>Restore +1</Button>
            </div>
          </section>
        ))}
        {recharges.length > 0 && (
          <section className="cb-res-section">
            <div className="cb-res-title"><span>Recharges</span></div>
            <ul className="cb-res-list">
              {recharges.map(({ c, a }) => (
                <li key={`${c.id}-${a.key}`}>
                  <button type="button" className="cb-link" onClick={() => onSelect(c.id)}>{c.label}: {a.name}</button>
                  <span className={`chip ${a.recharge!.ready ? "success" : "danger"}`}>{a.recharge!.ready ? "Ready" : `Spent · ${a.recharge!.min}–6`}</span>
                </li>
              ))}
            </ul>
          </section>
        )}
        {groups.length > 0 && (
          <section className="cb-res-section">
            <div className="cb-res-title"><span>Group limits (per round)</span></div>
            <ul className="cb-res-list">
              {groups.map(([tag, g]) => <li key={tag}><span>{tag}</span><span className={`chip ${g.used >= g.per_round ? "danger" : "success"}`}>{g.used}/{g.per_round} used</span></li>)}
            </ul>
          </section>
        )}
        {links.length > 0 && (
          <section className="cb-res-section">
            <div className="cb-res-title"><span>Mounts and links</span></div>
            <ul className="cb-res-list">
              {links.map((f) => {
                const leader = byId(state, f.linked_to);
                return (
                  <li key={f.id}>
                    <span>{f.label} ↳ {leader?.label}{linkActive(state, f) ? "" : " (leader down)"}</span>
                    <Button small variant="ghost" disabled={disabled} onClick={() => send({ command: "link", id: f.id })}>Unlink</Button>
                  </li>
                );
              })}
            </ul>
          </section>
        )}
      </div>
    </aside>
  );
}

/** The encounter rules as banners (C20, D42): text plus the abilities it switches; it ends for good with its creature. */
export function RuleBanners({ state, send, disabled }: { state: CombatState; send: (cmd: Command) => void; disabled: boolean }) {
  if (!state.rules?.length) return null;
  return (
    <div className="cb-rules">
      {state.rules.map((r: EncounterRule) => {
        const linked = byId(state, r.linked);
        return (
          <section key={r.id} className={`cb-rule${r.active ? "" : " off"}`} aria-label={`Encounter rule: ${r.name}`}>
            <div>
              <div className="cb-overline">{r.name}{linked ? ` · while ${linked.label} lives` : ""}</div>
              <div className="cb-rule-head">{r.active ? "● ACTIVE" : "✕ ENDED"}</div>
              <div className="cb-small">{r.active ? r.text_active : r.text_ended}</div>
            </div>
            <Button small variant="ghost" disabled={disabled} onClick={() => send({ command: "set_rule_active", id: r.id, active: !r.active })}>{r.active ? "End it" : "Turn on"}</Button>
          </section>
        );
      })}
    </div>
  );
}
