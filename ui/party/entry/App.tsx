import { useEffect } from "react";
import { Navigate, Route, Routes } from "react-router";
import { App as UpstreamApp } from "../../src/App";
import { AppShell } from "../../src/components/shell/AppShell";
import { isTauri } from "../../src/lib/tauri";
import { isOffline } from "../api/connectivity";
import { fetchMe } from "../api/party";
import { loadSession, updateUser } from "../api/session";
import { AccountBar } from "../pages/AccountBar";
import { AccountPage } from "../pages/AccountPage";
import { AdminPage } from "../pages/AdminPage";
import { OfflinePage } from "../pages/OfflineBanner";
import { startSync } from "../sync/runner";
import { hideUnofferedRows } from "./hideTransfer";
import "../pages/party.css";

// A browser tab has no window buttons to offer (party.css).
document.documentElement.classList.toggle("party-browser", !isTauri());
hideUnofferedRows();

/** Upstream's app with the account bar above it and the account and admin pages in front of it. */
export function App() {
  const admin = Boolean(loadSession()?.user.admin);
  // Offline (the apps without a server) the page plays from the device's copy until it reloads online.
  const offline = isOffline();
  useEffect(() => {
    if (offline) return;
    // An admin may have changed this account since it signed in on this device.
    fetchMe().then((user) => {
      const before = loadSession()?.user;
      updateUser(user);
      if (before && (before.admin !== user.admin || before.mustChangePassword !== user.mustChangePassword)) window.location.reload();
    }, () => { /* offline, or signed out: the next request says so */ });
  }, [offline]);
  // The apps keep a copy of the account (sync/); a browser doesn't. Offline it is the page's backend and stays still.
  useEffect(() => (offline ? undefined : startSync()), [offline]);
  return (
    <>
      <AccountBar />
      <Routes>
        <Route path="/account" element={<AppShell>{offline ? <OfflinePage /> : <AccountPage />}</AppShell>} />
        <Route path="/admin" element={admin ? <AppShell>{offline ? <OfflinePage /> : <AdminPage />}</AppShell> : <Navigate to="/" replace />} />
        {/* Notes read a folder on the device, which a server doesn't offer. */}
        <Route path="/notes" element={<Navigate to="/" replace />} />
        <Route path="*" element={<UpstreamApp />} />
      </Routes>
    </>
  );
}
