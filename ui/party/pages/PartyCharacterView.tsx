import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router";
import type { CharacterSnapshot } from "../../src/api/contract";
import { Alert, Card, Chip } from "../../src/components/Card";
import { errorMessage } from "../../src/state/useAsync";
import { BAND_LABEL, classLine, viewMember, watchParties, type MemberView } from "../api/overrides/parties";
import { bandOf } from "./PartiesPage";
import "./party.css";

const ABILITIES = [
  ["str", "STR"], ["dex", "DEX"], ["con", "CON"], ["int", "INT"], ["wis", "WIS"], ["cha", "CHA"],
] as const;
const signed = (n: number) => (n >= 0 ? `+${n}` : `${n}`);
/** Party events come in bursts while a player plays: one read for a burst. */
const SETTLE_MS = 300;

type Snap = CharacterSnapshot;

/** A labelled number, as on the sheet. */
function Box({ label, value, wide }: { label: string; value: string | number; wide?: boolean }) {
  return (
    <div className={`party-sheet-box${wide ? " wide" : ""}`} role="group" aria-label={`${label} ${value}`}>
      <span className="party-sheet-box-label">{label}</span>
      <span className="party-sheet-box-value">{value}</span>
    </div>
  );
}

function Sheet({ view }: { view: MemberView }) {
  const s: Snap = view.snapshot;
  const hp = s.play?.hitPoints ?? { current: 0, max: s.stats?.hpMax ?? 0, temp: 0 };
  const stats = view.card.stats;
  const band = stats ? bandOf(stats) : null;
  const classes = classLine(view.card.classes) || `Level ${s.level?.total ?? view.card.level ?? "?"}`;
  const slots = (s.play?.spellSlots ?? []).filter((x) => x.max > 0);
  const items = [...(s.inventory?.items ?? [])].sort((a, b) => Number(Boolean(b.equipped)) - Number(Boolean(a.equipped)));
  const skills = (s.stats?.skills ?? []).filter((k) => k.proficiency && k.proficiency !== "none");
  return (
    <>
      <Card className="party-sheet-head" title={s.name} aria-label={s.name}
        actions={band ? <Chip tone={band === "unhurt" ? "success" : band === "bloodied" ? "warning" : "danger"}>{BAND_LABEL[band]}</Chip> : undefined}>
        <p className="muted party-note">{classes} · {view.card.ownerName ? `${view.card.ownerName}'s character` : "a member's character"}</p>
        <div className="party-sheet-hp" role="group" aria-label={`Hit points ${hp.current} of ${hp.max}${hp.temp ? `, ${hp.temp} temporary` : ""}`}>
          <span className="party-sheet-hp-now">{hp.current}</span>
          <span className="muted">/ {hp.max} HP</span>
          {hp.temp > 0 && <Chip tone="info">+{hp.temp} temporary</Chip>}
          <span className="party-sheet-hp-bar" aria-hidden="true">
            <span style={{ width: `${hp.max > 0 ? Math.round((100 * hp.current) / hp.max) : 0}%` }} className={band ?? ""} />
          </span>
        </div>
        <div className="party-sheet-boxes">
          <Box label="AC" value={s.stats?.armorClass ?? "?"} />
          <Box label="Initiative" value={signed(s.stats?.initiative ?? 0)} />
          <Box label="Speed" value={`${s.stats?.speed ?? 0} ft.`} />
          <Box label="Passive Perception" value={s.stats?.passivePerception ?? "?"} wide />
          <Box label="Proficiency" value={signed(s.stats?.proficiencyBonus ?? 0)} />
        </div>
        {stats && (stats.concentration || stats.exhaustion > 0 || stats.inspiration) && (
          <div className="party-conditions">
            {stats.concentration && <Chip tone="info">Concentrating: {stats.concentration}</Chip>}
            {stats.exhaustion > 0 && <Chip tone="warning">Exhaustion {stats.exhaustion}</Chip>}
            {stats.inspiration && <Chip tone="gold">Inspiration</Chip>}
          </div>
        )}
      </Card>

      <Card title="Abilities">
        <div className="party-sheet-abilities">
          {ABILITIES.map(([key, label]) => {
            const save = s.abilities?.save?.[key];
            return (
              <div key={key} className="party-sheet-ability" role="group" aria-label={label}>
                <span className="party-sheet-box-label">{label}</span>
                <span className="party-sheet-box-value">{signed(s.abilities?.modifier?.[key] ?? 0)}</span>
                <span className="muted">{s.abilities?.final?.[key] ?? "?"}</span>
                <span className="muted party-sheet-save">Save {signed(save?.bonus ?? 0)}{save?.proficient ? " ●" : ""}</span>
              </div>
            );
          })}
        </div>
        {skills.length > 0 && (
          <p className="party-note"><strong>Skills:</strong> {skills.map((k) => `${k.name} ${signed(k.bonus)}`).join(", ")}</p>
        )}
      </Card>

      {(s.attacks ?? []).length > 0 && (
        <Card title="Attacks">
          <ul className="party-list">
            {s.attacks.map((a) => (
              <li key={a.id} className="party-line">
                <span className="grow"><strong>{a.name}</strong><span className="muted party-note">{[a.range, a.damage].filter(Boolean).join(" · ")}</span></span>
                <span className="party-sheet-bonus">{a.attackBonus}</span>
              </li>
            ))}
          </ul>
        </Card>
      )}

      {((s.spellcasting?.classes ?? []).length > 0 || slots.length > 0) && (
        <Card title="Spellcasting">
          {(s.spellcasting?.classes ?? []).map((c) => (
            <p key={c.classSlotId} className="party-note"><strong>{c.className}:</strong> save DC {c.saveDc}, attack {signed(c.attackRoll ?? c.attackBonus)}</p>
          ))}
          {slots.length > 0 && (
            <div className="party-sheet-slots" role="list" aria-label="Spell slots">
              {slots.map((x) => (
                <span key={x.level} role="listitem" className="party-stat" aria-label={`Level ${x.level} slots ${x.max - x.used} of ${x.max} left`}>
                  <span className="party-stat-label">L{x.level}</span><span className="party-stat-value">{x.max - x.used}<span className="muted">/{x.max}</span></span>
                </span>
              ))}
            </div>
          )}
        </Card>
      )}

      {(s.stats?.features ?? []).length > 0 && (
        <Card title="Features">
          <ul className="party-sheet-tags">
            {s.stats.features.map((f, i) => <li key={`${f.name}-${i}`} title={f.source}>{f.name}</li>)}
          </ul>
        </Card>
      )}

      {items.length > 0 && (
        <Card title="Equipment">
          <ul className="party-sheet-tags">
            {items.map((it) => (
              <li key={it.id} className={it.equipped ? "equipped" : ""}>
                {it.displayName || it.name}{it.quantity > 1 ? ` ×${it.quantity}` : ""}{it.equipped ? " (equipped)" : ""}
              </li>
            ))}
          </ul>
        </Card>
      )}

      {((s.stats?.languages ?? []).length > 0 || (s.stats?.proficiencies ?? []).length > 0) && (
        <Card title="Proficiencies and languages">
          {s.stats.proficiencies.filter((g) => g.values.length > 0).map((g) => (
            <p key={g.label} className="party-note"><strong>{g.label}:</strong> {g.values.join(", ")}</p>
          ))}
          {s.stats.languages.length > 0 && <p className="party-note"><strong>Languages:</strong> {s.stats.languages.join(", ")}</p>}
        </Card>
      )}
    </>
  );
}

/**
 * The DM's read-only view of a member character (plan 3.5): the server reads it as its owner has it now, without
 * opening it, and nothing here can change it. It follows the player's play live: a party event for this character
 * reads it again.
 */
export function PartyCharacterView() {
  const { party = "", owner = "", character = "" } = useParams();
  const [view, setView] = useState<MemberView | null | undefined>(undefined);
  const [error, setError] = useState("");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const read = useCallback(async () => {
    try {
      setView(await viewMember(party, owner, character));
      setError("");
    } catch (e) { setError(errorMessage(e)); }
  }, [party, owner, character]);
  useEffect(() => { void read(); }, [read]);
  useEffect(() => {
    const soon = () => { clearTimeout(timer.current); timer.current = setTimeout(() => void read(), SETTLE_MS); };
    const stop = watchParties((e) => {
      if (e.type === "party" ? e.party === party && (e.id === character || e.kind === "roster") : true) soon();
    });
    return () => { stop(); clearTimeout(timer.current); };
  }, [party, character, read]);
  useEffect(() => {
    const seen = () => { if (document.visibilityState === "visible") void read(); };
    document.addEventListener("visibilitychange", seen);
    return () => document.removeEventListener("visibilitychange", seen);
  }, [read]);

  return (
    <div className="party party-parties party-sheet">
      <div className="party-line">
        <Link className="btn btn-sm" to="/parties">Back to parties</Link>
        <span className="grow" />
        <Chip>Read-only</Chip>
      </div>
      {error && <Alert tone="danger">{error}</Alert>}
      {view === undefined && !error && <p className="muted">Loading the character…</p>}
      {view === null && <Alert tone="info">This character isn't in your party any more.</Alert>}
      {view && <Sheet view={view} />}
      {view && <p className="muted party-note">Only the player can change this character. You see it as they have it now.</p>}
    </div>
  );
}
