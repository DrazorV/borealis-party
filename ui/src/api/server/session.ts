/** Who is signed in, to which server, and where in it they are working. Survives restarts on this device. */
export type Role = "dm" | "player";

export interface ServerUser {
  id: string;
  username: string;
  role: Role;
}

/** Looking at a member's character as their Dungeon Master: read-only. */
export interface ViewTarget {
  groupId: string;
  playerId: string;
  characterName: string;
}

export interface ServerSession {
  /** "" when the page was served by the server itself. */
  baseUrl: string;
  token: string;
  user: ServerUser;
  /** `personal`, `library` or `group-<id>`. */
  scope: string;
  scopeLabel: string;
  view?: ViewTarget;
}

const KEY = "party.session";
const SERVER_KEY = "party.server";
const CURRENT_CHARACTER_KEY = "lk.currentCharacter";

function read(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}

function write(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch { /* private window: the session lasts as long as the page */ }
}

export function loadSession(): ServerSession | null {
  const text = read(KEY);
  if (!text) return null;
  try { return JSON.parse(text) as ServerSession; } catch { return null; }
}

export function saveSession(session: ServerSession): void {
  write(KEY, JSON.stringify(session));
}

export function clearSession(): void {
  write(KEY, null);
}

export function rememberedServer(): string {
  return read(SERVER_KEY) ?? "";
}

export function rememberServer(url: string): void {
  write(SERVER_KEY, url);
}

/** The place a new sign-in starts in. */
export function homeScope(role: Role): { scope: string; scopeLabel: string } {
  return role === "dm" ? { scope: "library", scopeLabel: "Library" } : { scope: "personal", scopeLabel: "My characters" };
}

export const signedIn = (): boolean => loadSession() !== null;

export const isDm = (): boolean => loadSession()?.user.role === "dm";

/** Everything the app holds comes from one place on the server, so changing place restarts the page there. */
export function moveTo(place: { scope: string; scopeLabel: string; view?: ViewTarget }, route: string, characterId?: string): void {
  const session = loadSession();
  if (!session) return;
  const { view: _old, ...rest } = session;
  saveSession({ ...rest, scope: place.scope, scopeLabel: place.scopeLabel, ...(place.view ? { view: place.view } : {}) });
  try {
    if (characterId) sessionStorage.setItem(CURRENT_CHARACTER_KEY, characterId);
    else sessionStorage.removeItem(CURRENT_CHARACTER_KEY);
  } catch { /* nothing to restore then */ }
  window.location.hash = route;
  window.location.reload();
}
