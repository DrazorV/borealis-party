import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { Link, useLocation, useNavigate } from "react-router";
import { Button } from "../../src/components/Button";
import { Icon } from "../../src/components/Icon";
import { phoneOs, usePhone } from "../../src/lib/phone";
import { isTauri } from "../../src/lib/tauri";
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

/** The sync state in full, for More's Sync row, where there is room. Empty when there is nothing to sync (a browser). */
export function syncText(status: SyncStatus): string {
  switch (status.state) {
    case "syncing":
      return status.first && status.total > 0 ? `Downloading your characters, ${status.done} of ${status.total}` : "Syncing…";
    case "synced": return status.waiting > 0 ? `Synced; ${status.waiting} changes made here wait for the server` : "Synced";
    case "problem": return `Sync problem: ${status.message}`;
    default: return "";
  }
}

/**
 * The account rows go on the More page, not in a row over the page: on a phone (plan D10), and anywhere in the Android
 * app, whose tablet-sized or wide windows get upstream's desktop shell (U9b) and with it no phone layer.
 */
export function compactAccount(phone: boolean, app: { tauri: boolean; os: string | null }): boolean {
  return phone || (app.tauri && app.os === "android");
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
  const nav = useNavigate();
  const status = useSyncStatus();
  const host = useHost("party-more", ".v5-list-pane > .v5-list");
  if (pathname !== "/more" || !host) return null;
  const name = loadSession()?.user.username;
  const sync = syncText(status);
  return createPortal(
    <ul className="v5-list" aria-label="Account">
      {sync && (
        <li>
          <button type="button" className="v5-row" disabled={status.state === "syncing"}
            onClick={() => { if (status.state === "problem") nav("/account"); void requestSync(); }}>
            <Icon name="update" /><span className="grow party-more-sync">{sync}</span>
          </button>
        </li>
      )}
      <li>
        <Link className="v5-row" to="/account" style={{ textDecoration: "none" }}>
          <Icon name="users" /><span className="grow">Account{name && <span className="muted party-more-name"> · {name}</span>}</span><Icon name="chevron" />
        </Link>
      </li>
      <li><Link className="v5-row" to={IMPORT_ROUTE} style={{ textDecoration: "none" }}><Icon name="upload" /><span className="grow">Import</span><Icon name="chevron" /></Link></li>
      <li><Link className="v5-row" to="/share" style={{ textDecoration: "none" }}><Icon name="send" /><span className="grow">Share with a code</span><Icon name="chevron" /></Link></li>
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
  const compact = compactAccount(phone, { tauri: isTauri(), os: phoneOs() });
  const status = useSyncStatus();
  const host = useHost("party-bar-host", ".aur-window > .aur-content");
  const session = loadSession();
  const exit = useSignOut();
  if (!session) return null;
  const admin = Boolean(session.user.admin);
  // Compact: the row only for a sync problem, which must not wait for a visit to More. Otherwise it is empty and
  // hidden (party.css), as on a phone without a sync chip.
  const chip = !compact || status.state === "problem";
  return (
    <>
      {compact && <MoreRows admin={admin} exit={exit} />}
      {host && createPortal(
        <>
          <div className={compact ? "party-bar party-bar-compact" : "party-bar"}>
            <span className="muted party-bar-name">{session.user.username}</span>
            <span className="spacer" />
            {chip && <SyncChip phone={compact} />}
            {!compact && (
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
