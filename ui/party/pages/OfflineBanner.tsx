import { Button } from "../../src/components/Button";
import { goOnline, useConnectivity } from "../api/connectivity";
import { OFFLINE_MESSAGE } from "../api/offline";
import { clearSession, loadSession } from "../api/session";

/** Offline in the app: what that means, and the way back once the server answers or wants a new sign-in. */
export function OfflineBanner() {
  const { mode, backOnline } = useConnectivity();
  if (mode !== "offline") return null;
  const expired = Boolean(loadSession()?.expired);
  const signInAgain = () => {
    // The device's copy stays: signing in again as the same account picks it up.
    clearSession();
    window.location.hash = "/";
    window.location.reload();
  };
  return (
    <div className="party-offline" role="status">
      {expired ? (
        <>
          <span>Signed out by the server. Changes are kept on this device until you sign in again.</span>
          <Button small onClick={signInAgain}>Sign in again</Button>
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
