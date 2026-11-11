import { type FormEvent, useEffect, useState } from "react";
import { checkServer, signIn, signUp, type ServerInfo } from "../api/party";
import { rememberServer, rememberedServer, saveSession, type ServerUser } from "../api/session";
import { isTauri } from "../../src/lib/tauri";
import "./party.css";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

function normalise(address: string): string {
  const trimmed = address.trim().replace(/\/+$/, "");
  return /^https?:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`;
}

/** The first screen on a device with no session: pick the server (installed apps only), then sign in or sign up. */
export function ConnectPage() {
  // A browser got this page from the server, so the server is already known.
  const needsAddress = isTauri();
  const [address, setAddress] = useState(rememberedServer());
  const [server, setServer] = useState<{ baseUrl: string; info: ServerInfo } | null>(null);
  const [mode, setMode] = useState<"in" | "up">("in");
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);

  const connect = async (baseUrl: string) => {
    setBusy(true);
    setError("");
    try {
      setServer({ baseUrl, info: await checkServer(baseUrl) });
      if (baseUrl) rememberServer(baseUrl);
    } catch (e) { setError(message(e)); }
    finally { setBusy(false); }
  };

  useEffect(() => {
    if (!needsAddress) void connect("");
  }, [needsAddress]);

  const submitAddress = (e: FormEvent) => {
    e.preventDefault();
    void connect(normalise(address));
  };

  const enter = (baseUrl: string, token: string, user: ServerUser) => {
    saveSession({ baseUrl, token, user });
    window.location.hash = "/";
    window.location.reload();
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!server) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      if (mode === "in") {
        const out = await signIn(server.baseUrl, username, password);
        enter(server.baseUrl, out.token, out.user);
        return;
      }
      const out = await signUp(server.baseUrl, username, email, password);
      if (out.status === "active" && out.token && out.user) {
        enter(server.baseUrl, out.token, out.user);
        return;
      }
      // Waiting for the emailed link or for an admin: back to signing in, with what happens next.
      setNotice(out.message ?? "Your account is waiting for approval.");
      setMode("in");
      setPassword("");
      setBusy(false);
    } catch (err) {
      setError(message(err));
      setBusy(false);
    }
  };

  const switchMode = () => {
    setMode(mode === "in" ? "up" : "in");
    setError("");
    setNotice("");
  };

  return (
    <div className="party-connect">
      <form className="card party-connect-card" onSubmit={(e) => void (server ? submit(e) : submitAddress(e))}>
        <h1>Borealis Party</h1>
        {!server && needsAddress && (
          <>
            <p className="muted">Enter the address of your server.</p>
            <label>
              Server address
              <input className="aur-text" autoFocus inputMode="url" autoCapitalize="none" spellCheck={false} placeholder="192.168.1.20:8787" value={address} onChange={(e) => setAddress(e.target.value)} required />
            </label>
            <button className="btn primary" disabled={busy}>Connect</button>
          </>
        )}
        {server && (
          <>
            {notice && <div className="alert success" role="status"><div>{notice}</div></div>}
            <label>
              {mode === "in" ? "Username or email" : "Username"}
              <input className="aur-text" autoFocus autoCapitalize="none" spellCheck={false} autoComplete="username" value={username} onChange={(e) => setUsername(e.target.value)} required />
            </label>
            {mode === "up" && (
              <label>
                Email
                <input className="aur-text" type="email" autoCapitalize="none" spellCheck={false} autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} required />
              </label>
            )}
            <label>
              Password
              <input className="aur-text" type="password" autoComplete={mode === "in" ? "current-password" : "new-password"} value={password} onChange={(e) => setPassword(e.target.value)} required minLength={mode === "up" ? 8 : undefined} />
            </label>
            {mode === "up" && (
              <p className="muted party-connect-note">
                {server.info.email ? "We'll email you a link to confirm your address. " : ""}
                {server.info.requireApproval ? "An admin approves new accounts before they can sign in." : ""}
              </p>
            )}
            <button className="btn primary" disabled={busy}>{mode === "in" ? "Sign in" : "Create account"}</button>
            {server.info.openRegistration && (
              <button type="button" className="btn" onClick={switchMode}>
                {mode === "in" ? "Create an account" : "I already have an account"}
              </button>
            )}
            {needsAddress && <button type="button" className="btn" onClick={() => setServer(null)}>Change server</button>}
          </>
        )}
        {error && <div className="alert danger" role="alert"><div>{error}</div></div>}
      </form>
    </div>
  );
}
