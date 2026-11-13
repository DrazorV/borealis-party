import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { Button } from "../../src/components/Button";
import { useConfirm } from "../../src/components/Confirm";
import { Dialog } from "../../src/components/controls/Dialog";
import { useToast } from "../../src/components/Toast";
import { usePhone } from "../../src/lib/phone";
import { isTauri } from "../../src/lib/tauri";
import { errorMessage } from "../../src/state/useAsync";
import { addCreature, combatPath, exportCombat, importCombat } from "../api/overrides/combat";
import { CombatantCard } from "../combat/dm/CombatantCard";
import { byId, isCharacter, ownerDefeated, rosterOrder, takesTurn, turnOrder } from "../combat/dm/derive";
import { AddCreatureDialog } from "../combat/dm/dialogs";
import { EventLog } from "../combat/dm/EventLog";
import { upNext } from "../combat/dm/mockEngine";
import { ResourcePanel, RuleBanners } from "../combat/dm/ResourcePanel";
import { Roster } from "../combat/dm/Roster";
import type { Combatant, Command, CombatState } from "../combat/dm/types";
import { useCombat } from "../combat/dm/useCombat";
import "../combat/dm/tracker.css";

/**
 * The DM's combat tracker (plan 4.4, 4.4b C1–C27): the live combat, phone first. Desktop shows the roster, the
 * selected combatant and the resources in three columns; a tablet two; a phone stacks them with the resource panel
 * sticky on top (More opens the rest), the selected card next, the roster below, and Next turn in the thumb zone.
 * Every change is a server write (`useCombat`); the screen shows the server's state, and keeps the last one through
 * a short loss of signal (D44).
 */
export function CombatTracker() {
  const { id = "" } = useParams();
  const combat = useCombat(id);
  const { view, connection, held, error, send, streamDown } = combat;
  const toast = useToast();
  const confirm = useConfirm();
  const nav = useNavigate();
  const phone = usePhone();
  const [selected, setSelected] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"add" | "export" | "import" | null>(null);
  const [exported, setExported] = useState("");
  const [logOpen, setLogOpen] = useState(false);
  /** Controlled creatures the DM chose to keep after a defeated owner (asked once each). */
  const [kept, setKept] = useState<Set<string>>(() => new Set());
  const cardRef = useRef<HTMLDivElement>(null);
  const lastTurn = useRef<string | undefined>(undefined);
  const state = view?.state;

  // C4/C26: a new turn selects its combatant; on a phone the card scrolls into view.
  useEffect(() => {
    if (!state) return;
    if (state.current && state.current !== lastTurn.current) {
      const first = lastTurn.current === undefined;
      lastTurn.current = state.current;
      setSelected(state.current);
      if (!first && phone) requestAnimationFrame(() => cardRef.current?.scrollIntoView?.({ behavior: "smooth", block: "start" }));
    }
    if (selected === null || !byId(state, selected) || byId(state, selected)?.removed) {
      setSelected(state.current ?? rosterOrder(state)[0]?.id ?? null);
    }
  }, [state, selected, phone]);

  if (error && !view) {
    return (
      <div className="cb-page"><div className="cb-panel cb-pad" role="alert">
        <h2>This combat can't be opened</h2><p>{error}</p>
        <Button onClick={() => nav("/parties")}>Back to parties</Button>
      </div></div>
    );
  }
  if (!view || !state) return <div className="cb-page"><p className="muted cb-pad">Loading the combat…</p></div>;

  const disabled = !combat.accepting || state.status === "ended";
  const current = byId(state, state.current);
  const chosen = byId(state, selected ?? undefined);
  const next = state.status === "active" ? upNext(state) : undefined;
  const ordered = turnOrder(state).map((x) => byId(state, x)!).filter(takesTurn);

  const orphan = ownerDefeated(state).find((c) => !kept.has(c.id));
  const orphanOwner = orphan ? byId(state, orphan.owner_id) : undefined;
  const keep = (id: string) => setKept((k) => new Set(k).add(id));

  const step = () => send({ command: state.status === "setup" ? "start" : "next_turn" });
  const rollAll = async () => {
    if (state.status === "active") {
      const r = await confirm({ title: "Reroll initiative?", body: "Every creature rolls again, during the fight. Players' initiatives stay.", confirmLabel: "Reroll" });
      if (r !== "confirm") return;
      send({ command: "roll_initiative" });
    } else send({ command: "roll_initiative", only_missing: false });
  };
  const undo = async () => {
    const out = await combat.client.undo(false);
    if (out === "needs_force") {
      const r = await confirm({ title: "Undo a player's change?", body: `The last change was a player's (${view.undo[0] ?? "a write"}). Undo it anyway?`, confirmLabel: "Undo it" });
      if (r === "confirm") await combat.client.undo(true);
    } else if (out === "done") toast.push("Undone.");
  };
  const end = async () => {
    const r = await confirm({ title: "End this combat?", body: "Players' Combat tab closes. The log stays for 90 days. A new fight needs a new combat.", confirmLabel: "End combat", danger: true });
    if (r === "confirm") send({ command: "end" });
  };
  const doExport = async () => {
    try {
      const text = await exportCombat(view.id);
      setExported(text);
      setDialog("export");
    } catch (e) { toast.push(errorMessage(e), "error"); }
  };
  const add = async (r: { from: Combatant; label: string; initiative: string; hidden: boolean }) => {
    const before = new Set(state.combatants.map((c) => c.id));
    const typed = r.initiative !== "" && Number.isInteger(Number(r.initiative));
    try {
      const next = await addCreature(view.id, { copyOf: r.from.id, count: 1, rollInitiative: !typed, hidden: r.hidden });
      combat.client.adopt(next);
      const made = next.state.combatants.find((c) => !before.has(c.id));
      if (!made) return;
      if (r.label) send({ command: "rename", id: made.id, label: r.label });
      if (typed) send({ command: "set_initiative", id: made.id, roll: { kind: "typed", total: Number(r.initiative) } });
      setSelected(made.id);
    } catch (e) { toast.push(errorMessage(e), "error"); }
  };

  return (
    <div className={`cb-page${phone ? " phone" : ""}`}>
      <header className="cb-top">
        <div className="cb-brand">
          <div className="cb-overline">Combat · DM</div>
          <h1>{state.status === "setup" ? "Ready to start" : state.status === "ended" ? "Combat ended" : current ? `${current.label}'s turn` : "Combat"}</h1>
          {next && <div className="cb-small muted">Up next: {next.label}</div>}
        </div>
        <div className="cb-top-actions">
          {streamDown && connection === "live" && <span className="chip warning cb-reconnecting" role="status">Reconnecting…</span>}
          {combat.queue.length > 0 && connection === "sending" && <span className="chip cb-saving" role="status" aria-label="Saving">Saving…</span>}
          <span className="cb-round" aria-label={`Round ${state.round}`}>ROUND {state.round || "–"}</span>
          {state.status !== "ended" && <Button variant="primary" className="cb-next" disabled={disabled || ordered.length === 0} onClick={step}>
            ▶ {state.status === "setup" ? "Start combat" : "Next turn"}</Button>}
          {state.status === "active" && <Button small disabled={disabled} onClick={() => send({ command: "previous_turn" })}>◀ Back</Button>}
          <Button small disabled={view.undo.length === 0 || connection !== "live" || state.status === "ended"} onClick={() => void undo()} title={view.undo[0] ? `Undo: ${view.undo[0]}` : "Nothing to undo"}>↶ Undo</Button>
          <Button small variant="ghost" disabled={disabled} onClick={() => void rollAll()}>Roll initiatives</Button>
        </div>
      </header>

      <ConnectionBanner connection={connection} queued={combat.queue.length} />
      {held.length > 0 && (
        <Dialog title="Changes made while offline" onClose={() => combat.client.resolveHeld(false)}
          footer={<><Button onClick={() => combat.client.resolveHeld(false)}>Drop them</Button><Button variant="primary" onClick={() => combat.client.resolveHeld(true)}>Send them</Button></>}>
          <p>The server is back. {held.length} change{held.length === 1 ? "" : "s"} waited while it couldn't be reached:</p>
          <ul>{held.map((q) => <li key={q.op}>{describe(state, q.command)}</li>)}</ul>
          <p className="muted cb-small">Send them only if they still fit what happened at the table.</p>
        </Dialog>
      )}

      {orphan && orphanOwner && held.length === 0 && (
        <Dialog title={`${orphanOwner.label} is down`} onClose={() => keep(orphan.id)}
          footer={<>
            <Button onClick={() => keep(orphan.id)}>Keep it after {orphanOwner.label}</Button>
            <Button onClick={() => { keep(orphan.id); send({ command: "set_initiative_mode", id: orphan.id, mode: "own" }); }}>Its own initiative</Button>
            <Button variant="danger" onClick={() => { keep(orphan.id); send({ command: "dismiss", id: orphan.id }); }}>Dismiss it</Button>
          </>}>
          <p>What happens to {orphan.label}, which acts right after {orphanOwner.label}?</p>
          <p className="cb-small muted">Its own initiative is rolled in the app; type a table roll in the roster afterwards if you rolled at the table.</p>
        </Dialog>
      )}

      <RuleBanners state={state} send={send} disabled={disabled} />

      {state.status === "setup" && <SetupHint state={state} />}

      <div className="cb-layout">
        <div className="cb-left">
          <Roster state={state} selected={selected} onSelect={(x) => { setSelected(x); if (phone) requestAnimationFrame(() => cardRef.current?.scrollIntoView?.({ behavior: "smooth", block: "start" })); }}
            send={send} disabled={disabled} onAdd={() => setDialog("add")} />
          <details className="cb-panel cb-tools" onToggle={(e) => setLogOpen((e.currentTarget as HTMLDetailsElement).open)}>
            <summary>Log, export and import</summary>
            <div className="cb-pad">
              <div className="cb-tool-row">
                <Button small onClick={() => void doExport()}>Export combat</Button>
                <Button small onClick={() => setDialog("import")}>Import combat</Button>
                {state.status !== "ended" && <Button small variant="danger" disabled={disabled} onClick={() => void end()}>End combat</Button>}
              </div>
              <div className="cb-tool-row">
                <label className="cb-check"><input type="checkbox" disabled={disabled} checked={state.settings?.auto_recharge !== false}
                  onChange={(e) => send({ command: "set_settings", settings: { show_enemy_turn: Boolean(state.settings?.show_enemy_turn), auto_recharge: e.target.checked } })} />
                  Roll recharges in the app at turn start (off: you type the d6)</label>
                <label className="cb-check"><input type="checkbox" disabled={disabled} checked={Boolean(state.settings?.show_enemy_turn)}
                  onChange={(e) => send({ command: "set_settings", settings: { auto_recharge: state.settings?.auto_recharge !== false, show_enemy_turn: e.target.checked } })} />
                  Show players an "Enemy turn" while hidden creatures act (tells them hidden enemies exist)</label>
              </div>
              {logOpen && <EventLog combat={view.id} seq={view.seq} />}
            </div>
          </details>
        </div>
        <div className="cb-main" ref={cardRef}>
          {chosen ? <CombatantCard state={state} c={chosen} block={view.creatures?.[chosen.id]?.statBlock} blockName={view.creatures?.[chosen.id]?.name ?? undefined} send={send} disabled={disabled} />
            : <div className="cb-panel cb-pad muted">Pick a combatant.</div>}
        </div>
        <div className="cb-right">
          <ResourcePanel state={state} send={send} disabled={disabled} onSelect={setSelected} />
        </div>
      </div>

      {phone && state.status !== "ended" && (
        <div className="cb-thumb" role="toolbar" aria-label="Turn">
          <span className="cb-round">R{state.round || "–"}</span>
          <span className="cb-thumb-who">{current ? current.label : "Not started"}</span>
          <Button variant="primary" disabled={disabled || ordered.length === 0} onClick={step}>▶ {state.status === "setup" ? "Start" : "Next turn"}</Button>
        </div>
      )}

      {dialog === "add" && <AddCreatureDialog creatures={state.combatants.filter((c) => !isCharacter(c) && !c.removed)} onDone={(r) => { setDialog(null); if (r) void add(r); }} />}
      {dialog === "export" && <ExportDialog text={exported} round={state.round} onClose={() => setDialog(null)} />}
      {dialog === "import" && <ImportDialog onClose={() => setDialog(null)} onImport={async (text) => {
        try {
          const made = await importCombat(view.partyId, text);
          setDialog(null);
          toast.push("Imported as a new combat.");
          nav(combatPath(made.id));
        } catch (e) { toast.push(errorMessage(e), "error"); }
      }} />}
    </div>
  );
}

function describe(state: CombatState, cmd: Command): string {
  const who = (id?: string) => byId(state, id)?.label ?? "someone";
  switch (cmd.command) {
    case "damage": return `Damage to ${who(cmd.target)}`;
    case "heal": return `Healing for ${who(cmd.target)}`;
    case "next_turn": return "Next turn";
    case "use_ability": return `${who(cmd.id)} uses ${cmd.ability}`;
    default: return cmd.command.replace(/_/g, " ") + ("id" in cmd && typeof cmd.id === "string" ? `: ${who(cmd.id)}` : "");
  }
}

function SetupHint({ state }: { state: CombatState }) {
  const pcs = state.combatants.filter((c) => isCharacter(c) && !c.removed);
  const waiting = pcs.filter((c) => c.initiative == null && !c.absent);
  const unrolled = state.combatants.filter((c) => !isCharacter(c) && !c.removed && c.initiative == null && !c.linked_to);
  return (
    <div className="cb-hint" role="status">
      {unrolled.length > 0 ? `${unrolled.length} creature${unrolled.length === 1 ? "" : "s"} without initiative: Roll initiatives, or type the table rolls in the roster. ` : ""}
      {waiting.length > 0 ? `Waiting for ${waiting.map((c) => c.label).join(", ")} (players enter their own; blank means not in this fight). ` : ""}
      {unrolled.length === 0 && waiting.length === 0 ? "Everyone has an initiative. Start combat when ready." : ""}
    </div>
  );
}

function ConnectionBanner({ connection, queued }: { connection: string; queued: number }) {
  if (connection === "live" || connection === "sending") return null;
  return (
    <div className={`cb-conn ${connection}`} role="status">
      {connection === "reconnecting"
        ? `No signal: ${queued} change${queued === 1 ? "" : "s"} waiting, retrying. The last state stays on screen.`
        : "Offline: the server hasn't answered for a minute. This is the last state it sent; changes wait until it answers."}
    </div>
  );
}

function ExportDialog({ text, round, onClose }: { text: string; round: number; onClose: () => void }) {
  const toast = useToast();
  const download = () => {
    const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `combat-round-${round}.json`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  const copy = async () => {
    try { await navigator.clipboard.writeText(text); toast.push("Copied."); } catch { toast.push("Copying isn't allowed here: select the text and copy it.", "warning"); }
  };
  return (
    <Dialog title="Export combat" onClose={onClose} wide
      footer={<><Button onClick={onClose}>Close</Button><Button onClick={() => void copy()}>Copy</Button>{!isTauri() && <Button variant="primary" onClick={download}>Download</Button>}</>}>
      <p className="cb-small muted">The whole combat as JSON (DM only: it has hidden creatures). Import makes a new combat from it.</p>
      <textarea className="input cb-export" readOnly value={text} aria-label="Exported combat" rows={10} onFocus={(e) => e.currentTarget.select()} />
    </Dialog>
  );
}

function ImportDialog({ onClose, onImport }: { onClose: () => void; onImport: (text: string) => Promise<void> }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const read = async (file: File | undefined) => { if (file) setText(await file.text()); };
  return (
    <Dialog title="Import combat" onClose={onClose} wide
      footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" disabled={!text.trim() || busy} onClick={() => { setBusy(true); void onImport(text).finally(() => setBusy(false)); }}>Import as a new combat</Button></>}>
      <p className="cb-small muted">An exported combat becomes a new combat in this party. Nothing is overwritten.</p>
      <input type="file" accept="application/json,.json" aria-label="Combat file" onChange={(e) => void read(e.target.files?.[0])} />
      <textarea className="input cb-export" value={text} onChange={(e) => setText(e.target.value)} placeholder="Or paste the exported JSON here" aria-label="Combat JSON" rows={8} />
    </Dialog>
  );
}
