import { type FormEvent, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { useToast } from "../../src/components/Toast";
import { isTauri } from "../../src/lib/tauri";
import { changeEmail, changePassword, checkServer, sameAccountAt } from "../api/party";
import { isLocalAddress, loadSession, normaliseAddress, setServerAddress, updateUser } from "../api/session";
import { clearNotices, loadMirror, loadNotices } from "../sync/mirror";
import { useSyncStatus } from "../sync/runner";
import { AiAppsCard } from "./AiApps";
import { DungeonMasterCard } from "./PartiesDm";
import { YourAiCard } from "./YourAiCard";
import "./party.css";

/**
 * The app's copy of the account: what the last sync could not do, and edits made here that the server did not take
 * (their copies are in the account's trash on the server). Nothing when all is well.
 */
export function SyncNotes({ userId }: { userId: string }) {
  // Read again after each sync, and after Dismiss.
  useSyncStatus();
  const [, setDismissed] = useState(0);
  const notices = loadNotices(userId);
  const problems = Object.values(loadMirror(userId)?.problems ?? {});
  if (problems.length === 0 && notices.length === 0) return null;
  return (
    <Card title="Sync">
      {problems.map((p) => <Alert key={p} tone="warning">{p}</Alert>)}
      {notices.map((n, i) => <Alert key={`${i}:${n}`} tone="info">{n}</Alert>)}
      {notices.length > 0 && (
        <div className="party-inline">
          <Button onClick={() => { clearNotices(userId); setDismissed((n) => n + 1); }}>Dismiss</Button>
        </div>
      )}
    </Card>
  );
}

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

/**
 * A new address for the same server (an IP address that became a domain name). Kept only when the server there
 * knows this sign-in as this account, so the device stays signed in; then the page reloads onto it.
 * Answers why the address was not kept, or null.
 */
export async function moveToAddress(typed: string): Promise<string | null> {
  const session = loadSession();
  if (!session) return "Not signed in.";
  const baseUrl = normaliseAddress(typed);
  if (baseUrl === session.baseUrl) return "That is already the server's address.";
  // The sign-in is sent to the new address: never unencrypted to a host outside the home network.
  if (baseUrl.startsWith("http://") && !isLocalAddress(baseUrl)) return "Use https:// for a domain name.";
  await checkServer(baseUrl);
  if (!(await sameAccountAt(baseUrl, session.token, session.user.id))) {
    return "That server doesn't know this account. To use another server, sign out and sign in there.";
  }
  setServerAddress(baseUrl);
  window.location.reload();
  return null;
}

/** The signed-in account: its email address and its password, and in the apps the server's address. */
export function AccountPage() {
  const toast = useToast();
  const session = loadSession();
  const user = session?.user;
  const [email, setEmail] = useState(user?.email ?? "");
  const [hasEmail, setHasEmail] = useState(Boolean(user?.email));
  const [current, setCurrent] = useState("");
  const [password, setPassword] = useState("");
  const [address, setAddress] = useState(session?.baseUrl ?? "");
  const [moving, setMoving] = useState(false);

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
  const saveAddress = async (e: FormEvent) => {
    e.preventDefault();
    setMoving(true);
    try {
      const refused = await moveToAddress(address);
      if (refused) toast.push(refused, "error");
    } catch (err) { toast.push(message(err), "error"); }
    finally { setMoving(false); }
  };

  return (
    <div className="party">
      <Card title="My account">
        <p className="muted">Signed in as <strong>{user?.username}</strong>{user?.admin ? " · admin" : ""}</p>
        {!hasEmail && <Alert tone="info">Add an email address, so an admin can reach you.</Alert>}
        <form className="party-inline" onSubmit={(e) => void saveEmail(e)}>
          <input className="input" type="email" aria-label="Email address" placeholder="Email address" autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} required />
          <Button type="submit">Save email</Button>
        </form>
        <form className="party-inline" onSubmit={(e) => void savePassword(e)}>
          <input className="input" type="password" aria-label="Current password" placeholder="Current password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} required />
          <input className="input" type="password" aria-label="New password" placeholder="New password" autoComplete="new-password" minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} required />
          <Button type="submit">Change password</Button>
        </form>
      </Card>
      <DungeonMasterCard />
      <AiAppsCard />
      <YourAiCard />
      {isTauri() && user && <SyncNotes userId={user.id} />}
      {/* A browser got the page from the server itself, so only the apps have an address to change. */}
      {isTauri() && (
        <Card title="Server address">
          <p className="muted">If your server moves to a new address, enter it here. You stay signed in.</p>
          <form className="party-inline" onSubmit={(e) => void saveAddress(e)}>
            <input className="input" aria-label="Server address" inputMode="url" autoCapitalize="none" spellCheck={false} placeholder="party.example.com" value={address} onChange={(e) => setAddress(e.target.value)} required />
            <Button type="submit" disabled={moving}>Change address</Button>
          </form>
        </Card>
      )}
    </div>
  );
}
