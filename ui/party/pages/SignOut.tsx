import { type ReactNode, useState } from "react";
import { createTauriApi } from "../../src/api/tauri";
import { MessageBox } from "../../src/components/controls/Dialog";
import { isTauri } from "../../src/lib/tauri";
import { isOffline, openCopies } from "../api/connectivity";
import { endSession } from "../api/party";
import { clearSession, loadSession } from "../api/session";
import { removeCopy, unsentEdits } from "../sync/account";
import { lastSync, resumeSync } from "../sync/runner";

/**
 * Changes made on this device that the server does not have would be lost. Cancel is the default: the first, focused
 * button, and what Escape and Close answer.
 */
export function DiscardDialog({ title, intro, names, action, onResult }: {
  title: string; intro: string; names: string[]; action: string; onResult: (discard: boolean) => void;
}) {
  return (
    <MessageBox
      title={title}
      message={<>
        <p>{intro}</p>
        <ul>{names.map((n, i) => <li key={i}>{n}</li>)}</ul>
      </>}
      buttons={[{ id: "cancel", label: "Cancel", variant: "primary" }, { id: "discard", label: action, variant: "danger" }]}
      cancelId="cancel"
      onResult={(id) => onResult(id === "discard")}
    />
  );
}

/**
 * Sign out of this device. In the apps the account's copy goes too: online after one last sync has sent this device's
 * edits; edits the server still does not have (offline, or refused) are named in a dialog first. With nothing unsent,
 * the apps still ask once (Cancel is the default), since the copy on the device goes.
 */
export function useSignOut(): { start: () => void; busy: boolean; dialog: ReactNode } {
  const [busy, setBusy] = useState(false);
  const [unsent, setUnsent] = useState<string[] | null>(null);
  const [asking, setAsking] = useState(false);

  const leave = async () => {
    setUnsent(null);
    setAsking(false);
    setBusy(true);
    const session = loadSession();
    // Storage first, files after (see removeCopy).
    clearSession();
    if (session && isTauri()) await removeCopy(createTauriApi(), session.user.id).catch(() => { /* left over, in no record */ });
    if (session && !session.expired) await endSession(session);
    window.location.hash = "/";
    window.location.reload();
  };

  const start = async () => {
    const session = loadSession();
    if (!session || busy) return;
    if (!isTauri()) return leave();
    setBusy(true);
    if (!isOffline() && !session.expired) await lastSync();
    let names: string[];
    try {
      names = await unsentEdits(createTauriApi(), session.user.id, openCopies());
    } catch {
      names = ["The characters on this device (they could not be read)"];
    }
    setBusy(false);
    if (names.length === 0) setAsking(true);
    else setUnsent(names);
  };

  const keep = () => {
    setUnsent(null);
    setAsking(false);
    resumeSync();
  };

  const dialog = asking ? (
    <MessageBox
      title="Sign out"
      message="Sign out and remove this device's copy? Your characters stay on the server."
      buttons={[{ id: "cancel", label: "Cancel", variant: "primary" }, { id: "out", label: "Sign out", variant: "danger" }]}
      cancelId="cancel"
      onResult={(id) => { if (id === "out") void leave(); else keep(); }}
    />
  ) : unsent && (
    <DiscardDialog
      title="Sign out"
      intro="These characters have changes the server doesn't have yet. Signing out now removes them from this device and the changes are lost."
      names={unsent}
      action="Sign out and discard"
      onResult={(discard) => { if (discard) void leave(); else keep(); }}
    />
  );
  return { start: () => void start(), busy, dialog };
}
