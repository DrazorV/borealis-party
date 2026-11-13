import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/** The accounts server/start.ts seeds on the throwaway server. Test values only: that server lives for one run. */
export const ACCOUNTS = {
  admin: { email: "admin@phone-rig.test", password: "phone-rig-admin-7731", admin: true },
  player: { email: "player@phone-rig.test", password: "phone-rig-player-7731", admin: false },
} as const;
export type AccountName = keyof typeof ACCOUNTS;

/**
 * More accounts seeded the same way, for specs that need one account per project and would otherwise sign up (sign-ups
 * are rate limited per address, and the suite makes several): content-refresh.spec.ts's.
 */
export const SEEDED: readonly { name: string; email: string; password: string }[] =
  ["android-portrait", "android-landscape", "iphone", "desktop"].map((project) => ({
    name: `cr-${project}`, email: `cr-${project}@phone-rig.test`, password: "content-refresh-test-5521",
  }));

/** Where global setup leaves the test server's address for the workers. */
export const SERVER_FILE = join(dirname(fileURLToPath(import.meta.url)), "..", ".server.json");
export interface ServerFile { url: string }
