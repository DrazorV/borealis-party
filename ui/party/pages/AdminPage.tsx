import { type FormEvent, useCallback, useEffect, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip, type ChipTone, EmptyState } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { Check, SearchInput } from "../../src/components/Fields";
import { Modal } from "../../src/components/Modal";
import { Segmented, Tabs } from "../../src/components/Tabs";
import { useToast } from "../../src/components/Toast";
import {
  adminAccounts, adminActivity, adminSettings, adminStats, deleteAccount, resetPassword, saveAdminSettings, setAccountAdmin,
  setAccountStatus, signOutEverywhere,
  type Account, type AccountStatus, type ActivityEntry, type AdminSettings, type Paged, type ServerStats,
} from "../api/party";
import { loadSession } from "../api/session";
import "./party.css";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

const PER_PAGE = 25;

type Section = "accounts" | "stats" | "settings" | "activity";

const STATUS_TONE: Record<AccountStatus, ChipTone> = { pending: "warning", active: "success", rejected: "danger", disabled: "default" };
const STATUS_LABEL: Record<AccountStatus, string> = { pending: "Waiting", active: "Active", rejected: "Rejected", disabled: "Disabled" };

const when = (seconds: number): string => new Date(seconds * 1000).toLocaleString();

function bytes(n: number): string {
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${unit === 0 ? value : value.toFixed(1)} ${units[unit]}`;
}

function duration(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `${days} d ${hours} h`;
  if (hours > 0) return `${hours} h ${minutes} min`;
  return `${minutes} min`;
}

/** The admin page: accounts, statistics, sign-up settings and the activity log. */
export function AdminPage() {
  const [section, setSection] = useState<Section>("accounts");
  return (
    <div className="party party-admin">
      <Tabs
        label="Admin"
        idPrefix="admin"
        active={section}
        onChange={(id) => setSection(id as Section)}
        tabs={[
          { id: "accounts", label: "Accounts" },
          { id: "stats", label: "Statistics" },
          { id: "settings", label: "Settings" },
          { id: "activity", label: "Activity" },
        ]}
      />
      {section === "accounts" && <Accounts />}
      {section === "stats" && <Statistics />}
      {section === "settings" && <Settings />}
      {section === "activity" && <ActivityLog />}
    </div>
  );
}

function Pager({ page, total, onPage }: { page: number; total: number; onPage: (page: number) => void }) {
  const pages = Math.max(1, Math.ceil(total / PER_PAGE));
  if (pages <= 1) return null;
  return (
    <div className="party-inline party-pager">
      <Button small disabled={page === 0} onClick={() => onPage(page - 1)}>Newer</Button>
      <span className="muted">Page {page + 1} of {pages}</span>
      <Button small disabled={page + 1 >= pages} onClick={() => onPage(page + 1)}>Older</Button>
    </div>
  );
}

function Accounts() {
  const toast = useToast();
  const [search, setSearch] = useState("");
  const [status, setStatus] = useState<AccountStatus | "">("");
  const [page, setPage] = useState(0);
  const [list, setList] = useState<Paged<Account> | null>(null);
  const [waiting, setWaiting] = useState<Account[]>([]);
  const [error, setError] = useState("");
  const [resetting, setResetting] = useState<Account | null>(null);

  const refresh = useCallback(() => {
    adminAccounts(search, status, page, PER_PAGE).then((l) => { setList(l); setError(""); }, (e) => setError(message(e)));
    adminAccounts("", "pending", 0, 200).then((l) => setWaiting(l.items), () => setWaiting([]));
  }, [search, status, page]);
  useEffect(refresh, [refresh]);

  const act = async (work: () => Promise<unknown>, done?: string) => {
    try {
      await work();
      if (done) toast.push(done, "info");
      refresh();
    } catch (e) { toast.push(message(e), "error"); }
  };

  return (
    <>
      {error && <Alert tone="danger">{error}</Alert>}
      <Card title={`Waiting for approval${waiting.length ? ` (${waiting.length})` : ""}`}>
        {waiting.length === 0 && <EmptyState title="Nobody is waiting">New sign-ups appear here until you approve or reject them.</EmptyState>}
        <ul className="party-list">
          {waiting.map((a) => (
            <li key={a.id} className="party-line">
              <span>
                <strong>{a.username}</strong> <span className="muted">{a.email ?? "no email"} · asked {when(a.createdAt)}</span>
                {a.awaitingEmail && <> <Chip tone="info">Email not confirmed yet</Chip></>}
              </span>
              <span className="spacer" />
              <Button small variant="primary" onClick={() => void act(() => setAccountStatus(a.id, "active"), `${a.username} approved.`)}>Approve</Button>
              <Button small onClick={() => void act(() => setAccountStatus(a.id, "rejected"), `${a.username} rejected.`)}>Reject</Button>
            </li>
          ))}
        </ul>
      </Card>
      <Card title="All accounts">
        <div className="party-inline">
          <SearchInput label="Search accounts" placeholder="Username or email" value={search} onChange={(v) => { setSearch(v); setPage(0); }} />
          <Segmented<AccountStatus | "">
            label="Status"
            value={status}
            onChange={(v) => { setStatus(v); setPage(0); }}
            options={[{ id: "", label: "All" }, { id: "active", label: "Active" }, { id: "pending", label: "Waiting" }, { id: "disabled", label: "Disabled" }, { id: "rejected", label: "Rejected" }]}
          />
        </div>
        {list?.items.length === 0 && <EmptyState title="No accounts match" />}
        <ul className="party-list">
          {list?.items.map((a) => <AccountRow key={a.id} account={a} act={act} onReset={() => setResetting(a)} />)}
        </ul>
        {list && <Pager page={page} total={list.total} onPage={setPage} />}
      </Card>
      {resetting && <ResetPassword account={resetting} onClose={() => setResetting(null)} onDone={refresh} />}
    </>
  );
}

function AccountRow({ account: a, act, onReset }: { account: Account; act: (work: () => Promise<unknown>, done?: string) => Promise<void>; onReset: () => void }) {
  const confirm = useConfirm();
  const me = loadSession()?.user.id;
  const remove = async () => {
    const answer = await confirm({
      title: "Delete account",
      body: <>Delete <strong>{a.username}</strong> and all their characters? Groups they run are deleted too, and those groups' characters go back to their players. This can't be undone.</>,
      confirmLabel: "Delete account",
      danger: true,
    });
    if (answer === "confirm") await act(() => deleteAccount(a.id), `${a.username} deleted.`);
  };
  const disable = async () => {
    const answer = await confirm({ title: "Disable account", body: `Disable ${a.username}? They are signed out everywhere and can't sign in until you enable the account again.`, confirmLabel: "Disable", danger: true });
    if (answer === "confirm") await act(() => setAccountStatus(a.id, "disabled"), `${a.username} disabled.`);
  };
  return (
    <li className="party-member">
      <div className="party-member-head">
        <div>
          <strong>{a.username}</strong>{a.id === me && <span className="muted"> (you)</span>}
          <div className="muted">{a.email ?? "no email"} · since {when(a.createdAt)} · {a.sessions} {a.sessions === 1 ? "session" : "sessions"}</div>
        </div>
        <span className="spacer" />
        {a.admin && <Chip tone="gold">Admin</Chip>}
        {a.mustChangePassword && <Chip tone="info">New password due</Chip>}
        <Chip tone={STATUS_TONE[a.status]}>{STATUS_LABEL[a.status]}</Chip>
      </div>
      <div className="party-inline">
        {a.status === "pending" && <Button small variant="primary" onClick={() => void act(() => setAccountStatus(a.id, "active"), `${a.username} approved.`)}>Approve</Button>}
        {a.status === "pending" && <Button small onClick={() => void act(() => setAccountStatus(a.id, "rejected"), `${a.username} rejected.`)}>Reject</Button>}
        {a.status === "active" && <Button small onClick={() => void disable()}>Disable</Button>}
        {(a.status === "disabled" || a.status === "rejected") && <Button small onClick={() => void act(() => setAccountStatus(a.id, "active"), `${a.username} enabled.`)}>Enable</Button>}
        {a.status === "active" && (a.admin
          ? <Button small onClick={() => void act(() => setAccountAdmin(a.id, false), `${a.username} is no longer an admin.`)}>Remove admin</Button>
          : <Button small onClick={() => void act(() => setAccountAdmin(a.id, true), `${a.username} is now an admin.`)}>Make admin</Button>)}
        <Button small onClick={onReset}>Reset password</Button>
        <Button small disabled={a.sessions === 0} onClick={() => void act(() => signOutEverywhere(a.id), `${a.username} signed out everywhere.`)}>Sign out everywhere</Button>
        <span className="spacer" />
        <Button small variant="danger" onClick={() => void remove()}>Delete</Button>
      </div>
    </li>
  );
}

function ResetPassword({ account, onClose, onDone }: { account: Account; onClose: () => void; onDone: () => void }) {
  const toast = useToast();
  const [temporary, setTemporary] = useState(true);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    setBusy(true);
    try {
      await resetPassword(account.id, temporary ? password : undefined);
      toast.push(temporary
        ? `${account.username} is signed out. Give them the temporary password; they choose a new one when they sign in.`
        : `${account.username} is signed out and chooses a new password when they next sign in.`, "info");
      onDone();
      onClose();
    } catch (err) {
      toast.push(message(err), "error");
      setBusy(false);
    }
  };
  return (
    <Modal
      title={`Reset ${account.username}'s password`}
      onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || (temporary && password.length < 8)} onClick={() => void submit()}>Reset password</Button>
      </>}
    >
      <form className="party-form" onSubmit={(e) => void submit(e)}>
        <Check label="Set a temporary password" description="You tell them this password. Without one, they keep their current password but must change it at their next sign-in." checked={temporary} onChange={(e) => setTemporary(e.target.checked)} />
        {temporary && (
          <input className="aur-text" type="text" aria-label="Temporary password" placeholder="Temporary password (8 or more characters)" autoComplete="off" spellCheck={false} value={password} onChange={(e) => setPassword(e.target.value)} minLength={8} required />
        )}
        <p className="muted">Either way, they are signed out on every device now.</p>
      </form>
    </Modal>
  );
}

function Statistics() {
  const [stats, setStats] = useState<ServerStats | null>(null);
  const [error, setError] = useState("");
  useEffect(() => { adminStats().then(setStats, (e) => setError(message(e))); }, []);
  if (error) return <Alert tone="danger">{error}</Alert>;
  if (!stats) return null;
  const tiles: [string, string | number][] = [
    ["Active accounts", stats.accounts.active],
    ["Waiting for approval", stats.accounts.pending],
    ["Disabled", stats.accounts.disabled],
    ["Rejected", stats.accounts.rejected],
    ["Admins", stats.admins],
    ["Groups", stats.groups],
    ["Characters", stats.characters],
    ["Characters in groups", stats.groupCharacters],
    ["Content packs", stats.contentPacks],
    ["Open invitations", stats.invites],
    ["Active sessions", stats.activeSessions],
    ["Data folder", bytes(stats.diskBytes)],
  ];
  return (
    <>
      <Card title="Statistics">
        <dl className="party-stats party-admin-stats">
          {tiles.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}
        </dl>
      </Card>
      <Card title="Server">
        <dl className="party-facts">
          <div><dt>Version</dt><dd>{stats.version}</dd></div>
          <div><dt>Database</dt><dd>{stats.database === "postgres" ? "PostgreSQL" : "SQLite"}</dd></div>
          <div><dt>Running for</dt><dd>{duration(stats.uptimeSeconds)}</dd></div>
          <div><dt>Sign-up email</dt><dd>{stats.email ? "Set up" : "Not set up"}</dd></div>
        </dl>
      </Card>
    </>
  );
}

function Settings() {
  const toast = useToast();
  const [settings, setSettings] = useState<AdminSettings | null>(null);
  const [error, setError] = useState("");
  useEffect(() => { adminSettings().then(setSettings, (e) => setError(message(e))); }, []);
  const save = async (changes: Partial<AdminSettings>) => {
    try {
      setSettings(await saveAdminSettings(changes));
      toast.push("Saved. It applies now.", "info");
    } catch (e) { toast.push(message(e), "error"); }
  };
  if (error) return <Alert tone="danger">{error}</Alert>;
  if (!settings) return null;
  return (
    <Card title="Sign-up">
      <div className="party-form">
        <Check
          label="Anyone can ask for an account"
          description="Off: new accounts come only from the add-user command on the server."
          checked={settings.openRegistration}
          onChange={(e) => void save({ openRegistration: e.target.checked })}
        />
        <Check
          label="New accounts wait for an admin"
          description="Off: a new account can sign in straight away (after confirming its email, when email is set up)."
          checked={settings.requireApproval}
          onChange={(e) => void save({ requireApproval: e.target.checked })}
        />
        <p className="muted">
          These are saved on the server and win over config.toml, which says: sign-up {settings.configFile.openRegistration ? "on" : "off"},
          approval {settings.configFile.requireApproval ? "required" : "not required"}.
        </p>
        <p className="muted">
          {settings.email
            ? `Sign-up email is set up: new accounts confirm their address through a link to ${settings.publicUrl ?? "this server"}, and admins with an email address hear about each request.`
            : "Sign-up email is not set up ([smtp] in config.toml), so addresses aren't confirmed: approval is the check."}
        </p>
      </div>
    </Card>
  );
}

const KIND_LABEL: Record<string, string> = {
  sign_up: "Signed up",
  email_confirmed: "Confirmed email",
  email_changed: "Changed email",
  sign_in: "Signed in",
  sign_in_failed: "Failed sign-in",
  approve: "Approved",
  reject: "Rejected",
  disable: "Disabled",
  enable: "Enabled",
  admin_granted: "Made admin",
  admin_removed: "Removed admin",
  password_reset: "Reset password",
  password_changed: "Changed password",
  signed_out_everywhere: "Signed out everywhere",
  account_deleted: "Deleted account",
  group_created: "Created group",
  group_deleted: "Deleted group",
  setting_changed: "Changed setting",
};

function describe(entry: ActivityEntry): string {
  const parts = [entry.actorName, KIND_LABEL[entry.kind] ?? entry.kind, entry.targetName].filter(Boolean);
  return parts.join(" · ");
}

function ActivityLog() {
  const [page, setPage] = useState(0);
  const [log, setLog] = useState<Paged<ActivityEntry> | null>(null);
  const [error, setError] = useState("");
  useEffect(() => { adminActivity(page, PER_PAGE).then(setLog, (e) => setError(message(e))); }, [page]);
  if (error) return <Alert tone="danger">{error}</Alert>;
  return (
    <Card title="Activity">
      {log?.items.length === 0 && <EmptyState title="Nothing yet" />}
      <ul className="party-list party-activity">
        {log?.items.map((e, i) => (
          <li key={`${e.at}-${i}`} className={`party-line ${e.kind === "sign_in_failed" ? "party-activity-warn" : ""}`}>
            <span className="muted party-activity-time">{when(e.at)}</span>
            <span>{describe(e)}{e.detail ? <span className="muted"> · {e.detail}</span> : null}</span>
            <span className="spacer" />
            {e.ip && <span className="muted">{e.ip}</span>}
          </li>
        ))}
      </ul>
      {log && <Pager page={page} total={log.total} onPage={setPage} />}
      <p className="muted">The newest 10,000 entries are kept.</p>
    </Card>
  );
}
