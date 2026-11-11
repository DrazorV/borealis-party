import { useEffect } from "react";
import { Navigate, Route, Routes } from "react-router";
import { App as UpstreamApp } from "../../src/App";
import { AppShell } from "../../src/components/shell/AppShell";
import { isTauri } from "../../src/lib/tauri";
import { fetchMe } from "../api/party";
import { actingAsDm, loadSession, updateUser } from "../api/session";
import { AdminPage } from "../pages/AdminPage";
import { GroupRulesNotice } from "../pages/GroupRulesNotice";
import { PartyPage } from "../pages/PartyPage";
import { ScopeBar } from "../pages/ScopeBar";
import { hideDeviceTransfer } from "./hideTransfer";
import "../pages/party.css";

// Hooks for party.css, which hides the upstream controls that don't apply in this place: building characters in a
// Library or a group one runs, adding content in My characters or a group one plays in.
const session = loadSession();
const root = document.documentElement;
root.classList.toggle("party-dm", actingAsDm());
root.classList.toggle("party-player", Boolean(session && !session.asDm));
root.classList.toggle("party-browser", !isTauri());
hideDeviceTransfer();

/** Upstream's app with the party routes in front of it. */
export function App() {
  const current = loadSession();
  const dungeonMaster = actingAsDm();
  useEffect(() => {
    // An admin may have changed this account since it signed in on this device.
    fetchMe().then((user) => {
      const before = loadSession()?.user;
      updateUser(user);
      if (before && (before.admin !== user.admin || before.mustChangePassword !== user.mustChangePassword)) window.location.reload();
    }, () => { /* offline, or signed out: the next request says so */ });
  }, []);
  return (
    <>
      <ScopeBar />
      <GroupRulesNotice />
      <Routes>
        <Route path="/" element={<Navigate to="/party" replace />} />
        {dungeonMaster && <Route path="/characters" element={<Navigate to="/party" replace />} />}
        <Route path="/party/:groupId?" element={<AppShell><PartyPage /></AppShell>} />
        <Route path="/admin" element={current?.user.admin ? <AppShell><AdminPage /></AppShell> : <Navigate to="/party" replace />} />
        <Route path="*" element={<UpstreamApp />} />
      </Routes>
    </>
  );
}
