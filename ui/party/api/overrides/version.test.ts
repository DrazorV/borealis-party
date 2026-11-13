import { afterEach, expect, it, vi } from "vitest";
import { createHttpApi } from "../http";
import type { ServerSession } from "../session";

const session: ServerSession = {
  baseUrl: "https://party.example.org", token: "t",
  user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
};

afterEach(() => vi.unstubAllGlobals());

it("Settings → Application shows the version the build shows (the party's), without asking the server", async () => {
  const fetched = vi.fn(async () => new Response("\"1.4.0\""));
  vi.stubGlobal("fetch", fetched);
  expect(await createHttpApi(session).appVersion()).toBe(__APP_VERSION__);
  expect(fetched).not.toHaveBeenCalled();
});
