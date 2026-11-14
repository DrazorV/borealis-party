import { AssistantButton } from "../assistant/AssistantButton";
import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { Button, IconButton } from "../../src/components/Button";
import { Alert, Card, Chip, EmptyState } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { Modal } from "../../src/components/Modal";
import { ToggleSwitch } from "../../src/components/controls/Controls";
import { useToast } from "../../src/components/Toast";
import { errorMessage } from "../../src/state/useAsync";
import {
  addCreatures, addEffect, addRule, deleteEncounter, EDITION_WORD, INIT_WORD, LIMITS, loadEncounter, nested,
  refreshCreature, removeCreature, removeEffect, removeRule, STATUS_WORD, swapCreature, updateCreature, updateEffect,
  updateEncounter, updateRule, watchEncounters, type Conflict, type Creature, type Effect, type Encounter,
  type EncounterFields, type Rule,
} from "../api/overrides/encounters";
import type { Ruleset } from "../api/overrides/parties";
import { LiveCombatLink, StartCombatButton } from "../combat/dm/StartCombatButton";
import { CreatureDialog, DetailsDialog, EffectDialog, RefreshDialog, RuleDialog, WarningsDialog } from "./EncounterDialogs";
import { BeforeFightCard, PrecastDialog } from "./EncounterSpells";
import { addPrecast, deletePrecast, listPrecasts, updatePrecast, type Precast } from "../combat/dm/spellApi";
import { EncounterPicker } from "./EncounterPicker";
import { useWide } from "./EncounterToolPanel";
import { encountersPath } from "./EncountersPage";
import { EncounterStatBlock } from "./EncounterStatBlock";
import "./party.css";
import "./encounters.css";

/**
 * The tool beside the creatures (EncounterToolPanel): a column on a big screen, a full-screen sheet on a phone. One at a
 * time; dialogs open over it without closing it. Part C's assistant joins this union.
 */
type Tool = { kind: "picker"; owner?: Creature };

type Open =
  | { kind: "details" }
  | { kind: "creature"; creature: Creature }
  | { kind: "block"; creature: Creature }
  | { kind: "refresh"; creature: Creature; conflicts: Conflict[] }
  | { kind: "effect"; effect?: Effect }
  | { kind: "precast"; precast?: Precast }
  | { kind: "rule"; rule?: Rule }
  | { kind: "warnings" }
  | { kind: "more"; creature: Creature };

/**
 * One creature row: what it is, whether players see it (hidden, or revealed with its whole stat block: D20 b), and
 * for a controlled creature, when it acts. Controlled creatures sit under their owner, indented.
 */
function CreatureRow({ c, encId, ruleset, owner, act, open }: {
  c: Creature; encId: string; ruleset: Ruleset; owner?: Creature;
  act: (work: () => Promise<Encounter>, done?: string) => Promise<void>;
  open: (o: Open) => void;
}) {
  const otherEdition = c.ruleset !== ruleset && Boolean(EDITION_WORD[c.ruleset]);
  return (
    <li className={`enc-creature ${c.hidden ? "is-hidden" : "is-revealed"} ${owner ? "is-pet" : ""}`} aria-label={c.dmLabel}>
      <div className="enc-creature-head">
        <span className="grow enc-creature-text">
          <strong>{c.dmLabel}</strong>
          <span className="muted party-note">
            {[c.cr ? `CR ${c.cr}` : null, c.ac != null ? `AC ${c.ac}` : null,
              c.hpMode === "fixed" && c.hpMax != null ? `HP ${c.hpMax} (typed)` : c.hp ? `HP ${c.hp.average}` : null].filter(Boolean).join(" · ")}
          </span>
          {owner && <span className="muted party-note">{c.initMode === "own" ? INIT_WORD.own : `Acts right after ${owner.dmLabel}`}</span>}
          <span className="muted party-note">{c.hidden ? "Hidden from players" : `Players see "${c.publicName}" and its stat block`}</span>
        </span>
        {c.kind === "companion" && <Chip tone="info">Companion</Chip>}
        {c.modified && <Chip tone="warning" title="Changed for this encounter">Changed</Chip>}
        {otherEdition && <Chip tone="info" title="A stat block of the other edition">{EDITION_WORD[c.ruleset]}</Chip>}
        <IconButton icon="more-vertical" label={`More for ${c.dmLabel}`} onClick={() => open({ kind: "more", creature: c })} />
      </div>
      <div className="enc-creature-controls">
        <ToggleSwitch checked={!c.hidden} label={`${c.dmLabel}: shown to players`} onLabel="Revealed" offLabel="Hidden"
          onChange={(on) => void act(() => updateCreature(encId, c.id, { hidden: !on }))} />
        <Button small onClick={() => open({ kind: "creature", creature: c })}>Edit</Button>
      </div>
    </li>
  );
}

/**
 * The encounter builder (plan 4.3, companion C19/C20/C23): pick monsters and Aurora companions from the DM's own
 * content into a draft, give villains pets and summons, hide or reveal each creature, change a creature's numbers for
 * this fight, add starting effects and encounter rules, and show the encounter to the players when it's ready. It
 * uses the party's edition (D23 b) and rates nothing (D16). Only the party's DM gets here; anyone else gets the
 * server's 404.
 */
export function EncounterBuilder() {
  const { party: partyId = "", encounter: id = "" } = useParams();
  const navigate = useNavigate();
  const toast = useToast();
  const confirm = useConfirm();
  const wide = useWide();
  const [enc, setEnc] = useState<Encounter | null | undefined>(undefined);
  const [error, setError] = useState("");
  const [dialog, setDialog] = useState<Open | null>(null);
  const [tool, setTool] = useState<Tool | null>(null);
  const [notes, setNotes] = useState<string | null>(null);
  /** 0.3.2: spells cast before the fight (their own route; a server without it shows none). */
  const [precasts, setPrecasts] = useState<Precast[]>([]);
  const loadPrecasts = useCallback(() => { listPrecasts(id).then(setPrecasts, () => setPrecasts([])); }, [id]);
  useEffect(loadPrecasts, [loadPrecasts]);
  const root = useRef<HTMLDivElement>(null);

  // "/" opens the creature picker (or puts the focus back in its search), unless the DM is typing somewhere.
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey || dialog) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
      if (!root.current) return;
      e.preventDefault();
      setTool((old) => (old?.kind === "picker" ? old : { kind: "picker" }));
      root.current.querySelector<HTMLInputElement>(".enc-search")?.focus();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [dialog]);

  const load = useCallback(async () => {
    try {
      setEnc(await loadEncounter(id));
      setError("");
    } catch (e) { setError(errorMessage(e)); }
  }, [id]);
  useEffect(() => { void load(); }, [load]);
  // Another device of the DM's changed it: read it again.
  useEffect(() => watchEncounters(partyId, (changed) => { if (changed === null || changed === id) void load(); }), [partyId, id, load]);

  /** A write that answers the new encounter. */
  const act = useCallback(async (work: () => Promise<Encounter>, done?: string) => {
    try {
      setEnc(await work());
      if (done) toast.push(done, "info");
    } catch (e) {
      toast.push(errorMessage(e), "error");
      void load();
    }
  }, [toast, load]);
  const patch = (fields: EncounterFields, done?: string) => act(() => updateEncounter(id, fields), done);

  if (enc === null) {
    return (
      <div className="party enc-page">
        <Alert tone="info">This encounter isn't there any more.</Alert>
        <Link className="btn" to={encountersPath(partyId)}>Encounters</Link>
      </div>
    );
  }
  if (!enc) return <div className="party enc-page">{error ? <Alert tone="danger">{error}</Alert> : <p className="muted">Loading…</p>}</div>;

  const creatures = enc.creatures;
  const groups = nested(creatures);
  const byId = (cid: string | null) => creatures.find((c) => c.id === cid);
  const named = (cid: string | null) => byId(cid)?.dmLabel ?? "a creature that's gone";
  const room = LIMITS.creatures - creatures.length;
  const shownCount = creatures.filter((c) => !c.hidden).length;
  const showToPlayers = async (on: boolean) => {
    if (on) {
      const hidden = creatures.length - shownCount;
      const answer = await confirm({
        title: `Show "${enc.publicName}" to players?`, confirmLabel: "Show it",
        body: `Players will see "Next: ${enc.publicName}" and the stat blocks of the creatures you revealed${hidden ? `. ${hidden} hidden ${hidden === 1 ? "creature stays" : "creatures stay"} hidden` : ""}.`,
      });
      if (answer === "confirm") await patch({ status: enc.status === "draft" ? "ready" : undefined, playerVisible: true }, "Shown to players.");
    } else {
      await patch({ playerVisible: false }, "Hidden from players.");
    }
  };
  const destroy = async () => {
    const answer = await confirm({ title: `Delete ${enc.name}?`, danger: true, confirmLabel: "Delete", body: "Its creatures, effects and rules go with it. This can't be undone." });
    if (answer !== "confirm") return;
    try {
      await deleteEncounter(enc.id);
      toast.push(`${enc.name} was deleted.`, "info");
      void navigate(encountersPath(partyId));
    } catch (e) { toast.push(errorMessage(e), "error"); }
  };
  const refresh = async (c: Creature) => {
    try {
      const r = await refreshCreature(enc.id, c.id);
      if (r.applied) {
        setEnc(r.encounter);
        toast.push(r.changed ? `${c.dmLabel}: the stat block was refreshed.` : `${c.dmLabel}: the source hasn't changed.`, "info");
      } else setDialog({ kind: "refresh", creature: c, conflicts: r.conflicts });
    } catch (e) { toast.push(errorMessage(e), "error"); }
  };
  const remove = async (c: Creature) => {
    const pets = creatures.filter((x) => x.ownerId === c.id).length;
    const answer = await confirm({
      title: `Remove ${c.dmLabel}?`, danger: true, confirmLabel: "Remove",
      body: `Its starting effects go with it${pets ? `, and so ${pets === 1 ? "does the creature it controls" : `do the ${pets} creatures it controls`}` : ""}.`,
    });
    if (answer === "confirm") await act(() => removeCreature(enc.id, c.id), `${c.dmLabel} was removed.`);
  };
  const activeRules = enc.rules.filter((r) => r.active);

  const statusChip = <Chip tone={enc.status === "draft" ? "default" : enc.status === "live" ? "danger" : "accent"}>{STATUS_WORD[enc.status]}</Chip>;
  const openPicker = () => {
    setTool({ kind: "picker" });
    root.current?.querySelector<HTMLInputElement>(".enc-search")?.focus();
  };

  const header = (
    <div className="party-line enc-head">
      {!wide && <Link className="btn btn-sm" to={encountersPath(partyId)} aria-label="Back to encounters">Encounters</Link>}
      <h2 className="enc-title">{enc.name}</h2>
      {statusChip}
      {enc.visibleToPlayers && <Chip tone="gold" title="Players see this encounter">Shown</Chip>}
      <span className="grow" />
      {/* Actions on the whole encounter: part E's Simulate and part C's assistant go here too. */}
      <div className="enc-head-actions">
        <AssistantButton purpose="encounter" context={{ partyId, encounterId: enc.id, label: enc.name }} onChanged={() => void load()} />
        {wide && enc.warnings.length > 0 && (
          <Button small icon="info" onClick={() => setDialog({ kind: "warnings" })} aria-label={`Edition notes (${enc.warnings.length})`}>Edition notes ({enc.warnings.length})</Button>
        )}
      </div>
    </div>
  );

  const banners = activeRules.map((r) => (
    <div key={r.id} className="enc-banner" role="note" aria-label={r.name}>
      <strong>{r.name}</strong>
      {r.text && <span>{r.text}</span>}
      <span className="muted party-note">{r.endsWhen === "creature_down" && r.creatureId ? `Ends when ${named(r.creatureId)} drops.` : "Ends when you end it."}</span>
    </div>
  ));

  const players = (
    <Card title="Players and combat" className="enc-card enc-players">
      <div className="enc-field">
        <span className="enc-field-text">
          <strong>Shown to players</strong>
          <span className="muted party-note">{enc.visibleToPlayers ? `They see "Next: ${enc.publicName}" and the creatures you revealed.` : "Players see nothing of this encounter."}</span>
        </span>
        <ToggleSwitch checked={enc.visibleToPlayers} label="Shown to players" onLabel="Shown" offLabel="Hidden"
          disabled={enc.status === "ended"} onChange={(on) => void showToPlayers(on)} />
      </div>
      <div className="enc-field">
        <span className="enc-field-text">
          <strong>Name players see</strong>
          <span className="muted party-note">"{enc.publicName}"</span>
        </span>
        <Button small icon="pen" onClick={() => setDialog({ kind: "details" })}>Names</Button>
      </div>
      <div className="enc-combat">
        {enc.status === "live" ? <LiveCombatLink partyId={partyId} /> : <StartCombatButton encounterId={enc.id} disabled={enc.status !== "ready"} />}
        {(enc.status === "draft" || enc.status === "ended") && (
          <span className="muted party-note">{enc.status === "draft" ? "Set the status to Ready to start its combat." : "This encounter has ended."}</span>
        )}
      </div>
    </Card>
  );

  const setup = (
    <Card title="Setup" className="enc-card">
      <p className="muted party-note enc-setting-row">Rules: the party's edition ({EDITION_WORD[enc.ruleset]}).</p>
      <div className="enc-setting-row">
        <span className="enc-setting-label">Monster HP</span>
        <div className="segmented enc-segmented" role="radiogroup" aria-label="Monster hit points">
          <button type="button" role="radio" aria-checked={enc.hpMode === "average"} onClick={() => void patch({ hpMode: "average" })}>Average</button>
          <button type="button" role="radio" aria-checked={enc.hpMode === "rolled"} onClick={() => void patch({ hpMode: "rolled" })}>Roll at the start</button>
        </div>
      </div>
      <div className="enc-setting-row">
        <span className="enc-setting-label">Status</span>
        <div className="segmented enc-segmented" role="radiogroup" aria-label="Status">
          {(["draft", "ready", "ended"] as const).map((s) => (
            <button key={s} type="button" role="radio" aria-checked={enc.status === s} disabled={enc.status === "live"} onClick={() => void patch({ status: s })}>{STATUS_WORD[s]}</button>
          ))}
        </div>
      </div>
    </Card>
  );

  const creatureCard = (
    <Card title={`Creatures (${creatures.length})`} aria-label="Creatures" className="enc-card enc-creatures-card"
      actions={<>
        {wide && creatures.length > 0 && <span className="muted party-note">{shownCount === 0 ? "none revealed" : `${shownCount} revealed`}</span>}
        {room > 0 && <Button small icon="plus" variant="primary" onClick={openPicker} title={wide ? "Add creatures (/)" : undefined}>Add creatures</Button>}
      </>}>
      {creatures.length === 0
        ? <EmptyState title="No creatures yet">Add monsters or companions from your content. They start hidden from players.{wide ? " Press / to search." : ""}</EmptyState>
        : (
          <ul className="party-list enc-creatures">
            {groups.flatMap(({ creature, pets }) => [
              <CreatureRow key={creature.id} c={creature} encId={enc.id} ruleset={enc.ruleset} act={act} open={setDialog} />,
              ...pets.map((p) => <CreatureRow key={p.id} c={p} owner={creature} encId={enc.id} ruleset={enc.ruleset} act={act} open={setDialog} />),
            ])}
          </ul>
        )}
    </Card>
  );

  const effects = (
    <BeforeFightCard creatures={creatures} precasts={precasts} effects={enc.effects} named={(cid) => named(cid ?? "")}
      canAddEffect={enc.effects.length < LIMITS.effects}
      onPrecast={() => setDialog({ kind: "precast" })} onEditPrecast={(pc) => setDialog({ kind: "precast", precast: pc })}
      onRemovePrecast={(pc) => { void deletePrecast(enc.id, pc.id).then(loadPrecasts, (e: unknown) => toast.push(errorMessage(e), "error")); }}
      onCustom={() => setDialog({ kind: "effect" })} onEditEffect={(e) => setDialog({ kind: "effect", effect: e })}
      onRemoveEffect={(e) => void act(() => removeEffect(enc.id, e.id))} />
  );

  const rules = (
    <Card title="Encounter rules" aria-label="Encounter rules" className="enc-card"
      actions={enc.rules.length < LIMITS.rules ? <Button small icon="plus" onClick={() => setDialog({ kind: "rule" })}>Add</Button> : undefined}>
      {enc.rules.length === 0
        ? <p className="muted party-note">A note that holds while a creature stands, shown to you as a banner. Only you see it.</p>
        : (
          <ul className="party-list">
            {enc.rules.map((r) => (
              <li key={r.id} className="party-line enc-item">
                <span className="grow">
                  <strong>{r.name}</strong>
                  <span className="muted party-note">{r.creatureId ? `Tied to ${named(r.creatureId)}` : "Not tied to a creature"}{r.flags.length ? ` · switches ${r.flags.length}` : ""}</span>
                </span>
                <ToggleSwitch checked={r.active} label={`${r.name} active`} onChange={(on) => void act(() => updateRule(enc.id, r.id, { active: on }))} />
                <Button small onClick={() => setDialog({ kind: "rule", rule: r })}>Edit</Button>
                <IconButton icon="trash" label={`Remove ${r.name}`} onClick={() => void act(() => removeRule(enc.id, r.id))} />
              </li>
            ))}
          </ul>
        )}
    </Card>
  );

  const notesCard = (
    <Card title="Notes (only you see them)" className="enc-card enc-notes-card">
      <textarea className="input enc-notes" aria-label="Notes" rows={wide ? 6 : 3} maxLength={LIMITS.notes} value={notes ?? enc.dmNotes}
        onChange={(e) => setNotes(e.target.value)}
        onBlur={() => { if (notes !== null && notes !== enc.dmNotes) void patch({ dmNotes: notes }).then(() => setNotes(null)); }} />
    </Card>
  );

  const danger = (
    <div className="party-line enc-danger">
      <span className="grow" />
      <Button small variant="danger" icon="trash" onClick={() => void destroy()}>Delete encounter</Button>
    </div>
  );

  const picker = tool?.kind === "picker" && (
    <EncounterPicker key={tool.owner?.id ?? "-"} ruleset={enc.ruleset} room={room} owner={tool.owner?.dmLabel} onClose={() => setTool(null)}
      onAdd={async (picked) => {
        let latest: Encounter | null = null;
        for (const p of picked) {
          latest = await addCreatures(enc.id, { elementId: p.monster.id, count: p.count, ...(tool.owner ? { ownerId: tool.owner.id } : {}) });
        }
        if (latest) setEnc(latest);
        const n = picked.reduce((s, p) => s + p.count, 0);
        // A big screen's docked picker says so itself ("Added …"), and stays open for the next.
        if (!wide) toast.push(`${n === 1 ? "1 creature" : `${n} creatures`} added, hidden from players.`, "info");
      }} />
  );

  return (
    <div ref={root} className={`party enc-page enc-builder ${wide ? "is-wide" : ""} ${tool ? "has-tool" : ""}`}>
      {header}
      {error && <Alert tone="danger">{error}</Alert>}
      {banners}
      {wide ? (
        <div className="enc-cols">
          <div className="enc-col-main">
            {creatureCard}
            <div className="enc-panels">{effects}{rules}{notesCard}</div>
          </div>
          {picker}
          <div className="enc-col-side">{players}{setup}{danger}</div>
        </div>
      ) : (
        <>
          {/* The creatures first: they are the work; the bar below keeps Add in reach. */}
          {creatureCard}
          {players}
          {setup}
          {effects}
          {rules}
          {notesCard}
          {danger}
          <div className="enc-bar" role="status" aria-label="Encounter summary">
            <span className="grow enc-bar-text">
              <strong>{creatures.length === 1 ? "1 creature" : `${creatures.length} creatures`}</strong>
              <span className="muted party-note">{shownCount === 0 ? "none revealed" : `${shownCount} revealed`}</span>
            </span>
            {enc.warnings.length > 0 && (
              <Button small onClick={() => setDialog({ kind: "warnings" })} aria-label={`Edition notes (${enc.warnings.length})`}>Notes ({enc.warnings.length})</Button>
            )}
            {room > 0 && <Button small icon="plus" variant="primary" onClick={openPicker}>Add</Button>}
          </div>
          {picker}
        </>
      )}

      {dialog?.kind === "details" && (
        <DetailsDialog enc={enc} onClose={() => setDialog(null)} onSave={async (f) => setEnc(await updateEncounter(enc.id, f))} />
      )}
      {dialog?.kind === "creature" && (
        <CreatureDialog creature={dialog.creature} owner={byId(dialog.creature.ownerId)?.dmLabel} onClose={() => setDialog(null)}
          onSave={async (f) => setEnc(await updateCreature(enc.id, dialog.creature.id, f))} />
      )}
      {dialog?.kind === "refresh" && (
        <RefreshDialog creature={dialog.creature} conflicts={dialog.conflicts} onClose={() => setDialog(null)}
          onApply={async (choices) => {
            const r = await refreshCreature(enc.id, dialog.creature.id, choices);
            setEnc(r.encounter);
            if (!r.applied) throw new Error("The source changed again. Try once more.");
          }} />
      )}
      {dialog?.kind === "effect" && (
        <EffectDialog creatures={creatures} effect={dialog.effect} onClose={() => setDialog(null)}
          onSave={async (f) => setEnc(await (dialog.effect ? updateEffect(enc.id, dialog.effect.id, f) : addEffect(enc.id, f)))} />
      )}
      {dialog?.kind === "precast" && (
        <PrecastDialog creatures={creatures} precasts={precasts} precast={dialog.precast} onClose={() => setDialog(null)}
          onSave={async (f) => { await (dialog.precast ? updatePrecast(enc.id, dialog.precast.id, f) : addPrecast(enc.id, f)); loadPrecasts(); }} />
      )}
      {dialog?.kind === "rule" && (
        <RuleDialog creatures={creatures} rule={dialog.rule} onClose={() => setDialog(null)}
          onSave={async (f) => setEnc(await (dialog.rule ? updateRule(enc.id, dialog.rule.id, f) : addRule(enc.id, f)))} />
      )}
      {dialog?.kind === "warnings" && <WarningsDialog warnings={enc.warnings} onClose={() => setDialog(null)} />}
      {dialog?.kind === "block" && (
        <Modal title={dialog.creature.dmLabel} onClose={() => setDialog(null)} footer={<Button onClick={() => setDialog(null)}>Close</Button>}>
          <EncounterStatBlock title={dialog.creature.name} block={dialog.creature.statBlock} />
        </Modal>
      )}
      {dialog?.kind === "more" && (() => {
        const c = dialog.creature;
        const swap = c.ruleset !== enc.ruleset && c.mapsTo;
        const close = () => setDialog(null);
        const index = creatures.findIndex((x) => x.id === c.id);
        return (
          <Modal title={c.dmLabel} onClose={close} footer={<Button onClick={close}>Close</Button>}>
            <div className="enc-menu">
              <Button onClick={() => setDialog({ kind: "block", creature: c })}>Stat block</Button>
              {!c.ownerId && room > 0 && <Button icon="plus" onClick={() => { close(); setTool({ kind: "picker", owner: c }); }}>Add a pet or summon</Button>}
              {c.ownerId && <Button onClick={() => { close(); void act(() => updateCreature(enc.id, c.id, { ownerId: null }), `${c.dmLabel} acts on its own now.`); }}>Not controlled any more</Button>}
              <Button onClick={() => { close(); void act(() => updateCreature(enc.id, c.id, { sort: Math.max(0, index - 1) })); }} disabled={index === 0}>Move up</Button>
              <Button onClick={() => { close(); void refresh(c); }}>Refresh stat block</Button>
              {swap && <Button onClick={() => { close(); void act(() => swapCreature(enc.id, c.id), `${c.dmLabel} now uses the ${EDITION_WORD[enc.ruleset]} stat block.`); }}>Use the {EDITION_WORD[enc.ruleset]} stat block</Button>}
              <Button variant="danger" onClick={() => { close(); void remove(c); }}>Remove</Button>
            </div>
          </Modal>
        );
      })()}
    </div>
  );
}
