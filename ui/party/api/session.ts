/** Who is signed in, to which server, and where in it they are working. Survives restarts on this device. */
export interface ServerUser {
  id: string;
  username: string;
  email: string | null;
  /** Runs the server: sees the admin page. */
  admin: boolean;
  /** Has to choose a new password before anything else. */
  mustChangePassword: boolean;
}

/** Looking at a member's character as their Dungeon Master: read-only. */
export interface ViewTarget {
  groupId: string;
  playerId: string;
  characterName: string;
}

/** A place on the server. Every account has "My characters" (`personal`) and a Library (`library`). */
export interface Place {
  /** `personal`, `library` or `group-<id>`. */
  scope: string;
  scopeLabel: string;
  /** Working as a Dungeon Master here: the Library, or a group this account runs. */
  asDm?: boolean;
  view?: ViewTarget;
}

export interface ServerSession extends Place {
  /** "" when the page was served by the server itself. */
  baseUrl: string;
  token: string;
  user: ServerUser;
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
  try {
    const session = JSON.parse(text) as ServerSession & { user: { role?: string } };
    // Saved before accounts stopped being either a DM or a player: a DM's session sat in their Library.
    if (session.user.role !== undefined && session.asDm === undefined) session.asDm = session.scope === "library";
    return session;
  } catch { return null; }
}

export function saveSession(session: ServerSession): void {
  write(KEY, JSON.stringify(session));
}

export function clearSession(): void {
  write(KEY, null);
}

/** Keep the stored account details in step with the server's. */
export function updateUser(user: ServerUser): void {
  const session = loadSession();
  if (session) saveSession({ ...session, user });
}

export function rememberedServer(): string {
  return read(SERVER_KEY) ?? "";
}

export function rememberServer(url: string): void {
  write(SERVER_KEY, url);
}

/** The place a new sign-in starts in. */
export function homeScope(): Place {
  return { scope: "personal", scopeLabel: "My characters", asDm: false };
}

/** Every account's content library. */
export const libraryPlace: Place = { scope: "library", scopeLabel: "Library", asDm: true };

export const signedIn = (): boolean => loadSession() !== null;

/** Working as a Dungeon Master in the current place (and not looking at someone's character). */
export const actingAsDm = (): boolean => {
  const session = loadSession();
  return Boolean(session?.asDm && !session.view);
};

/** Everything the app holds comes from one place on the server, so changing place restarts the page there. */
export function moveTo(place: Place, route: string, characterId?: string): void {
  const session = loadSession();
  if (!session) return;
  const { view: _old, ...rest } = session;
  saveSession({
    ...rest,
    scope: place.scope,
    scopeLabel: place.scopeLabel,
    asDm: Boolean(place.asDm),
    ...(place.view ? { view: place.view } : {}),
  });
  try {
    if (characterId) sessionStorage.setItem(CURRENT_CHARACTER_KEY, characterId);
    else sessionStorage.removeItem(CURRENT_CHARACTER_KEY);
  } catch { /* nothing to restore then */ }
  window.location.hash = route;
  window.location.reload();
}
