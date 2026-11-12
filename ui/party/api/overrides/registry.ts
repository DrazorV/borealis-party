import type { CommandName } from "../../../src/api/contract";
import type { ServerSession } from "../session";

export type Method = (args?: Record<string, unknown>) => unknown;
/** The API's commands by name, each calling the server unless an override replaced it. */
export type Methods = Record<CommandName, Method>;
/** One feature's changes to the commands: it replaces or wraps some of `methods`. */
export type Override = (methods: Methods, session: ServerSession) => void;

/**
 * The overrides, in the order ../http.ts imports their modules, which is the order they are applied in.
 * Each module in this folder registers itself once with `override(<its file name>, ...)`.
 */
export const OVERRIDES: { name: string; apply: Override }[] = [];

export function override(name: string, apply: Override): void {
  OVERRIDES.push({ name, apply });
}
