import { serverFetch } from "./http";
import { loadSession, type ServerSession, type ServerUser } from "./session";

export interface ServerInfo {
  app: string;
  version: string;
  openRegistration: boolean;
  requireApproval: boolean;
  /** The server sends a confirmation email at sign-up. */
  email: boolean;
  /** The folder the apps are downloaded from (`server.app_download`); null when the server offers none. */
  appDownload?: string | null;
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
  if (info?.app !== "borealis-party" && info?.app !== "borealis-connect") throw new Error("That address isn't a Borealis Connect server.");
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

export const signUp = (baseUrl: string, username: string, email: string, password: string, joinCode?: string) =>
  serverFetch({ baseUrl, token: "" }, "POST", "/api/auth/register", { username, email, password, ...(joinCode ? { joinCode } : {}) }) as Promise<SignUpResult>;

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
/** What "Sign out everywhere" ended: other sessions, personal API tokens and connected AI apps. */
export interface SignedOutEverywhere { sessions: number; apiTokens: number; apps: number }
/** Ends every other session, API token and connected AI app of the account; this one stays signed in. */
export const signOutEverywhereElse = () => call<SignedOutEverywhere>("POST", "/api/me/sign-out-everywhere", {});

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
  /** Share codes: counts and bytes only (absent on servers before 0.1.5). */
  shares?: ShareStats;
  /** The automatic backup before upgrades: the newest one's time (seconds) and size (absent before 0.3.1). */
  backup?: { last: { at: number; bytes: number } | null; count: number } | null;
}

export interface ShareStats {
  enabled: boolean;
  byStatus: Record<"pending" | "claimed" | "done" | "declined" | "cancelled" | "expired", number>;
  blocks: number;
  outgoingBytes: number;
  stagedBytes: number;
  maxBytes: number;
  diskBytesPerAccount: number;
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
/** Ends every session, API token and connected AI app of the account. */
export const signOutEverywhere = (id: string) => call<{ ended: number; apiTokens: number; apps: number }>("POST", `/api/admin/accounts/${id}/sign-out`, {});
export const deleteAccount = (id: string) => call<void>("DELETE", `/api/admin/accounts/${id}`);
export const adminSettings = () => call<AdminSettings>("GET", "/api/admin/settings");
export const saveAdminSettings = (changes: Partial<SignUpSettings>) => call<AdminSettings>("PATCH", "/api/admin/settings", changes);
/** Admin › Settings › Trusted proxies, with how the server sees the admin's own request. */
export interface ProxySettings {
  /** In force. */
  proxies: string[];
  /** Saved on the admin page, from config.toml, or neither. */
  source: "admin" | "config" | "none";
  /** `server.trusted_proxies` in config.toml or the environment. */
  configFile: string[];
  /** The address that connected for this request. */
  peer: string;
  peerTrusted: boolean;
  headers: { cfConnectingIp: string | null; xForwardedFor: string | null };
  /** The client the server takes this request to come from. */
  clientIp: string;
}

export const adminProxies = () => call<ProxySettings>("GET", "/api/admin/proxies");
/** Saves the list, or with null goes back to config.toml. */
export const saveAdminProxies = (proxies: string[] | null) => call<ProxySettings>("POST", "/api/admin/proxies", { proxies });
export const adminActivity = (page: number, perPage: number) => call<Paged<ActivityEntry>>("GET", `/api/admin/activity${query({ page, perPage })}`);
/** A deleted character in an account's trash: names and dates only, never the sheet. */
export interface TrashItem {
  accountId: string;
  username: string;
  kind: "character";
  characterId: string;
  /** null when the kept copy doesn't read. */
  name: string | null;
  deletedAtMs: number;
  purgeAtMs: number;
}
export const adminTrash = () => call<{ items: TrashItem[]; retentionDays: number }>("GET", "/api/admin/trash");
export const restoreFromTrash = (accountId: string, characterId: string) =>
  call<{ characterId: string; name: string | null }>("POST", "/api/admin/trash/restore", { accountId, characterId });
