import { type FormEvent, useEffect, useState } from "react";
import { createTauriApi } from "../../src/api/tauri";
import { checkServer, endSession, signIn, signUp, type ServerInfo } from "../api/party";
import {
  dropSignInAgain, markExpired, normaliseAddress, rememberServer, rememberedServer, saveSession, type ServerSession, type ServerUser,
} from "../api/session";
import { isTauri } from "../../src/lib/tauri";
import { othersUnsentEdits, removeOtherCopies } from "../sync/account";
import { AppOffer } from "./AppOffer";
import { DiscardDialog } from "./SignOut";
import "./party.css";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

/** What the server answers a sign-in of an account an admin turned off or turned down. */
const BLOCKED = ["account_disabled", "account_rejected"];

/** A sign-in that waits for the answer to the discard dialog. */
interface Waiting { baseUrl: string; token: string; user: ServerUser; names: string[] }

/**
 * The first screen on a device with no session: pick the server (installed apps only), then sign in or sign up.
 * `again`: the server signed this device out while the app kept its copy (OfflineBanner); the same account picks the
 * copy up and its next sync sends the changes, and "Keep playing offline" goes back to it. In the apps, a sign-in as
 * another account removes the copies of the accounts before it, after a dialog when they have unsent changes.
 */
export function ConnectPage({ again }: { again?: ServerSession } = {}) {
  // A browser got this page from the server, so the server is already known.
  const needsAddress = isTauri();
  const [address, setAddress] = useState(again?.baseUrl ?? rememberedServer());
  const [server, setServer] = useState<{ baseUrl: string; info: ServerInfo } | null>(null);
  const [mode, setMode] = useState<"in" | "up">("in");
  const [username, setUsername] = useState(again ? again.user.email ?? again.user.username : "");
  const [waiting, setWaiting] = useState<Waiting | null>(null);
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

  const againAt = again?.baseUrl;
  useEffect(() => {
    if (againAt !== undefined) void connect(againAt);
    else if (!needsAddress) void connect("");
  }, [needsAddress, againAt]);

  const submitAddress = (e: FormEvent) => {
    e.preventDefault();
    void connect(normaliseAddress(address));
  };

  const go = (baseUrl: string, token: string, user: ServerUser) => {
    dropSignInAgain();
    saveSession({ baseUrl, token, user });
    window.location.hash = "/";
    window.location.reload();
  };

  const enter = async (baseUrl: string, token: string, user: ServerUser) => {
    if (isTauri()) {
      const local = createTauriApi();
      let names: string[];
      try { names = await othersUnsentEdits(local, user.id); } catch { names = ["The characters on this device (they could not be read)"]; }
      if (names.length > 0) {
        setWaiting({ baseUrl, token, user, names });
        return;
      }
      await removeOtherCopies(local, user.id);
    }
    go(baseUrl, token, user);
  };

  const answer = async (discard: boolean) => {
    const w = waiting;
    if (!w) return;
    setWaiting(null);
    if (discard) {
      await removeOtherCopies(createTauriApi(), w.user.id);
      go(w.baseUrl, w.token, w.user);
      return;
    }
    // Not this account after all: the session just made ends, and the other account's changes stay.
    await endSession(w);
    setPassword("");
    setBusy(false);
  };

  const keepPlaying = () => {
    dropSignInAgain();
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
        await enter(server.baseUrl, out.token, out.user);
        return;
      }
      const out = await signUp(server.baseUrl, username, email, password);
      if (out.status === "active" && out.token && out.user) {
        await enter(server.baseUrl, out.token, out.user);
        return;
      }
      // Waiting for the emailed link or for an admin: back to signing in, with what happens next.
      setNotice(out.message ?? "Your account is waiting for approval.");
      setMode("in");
      setPassword("");
      setBusy(false);
    } catch (err) {
      // The account itself was turned off: the copy keeps its changes, and the banner says why.
      const typed = username.trim().toLowerCase();
      const same = again && [again.user.username, again.user.email].some((n) => n?.toLowerCase() === typed);
      if (same && BLOCKED.includes(String((err as { code?: unknown }).code))) markExpired(true);
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
        {again && (
          <p className="muted">
            Sign in again as {again.user.username} to send the changes made on this device. Until then they stay here.
          </p>
        )}
        {!server && needsAddress && (
          <>
            <p className="muted">Enter the address of your server.</p>
            <label>
              Server address
              <input className="aur-text" autoFocus inputMode="url" autoCapitalize="none" spellCheck={false} placeholder="party.example.com" value={address} onChange={(e) => setAddress(e.target.value)} required />
            </label>
            <button className="btn primary" disabled={busy}>Connect</button>
          </>
        )}
        {server && (
          <>
            {!needsAddress && <AppOffer folder={server.info.appDownload ?? null} />}
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
        {again && <button type="button" className="btn" onClick={keepPlaying}>Keep playing offline</button>}
      </form>
      {waiting && (
        <DiscardDialog
          title={`Sign in as ${waiting.user.username}`}
          intro={`${again && again.user.id !== waiting.user.id ? `${again.user.username}'s` : "Another account's"} characters on this device have changes the server doesn't have yet. Signing in as ${waiting.user.username} removes them from this device and the changes are lost.`}
          names={waiting.names}
          action="Discard and sign in"
          onResult={(discard) => void answer(discard)}
        />
      )}
    </div>
  );
}
