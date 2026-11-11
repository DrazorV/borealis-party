/** Who is signed in, and to which server. Survives restarts on this device. */
export interface ServerUser {
  id: string;
  username: string;
  email: string | null;
  /** Runs the server: sees the admin page. */
  admin: boolean;
  /** Has to choose a new password before anything else. */
  mustChangePassword: boolean;
}

export interface ServerSession {
  /** "" when the page was served by the server itself. */
  baseUrl: string;
  token: string;
  user: ServerUser;
}

const KEY = "party.session";
const SERVER_KEY = "party.server";
/** Where upstream remembers the open character, which belongs to the account that opened it. */
const CURRENT_CHARACTER_KEY = "lk.currentCharacter";
/** What upstream keeps on the device for its one user. Here it belongs to the account named under OWNER_KEY. */
const DEVICE_KEYS = ["borealis.portraits", "borealis.defaultSources"];
const OWNER_KEY = "party.deviceOwner";

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
    const { baseUrl, token, user } = JSON.parse(text) as Partial<ServerSession>;
    if (typeof baseUrl !== "string" || typeof token !== "string" || !user) return null;
    return { baseUrl, token, user };
  } catch { return null; }
}

export function saveSession(session: ServerSession): void {
  if (read(OWNER_KEY) !== session.user.id) {
    // Another account on this browser: it doesn't get the previous one's portraits and default sources.
    for (const key of DEVICE_KEYS) write(key, null);
    write(OWNER_KEY, session.user.id);
  }
  write(KEY, JSON.stringify(session));
}

export function clearSession(): void {
  write(KEY, null);
  try { sessionStorage.removeItem(CURRENT_CHARACTER_KEY); } catch { /* nothing was remembered then */ }
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
