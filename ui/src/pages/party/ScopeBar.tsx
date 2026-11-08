import { useNavigate } from "react-router";
import { Button } from "../../components/Button";
import { signOut } from "../../api/server/party";
import { clearSession, homeScope, loadSession, moveTo } from "../../api/server/session";
import "./party.css";

/** Which place on the server this screen belongs to, and the way back to the party. */
export function ScopeBar() {
  const nav = useNavigate();
  const session = loadSession();
  if (!session) return null;
  const leaveView = () => moveTo(homeScope(session.user.role), "/party");
  const exit = async () => {
    try { await signOut(); } catch { /* the token goes either way */ }
    clearSession();
    window.location.hash = "/";
    window.location.reload();
  };
  return (
    <div className={`party-scope ${session.view ? "viewing" : ""}`} data-party-scope>
      {session.view
        ? <span><strong>{session.view.characterName}</strong> · read only</span>
        : <span>{session.scopeLabel}</span>}
      <span className="spacer" />
      <span className="muted party-scope-user">{session.user.username}</span>
      {session.view
        ? <Button small onClick={leaveView}>Back to party</Button>
        : <Button small onClick={() => nav("/party")}>Party</Button>}
      <Button small onClick={() => void exit()}>Sign out</Button>
    </div>
  );
}
