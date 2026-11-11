import { type FormEvent, useState } from "react";
import { changePassword, signOut } from "../api/party";
import { clearSession, loadSession, updateUser } from "../api/session";
import "./party.css";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

/** An admin reset this account's password: a new one comes before anything else. */
export function ChangePasswordPage() {
  const session = loadSession();
  const [current, setCurrent] = useState("");
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (password !== again) {
      setError("The two new passwords don't match.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      updateUser(await changePassword(current, password));
      window.location.hash = "/";
      window.location.reload();
    } catch (err) {
      setError(message(err));
      setBusy(false);
    }
  };

  const leave = async () => {
    try { await signOut(); } catch { /* the token goes either way */ }
    clearSession();
    window.location.hash = "/";
    window.location.reload();
  };

  return (
    <div className="party-connect">
      <form className="card party-connect-card" onSubmit={(e) => void submit(e)}>
        <h1>Choose a new password</h1>
        <p className="muted">An admin asked {session?.user.username ?? "you"} to choose a new password before going on.</p>
        <label>
          Current or temporary password
          <input className="aur-text" type="password" autoFocus autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} required />
        </label>
        <label>
          New password
          <input className="aur-text" type="password" autoComplete="new-password" minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} required />
        </label>
        <label>
          New password again
          <input className="aur-text" type="password" autoComplete="new-password" minLength={8} value={again} onChange={(e) => setAgain(e.target.value)} required />
        </label>
        <button className="btn primary" disabled={busy}>Save password</button>
        <button type="button" className="btn" onClick={() => void leave()}>Sign out</button>
        {error && <div className="alert danger" role="alert"><div>{error}</div></div>}
      </form>
    </div>
  );
}
