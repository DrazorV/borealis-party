import { useEffect } from "react";
import { Navigate, Route, Routes, useNavigate } from "react-router";
import { App as UpstreamApp } from "../../src/App";
import { AppShell } from "../../src/components/shell/AppShell";
import { isTauri } from "../../src/lib/tauri";
import { isOffline } from "../api/connectivity";
import { registerDmLiveCombats, startCombatWatch } from "../combat/player/store";
import { dmLiveCombats } from "../api/overrides/combat";
import { watchOpened } from "../api/overrides/opened";
import { fetchMe } from "../api/party";
import { loadSession, updateUser } from "../api/session";
import { AccountBar } from "../pages/AccountBar";
import { AccountPage } from "../pages/AccountPage";
import { AdminPage } from "../pages/AdminPage";
import { CombatTracker } from "../pages/CombatTracker";
import { JoinRoute, PartiesPage } from "../pages/PartiesPage";
import { PartyCharacterView } from "../pages/PartyCharacterView";
import { EncounterBuilder } from "../pages/EncounterBuilder";
import { EncountersPage } from "../pages/EncountersPage";
import { OfflinePage } from "../pages/OfflineBanner";
import { IMPORT_PATH, OLD_IMPORT_PATH, setImportNavigate } from "../import/door";
import { SharePage } from "../pages/SharePage";
import { ShareReceivePage } from "../pages/ShareReceivePage";
import { followReplaced } from "../sync/replaced";
import { startSync } from "../sync/runner";
import { hideUnofferedRows } from "./hideTransfer";
import { startPhoneBrowser } from "./phoneBrowser";
import { unifyImports } from "./unifyImports";
import "../pages/party.css";

// A browser tab has no window buttons to offer (party.css).
document.documentElement.classList.toggle("party-browser", !isTauri());
hideUnofferedRows();
// Phone browsers get the apps' phone layer and a Back that closes the top sheet (plan M4).
startPhoneBrowser();
// Every import button, menu item, drop and "Open with" leads to the one Import section on Additional Content (0.2.1).
unifyImports();
// The Live chip also shows the DM's own live combat and opens its tracker (/combat/<id>). Registered here, not in
// overrides/combat.ts, which would make an import cycle through api/http.
registerDmLiveCombats(dmLiveCombats);

/** Upstream's app with the account bar above it and the account and admin pages in front of it. */
export function App() {
  const admin = Boolean(loadSession()?.user.admin);
  // Offline (the apps without a server) the page plays from the device's copy until it reloads online.
  const offline = isOffline();
  const navigate = useNavigate();
  useEffect(() => {
    setImportNavigate((to) => void navigate(to));
    return () => setImportNavigate(null);
  }, [navigate]);
  // Files opened with the app go to the Import section (online; offline they wait in the device's queue).
  useEffect(() => {
    const session = loadSession();
    return offline || !session ? undefined : watchOpened(session);
  }, [offline]);
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
  // The player's live combats (plan 4.4): the Play tab's Combat sub-tab, the Live chip and "Your turn" follow them.
  useEffect(() => (offline ? undefined : startCombatWatch()), [offline]);
  return (
    <>
      <AccountBar />
      <Routes>
        <Route path="/account" element={<AppShell>{offline ? <OfflinePage /> : <AccountPage />}</AppShell>} />
        <Route path="/admin" element={admin ? <AppShell>{offline ? <OfflinePage /> : <AdminPage />}</AppShell> : <Navigate to="/" replace />} />
        {/* The separate Import section is gone: imports happen on Additional Content (0.2.1). */}
        <Route path={OLD_IMPORT_PATH} element={<Navigate to={IMPORT_PATH} replace />} />
        {/* Share codes (plan E4): send from any device with no file, and receive by code or link. */}
        <Route path="/share" element={<AppShell>{offline ? <OfflinePage /> : <SharePage />}</AppShell>} />
        <Route path="/share/receive/:code?" element={<AppShell>{offline ? <OfflinePage /> : <ShareReceivePage />}</AppShell>} />
        {/* Parties (plan 4.1); a join link keeps its code and opens them. */}
        <Route path="/parties" element={<AppShell>{offline ? <OfflinePage /> : <PartiesPage />}</AppShell>} />
        <Route path="/join/:code" element={<JoinRoute />} />
        {/* The DM's read-only view of a member character (plan 3.5, 0.2.1). */}
        <Route path="/parties/:party/view/:owner/:character" element={<AppShell>{offline ? <OfflinePage /> : <PartyCharacterView />}</AppShell>} />
        {/* Encounters (0.3.0): the DM's builder; players see only what the server projects for them. */}
        <Route path="/parties/:party/encounters" element={<AppShell>{offline ? <OfflinePage /> : <EncountersPage />}</AppShell>} />
        <Route path="/parties/:party/encounters/:encounter" element={<AppShell>{offline ? <OfflinePage /> : <EncounterBuilder />}</AppShell>} />
        {/* The DM's live combat tracker (plan 4.4, 0.3.0); the Live chip in the top bar leads back to it. */}
        <Route path="/combat/:id" element={<AppShell>{offline ? <OfflinePage /> : <CombatTracker />}</AppShell>} />
        {/* Notes read a folder on the device, which a server doesn't offer. */}
        <Route path="/notes" element={<Navigate to="/" replace />} />
        <Route path="*" element={<UpstreamApp />} />
      </Routes>
    </>
  );
}
