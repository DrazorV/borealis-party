import { useCallback, useEffect, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { Modal } from "../../src/components/Modal";
import { useToast } from "../../src/components/Toast";
import { errorMessage } from "../../src/state/useAsync";
import { serverFetch } from "../api/http";
import { loadSession } from "../api/session";
import { AssistantInstructions, ConnectGuides } from "./AiAppsGuides";
import "./AiApps.css";

// The Account page's "AI apps & API tokens" (0.3.1, plan 4.5a part A; server: tokens.rs and oauth.rs). Since 0.3.3 it is
// the only AI in Borealis Connect: ChatGPT or Claude connect to it over MCP, and the server never calls an AI service.

export interface ScopeInfo { id: string; label: string; default: boolean }

export interface AiOverview {
  /** The MCP endpoint, or null when the server has no public address. */
  mcpUrl: string | null;
  /** Apps can connect with OAuth (the server has a public address). */
  oauth: boolean;
  scopes: ScopeInfo[];
  defaultDays: number;
  maxDays: number;
  maxTokens: number;
  callsPerMinute: number;
  /** What connected apps are told (the MCP instructions); absent from older servers. */
  instructions?: string;
}

export interface ApiToken {
  id: string;
  name: string;
  /** The first characters, to tell tokens apart; never the secret. */
  hint: string;
  scopes: string[];
  createdAt: number;
  expiresAt: number;
  lastUsedAt: number | null;
}

/** A new token: the only time its secret is shown. */
export interface MadeToken extends ApiToken { token: string }

export interface ConnectedApp {
  id: string;
  /** As the app registered itself: not verified. */
  name: string;
  uri: string | null;
  redirectHost: string;
  scopes: string[];
  connectedAt: number;
  lastUsedAt: number | null;
}

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject(new Error("Not signed in."));
  return serverFetch(session, method, path, body) as Promise<T>;
}

export const aiOverview = () => call<AiOverview>("GET", "/api/me/ai");
export const listTokens = () => call<{ tokens: ApiToken[] }>("GET", "/api/me/ai/tokens").then((r) => r.tokens);
export const createToken = (name: string, scopes: string[], expiresInDays: number) =>
  call<MadeToken>("POST", "/api/me/ai/tokens", { name, scopes, expiresInDays });
export const revokeToken = (id: string) => call<unknown>("DELETE", `/api/me/ai/tokens/${encodeURIComponent(id)}`);
export const listApps = () => call<{ apps: ConnectedApp[] }>("GET", "/api/me/ai/apps").then((r) => r.apps);
export const disconnectApp = (id: string) => call<unknown>("DELETE", `/api/me/ai/apps/${encodeURIComponent(id)}`);

/** The ways a new token may last. */
export const EXPIRY_CHOICES = [7, 30, 90, 365];

const date = (secs: number) => new Date(secs * 1000).toLocaleDateString();
const ago = (secs: number | null) => (secs === null ? "never used" : `last used ${date(secs)}`);

async function copy(text: string): Promise<boolean> {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}

/** Setup lines for the popular clients, for a token just made. */
export function setupSnippets(mcpUrl: string, token: string): { label: string; text: string }[] {
  return [
    { label: "Claude Code", text: `claude mcp add --transport http borealis-connect ${mcpUrl} --header "Authorization: Bearer ${token}"` },
    {
      label: "Other MCP clients (Cursor, Gemini CLI, Codex, …)",
      text: JSON.stringify({ mcpServers: { "borealis-connect": { url: mcpUrl, headers: { Authorization: `Bearer ${token}` } } } }, null, 2),
    },
  ];
}

/** Scope checkboxes, the defaults ticked. There is no scope for live combat (D25) or for publishing (D47). */
export function ScopePicker({ scopes, chosen, onChange }: { scopes: ScopeInfo[]; chosen: string[]; onChange: (next: string[]) => void }) {
  const toggle = (id: string, on: boolean) => onChange(on ? [...chosen, id] : chosen.filter((s) => s !== id));
  return (
    <fieldset className="ai-scopes">
      <legend>It may</legend>
      {scopes.map((s) => (
        <label key={s.id} className="ai-scope">
          <input type="checkbox" checked={chosen.includes(s.id)} onChange={(e) => toggle(s.id, e.target.checked)} />
          <span className="ai-scope-text">
            <span>{s.label}</span>
            <code className="muted">{s.id}</code>
          </span>
        </label>
      ))}
    </fieldset>
  );
}

function NewTokenDialog({ info, onMade, onClose }: { info: AiOverview; onMade: (t: MadeToken) => void; onClose: () => void }) {
  const [name, setName] = useState("");
  const [days, setDays] = useState(info.defaultDays);
  const [chosen, setChosen] = useState(info.scopes.filter((s) => s.default).map((s) => s.id));
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    if (!name.trim() || chosen.length === 0) return;
    setBusy(true);
    setError("");
    try { onMade(await createToken(name.trim(), chosen, days)); } catch (e) { setError(errorMessage(e)); setBusy(false); }
  };
  return (
    <Modal title="New API token" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || !name.trim() || chosen.length === 0} onClick={() => void submit()}>Create token</Button>
      </>}>
      <form className="party-form" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        <label className="party-field">
          Name
          <input className="input" aria-label="Token name" maxLength={60} placeholder="Claude Code on my laptop" value={name} onChange={(e) => setName(e.target.value)} required />
        </label>
        <label className="party-field">
          Expires after
          <select className="input" aria-label="Expires after" value={days} onChange={(e) => setDays(Number(e.target.value))}>
            {EXPIRY_CHOICES.filter((d) => d <= info.maxDays).map((d) => <option key={d} value={d}>{d} days</option>)}
          </select>
        </label>
        <ScopePicker scopes={info.scopes} chosen={chosen} onChange={setChosen} />
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
    </Modal>
  );
}

function MadeTokenDialog({ made, mcpUrl, onClose }: { made: MadeToken; mcpUrl: string | null; onClose: () => void }) {
  const toast = useToast();
  const copyIt = async (text: string) => {
    toast.push((await copy(text)) ? "Copied." : "Copying isn't allowed here: select the text and copy it.", "info");
  };
  return (
    <Modal title="Your new token" onClose={onClose} footer={<Button variant="primary" onClick={onClose}>Done</Button>}>
      <div className="party-form">
        <Alert tone="warning">Copy it now: it is shown only this once. Anyone with it can act as you within its scopes.</Alert>
        <input className="input ai-secret" readOnly aria-label="API token" value={made.token} onFocus={(e) => e.currentTarget.select()} />
        <div className="party-inline"><Button variant="primary" onClick={() => void copyIt(made.token)}>Copy token</Button></div>
        {mcpUrl && setupSnippets(mcpUrl, made.token).map((s) => (
          <div key={s.label} className="party-field">
            <span>{s.label}</span>
            <textarea className="input ai-snippet" readOnly rows={s.text.split("\n").length + 1} aria-label={`${s.label} setup`} value={s.text} onFocus={(e) => e.currentTarget.select()} />
            <div className="party-inline"><Button onClick={() => void copyIt(s.text)}>Copy</Button></div>
          </div>
        ))}
      </div>
    </Modal>
  );
}

/** Asks before revoking a token or disconnecting an app. */
function ConfirmDialog({ title, text, action, onConfirm, onClose }: { title: string; text: string; action: string; onConfirm: () => Promise<void>; onClose: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const go = async () => {
    setBusy(true);
    try { await onConfirm(); onClose(); } catch (e) { setError(errorMessage(e)); setBusy(false); }
  };
  return (
    <Modal title={title} onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="danger" disabled={busy} onClick={() => void go()}>{action}</Button>
      </>}>
      <p>{text}</p>
      {error && <Alert tone="danger">{error}</Alert>}
    </Modal>
  );
}

function Chips({ scopes }: { scopes: string[] }) {
  return <span className="ai-chips">{scopes.map((s) => <code key={s} className="ai-chip">{s}</code>)}</span>;
}

type Asking = { kind: "token"; token: ApiToken } | { kind: "app"; app: ConnectedApp };

/** Account → AI apps & API tokens: connect ChatGPT or Claude first, then the instructions, apps and keys. Hidden when the server turned AI apps off (BOREALIS__FEATURES__AI=false). */
export function AiAppsCard() {
  const toast = useToast();
  const [info, setInfo] = useState<AiOverview | null>(null);
  const [offered, setOffered] = useState(true);
  const [tokens, setTokens] = useState<ApiToken[]>([]);
  const [apps, setApps] = useState<ConnectedApp[]>([]);
  const [making, setMaking] = useState(false);
  const [made, setMade] = useState<MadeToken | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);

  const reload = useCallback(async () => {
    try {
      const [t, a] = await Promise.all([listTokens(), listApps().catch(() => [])]);
      setTokens(t);
      setApps(a);
    } catch (e) { toast.push(errorMessage(e), "error"); }
  }, [toast]);

  useEffect(() => {
    let live = true;
    aiOverview().then((i) => { if (!live) return; setInfo(i); void reload(); }, (e) => {
      if (live && (e as { code?: string }).code === "not_found") setOffered(false);
    });
    return () => { live = false; };
  }, [reload]);

  if (!offered) return null;
  const copyUrl = async (url: string) => {
    toast.push((await copy(url)) ? "Copied." : "Copying isn't allowed here: select the address and copy it.", "info");
  };
  const copyText = async (text: string) => {
    toast.push((await copy(text)) ? "Copied." : "Copying isn't allowed here: select the text and copy it.", "info");
  };
  return (
    <Card title="AI apps & API tokens" className="ai-apps">
      <p className="muted ai-lead">
        Borealis Connect uses your own ChatGPT or Claude: you connect the app to your account here, and your own plan
        pays for it. Borealis Connect never calls an AI service itself.
      </p>
      <p className="muted">
        A connected app can do only what you can: DM-only actions still need you to be that party's DM, and players
        never see hidden monsters through it. It has no access to live combat.
      </p>
      {info?.mcpUrl && (
        <div className="party-field">
          <span>MCP server address</span>
          <div className="party-inline">
            <input className="input" readOnly aria-label="MCP server address" value={info.mcpUrl} onFocus={(e) => e.currentTarget.select()} />
            <Button onClick={() => void copyUrl(info.mcpUrl ?? "")}>Copy</Button>
          </div>
          <span className="party-note muted">
            {info.oauth
              ? "Add it to ChatGPT or Claude as a custom connector (the steps are below); you approve it here."
              : "Use it with a key for developer tools."}
          </span>
        </div>
      )}
      {info?.mcpUrl && <ConnectGuides url={info.mcpUrl} oauth={info.oauth} />}

      {info?.instructions && <AssistantInstructions text={info.instructions} onCopy={(t) => void copyText(t)} />}

      <h3 className="ai-head">Connected apps</h3>
      {apps.length === 0 ? <p className="muted">No apps are connected.</p> : (
        <ul className="party-list">
          {apps.map((a) => (
            <li key={a.id} className="ai-item">
              <div className="party-line">
                <span className="grow"><strong>{a.name}</strong> <span className="muted">· {a.redirectHost}</span></span>
                <Button variant="danger" onClick={() => setAsking({ kind: "app", app: a })}>Disconnect</Button>
              </div>
              <span className="party-note muted">Connected {date(a.connectedAt)}, {ago(a.lastUsedAt)}</span>
              <Chips scopes={a.scopes} />
            </li>
          ))}
        </ul>
      )}

      <details className="ai-keys">
      <summary className="ai-head">Keys for developer tools</summary>
      <p className="muted">For Cursor, Gemini CLI and other tools that don't sign in: a key (an API token) that acts as you within its scopes.</p>
      {tokens.length === 0 ? <p className="muted">You have no tokens.</p> : (
        <ul className="party-list">
          {tokens.map((t) => (
            <li key={t.id} className="ai-item">
              <div className="party-line">
                <span className="grow"><strong>{t.name}</strong> <code className="muted">{t.hint}</code></span>
                <Button variant="danger" onClick={() => setAsking({ kind: "token", token: t })}>Revoke</Button>
              </div>
              <span className="party-note muted">Expires {date(t.expiresAt)}, {ago(t.lastUsedAt)}</span>
              <Chips scopes={t.scopes} />
            </li>
          ))}
        </ul>
      )}
      <div className="party-inline">
        <Button variant="primary" disabled={!info || (info && tokens.length >= info.maxTokens)} onClick={() => setMaking(true)}>New token</Button>
      </div>
      </details>

      {making && info && (
        <NewTokenDialog info={info} onClose={() => setMaking(false)}
          onMade={(t) => { setMaking(false); setMade(t); void reload(); }} />
      )}
      {made && <MadeTokenDialog made={made} mcpUrl={info?.mcpUrl ?? null} onClose={() => setMade(null)} />}
      {asking?.kind === "token" && (
        <ConfirmDialog title="Revoke token" action="Revoke"
          text={`"${asking.token.name}" stops working at once. Apps using it will need a new token.`}
          onConfirm={async () => { await revokeToken(asking.token.id); await reload(); toast.push("Token revoked.", "info"); }}
          onClose={() => setAsking(null)} />
      )}
      {asking?.kind === "app" && (
        <ConfirmDialog title="Disconnect app" action="Disconnect"
          text={`"${asking.app.name}" loses access at once. To use it again, connect it again from the app.`}
          onConfirm={async () => { await disconnectApp(asking.app.id); await reload(); toast.push("App disconnected.", "info"); }}
          onClose={() => setAsking(null)} />
      )}
    </Card>
  );
}
