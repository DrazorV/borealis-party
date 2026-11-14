import { useState } from "react";
import { Button } from "../../src/components/Button";
import { Modal } from "../../src/components/Modal";
import { isOffline } from "../api/connectivity";
import { loadSession } from "../api/session";
import { Alert } from "../../src/components/Card";
import { chooseConflict, pendingConflicts, syncRound, useConflictsVersion, type ConflictChoice } from "../sync/conflicts";
import { requestSync } from "../sync/runner";

/**
 * A character edited on this device offline whose server copy changed meanwhile (0.3.6, sync/conflicts.ts): one
 * dialog per character, asking which copy stays. Nothing was overwritten on either side; closing it decides nothing,
 * the copy here waits and the dialog comes back after the next sync. A choice the server refused comes back with why.
 */
export function SyncConflicts() {
  useConflictsVersion();
  /** Closed for later: the conflict, and the sync round it was closed in (it comes back after the next sync). */
  const [later, setLater] = useState<Map<string, number>>(() => new Map());
  const userId = loadSession()?.user.id;
  if (!userId || isOffline()) return null;
  const conflict = pendingConflicts(userId).find((c) => (later.get(c.id) ?? -1) < syncRound());
  if (!conflict) return null;

  const choose = (choice: ConflictChoice) => {
    chooseConflict(userId, conflict.id, choice);
    void requestSync();
  };
  const name = conflict.name || "A character";
  return (
    <Modal title="Changed in two places" className="party-conflict" onClose={() => setLater(new Map([...later, [conflict.id, syncRound()]]))}>
      <p className="party-conflict-text">
        {conflict.deleted
          ? <><strong>{name}</strong> was deleted on the server while you changed it on this device. Nothing has been lost yet: choose what to keep.</>
          : <><strong>{name}</strong> changed on the server (on another device, or by your DM) while you changed it on this device. Nothing has been overwritten yet: choose which copy to keep.</>}
      </p>
      {conflict.error && <Alert tone="danger">{conflict.error}. Choose again.</Alert>}
      <div className="party-conflict-choices">
        <Button variant="primary" data-autofocus="" onClick={() => choose("mine")}>
          {conflict.deleted ? "Keep mine (restore it)" : "Keep mine (overwrite)"}
        </Button>
        <Button onClick={() => choose("server")}>{conflict.deleted ? "Use the server's (delete mine)" : "Use the server's"}</Button>
        {!conflict.deleted && <Button onClick={() => choose("both")}>Keep both</Button>}
      </div>
      <p className="party-conflict-hint">
        {conflict.deleted
          ? "Use the server's keeps your copy in the account's trash for 30 days. Close to decide later: your copy stays on this device until you choose."
          : "Keep both adds your copy as a new character, “" + name + " (this device)”. Use the server's keeps your copy in the account's trash for 30 days. Close to decide later."}
      </p>
    </Modal>
  );
}
