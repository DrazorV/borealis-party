import { type ComponentProps, useEffect, useState } from "react";
import { AppProviders as UpstreamProviders } from "../../src/AppProviders";
import { isTauri } from "../../src/lib/tauri";
import { decideBoot, offlinePageApi, type Boot } from "../api/connectivity";
import { loadSession, wantsSignInAgain, type ServerSession } from "../api/session";
import { ChangePasswordPage } from "../pages/ChangePasswordPage";
import { ConnectPage } from "../pages/ConnectPage";
import { DesktopUpdate } from "../pages/DesktopUpdate";

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
  // Signed out by the server, and the banner's "Sign in again" was tapped: the sign-in, which can go back to playing.
  const again = session?.expired && wantsSignInAgain() ? session : null;
  // A browser is always online and has nothing to wait for.
  const [mode, setMode] = useState<Boot | null>(() => (isTauri() ? null : "online"));
  // The session is read once per page: signing in or out reloads it.
  useEffect(() => {
    const current = loadSession();
    if (isTauri() && current && !(current.expired && wantsSignInAgain())) void bootOnce(current).then(setMode);
  }, []);
  // Signed out, the update offer stands above the sign-in (an old app may be what keeps it from the server); signed in,
  // the account bar shows it.
  if (!session || mode === "signed-out") return <><DesktopUpdate floating /><ConnectPage /></>;
  if (again) return <><DesktopUpdate floating /><ConnectPage again={again} /></>;
  if (mode === null) return <div className="party-connect"><p className="muted">Connecting…</p></div>;
  if (mode === "no-copy") {
    return (
      <div className="party-connect">
        <DesktopUpdate floating />
        <div className="card party-connect-card">
          <h1>Borealis Party</h1>
          <p>Can't reach the server and this device has no offline copy yet.</p>
          <button className="btn primary" onClick={() => window.location.reload()}>Try again</button>
        </div>
      </div>
    );
  }
  // A new password comes first when the server can be reached; offline the app plays from its copy meanwhile.
  if (session.user.mustChangePassword && mode !== "offline") return <ChangePasswordPage />;
  return <UpstreamProviders {...props} api={mode === "offline" ? offlinePageApi() : props.api} />;
}
