import { serverFetch } from "./http";
import { loadSession } from "./session";

/**
 * The server's default 5etools source (party/crates/borealis-server/src/import_source.rs). The party ships none: a
 * server's admin may set one, and the Import section then offers it in one tap.
 */

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const session = loadSession();
  if (!session) return Promise.reject(new Error("Not signed in."));
  return serverFetch(session, method, path, body) as Promise<T>;
}

/** The default source the server offers, or null (none set, or a server older than this). */
export async function defaultFiveToolsSource(): Promise<string | null> {
  try {
    const out = await call<{ url?: string | null } | null>("GET", "/api/import/default-5etools");
    return typeof out?.url === "string" && out.url ? out.url : null;
  } catch {
    return null;
  }
}

/** Admin › Settings › Default 5etools source. */
export interface DefaultSourceSettings {
  /** In force, or null. */
  url: string | null;
  /** Saved on the admin page (an address, or none), from config.toml, or neither. */
  source: "admin" | "config" | "none";
  /** `import.default_5etools_url` in config.toml or the environment. */
  configFile: string | null;
}

export const adminDefaultSource = () => call<DefaultSourceSettings>("GET", "/api/admin/default-5etools");
/** An address to offer, "" for none (winning over config.toml), or null to go back to config.toml. */
export const saveAdminDefaultSource = (url: string | null) => call<DefaultSourceSettings>("POST", "/api/admin/default-5etools", { url });
