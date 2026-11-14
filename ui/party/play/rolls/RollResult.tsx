import { useEffect, useState, useSyncExternalStore } from "react";
import { Button, EmptyState, IconButton, Modal } from "../../../src/components";
import { OverlayPortal } from "../../../src/components/controls/OverlayHost";
import { useToast } from "../../../src/components/Toast";
import { type RollEntry, clearRolls, useRolls } from "../../../src/pages/play/rolls/rollStore";
import { useCharacter } from "../../../src/state/CharacterContext";
import { myCombatant } from "../../combat/player/api";
import { playerWrite, useCombatFor } from "../../combat/player/store";
import type { Shown } from "./RollContext";
import { markShared, shareKind, sharedVersion, subscribeShared, wasShared } from "./shareable";

/**
 * Upstream's roll result card and history (`src/pages/play/rolls/RollResult.tsx`), for the party's RollProvider
 * (./RollContext.tsx): the same, plus "Share" (0.3.5, roadmap "personal roll history"; P22). During a live combat of
 * the open character's party, a d20 test or an attack's damage can be sent to the combat log, where the DM sees its
 * label and total: "Stealth check 17". Only the roll is sent: never hit points, hit dice, death saves or other
 * resources (the DM-privacy rule).
 */

export const RESULT_MS = 12000;

const time = (at: number) => new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });

/** Share a roll with the DM: a function while the open character is in a live combat, else null. */
function useShare(): ((e: RollEntry) => Promise<void>) | null {
  const { snapshot } = useCharacter();
  const toast = useToast();
  const open = snapshot ? { id: snapshot.id, name: snapshot.name } : null;
  const combat = useCombatFor(open);
  const me = combat && open ? myCombatant(combat, open) : null;
  if (!combat || !me) return null;
  return async (e: RollEntry) => {
    const kind = shareKind(e.id);
    if (!kind) return;
    const roll = kind === "d20" ? { d20: { kind: "typed", total: e.total } } : { dice: { kind: "typed", total: e.total } };
    try {
      await playerWrite(combat, "roll", { id: me.id, label: e.label, roll });
      markShared(e.id);
      toast.push(`Shared with the DM: ${e.label} ${e.total}.`);
    } catch {
      toast.push("The roll was not shared. Try again.", "error");
    }
  };
}

function ShareButton({ e, share, small = true }: { e: RollEntry; share: ((e: RollEntry) => Promise<void>) | null; small?: boolean }) {
  useSyncExternalStore(subscribeShared, sharedVersion, sharedVersion);
  const [busy, setBusy] = useState(false);
  if (!share || !shareKind(e.id)) return null;
  if (wasShared(e.id)) return <span className="meta bp-shared">Shared</span>;
  const go = async () => { setBusy(true); try { await share(e); } finally { setBusy(false); } };
  return <Button small={small} icon="send" disabled={busy} aria-label={`Share ${e.label} ${e.total} with the DM`} onClick={() => void go()}>Share</Button>;
}

export function RollResultCard({ shown, onDismiss }: { shown: Shown | null; onDismiss: () => void }) {
  const [history, setHistory] = useState(false);
  const share = useShare();
  const id = shown?.entry.id;
  useEffect(() => {
    if (id === undefined) return undefined;
    const t = setTimeout(onDismiss, RESULT_MS);
    return () => clearTimeout(t);
  }, [id, onDismiss]);
  const e = shown?.entry;
  return (
    <OverlayPortal>
      <div className="pr-results">
        <p className="sr-only" role="status" aria-live="polite" aria-label="Roll result">{e ? `${e.label} ${e.total}. ${e.breakdown}` : ""}</p>
        {e && (
          <div className="toast pr-result" data-testid="roll-result">
            <div className="pr-result-text">
              <span className="pr-result-head"><span className="pr-result-label">{e.label}</span> <strong className="pr-result-total">{e.total}</strong></span>
              <span className="pr-result-dice">{e.breakdown}</span>
            </div>
            <span className="pr-result-cmds">
              {shown.action && <Button small variant="primary" onClick={shown.action.run}>{shown.action.label}</Button>}
              <ShareButton e={e} share={share} />
              <Button small onClick={() => setHistory(true)}>History</Button>
              <IconButton icon="x" label="Dismiss roll" onClick={onDismiss} />
            </span>
          </div>
        )}
      </div>
      {history && e && <RollHistoryDialog characterId={e.characterId} onClose={() => setHistory(false)} />}
    </OverlayPortal>
  );
}

function HistoryRow({ r, share }: { r: RollEntry; share: ((e: RollEntry) => Promise<void>) | null }) {
  return (
    <li className="v5-row static pr-history-row">
      <span className="grow">{r.label}<span className="v5-note" style={{ display: "block" }}>{r.breakdown}</span></span>
      <span className="meta">{time(r.at)}</span>
      <ShareButton e={r} share={share} />
      <span className="v5-count">{r.total}</span>
    </li>
  );
}

/** The session's rolls of this character, newest first (the last 50); Clear empties them. Not saved with the character. */
export function RollHistoryDialog({ characterId, onClose }: { characterId: string; onClose: () => void }) {
  const rolls = useRolls(characterId);
  const share = useShare();
  const footer = (<>
    <Button disabled={rolls.length === 0} onClick={() => clearRolls(characterId)}>Clear</Button>
    <Button variant="primary" onClick={onClose}>Close</Button>
  </>);
  return (
    <Modal title="Recent rolls" onClose={onClose} footer={footer}>
      {rolls.length === 0
        ? <EmptyState title="No rolls yet" />
        : <ul className="v5-list pr-history" aria-label="Recent rolls">{rolls.map((r) => <HistoryRow key={r.id} r={r} share={share} />)}</ul>}
      <p className="v5-note">
        Rolls are kept until the app closes. They are not saved with the character.
        {share ? " Share sends a roll's name and total to the combat log for the DM." : " During a live combat, Share sends a roll to the DM."}
      </p>
    </Modal>
  );
}

export function RollHistoryButton({ characterId }: { characterId: string }) {
  const [open, setOpen] = useState(false);
  const count = useRolls(characterId).length;
  return (<>
    <Button small icon="dice" onClick={() => setOpen(true)}>{`Recent rolls${count ? ` (${count})` : ""}`}</Button>
    {open && <RollHistoryDialog characterId={characterId} onClose={() => setOpen(false)} />}
  </>);
}
