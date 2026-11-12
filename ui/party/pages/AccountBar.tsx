import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useNavigate } from "react-router";
import { Button } from "../../src/components/Button";
import { loadSession } from "../api/session";
import { requestSync, useSyncStatus, type SyncStatus } from "../sync/runner";
import { OfflineBanner } from "./OfflineBanner";
import { useSignOut } from "./SignOut";

/** A host element placed in upstream's window, above the content, for as long as that window exists. */
function useHost(): HTMLElement | null {
  const [host] = useState(() => {
    const element = document.createElement("div");
    element.className = "party-bar-host";
    return element;
  });
  const [placed, setPlaced] = useState(false);
  useEffect(() => {
    const place = () => {
      const content = document.querySelector(".aur-window > .aur-content");
      if (content && host.nextSibling !== content) content.parentElement?.insertBefore(host, content);
      setPlaced(Boolean(content));
    };
    place();
    const watcher = new MutationObserver(place);
    watcher.observe(document.body, { childList: true, subtree: true });
    return () => {
      watcher.disconnect();
      host.remove();
    };
  }, [host]);
  return placed ? host : null;
}

function syncLabel(status: SyncStatus): string {
  switch (status.state) {
    case "syncing":
      // Short enough for a phone's account bar; the title says more.
      if (!status.first) return "Syncing…";
      return status.total > 0 ? `Syncing ${status.done}/${status.total}` : "Syncing…";
    case "synced": return status.waiting > 0 ? `${status.waiting} waiting` : "Synced";
    case "problem": return "Sync problem";
    default: return "";
  }
}

/** The app's copy of the account (sync/): how it stands, and a tap syncs now. Shown in the apps only. */
function SyncChip() {
  const status = useSyncStatus();
  const label = syncLabel(status);
  if (!label) return null;
  return (
    <Button small onClick={() => void requestSync()} disabled={status.state === "syncing"}
      title={status.state === "problem" ? status.message
        : status.state === "syncing" && status.first ? "Downloading your characters to this device"
        : status.state === "synced" && status.waiting > 0 ? "Changes made offline on this device, not sent to the server yet"
        : "Copy the account to this device now"}>
      <span className="party-sync-label">{label}</span>
    </Button>
  );
}

/** Who is signed in, with the ways to the account page, the admin page and out. */
export function AccountBar() {
  const nav = useNavigate();
  const host = useHost();
  const session = loadSession();
  const exit = useSignOut();
  if (!session || !host) return null;
  return createPortal(
    <>
      <div className="party-bar">
        <span className="muted party-bar-name">{session.user.username}</span>
        <span className="spacer" />
        <SyncChip />
        <Button small onClick={() => nav("/account")}>Account</Button>
        {session.user.admin && <Button small onClick={() => nav("/admin")}>Admin</Button>}
        <Button small onClick={exit.start} disabled={exit.busy}>{exit.busy ? "Signing out…" : "Sign out"}</Button>
      </div>
      <OfflineBanner />
      {exit.dialog}
    </>,
    host,
  );
}
