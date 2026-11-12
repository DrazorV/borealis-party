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
import { ImportHub } from "../pages/ImportHub";
import { OfflinePage } from "../pages/OfflineBanner";
import { followReplaced } from "../sync/replaced";
import { startSync } from "../sync/runner";
import { hideUnofferedRows } from "./hideTransfer";
import { startPhoneBrowser } from "./phoneBrowser";
import "../pages/party.css";

// A browser tab has no window buttons to offer (party.css).
document.documentElement.classList.toggle("party-browser", !isTauri());
hideUnofferedRows();
// Phone browsers get the apps' phone layer and a Back that closes the top sheet (plan M4).
startPhoneBrowser();

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
  // A browser's open character follows a device's edit the server took (the apps hear it in their sync's events).
  useEffect(() => (isTauri() ? undefined : followReplaced()), []);
  return (
    <>
      <AccountBar />
      <Routes>
        <Route path="/account" element={<AppShell>{offline ? <OfflinePage /> : <AccountPage />}</AppShell>} />
        <Route path="/admin" element={admin ? <AppShell>{offline ? <OfflinePage /> : <AdminPage />}</AppShell> : <Navigate to="/" replace />} />
        {/* One Import screen for every kind of file and link (plan I9). */}
        <Route path="/import-files" element={<AppShell>{offline ? <OfflinePage /> : <ImportHub />}</AppShell>} />
        {/* Notes read a folder on the device, which a server doesn't offer. */}
        <Route path="/notes" element={<Navigate to="/" replace />} />
        <Route path="*" element={<UpstreamApp />} />
      </Routes>
    </>
  );
}
