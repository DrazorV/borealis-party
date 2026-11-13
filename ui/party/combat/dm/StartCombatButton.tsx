import { useState } from "react";
import { useNavigate } from "react-router";
import { Button } from "../../../src/components/Button";
import { useToast } from "../../../src/components/Toast";
import { errorMessage } from "../../../src/state/useAsync";
import { combatPath, startCombat } from "../../api/overrides/combat";

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
