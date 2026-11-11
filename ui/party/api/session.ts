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

/** The same server under a new address (an IP address that became a domain name): the session stays signed in. */
export function setServerAddress(baseUrl: string): void {
  const session = loadSession();
  if (session) write(KEY, JSON.stringify({ ...session, baseUrl }));
  rememberServer(baseUrl);
}

/**
 * What a typed address means: https for a domain name, http for an IP address, localhost, a `.local` name or a
 * one-word name on the home network (those have no certificate). A typed scheme is kept. Scheme and host are
 * lower-cased, so the same address typed in another case is the same string.
 */
export function normaliseAddress(address: string): string {
  const trimmed = address.trim().replace(/\/+$/, "");
  const typed = /^(https?):\/\/(.*)$/i.exec(trimmed);
  const rest = typed?.[2] ?? trimmed;
  const slash = rest.indexOf("/");
  const authority = (slash < 0 ? rest : rest.slice(0, slash)).toLowerCase();
  const path = slash < 0 ? "" : rest.slice(slash);
  const scheme = typed?.[1]?.toLowerCase() ?? (isLocalHost(authority) ? "http" : "https");
  return `${scheme}://${authority}${path}`;
}

/** An address on the home network (IP address, localhost, `.local` or one-word name), given a normalised URL. */
export function isLocalAddress(baseUrl: string): boolean {
  return isLocalHost(baseUrl.replace(/^https?:\/\//, "").split("/")[0] ?? "");
}

function isLocalHost(authority: string): boolean {
  const host = authority.replace(/:\d+$/, "");
  return host.startsWith("[") || /^\d{1,3}(\.\d{1,3}){3}$/.test(host) || host === "localhost" || host.endsWith(".local") || !host.includes(".");
}
