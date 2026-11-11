import { type FormEvent, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { useToast } from "../../src/components/Toast";
import { changeEmail, changePassword } from "../api/party";
import { loadSession, updateUser } from "../api/session";
import "./party.css";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

/** The signed-in account: its email address and its password. */
export function AccountPage() {
  const toast = useToast();
  const user = loadSession()?.user;
  const [email, setEmail] = useState(user?.email ?? "");
  const [hasEmail, setHasEmail] = useState(Boolean(user?.email));
  const [current, setCurrent] = useState("");
  const [password, setPassword] = useState("");

  const saveEmail = async (e: FormEvent) => {
    e.preventDefault();
    try {
      const saved = await changeEmail(email);
      updateUser(saved);
      setHasEmail(Boolean(saved.email));
      toast.push("Email address saved.", "info");
    } catch (err) { toast.push(message(err), "error"); }
  };
  const savePassword = async (e: FormEvent) => {
    e.preventDefault();
    try {
      updateUser(await changePassword(current, password));
      setCurrent("");
      setPassword("");
      toast.push("Password changed. Your other devices are signed out.", "info");
    } catch (err) { toast.push(message(err), "error"); }
  };

  return (
    <div className="party">
      <Card title="My account">
        <p className="muted">Signed in as <strong>{user?.username}</strong>{user?.admin ? " · admin" : ""}</p>
        {!hasEmail && <Alert tone="info">Add an email address, so an admin can reach you.</Alert>}
        <form className="party-inline" onSubmit={(e) => void saveEmail(e)}>
          <input className="aur-text" type="email" aria-label="Email address" placeholder="Email address" autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} required />
          <Button type="submit">Save email</Button>
        </form>
        <form className="party-inline" onSubmit={(e) => void savePassword(e)}>
          <input className="aur-text" type="password" aria-label="Current password" placeholder="Current password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} required />
          <input className="aur-text" type="password" aria-label="New password" placeholder="New password" autoComplete="new-password" minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} required />
          <Button type="submit">Change password</Button>
        </form>
      </Card>
    </div>
  );
}
