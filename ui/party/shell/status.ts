import { useEffect, useState } from "react";
import { useApi } from "../../src/state/ApiContext";
import { useCharacter } from "../../src/state/CharacterContext";
import { getStatus, setStatus, useStatusMessage as useUpstreamMessage } from "../../src/state/status";

export { getStatus, setStatus };

/**
 * The status bar's idle line with the account's REAL last content check (0.3.3; server: content_update.rs).
 *
 * Upstream's bar (src/components/shell/Chrome.tsx, which imports this file in place of `state/status`, see
 * ui/party/vite.config.ts) shows `message`, else "Loaded '<name>'" for an open character, else "Last checked for
 * content updates at <the time the page loaded>". Here the message is the same, a loaded character still shows as
 * loaded (the message stays empty), and otherwise the line says when the server last checked this account's index
 * files and 5etools sources, automatically or by Update all content. Until that is known, the upstream line shows.
 */

/** "Last checked for content updates at 11:24", with the day when it was not today; null for no time. */
export function lastCheckedLine(iso: string | null | undefined, now: Date = new Date()): string | null {
  if (!iso) return null;
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return null;
  const time = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  const today = at.toDateString() === now.toDateString();
  const day = today ? "" : `${at.toLocaleDateString([], { day: "numeric", month: "short" })} `;
  return `Last checked for content updates at ${day}${time}`;
}

export function useStatusMessage(): string {
  const message = useUpstreamMessage();
  const { snapshot } = useCharacter();
  const api = useApi();
  const [checked, setChecked] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    const look = () => {
      void api.contentUpdateStatus().then(
        (s) => { if (live) setChecked(s?.checkedAt ?? null); },
        () => undefined,
      );
    };
    look();
    // A check that ends (the automatic one, or Update all content) sends its "done" progress.
    const off = api.subscribe("content:progress", (p) => { if (p.phase === "done") look(); });
    return () => { live = false; off(); };
  }, [api]);
  if (message || snapshot) return message;
  return lastCheckedLine(checked) ?? "";
}
