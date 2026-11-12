import { type ComponentProps, useEffect, useState } from "react";
import { AppProviders as UpstreamProviders } from "../../src/AppProviders";
import { isTauri } from "../../src/lib/tauri";
import { decideBoot, offlinePageApi, type Boot } from "../api/connectivity";
import { loadSession, type ServerSession } from "../api/session";
import { ChangePasswordPage } from "../pages/ChangePasswordPage";
import { ConnectPage } from "../pages/ConnectPage";

let boot: Promise<Boot> | null = null;

/** Once per page: StrictMode runs effects twice. */
function bootOnce(session: ServerSession): Promise<Boot> {
  boot ??= decideBoot(session);
  return boot;
}

/**
 * Nobody signed in on this device: the connect screen instead of the app. A password to replace comes first too.
 * The apps first ask whether the server can be reached, and without it play from the device's copy (offline).
 */
export function AppProviders(props: ComponentProps<typeof UpstreamProviders>) {
  const session = loadSession();
  // A browser is always online and has nothing to wait for.
  const [mode, setMode] = useState<Boot | null>(() => (isTauri() ? null : "online"));
  // The session is read once per page: signing in or out reloads it.
  useEffect(() => {
    const current = loadSession();
    if (isTauri() && current && !current.user.mustChangePassword) void bootOnce(current).then(setMode);
  }, []);
  if (!session || mode === "signed-out") return <ConnectPage />;
  if (session.user.mustChangePassword) return <ChangePasswordPage />;
  if (mode === null) return <div className="party-connect"><p className="muted">Connecting…</p></div>;
  if (mode === "no-copy") {
    return (
      <div className="party-connect">
        <div className="card party-connect-card">
          <h1>Borealis Party</h1>
          <p>Can't reach the server and this device has no offline copy yet.</p>
          <button className="btn primary" onClick={() => window.location.reload()}>Try again</button>
        </div>
      </div>
    );
  }
  return <UpstreamProviders {...props} api={mode === "offline" ? offlinePageApi() : props.api} />;
}
