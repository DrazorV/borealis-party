import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useNavigate } from "react-router";
import { Button } from "../../src/components/Button";
import { signOut } from "../api/party";
import { clearSession, loadSession } from "../api/session";

/** A host element placed in upstream's window, above the content, for as long as that window exists. */
function useHost(): HTMLElement | null {
  const [host] = useState(() => {
    const element = document.createElement("div");
    element.className = "party-bar-host";
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

/** Who is signed in, with the ways to the account page, the admin page and out. */
export function AccountBar() {
  const nav = useNavigate();
  const host = useHost();
  const session = loadSession();
  if (!session || !host) return null;
  const exit = async () => {
    try { await signOut(); } catch { /* the token goes either way */ }
    clearSession();
    window.location.hash = "/";
    window.location.reload();
  };
  return createPortal(
    <div className="party-bar">
      <span className="muted">{session.user.username}</span>
      <span className="spacer" />
      <Button small onClick={() => nav("/account")}>Account</Button>
      {session.user.admin && <Button small onClick={() => nav("/admin")}>Admin</Button>}
      <Button small onClick={() => void exit()}>Sign out</Button>
    </div>,
    host,
  );
}
