import { type FormEvent, useState } from "react";
import { changePassword } from "../api/party";
import { loadSession, updateUser } from "../api/session";
import { useSignOut } from "./SignOut";
import "./party.css";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

/**
 * An admin reset this account's password: a new one comes before anything else, the app's sync too (the server
 * refuses it until then; the reload after the change starts it).
 */
export function ChangePasswordPage() {
  const session = loadSession();
  const leave = useSignOut();
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
        <button type="button" className="btn" onClick={leave.start} disabled={leave.busy}>Sign out</button>
        {error && <div className="alert danger" role="alert"><div>{error}</div></div>}
      </form>
      {leave.dialog}
    </div>
  );
}
