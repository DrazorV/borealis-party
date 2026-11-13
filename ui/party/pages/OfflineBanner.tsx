import { useEffect } from "react";
import { Button } from "../../src/components/Button";
import { goOffline, goOnline, goSignInAgain, retryNow, useConnectivity, watchConnection } from "../api/connectivity";
import { OFFLINE_MESSAGE } from "../api/offline";
import { loadSession } from "../api/session";

/**
 * How the server stands, above the content: offline in the app (what that means, and the way back once the server
 * answers or wants a new sign-in), and online while the signal is lost ("Reconnecting…", then "Back online"). Nothing
 * here reloads on its own; connectivity.ts waits for a moment that disturbs nobody, and the buttons do it now.
 */
export function OfflineBanner() {
  const { mode, backOnline, expired, link, waiting } = useConnectivity();
  // A browser watches the server from here too (the apps start watching when the page boots).
  useEffect(() => { watchConnection(); }, []);
  const session = loadSession();
  // The device's copy stays: signing in again as the same account sends its changes.
  const signInAgain = <Button small onClick={() => void goSignInAgain()}>Sign in again</Button>;
  if (mode === "online") {
    if (expired) {
      return (
        <div className="party-offline" role="status" data-link="expired">
          <span>Signed out by the server. Changes are kept on this device until you sign in again.</span>
          {signInAgain}
        </div>
      );
    }
    if (link === "down") {
      return (
        <div className="party-offline" role="status" data-link="down">
          <span>
            Can't reach the server. Reconnecting…
            {waiting === "offline" ? " Your characters on this device still work offline." : ""}
          </span>
          {waiting === "offline"
            ? <Button small onClick={goOffline}>Play offline</Button>
            : <Button small onClick={retryNow}>Try now</Button>}
        </div>
      );
    }
    if (link === "back") return <div className="party-offline" role="status" data-link="back"><span>Back online.</span></div>;
    return null;
  }
  return (
    <div className="party-offline" role="status" data-link={backOnline ? "back" : "offline"}>
      {session?.blocked ? (
        <>
          <span>This account is turned off on the server. Your changes stay on this device; ask the server's admin.</span>
          {signInAgain}
        </>
      ) : expired || session?.expired ? (
        <>
          <span>Signed out by the server. Changes are kept on this device until you sign in again.</span>
          {signInAgain}
        </>
      ) : backOnline ? (
        <>
          <span>Back online.</span>
          <Button small onClick={() => void goOnline()}>Sync now</Button>
        </>
      ) : (
        <span>Offline. Changes to your characters are kept on this device and sync when you're connected.</span>
      )}
    </div>
  );
}

/** The account and admin pages, offline. */
export function OfflinePage() {
  return <div className="party"><p className="muted">{OFFLINE_MESSAGE}</p></div>;
}
