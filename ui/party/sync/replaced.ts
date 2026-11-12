import { announceBundleApplied } from "../../src/pages/transfer/openBundle";
import type { ServerEvent } from "../api/http";
import { liveEvents } from "../api/liveEvents";
import { loadSession } from "../api/session";

/**
 * A device's edit that the server took replaces the character's file and its open session there
 * (`party://characters-changed` with `replaced`). An open page re-reads it the way it does after an accepted bundle,
 * so it never shows the old copy until its next edit. A save of the page's own sends no `replaced`.
 */
const DEBOUNCE_MS = 300;
let timer: ReturnType<typeof setTimeout> | null = null;

export function rereadIfReplaced(e: ServerEvent): void {
  if (e.event !== "party://characters-changed" || !(e.payload as { replaced?: unknown } | null)?.replaced) return;
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => { timer = null; announceBundleApplied(); }, DEBOUNCE_MS);
}

/** In a browser (the apps hear it in their sync's event stream). Returns how to stop. */
export function followReplaced(): () => void {
  const session = loadSession();
  if (!session) return () => {};
  return liveEvents(session, rereadIfReplaced);
}
