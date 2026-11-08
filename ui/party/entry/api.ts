import type { Api } from "../../src/api/contract";
import { createHttpApi } from "../api/http";
import { loadSession } from "../api/session";

/** Replaces upstream's `createApi` in the party build. Without a session nothing renders that would use the result. */
export function createApi(): Api {
  const session = loadSession();
  return session ? createHttpApi(session) : ({} as Api);
}
