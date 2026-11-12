import { Button } from "../../src/components/Button";
import { goOnline, goSignInAgain, useConnectivity } from "../api/connectivity";
import { OFFLINE_MESSAGE } from "../api/offline";
import { loadSession } from "../api/session";

/** Offline in the app: what that means, and the way back once the server answers or wants a new sign-in. */
export function OfflineBanner() {
  const { mode, backOnline, expired } = useConnectivity();
  if (mode !== "offline") return null;
  const session = loadSession();
  // The device's copy stays: signing in again as the same account sends its changes.
  const signInAgain = <Button small onClick={() => void goSignInAgain()}>Sign in again</Button>;
  return (
    <div className="party-offline" role="status">
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
