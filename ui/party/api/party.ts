import { serverFetch } from "./http";
import { loadSession, type ServerUser } from "./session";

export interface GroupSummary {
  id: string;
  name: string;
  dmName: string;
  memberCount: number;
  characterId: string | null;
  /** This account runs the group; otherwise it plays in it. */
  isDm: boolean;
}

export interface GroupDetail {
  id: string;
  name: string;
  dmName: string;
  isDm: boolean;
  members: { playerId: string; username: string; hasCharacter: boolean }[];
  invites: { id: string; username: string }[];
  characterId?: string;
}

export interface Invitation {
  id: string;
  groupName: string;
  dmName: string;
}

export interface OwnedCharacter {
  id: string;
  name: string;
  build: string;
  level: number;
  scope: string;
  groupName: string | null;
}

export interface PartyRow {
  playerId: string;
  username: string;
  characterId: string | null;
  unreadable?: boolean;
  name?: string;
  ruleset?: string;
  portrait?: string | null;
  level?: {
    total: number;
    xp: number;
    nextLevelXp: number | null;
    currentLevelXp: number;
    levelUpAvailable: boolean;
    classes: { name: string | null; level: number; subclass: string | null }[];
  };
  stats?: { armorClass: number; maxHp: number; initiative: number; speed: number; passivePerception: number; proficiencyBonus: number };
  hitPoints?: { current: number; max: number; temp: number };
  progress?: { remaining: number };
}

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
export interface SignUpResult {
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

export const signIn = (baseUrl: string, username: string, password: string) =>
  serverFetch({ baseUrl, token: "" }, "POST", "/api/auth/login", { username, password }) as Promise<SignedIn>;

export const signUp = (baseUrl: string, username: string, email: string, password: string) =>
  serverFetch({ baseUrl, token: "" }, "POST", "/api/auth/register", { username, email, password }) as Promise<SignUpResult>;

export const signOut = () => call<void>("POST", "/api/auth/logout");
export const fetchMe = () => call<ServerUser>("GET", "/api/me");
export const changeEmail = (email: string) => call<ServerUser>("PATCH", "/api/me", { email });
export const changePassword = (current: string, password: string) => call<ServerUser>("POST", "/api/me/password", { current, password });

export const listGroups = () => call<GroupSummary[]>("GET", "/api/groups");
export const createGroup = (name: string) => call<{ id: string }>("POST", "/api/groups", { name });
export const groupDetail = (id: string) => call<GroupDetail>("GET", `/api/groups/${id}`);
export const renameGroup = (id: string, name: string) => call<void>("PATCH", `/api/groups/${id}`, { name });
export const deleteGroup = (id: string) => call<void>("DELETE", `/api/groups/${id}`);
export const partyOf = (id: string) => call<PartyRow[]>("GET", `/api/groups/${id}/party`);
export const invitePlayer = (id: string, username: string) => call<unknown>("POST", `/api/groups/${id}/invites`, { username });
export const cancelInvite = (id: string, invite: string) => call<void>("DELETE", `/api/groups/${id}/invites/${invite}`);
export const removeMember = (id: string, player: string) => call<void>("DELETE", `/api/groups/${id}/members/${player}`);
export const myInvitations = () => call<Invitation[]>("GET", "/api/invites");
export const acceptInvitation = (id: string) => call<{ groupId: string }>("POST", `/api/invites/${id}/accept`, {});
export const declineInvitation = (id: string) => call<void>("POST", `/api/invites/${id}/decline`, {});
export const myCharacters = () => call<OwnedCharacter[]>("GET", "/api/characters");
export const bringCharacter = (group: string, characterId: string) => call<void>("POST", `/api/groups/${group}/character`, { characterId });
export const withdrawCharacter = (group: string) => call<void>("DELETE", `/api/groups/${group}/character`);

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
  groups: number;
  characters: number;
  groupCharacters: number;
  contentPacks: number;
  invites: number;
  activeSessions: number;
  diskBytes: number;
  version: string;
  database: "sqlite" | "postgres";
  uptimeSeconds: number;
  email: boolean;
}

export interface SignUpSettings {
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
