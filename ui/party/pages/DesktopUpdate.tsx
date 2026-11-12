import { useEffect, useState } from "react";
import { Button } from "../../src/components/Button";
import { errorMessage } from "../../src/state/useAsync";
import { checkForUpdate, installUpdate, updatesPossible, type AvailableUpdate } from "../api/desktopUpdate";

/** First look a little after start, so launching is never slowed; then again while the app stays open. */
export const FIRST_CHECK_MS = 15_000;
export const RECHECK_MS = 6 * 60 * 60_000;

type Phase = { kind: "offer" } | { kind: "installing"; fraction: number | null } | { kind: "failed"; message: string };

/**
 * The desktop app: "Borealis Party <version> is available", and Install and restart. Nothing shows elsewhere.
 * `floating`: over the top of a full-height page (the sign-in), instead of in line under the account bar.
 */
export function DesktopUpdate({ floating = false }: { floating?: boolean }) {
  const [update, setUpdate] = useState<AvailableUpdate | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: "offer" });
  const [later, setLater] = useState(false);

  useEffect(() => {
    if (!updatesPossible()) return;
    let stopped = false;
    // No updater in this copy, offline or a feed that can't be read: say nothing and ask again later.
    const look = () => void checkForUpdate().then((u) => { if (!stopped && u) setUpdate(u); }, () => {});
    const first = setTimeout(look, FIRST_CHECK_MS);
    const again = setInterval(look, RECHECK_MS);
    return () => { stopped = true; clearTimeout(first); clearInterval(again); };
  }, []);

  if (!update || later) return null;
  const install = () => {
    setPhase({ kind: "installing", fraction: null });
    installUpdate(update, (fraction) => setPhase({ kind: "installing", fraction }))
      .catch((e: unknown) => setPhase({ kind: "failed", message: errorMessage(e) }));
  };
  return (
    <div className={floating ? "party-offline party-update floating" : "party-offline party-update"} role="status">
      {phase.kind === "installing" ? (
        <span>
          Downloading Borealis Party {update.version}
          {phase.fraction === null ? "…" : ` (${Math.round(phase.fraction * 100)}%)`}. It restarts by itself when the update is in.
        </span>
      ) : phase.kind === "failed" ? (
        <>
          <span>The update could not be installed: {phase.message}</span>
          <Button small onClick={install}>Try again</Button>
          <Button small onClick={() => setLater(true)}>Later</Button>
        </>
      ) : (
        <>
          <span>Borealis Party {update.version} is available (this is {update.currentVersion}). Your characters stay as they are.</span>
          <Button small variant="highlight" onClick={install}>Install and restart</Button>
          <Button small onClick={() => setLater(true)}>Later</Button>
        </>
      )}
    </div>
  );
}
