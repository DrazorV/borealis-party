/**
 * Which pending cast's resolution sheet is open on THIS device (0.3.2). Kept outside React so it survives the
 * tracker page being mounted again (the phone shell does that), and so only the device that tapped Cast opens the
 * sheet by itself; another DM tab sees the "1 pending" badge instead.
 */
import type { PendingCast } from "./spellTypes";

let want: { caster: string; at: number } | null = null;
let open: string | null = null;
/** Who cast the open one, and when the DM tapped Cast: its server copy may come under another id. */
let openFor: { caster: string; at: number } | null = null;
const seen = new Set<string>();
const WANT_MS = 60_000;

/** The DM tapped Cast (or Use, or Apply zone) for `caster`: its next new pending cast opens. */
export function expectCast(caster: string): void {
  want = { caster, at: Date.now() };
}

/** The sheet to show for these open casts: the one already open, or a new one this device asked for. */
export function castToShow(casts: PendingCast[], now = Date.now()): string | null {
  // The open cast went away. Either it was applied or cancelled, or it was the copy the tracker shows at once and
  // the server's answer brings the same cast under the server's own id (the server picks every cast id, spells-api.md):
  // then that one opens instead, still within WANT_MS of the tap.
  if (open && !casts.some((k) => k.id === open)) {
    if (openFor && !want) want = openFor;
    open = null;
    openFor = null;
  }
  for (const k of casts) {
    if (seen.has(k.id)) continue;
    seen.add(k.id);
    if (want && k.caster === want.caster && now - want.at < WANT_MS) { open = k.id; openFor = want; want = null; }
  }
  return open;
}

export function showCast(id: string | null): void {
  open = id;
  openFor = null;
}

/** Tests start clean. */
export function resetCastFocus(): void {
  want = null;
  open = null;
  openFor = null;
  seen.clear();
}
