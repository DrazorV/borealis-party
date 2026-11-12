import { serverFetch } from "./http";
import { loadSession, type ServerSession, type ServerUser } from "./session";

export interface ServerInfo {
  app: string;
  version: string;
  openRegistration: boolean;
  requireApproval: boolean;
  /** The server sends a confirmation email at sign-up. */
  email: boolean;
}

interface SignedIn {
  token: string;
  user: ServerUser;
}

/** A sign-up: signed in when the account is active at once, otherwise what it waits for. */
interface SignUpResult {
  status: "active" | "pending";
  token?: string;
  user?: ServerUser;
  awaitingEmail?: boolean;
  message?: string;
}

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject(new Error("Not signed in."));
  return serverFetch(session, method, path, body) as Promise<T>;
}

/** Checks that an address really is one of our servers. */
export async function checkServer(baseUrl: string): Promise<ServerInfo> {
  const info = (await serverFetch({ baseUrl, token: "" }, "GET", "/api/server")) as ServerInfo | null;
  if (info?.app !== "borealis-party") throw new Error("That address isn't a Borealis Party server.");
  return info;
}

/**
 * Whether the server at `baseUrl` knows this sign-in as the same account, so it is the same server under another
 * address. A refusal there answers false and, unlike every other request, does not sign this device out.
 */
export async function sameAccountAt(baseUrl: string, token: string, userId: string): Promise<boolean> {
  let response: Response;
  try {
    response = await fetch(`${baseUrl}/api/me`, { headers: { authorization: `Bearer ${token}` } });
  } catch {
    throw new Error("The server can't be reached. Check your connection.");
  }
  if (!response.ok) return false;
  const user = (await response.json().catch(() => null)) as Partial<ServerUser> | null;
  return user?.id === userId;
}

export const signIn = (baseUrl: string, username: string, password: string) =>
  serverFetch({ baseUrl, token: "" }, "POST", "/api/auth/login", { username, password }) as Promise<SignedIn>;

export const signUp = (baseUrl: string, username: string, email: string, password: string) =>
  serverFetch({ baseUrl, token: "" }, "POST", "/api/auth/register", { username, email, password }) as Promise<SignUpResult>;

export const signOut = () => call<void>("POST", "/api/auth/logout");

/**
 * End a session on the server while this device leaves it, whatever the answer: unreachable, or a token the server
 * already refuses, changes nothing here (unlike every other request, nothing reloads).
 */
export async function endSession(session: Pick<ServerSession, "baseUrl" | "token">, timeoutMs = 4000): Promise<void> {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), timeoutMs);
  try {
    await fetch(`${session.baseUrl}/api/auth/logout`, { method: "POST", headers: { authorization: `Bearer ${session.token}` }, signal: abort.signal });
  } catch { /* offline: the session ends on the server when unused for 90 days */ } finally {
    clearTimeout(timer);
  }
}
export const fetchMe = () => call<ServerUser>("GET", "/api/me");
export const changeEmail = (email: string) => call<ServerUser>("PATCH", "/api/me", { email });
export const changePassword = (current: string, password: string) => call<ServerUser>("POST", "/api/me/password", { current, password });

// The admin page.

export type AccountStatus = "pending" | "active" | "rejected" | "disabled";

export interface Account {
  id: string;
  username: string;
  email: string | null;
  admin: boolean;
  mustChangePassword: boolean;
  status: AccountStatus;
  createdAt: number;
  emailConfirmedAt: number | null;
  awaitingEmail: boolean;
  sessions: number;
}

export interface Paged<T> {
  items: T[];
  total: number;
}

export interface ServerStats {
  accounts: Record<AccountStatus | "total", number>;
  admins: number;
  characters: number;
  contentPacks: number;
  activeSessions: number;
  diskBytes: number;
  version: string;
  database: "sqlite" | "postgres";
  uptimeSeconds: number;
  email: boolean;
}

interface SignUpSettings {
  openRegistration: boolean;
  requireApproval: boolean;
}

export interface AdminSettings extends SignUpSettings {
  configFile: SignUpSettings;
  email: boolean;
  publicUrl: string | null;
}

export interface ActivityEntry {
  at: number;
  kind: string;
  actorId: string | null;
  actorName: string | null;
  targetId: string | null;
  targetName: string | null;
  detail: string | null;
  ip: string | null;
}

const query = (params: Record<string, string | number | undefined>): string => {
  const pairs = Object.entries(params).filter((p): p is [string, string | number] => p[1] !== undefined && p[1] !== "");
  return pairs.length ? `?${new URLSearchParams(pairs.map(([k, v]) => [k, String(v)])).toString()}` : "";
};

export const adminStats = () => call<ServerStats>("GET", "/api/admin/stats");
export const adminAccounts = (search: string, status: AccountStatus | "", page: number, perPage: number) =>
  call<Paged<Account>>("GET", `/api/admin/accounts${query({ search, status, page, perPage })}`);
export const setAccountStatus = (id: string, status: AccountStatus) => call<Account>("POST", `/api/admin/accounts/${id}/status`, { status });
export const setAccountAdmin = (id: string, admin: boolean) => call<Account>("POST", `/api/admin/accounts/${id}/admin`, { admin });
/** With a password, that becomes a temporary one; without, the current one has to be changed at the next sign-in. */
export const resetPassword = (id: string, password?: string) => call<Account>("POST", `/api/admin/accounts/${id}/password`, password ? { password } : {});
export const signOutEverywhere = (id: string) => call<{ ended: number }>("POST", `/api/admin/accounts/${id}/sign-out`, {});
export const deleteAccount = (id: string) => call<void>("DELETE", `/api/admin/accounts/${id}`);
export const adminSettings = () => call<AdminSettings>("GET", "/api/admin/settings");
export const saveAdminSettings = (changes: Partial<SignUpSettings>) => call<AdminSettings>("PATCH", "/api/admin/settings", changes);
export const adminActivity = (page: number, perPage: number) => call<Paged<ActivityEntry>>("GET", `/api/admin/activity${query({ page, perPage })}`);
