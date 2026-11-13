import { type FormEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { Button } from "../../src/components/Button";
import { Alert, Chip, type ChipTone } from "../../src/components/Card";
import { Dialog } from "../../src/components/controls/Dialog";
import { downloadBase64 } from "../../src/lib/files";
import { errorMessage } from "../../src/state/useAsync";
import { ask } from "../api/ask";
import { aiStatus, toolbox, type AiStatus, type Purpose, type ToolCall, type ToolInfo, type Toolbox, type Turn } from "../api/overrides/assistant";
import { providerName } from "../pages/YourAiCard";
import { runAssistant, type ToolEntry, type ToolStatus } from "./loop";
import { indexFiles, storedZip, toBase64 } from "./zip";
import "./assistant.css";

/**
 * The assistant panel (0.3.1 part C): an app dialog, a bottom sheet on phones, opened from the encounter builder, the
 * encounter list and Additional Content. The person's own model ("Your AI" on the Account page) does the talking; the
 * server's tools do the work, as the signed-in account.
 *
 * Every tool call shows as a card with its arguments and what became of it. A call that writes asks first in an app
 * dialog, unless it's a draft and the person turned on auto-apply. The model's text is shown as plain text, never as
 * HTML. There is no XP or difficulty anywhere (the user's D16); the server's prompt says so too.
 */

export interface AssistantContext {
  partyId?: string;
  encounterId?: string;
  /** Shown above the conversation, e.g. "Goblin ambush". */
  label?: string;
}

const PURPOSES: { value: Purpose; label: string; hint: string }[] = [
  { value: "encounter", label: "Encounters", hint: "For example: a forest ambush for my level 5 party." },
  { value: "homebrew", label: "Homebrew", hint: "For example: a magic dagger that returns when thrown." },
  { value: "index", label: "Custom index", hint: "For example: a custom index of my homebrew packs, to download." },
];

/** The canned request behind "Simulate encounter" (D26). */
export const SIMULATE = "Simulate this encounter on a copy: sim_start, then sim_auto until the end, then sim_report. Tell me what happened and what needs my ruling.";

/** The simulation tools whose results the panel shows as a play-by-play. */
const SIM_VIEWS = new Set(["sim_act", "sim_auto", "sim_report", "sim_batch", "sim_replay"]);

const TOOL_LABELS: Record<string, string> = {
  character_list: "Read your characters and parties",
  character_get: "Read a character",
  bestiary_search: "Search the bestiary",
  monster_get: "Read a stat block",
  encounter_list: "Read a party's encounters",
  encounter_get: "Read an encounter",
  encounter_create: "Create an encounter draft",
  encounter_update: "Change an encounter draft",
  encounter_add_creature: "Add creatures to an encounter",
  encounter_remove_creature: "Remove a creature from an encounter",
  sim_start: "Start a simulation (a copy)",
  sim_card: "Read a simulation's choice",
  sim_act: "Choose in a simulation",
  sim_auto: "Run the simulation",
  sim_state: "Read the simulation",
  sim_report: "Read the simulation's report",
  sim_batch: "Run many simulations",
  sim_replay: "Replay a simulation",
  homebrew_list: "Read your homebrew",
  homebrew_get: "Read a homebrew element",
  homebrew_validate: "Check a homebrew element",
  homebrew_create: "Save a homebrew element",
  homebrew_update: "Change a homebrew element",
  homebrew_delete: "Delete a homebrew element",
  index_build: "Build a custom index",
};

export const toolLabel = (name: string) => TOOL_LABELS[name] ?? name;

const STATUS: Record<ToolStatus, { label: string; tone: ChipTone }> = {
  running: { label: "Running", tone: "info" },
  done: { label: "Done", tone: "success" },
  failed: { label: "Refused", tone: "warning" },
  declined: { label: "You declined", tone: "default" },
  refused: { label: "Not run", tone: "danger" },
};

/** Arguments as readable lines, plain text. */
function argLines(args: Record<string, unknown>): string {
  return Object.entries(args)
    .map(([k, v]) => `${k}: ${typeof v === "string" ? v : JSON.stringify(v)}`)
    .join("\n") || "(no details)";
}

/** Ask before a call that writes: an app message box. */
export async function confirmCall(call: ToolCall, tool: ToolInfo): Promise<boolean> {
  const what = tool.effect === "draft" ? "This saves a draft only you see." : "";
  const id = await ask(
    `Allow: ${toolLabel(call.name)}?`,
    <div className="ai-confirm">
      <p>The assistant wants to {toolLabel(call.name).toLowerCase()}. {what}</p>
      <pre className="ai-args">{argLines(call.args)}</pre>
    </div>,
    [{ id: "apply", label: "Allow", variant: "primary" }, { id: "no", label: "Don't allow" }],
  );
  return id === "apply";
}

/** Part E's simulation outputs (plans/borealis-party-sim-design-2026-11-13.md, section 5), as far as the panel shows them. */
interface SimResult {
  /** sim_act / sim_auto: the engine's lines for what just happened. */
  lines?: unknown[];
  /** sim_report. */
  outcome?: unknown;
  rounds?: unknown;
  timeline?: unknown[];
  pcs?: unknown[];
  creatures?: unknown[];
  adjudications?: unknown[];
  steering?: unknown;
  coverage?: unknown;
  /** sim_batch: counts out of N, and notable facts. */
  runs?: unknown;
  counts?: unknown;
  notable?: unknown[];
}

const line = (v: unknown): string => (typeof v === "string" ? v : JSON.stringify(v));
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/** One PC's or creature's facts on one line: its name, then each fact. */
function facts(v: unknown): string {
  if (!v || typeof v !== "object") return line(v);
  const { name, id, ...rest } = v as Record<string, unknown>;
  const who = typeof name === "string" ? name : typeof id === "string" ? id : "?";
  const what = Object.entries(rest).map(([k, x]) => `${k} ${typeof x === "object" ? JSON.stringify(x) : String(x)}`).join(", ");
  return what ? `${who}: ${what}` : who;
}

/**
 * A simulation's play-by-play and report (D26), as plain text. Descriptive only (D16): what happened, the outcome,
 * the key moments and counts out of N; never a rating, odds or difficulty word.
 */
function SimulationView({ result }: { result: SimResult }) {
  const section = (label: string, items: unknown[], show: (v: unknown) => string = line) =>
    items.length > 0 && (
      <section aria-label={label}>
        <strong>{label}</strong>
        <ul>{items.map((x, i) => <li key={i}>{show(x)}</li>)}</ul>
      </section>
    );
  return (
    <div className="ai-sim" aria-label="Simulation">
      {section("Play-by-play", list(result.lines))}
      {result.outcome !== undefined && <p><strong>Outcome:</strong> {line(result.outcome)}{result.rounds !== undefined ? ` (rounds: ${line(result.rounds)})` : ""}</p>}
      {result.runs !== undefined && <p><strong>Runs:</strong> {line(result.runs)}</p>}
      {result.counts !== undefined && section("Out of all runs", Object.entries(result.counts as Record<string, unknown>).map(([k, v]) => `${k}: ${line(v)}`))}
      {section("Key moments", list(result.timeline))}
      {section("Characters", list(result.pcs), facts)}
      {section("Creatures", list(result.creatures), facts)}
      {section("Needs your ruling", list(result.adjudications))}
      {section("Notable", list(result.notable))}
      {result.steering !== undefined && <p className="muted">Decided by: {line(result.steering)}</p>}
    </div>
  );
}

/** A built index offered as a zip (D47: never published): index_build's files, zipped here. */
function downloadOf(name: string, result: unknown): { fileName: string; contentBase64: string } | null {
  if (name !== "index_build") return null;
  const files = indexFiles(result);
  if (files.length === 0) return null;
  const title = (result as { name?: unknown }).name;
  const stem = (typeof title === "string" && title.trim() ? title.trim() : "custom-index").replace(/[^\w .-]+/g, "_").slice(0, 60);
  return { fileName: `${stem}.zip`, contentBase64: toBase64(storedZip(files)) };
}

type Item = { kind: "turn"; turn: Turn } | { kind: "tool"; entry: ToolEntry } | { kind: "note"; text: string };

/**
 * What holds the panel: an app dialog (a bottom sheet on phones) by default. A page with its own tool column (the
 * encounter builder's `EncounterToolPanel`) passes one that renders `body` with `footer` (the message box) in it.
 */
export type AssistantFrame = (parts: { body: ReactNode; footer: ReactNode; onClose: () => void }) => ReactNode;

const dialogFrame: AssistantFrame = ({ body, footer, onClose }) => (
  <Dialog title="Assistant" onClose={onClose} wide className="ai-panel" footer={footer ?? undefined}>{body}</Dialog>
);

export function AssistantPanel({ purpose: first = "encounter", context, onClose, onChanged, frame = dialogFrame }: {
  purpose?: Purpose;
  context?: AssistantContext;
  onClose: () => void;
  /** After a tool that writes ran: the page reloads what it shows. */
  onChanged?: () => void;
  frame?: AssistantFrame;
}) {
  const navigate = useNavigate();
  const [status, setStatus] = useState<AiStatus | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [purpose, setPurpose] = useState<Purpose>(first);
  const [box, setBox] = useState<Toolbox | null>(null);
  const [items, setItems] = useState<Item[]>([]);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [text, setText] = useState("");
  const [running, setRunning] = useState(false);
  const stop = useRef(false);
  const end = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    aiStatus().then((s) => { if (live) setStatus(s); }, (e) => {
      if (!live) return;
      setProblem((e as { code?: string }).code === "not_found" ? "This server doesn't offer the assistant." : errorMessage(e));
    });
    return () => { live = false; stop.current = true; };
  }, []);
  useEffect(() => {
    if (!status?.provider) return;
    let live = true;
    setBox(null);
    toolbox(purpose).then((b) => { if (live) setBox(b); }, (e) => { if (live) setProblem(errorMessage(e)); });
    return () => { live = false; };
  }, [status, purpose]);
  useEffect(() => { end.current?.scrollIntoView?.({ block: "end" }); }, [items]);

  const saved = status?.provider ?? null;
  const switchTo = (p: Purpose) => {
    if (running || p === purpose) return;
    setPurpose(p);
    setItems([]);
    setTurns([]);
  };

  const send = async (e?: FormEvent, canned?: string) => {
    e?.preventDefault();
    const typed = (canned ?? text).trim();
    if (!typed || !saved || !box || running) return;
    // The ids the tools need, from the page the panel was opened on. App data, not the person's words.
    const where = turns.length === 0 && (context?.partyId || context?.encounterId)
      ? `\n\n(From the app: ${[context.partyId && `partyId ${context.partyId}`, context.encounterId && `encounterId ${context.encounterId}`].filter(Boolean).join(", ")})`
      : "";
    const mine: Turn = { role: "user", text: typed + where };
    setText("");
    setItems((list) => [...list, { kind: "turn", turn: { role: "user", text: typed } }]);
    setRunning(true);
    stop.current = false;
    let wrote = false;
    try {
      const out = await runAssistant({ purpose, saved, toolbox: box, turns: [...turns, mine] }, {
        onTurn: (turn) => {
          if (turn.role === "assistant" && turn.text.trim()) setItems((list) => [...list, { kind: "turn", turn }]);
        },
        onTool: (entry) => {
          if (entry.status === "done" && entry.effect !== "read") wrote = true;
          setItems((list) => {
            const at = list.findIndex((i) => i.kind === "tool" && i.entry.id === entry.id);
            if (at < 0) return [...list, { kind: "tool", entry }];
            const next = [...list];
            next[at] = { kind: "tool", entry };
            return next;
          });
        },
        confirm: confirmCall,
        stopped: () => stop.current,
      });
      setTurns(out.turns);
      if (out.limited) setItems((list) => [...list, { kind: "note", text: "The assistant stopped after many steps. Send a message to let it go on." }]);
    } catch (err) {
      setItems((list) => [...list, { kind: "note", text: errorMessage(err) }]);
    } finally {
      setRunning(false);
      if (wrote) onChanged?.();
    }
  };

  const hint = PURPOSES.find((p) => p.value === purpose)?.hint ?? "";
  return (
    frame({ onClose, footer: saved && box ? (
        <form className="ai-compose" onSubmit={(e) => void send(e)}>
          <textarea className="input" aria-label="Message to the assistant" rows={2} placeholder={hint} value={text} disabled={running}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); e.currentTarget.form?.requestSubmit(); } }} />
          {running
            ? <Button onClick={() => { stop.current = true; }}>Stop</Button>
            : <Button type="submit" variant="primary" icon="send" disabled={!text.trim()}>Send</Button>}
        </form>
      ) : null, body: <div className="ai-body">
      {problem && <Alert tone="warning">{problem}</Alert>}
      {!problem && status && !saved && (
        <div className="ai-setup">
          <p>Set up your own AI first: an OpenAI-compatible provider, Anthropic or Google Gemini, or a model on this device.</p>
          <Button variant="primary" onClick={() => { onClose(); void navigate("/account"); }}>Set up Your AI</Button>
        </div>
      )}
      {saved && (
        <>
          <div className="ai-purposes" role="group" aria-label="What the assistant helps with">
            {PURPOSES.map((p) => (
              <Button key={p.value} small aria-pressed={purpose === p.value} variant={purpose === p.value ? "primary" : "default"} disabled={running} onClick={() => switchTo(p.value)}>{p.label}</Button>
            ))}
          </div>
          <p className="muted ai-note">
            Uses {providerName(saved.provider, saved.mode, saved.baseUrl)} ({saved.model}). What you write here, and what its tools read, goes there. Changes ask you first{box?.autoApply ? ", except drafts" : ""}.
          </p>
          {context?.label && <p className="ai-context">For: <strong>{context.label}</strong></p>}
          {purpose === "encounter" && context?.encounterId && box?.tools.some((t) => t.name === "sim_start") && (
            <div className="party-inline">
              <Button small icon="dice" disabled={running} onClick={() => void send(undefined, SIMULATE)}>Simulate encounter</Button>
            </div>
          )}
          <div className="ai-log" aria-live="polite">
            {items.length === 0 && <p className="muted">{hint}</p>}
            {items.map((item, i) => {
              if (item.kind === "note") return <Alert key={i} tone="info">{item.text}</Alert>;
              if (item.kind === "tool") {
                const s = STATUS[item.entry.status];
                return (
                  <div key={item.entry.id || i} className="ai-tool-wrap">
                  <details className="ai-tool" aria-label={`Tool: ${toolLabel(item.entry.name)}`}>
                    <summary>
                      <span className="ai-tool-name">{toolLabel(item.entry.name)}</span>
                      <Chip tone={s.tone}>{s.label}</Chip>
                    </summary>
                    <pre className="ai-args">{argLines(item.entry.args)}</pre>
                    {item.entry.message && <p className="muted">{item.entry.message}</p>}
                  </details>
                  {item.entry.status === "done" && SIM_VIEWS.has(item.entry.name) && item.entry.result != null && <SimulationView result={item.entry.result as SimResult} />}
                  {item.entry.status === "done" && (() => {
                    const file = downloadOf(item.entry.name, item.entry.result);
                    return file && <Button small icon="download" onClick={() => downloadBase64(file.fileName, "application/zip", file.contentBase64)}>Download {file.fileName}</Button>;
                  })()}
                  </div>
                );
              }
              const t = item.turn;
              if (t.role === "tool") return null;
              return <div key={i} className={`ai-msg ai-${t.role}`}>{t.text}</div>;
            })}
            {running && <p className="muted" role="status">Working…</p>}
            <div ref={end} />
          </div>
        </>
      )}
    </div> })
  );
}
