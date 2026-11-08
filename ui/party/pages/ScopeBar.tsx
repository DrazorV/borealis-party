import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useNavigate } from "react-router";
import { Button } from "../../src/components/Button";
import { signOut } from "../api/party";
import { clearSession, homeScope, loadSession, moveTo } from "../api/session";

/** A host element placed in upstream's window, above the content, for as long as that window exists. */
function useHost(): HTMLElement | null {
  const [host] = useState(() => {
    const element = document.createElement("div");
    element.className = "party-scope-host";
    return element;
  });
  const [placed, setPlaced] = useState(false);
  useEffect(() => {
    const place = () => {
      const content = document.querySelector(".aur-window > .aur-content");
      if (content && host.nextSibling !== content) content.parentElement?.insertBefore(host, content);
      setPlaced(Boolean(content));
    };
    place();
    const watcher = new MutationObserver(place);
    watcher.observe(document.body, { childList: true, subtree: true });
    return () => {
      watcher.disconnect();
      host.remove();
    };
  }, [host]);
  return placed ? host : null;
}

/** Which place on the server this screen belongs to, and the way back to the party. */
export function ScopeBar() {
  const nav = useNavigate();
  const host = useHost();
  const session = loadSession();
  if (!session || !host) return null;
  const leaveView = () => moveTo(homeScope(session.user.role), "/party");
  const exit = async () => {
    try { await signOut(); } catch { /* the token goes either way */ }
    clearSession();
    window.location.hash = "/";
    window.location.reload();
  };
  return createPortal(
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
    </div>,
    host,
  );
}
