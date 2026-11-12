import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { Link, useLocation, useNavigate } from "react-router";
import { Button } from "../../src/components/Button";
import { Icon } from "../../src/components/Icon";
import { usePhone } from "../../src/lib/phone";
import { loadSession } from "../api/session";
import { requestSync, useSyncStatus, type SyncStatus } from "../sync/runner";
import { AppOffer } from "./AppOffer";
import { DesktopUpdate } from "./DesktopUpdate";
import { OfflineBanner } from "./OfflineBanner";
import { useSignOut } from "./SignOut";

/**
 * A host element placed in upstream's page, in front of the element `before` selects, for as long as that element
 * exists. Upstream's window: the account bar goes above the content.
 */
function useHost(className: string, before: string): HTMLElement | null {
  const [host] = useState(() => {
    const element = document.createElement("div");
    element.className = className;
    return element;
  });
  const [placed, setPlaced] = useState(false);
  useEffect(() => {
    const place = () => {
      const next = document.querySelector(before);
      if (next && host.nextSibling !== next) next.parentElement?.insertBefore(host, next);
      setPlaced(Boolean(next));
    };
    place();
    const watcher = new MutationObserver(place);
    watcher.observe(document.body, { childList: true, subtree: true });
    return () => {
      watcher.disconnect();
      host.remove();
    };
  }, [host, before]);
  return placed ? host : null;
}

/** The Import screen (ImportHub, plan I9). */
const IMPORT_ROUTE = "/import-files";
/** The Parties page (plan 4.1). */
const PARTIES_ROUTE = "/parties";

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

/**
 * The app's copy of the account (sync/): how it stands, and a tap syncs now. Shown in the apps only. On a phone, where
 * the title can't be read, a problem opens the account page, which names it (SyncNotes), and tries again.
 */
function SyncChip({ phone }: { phone: boolean }) {
  const nav = useNavigate();
  const status = useSyncStatus();
  const label = syncLabel(status);
  if (!label) return null;
  const tap = () => {
    if (phone && status.state === "problem") nav("/account");
    void requestSync();
  };
  return (
    <Button small onClick={tap} disabled={status.state === "syncing"}
      title={status.state === "problem" ? status.message
        : status.state === "syncing" && status.first ? "Downloading your characters to this device"
        : status.state === "synced" && status.waiting > 0 ? "Changes made offline on this device, not sent to the server yet"
        : "Copy the account to this device now"}>
      <span className="party-sync-label">{label}</span>
    </Button>
  );
}

/**
 * The phone's More page (plan D10): the account's rows (Account, Import, Parties, Admin, Sign out), first, in upstream's list look. A phone's top bar keeps only
 * the Sync chip, so these are the way to the account page, the admin page and out.
 */
function MoreRows({ admin, exit }: { admin: boolean; exit: ReturnType<typeof useSignOut> }) {
  const { pathname } = useLocation();
  const host = useHost("party-more", ".v5-list-pane > .v5-list");
  if (pathname !== "/more" || !host) return null;
  const name = loadSession()?.user.username;
  return createPortal(
    <ul className="v5-list" aria-label="Account">
      <li>
        <Link className="v5-row" to="/account" style={{ textDecoration: "none" }}>
          <Icon name="users" /><span className="grow">Account{name && <span className="muted party-more-name"> · {name}</span>}</span><Icon name="chevron" />
        </Link>
      </li>
      <li><Link className="v5-row" to={IMPORT_ROUTE} style={{ textDecoration: "none" }}><Icon name="upload" /><span className="grow">Import</span><Icon name="chevron" /></Link></li>
      <li><Link className="v5-row" to={PARTIES_ROUTE} style={{ textDecoration: "none" }}><Icon name="swords" /><span className="grow">Parties</span><Icon name="chevron" /></Link></li>
      {admin && (
        <li><Link className="v5-row" to="/admin" style={{ textDecoration: "none" }}><Icon name="shield" /><span className="grow">Admin</span><Icon name="chevron" /></Link></li>
      )}
      <li>
        <button type="button" className="v5-row" onClick={exit.start} disabled={exit.busy}>
          <Icon name="lock" /><span className="grow">{exit.busy ? "Signing out…" : "Sign out"}</span>
        </button>
      </li>
    </ul>,
    host,
  );
}

/**
 * Who is signed in, with the ways to the account page, the admin page and out. On a phone the bar keeps only the Sync
 * chip (and nothing at all in a browser, which keeps no copy) and the rest is on the More page (plan D10).
 */
export function AccountBar() {
  const nav = useNavigate();
  const { pathname } = useLocation();
  const phone = usePhone();
  const host = useHost("party-bar-host", ".aur-window > .aur-content");
  const session = loadSession();
  const exit = useSignOut();
  if (!session) return null;
  const admin = Boolean(session.user.admin);
  return (
    <>
      {phone && <MoreRows admin={admin} exit={exit} />}
      {host && createPortal(
        <>
          <div className="party-bar">
            <span className="muted party-bar-name">{session.user.username}</span>
            <span className="spacer" />
            <SyncChip phone={phone} />
            {!phone && (
              <>
                <Button small onClick={() => nav(IMPORT_ROUTE)}>Import</Button>
                <Button small onClick={() => nav(PARTIES_ROUTE)}>Parties</Button>
                <Button small onClick={() => nav("/account")}>Account</Button>
                {admin && <Button small onClick={() => nav("/admin")}>Admin</Button>}
                <Button small onClick={exit.start} disabled={exit.busy}>{exit.busy ? "Signing out…" : "Sign out"}</Button>
              </>
            )}
          </div>
          <OfflineBanner />
          <DesktopUpdate />
          {/* The Start page (the character list) in a browser: the app for this system. */}
          {pathname === "/characters" && <AppOffer />}
        </>,
        host,
      )}
      {exit.dialog}
    </>
  );
}
