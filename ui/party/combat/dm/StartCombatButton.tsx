import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router";
import { Button } from "../../../src/components/Button";
import { useToast } from "../../../src/components/Toast";
import { errorMessage } from "../../../src/state/useAsync";
import { combatPath, partyCombat, startCombat } from "../../api/overrides/combat";

/**
 * "Start combat" for the encounter builder (part B): starts the encounter's live combat on the server and opens it
 * in the DM tracker. The Live chip leads back to it from any tab.
 */
export function StartCombatButton({ encounterId, disabled }: { encounterId: string; disabled?: boolean }) {
  const nav = useNavigate();
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const start = async () => {
    setBusy(true);
    try {
      const combat = await startCombat(encounterId);
      nav(combatPath(combat.id));
    } catch (e) {
      toast.push(errorMessage(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return <Button variant="primary" icon="swords" disabled={disabled || busy} onClick={() => void start()}>{busy ? "Starting…" : "Start combat"}</Button>;
}

/**
 * For a live encounter: a link to its party's live combat in the tracker (the combat has its own id, so it is looked
 * up: `GET /api/parties/{party}/combat`). Nothing while there is none.
 */
export function LiveCombatLink({ partyId, label = "Open the live combat" }: { partyId: string; label?: string }) {
  const [combat, setCombat] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    partyCombat(partyId).then((id) => { if (live) setCombat(id); }, () => { /* offline: no link */ });
    return () => { live = false; };
  }, [partyId]);
  if (!combat) return null;
  return <Link className="btn btn-primary" to={combatPath(combat)}>{label}</Link>;
}
