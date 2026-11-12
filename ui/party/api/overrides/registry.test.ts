import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setPhoneOsForTests } from "../../../src/lib/phone";
import { createHttpApi } from "../http";
import type { ServerSession } from "../session";
import { OVERRIDES } from "./registry";

const dir = __dirname;
const modules = readdirSync(dir)
  .filter((f) => f.endsWith(".ts") && !f.endsWith(".test.ts") && f !== "registry.ts")
  .map((f) => f.replace(/\.ts$/, ""))
  .sort();

describe("the override modules", () => {
  it("each registers once, under its file name", () => {
    expect(OVERRIDES.map((o) => o.name).sort()).toEqual(modules);
  });

  it("each has exactly one import line in http.ts, in the order they are applied", () => {
    const http = readFileSync(join(dir, "../http.ts"), "utf8");
    const imported = [...http.matchAll(/^import "\.\/overrides\/([\w-]+)";/gm)].map((m) => m[1]);
    expect(imported).toEqual(OVERRIDES.map((o) => o.name));
  });
});

describe("bundleShare on the phones", () => {
  const session: ServerSession = {
    baseUrl: "http://192.168.1.20:8787", token: "t",
    user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
  };
  let fetched: string[];

  beforeEach(() => {
    fetched = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      fetched.push(url);
      return new Response("{}", { status: 200 });
    }));
  });

  afterEach(() => {
    setPhoneOsForTests(null);
    vi.unstubAllGlobals();
  });

  it("Android: not offered, and nothing is asked of the server", async () => {
    setPhoneOsForTests("android");
    await expect(createHttpApi(session).bundleShare({ characterIds: ["hero-1"] } as never)).rejects.toMatchObject({ code: "not_available" });
    expect(fetched).toEqual([]);
  });
});
