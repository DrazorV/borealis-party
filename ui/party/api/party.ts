import { serverFetch } from "./http";
import { loadSession, type Role, type ServerUser } from "./session";

export interface GroupSummary {
  id: string;
  name: string;
  dmName: string;
  memberCount: number;
  characterId: string | null;
}

export interface GroupDetail {
  id: string;
  name: string;
  dmName: string;
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
}

interface SignedIn {
  token: string;
  user: ServerUser;
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

export const signUp = (baseUrl: string, username: string, password: string, role: Role) =>
  serverFetch({ baseUrl, token: "" }, "POST", "/api/auth/register", { username, password, role }) as Promise<SignedIn>;

export const signOut = () => call<void>("POST", "/api/auth/logout");

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
