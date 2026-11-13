import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/** The accounts server/start.ts seeds on the throwaway server. Test values only: that server lives for one run. */
export const ACCOUNTS = {
  admin: { email: "admin@phone-rig.test", password: "phone-rig-admin-7731", admin: true },
  player: { email: "player@phone-rig.test", password: "phone-rig-player-7731", admin: false },
} as const;
export type AccountName = keyof typeof ACCOUNTS;

/** Where global setup leaves the test server's address for the workers. */
export const SERVER_FILE = join(dirname(fileURLToPath(import.meta.url)), "..", ".server.json");
export interface ServerFile { url: string }
