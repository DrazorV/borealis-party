import { useEffect, useState } from "react";
import { useParams } from "react-router";
import { Button } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { errorMessage } from "../../src/state/useAsync";
import { serverFetch } from "../api/http";
import { loadSession } from "../api/session";
import { ScopePicker, type ScopeInfo } from "./AiApps";
import "./AiApps.css";

// The OAuth consent screen (0.3.1, plan 4.5a part A). The server's /api/oauth/authorize checked the app's request and
// sends the browser here as #/oauth/consent/<id>. The account allows or denies with its session (a header, never a
// cookie, so no other site can answer for it), and the browser goes back to the app.

export const CONSENT_PATH = "/oauth/consent";
const PENDING_KEY = "party.oauthConsent";
const CONSENT_HASH = /^#\/oauth\/consent\/([A-Za-z0-9_-]{20,100})$/;

export interface ConsentRequest {
  client: { name: string; uri: string | null; redirectUri: string; redirectHost: string };
  scopes: ScopeInfo[];
  account: { username: string };
  expiresAt: number;
}

/**
 * Keep a consent link through signing in: the sign-in page sends the browser to `#/` afterwards. Called once at
 * start-up, signed in or not.
 */
export function keepConsentThroughSignIn(storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> = sessionStorage, loc: Pick<Location, "hash"> = window.location) {
  try {
    const found = CONSENT_HASH.exec(loc.hash);
    if (found) {
      if (!loadSession()) storage.setItem(PENDING_KEY, found[1]!);
      else storage.removeItem(PENDING_KEY);
      return;
    }
    const pending = storage.getItem(PENDING_KEY);
    if (pending && loadSession()) {
      storage.removeItem(PENDING_KEY);
      if (/^[A-Za-z0-9_-]{20,100}$/.test(pending)) loc.hash = `#${CONSENT_PATH}/${pending}`;
    }
  } catch { /* no storage: the link has to be opened again after signing in */ }
}

/** Whether the browser may be sent to `url`: never a scheme that would run in this page. */
export function safeRedirect(url: string): boolean {
  try {
    const protocol = new URL(url).protocol.toLowerCase();
    return !["javascript:", "data:", "vbscript:", "file:", "blob:", "about:"].includes(protocol);
  } catch { return false; }
}

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject(new Error("Not signed in."));
  return serverFetch(session, method, path, body) as Promise<T>;
}

export const consentRequest = (id: string) => call<ConsentRequest>("GET", `/api/oauth/requests/${encodeURIComponent(id)}`);
export const approveRequest = (id: string, scopes: string[]) =>
  call<{ redirect: string }>("POST", `/api/oauth/requests/${encodeURIComponent(id)}/approve`, { scopes });
export const denyRequest = (id: string) => call<{ redirect: string }>("POST", `/api/oauth/requests/${encodeURIComponent(id)}/deny`, {});

/** `#/oauth/consent/:id`. */
export function AiAppsConsent({ go = (url: string) => window.location.assign(url) }: { go?: (url: string) => void }) {
  const { id = "" } = useParams();
  const [request, setRequest] = useState<ConsentRequest | null>(null);
  const [chosen, setChosen] = useState<string[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    let live = true;
    consentRequest(id).then((r) => {
      if (!live) return;
      setRequest(r);
      setChosen(r.scopes.map((s) => s.id));
    }, (e) => { if (live) setError(errorMessage(e)); });
    return () => { live = false; };
  }, [id]);

  const answer = async (allow: boolean) => {
    setBusy(true);
    setError("");
    try {
      const { redirect } = allow ? await approveRequest(id, chosen) : await denyRequest(id);
      if (!safeRedirect(redirect)) throw new Error("The app's address can't be opened.");
      setLeaving(true);
      go(redirect);
    } catch (e) { setError(errorMessage(e)); setBusy(false); }
  };

  return (
    <div className="party ai-consent">
      <Card title="Connect an app">
        {!request && !error && <p className="muted">Loading…</p>}
        {error && <Alert tone="danger">{error}</Alert>}
        {request && !leaving && (
          <div className="party-form">
            <p className="ai-consent-app">
              <strong>{request.client.name}</strong> wants to use your Borealis Party account
              {" "}<strong>{request.account.username}</strong>.
            </p>
            <p className="party-note muted">
              The name is the one the app gave itself. After you answer, you go back to
              {" "}<code>{request.client.redirectHost}</code>. Only allow apps you started connecting yourself.
            </p>
            <ScopePicker scopes={request.scopes} chosen={chosen} onChange={setChosen} />
            <p className="party-note muted">
              It can never do more than you can: DM-only actions still need you to be that party's DM. You can
              disconnect it at any time under Account, AI apps &amp; API tokens.
            </p>
            <div className="party-inline ai-consent-actions">
              <Button onClick={() => void answer(false)} disabled={busy}>Deny</Button>
              <Button variant="primary" onClick={() => void answer(true)} disabled={busy || chosen.length === 0}>Allow</Button>
            </div>
          </div>
        )}
        {leaving && <p className="muted">Going back to the app…</p>}
      </Card>
    </div>
  );
}
