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
import { dungeonMaster } from "../api/overrides/parties";
import { loadSession, updateUser } from "../api/session";
import { AccountBar } from "../pages/AccountBar";
import { AccountPage } from "../pages/AccountPage";
import { AiAppsConsent, CONSENT_PATH, keepConsentThroughSignIn } from "../pages/AiAppsConsent";
import { AdminPage } from "../pages/AdminPage";
import { CombatTracker } from "../pages/CombatTracker";
import { JoinRoute, PartiesPage } from "../pages/PartiesPage";
import { PartyCharacterView } from "../pages/PartyCharacterView";
import { HomebrewEditor } from "../pages/HomebrewEditor";
import { HomebrewPackPage, HomebrewPage } from "../pages/HomebrewPage";
import { EncountersPage, OldEncounterRoute } from "../pages/EncountersPage";
import { DmOnly, EncounterWorkspace } from "../pages/EncounterWorkspace";
import { CombatHome } from "../pages/CombatHome";
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
// An AI app's consent link (#/oauth/consent/<id>) survives signing in first (0.3.1).
keepConsentThroughSignIn();

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
  // The DM switch, for the Encounters tab: the device's last answer draws it at once; this keeps it current (api/dmRole.ts).
  useEffect(() => {
    if (!offline) dungeonMaster().catch(() => { /* parties off, or offline: the last answer stands */ });
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
        {/* An AI app asks to connect (OAuth consent, 0.3.1). */}
        <Route path={`${CONSENT_PATH}/:id`} element={<AppShell>{offline ? <OfflinePage /> : <AiAppsConsent />}</AppShell>} />
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
        {/* Encounters (0.3.1): the DM's main tab, DMs only (entry/encountersTab.ts). The server refuses the rest anyway. */}
        {["/encounters", "/encounters/:party", "/encounters/:party/:encounter"].map((path) => (
          <Route key={path} path={path} element={<AppShell>{offline ? <OfflinePage /> : <DmOnly><EncounterWorkspace /></DmOnly>}</AppShell>} />
        ))}
        {/* 0.3.0's addresses: the players' page of shown encounters (a DM is sent on to the tab), and the builder's. */}
        <Route path="/parties/:party/encounters" element={<AppShell>{offline ? <OfflinePage /> : <EncountersPage />}</AppShell>} />
        <Route path="/parties/:party/encounters/:encounter" element={<OldEncounterRoute />} />
        {/* The DM's live combat tracker (plan 4.4, 0.3.0); the Live chip in the top bar leads back to it. */}
        {/* The Combat main tab (0.3.1, plan 4.5b): the DM's encounters to run, live ones first; DMs only. */}
        <Route path="/combat" element={<AppShell>{offline ? <OfflinePage /> : <DmOnly><CombatHome /></DmOnly>}</AppShell>} />
        {/* A combat only opens for its party's DM (the server's 404 for anyone else), so it needs no guard of its own. */}
        <Route path="/combat/:id" element={<AppShell>{offline ? <OfflinePage /> : <CombatTracker />}</AppShell>} />
        {/* Homebrew (0.3.1): the account's own packs, a form or XML per element, and publishing as an index. */}
        <Route path="/homebrew" element={<AppShell>{offline ? <OfflinePage /> : <HomebrewPage />}</AppShell>} />
        <Route path="/homebrew/:pack" element={<AppShell>{offline ? <OfflinePage /> : <HomebrewPackPage />}</AppShell>} />
        <Route path="/homebrew/:pack/e/:element" element={<AppShell>{offline ? <OfflinePage /> : <HomebrewEditor />}</AppShell>} />
        <Route path="/homebrew/:pack/new/:type" element={<AppShell>{offline ? <OfflinePage /> : <HomebrewEditor />}</AppShell>} />
        {/* Notes read a folder on the device, which a server doesn't offer. */}
        <Route path="/notes" element={<Navigate to="/" replace />} />
        <Route path="*" element={<UpstreamApp />} />
      </Routes>
    </>
  );
}
