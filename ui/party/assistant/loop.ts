import {
  deviceChat, runTool, serverChat,
  type Purpose, type Reply, type SavedProvider, type ToolCall, type ToolInfo, type ToolOutcome, type Toolbox, type Turn,
} from "../api/overrides/assistant";

/**
 * The assistant's tool loop: ask the model, show every tool call it makes, ask the person before any call that writes,
 * run the call on the server, and give the model the result, until it answers without tools.
 *
 * Safety:
 * - only tools in the server's list for this account and purpose run; any other name is answered "no such tool" and
 *   never sent anywhere;
 * - a tool that writes (a draft) runs only after the person confirms it in an app dialog, unless they turned on
 *   auto-apply (the server checks the same and refuses otherwise). There are no combat or publishing tools (D25, D47);
 * - tool results and the model's text are data: results go back to the model inside a note saying so, and the panel
 *   shows the text as plain text;
 * - at most {@link MAX_STEPS} model turns and {@link MAX_CALLS} tool calls per turn.
 */

export const MAX_STEPS = 8;
export const MAX_CALLS = 8;

export type ToolStatus = "running" | "done" | "failed" | "declined" | "refused";

/** One tool call as the panel shows it. */
export interface ToolEntry {
  id: string;
  name: string;
  args: Record<string, unknown>;
  effect: ToolInfo["effect"] | "unknown";
  status: ToolStatus;
  /** The refusal or failure, in words. */
  message?: string;
  /** What it returned, for the panel (a simulation's play-by-play, an index to download). */
  result?: unknown;
}

export interface LoopHooks {
  /** A turn to show. */
  onTurn: (turn: Turn) => void;
  /** A tool call started or changed. */
  onTool: (entry: ToolEntry) => void;
  /** Ask the person before a call that writes; true runs it. */
  confirm: (call: ToolCall, tool: ToolInfo) => Promise<boolean>;
  /** Stop between steps. */
  stopped?: () => boolean;
}

export interface LoopInput {
  purpose: Purpose;
  saved: SavedProvider;
  toolbox: Toolbox;
  /** The conversation so far, ending with the person's new message. */
  turns: Turn[];
}

/** Whether `tool` needs the person's yes first. */
export const needsConfirm = (tool: ToolInfo, autoApply: boolean): boolean => tool.effect !== "read" && !autoApply;

/** A result as the model gets it: file bytes stay with the person (they're offered as a download, not sent on). */
export function forModel(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(forModel);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.entries(value as Record<string, unknown>).map(([k, v]) =>
    [k, k === "contentBase64" || (k === "content" && typeof v === "string" && v.length > 2000)
      ? `(${typeof v === "string" ? v.length : 0} characters: the file is offered to the person to download)`
      : forModel(v)]));
}

function ask(input: LoopInput, turns: Turn[]): Promise<Reply> {
  return input.saved.mode === "server"
    ? serverChat(input.purpose, turns)
    : deviceChat(input.saved, input.toolbox.system, input.toolbox.tools, turns);
}

const refusal = (code: string, message: string): ToolOutcome => ({ ok: false, error: { code, message } });

/** Run the loop; answers the whole conversation and whether it stopped at the step limit. */
export async function runAssistant(input: LoopInput, hooks: LoopHooks): Promise<{ turns: Turn[]; limited: boolean }> {
  const turns = [...input.turns];
  for (let step = 0; step < MAX_STEPS; step++) {
    if (hooks.stopped?.()) return { turns, limited: false };
    const reply = await ask(input, turns);
    const calls = reply.toolCalls.slice(0, MAX_CALLS);
    const said: Turn = { role: "assistant", text: reply.text, toolCalls: calls };
    turns.push(said);
    hooks.onTurn(said);
    if (calls.length === 0) return { turns, limited: false };
    for (const c of calls) {
      const tool = input.toolbox.tools.find((t) => t.name === c.name);
      const entry: ToolEntry = { id: c.id, name: c.name, args: c.args ?? {}, effect: tool?.effect ?? "unknown", status: "running" };
      hooks.onTool(entry);
      let outcome: ToolOutcome;
      if (!tool) {
        outcome = refusal("unknown_tool", "There is no such tool.");
        hooks.onTool({ ...entry, status: "refused", message: "Not one of the assistant's tools." });
      } else if (needsConfirm(tool, input.toolbox.autoApply) && !(await hooks.confirm(c, tool))) {
        outcome = refusal("declined", "The person declined this change. Don't try it again unless they ask.");
        hooks.onTool({ ...entry, status: "declined" });
      } else {
        try {
          outcome = await runTool(c.name, entry.args, tool.effect !== "read", c.id);
          hooks.onTool(outcome.ok ? { ...entry, status: "done", result: outcome.result } : { ...entry, status: "failed", message: outcome.error.message });
        } catch (e) {
          const message = (e as { message?: string }).message ?? "The tool failed.";
          outcome = refusal((e as { code?: string }).code ?? "failed", message);
          hooks.onTool({ ...entry, status: "refused", message });
        }
      }
      const result: Turn = { role: "tool", toolCallId: c.id, name: c.name, result: forModel(outcome) };
      turns.push(result);
      hooks.onTurn(result);
    }
  }
  return { turns, limited: true };
}
