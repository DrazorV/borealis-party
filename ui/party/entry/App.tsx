import { Navigate, Route, Routes } from "react-router";
import { App as UpstreamApp } from "../../src/App";
import { AppShell } from "../../src/components/shell/AppShell";
import { isTauri } from "../../src/lib/tauri";
import { loadSession } from "../api/session";
import { PartyPage } from "../pages/PartyPage";
import { ScopeBar } from "../pages/ScopeBar";
import "../pages/party.css";

// Hooks for party.css, which hides the upstream controls that don't apply to this account.
const session = loadSession();
const root = document.documentElement;
root.classList.toggle("party-dm", session?.user.role === "dm" && !session.view);
root.classList.toggle("party-player", session?.user.role === "player");
root.classList.toggle("party-browser", !isTauri());

/** Upstream's app with the party routes in front of it. */
export function App() {
  const dungeonMaster = loadSession()?.user.role === "dm";
  return (
    <>
      <ScopeBar />
      <Routes>
        <Route path="/" element={<Navigate to="/party" replace />} />
        {dungeonMaster && <Route path="/characters" element={<Navigate to="/party" replace />} />}
        <Route path="/party/:groupId?" element={<AppShell><PartyPage /></AppShell>} />
        <Route path="*" element={<UpstreamApp />} />
      </Routes>
    </>
  );
}
