import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { onBundleRequest, type BundleRequest } from "../../src/pages/transfer/openBundle";
import { createHttpApi } from "./http";
import type { ServerSession } from "./session";

const session: ServerSession = {
  baseUrl: "http://server", token: "t",
  user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
};

let calls: { path: string; body: Record<string, unknown> }[];

beforeEach(() => {
  calls = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
    const path = url.replace(session.baseUrl, "");
    calls.push({ path, body: JSON.parse(String(init.body ?? "{}")) as Record<string, unknown> });
    const reply = path === "/api/rpc/stage_bundle" ? { token: "tok-1", name: "Hero.borealis" } : { summary: { name: "Hero" } };
    return new Response(JSON.stringify(reply), { status: 200 });
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

const opened = (): BundleRequest[] => {
  const seen: BundleRequest[] = [];
  const off = onBundleRequest((r) => seen.push(r));
  onTestFinished(off);
  return seen;
};

describe("importCharacter on a server", () => {
  it("stages a .borealis file and opens it in the Open bundle window, with a message for the import report", async () => {
    const seen = opened();
    const api = createHttpApi(session);
    await expect(api.importCharacter({ fileName: "Hero.BOREALIS", contentBase64: "AAAA" }))
      .rejects.toMatchObject({ message: "it is a Borealis bundle, so it opened in the Open bundle window for review." });
    expect(calls).toEqual([{ path: "/api/rpc/stage_bundle", body: { fileName: "Hero.BOREALIS", contentBase64: "AAAA" } }]);
    expect(seen).toEqual([{ token: "tok-1", name: "Hero.borealis" }]);
  });

  it("leaves a zip with an extension (a content .zip, a misnamed .json) to import_character", async () => {
    const seen = opened();
    for (const fileName of ["pack.zip", "Hero.json"]) {
      await createHttpApi(session).importCharacter({ fileName, contentBase64: "UEsDBBQAAAA" });
    }
    expect(calls.map((c) => c.path)).toEqual(["/api/rpc/import_character", "/api/rpc/import_character"]);
    expect(seen).toEqual([]);
  });

  it("treats a zip with no extension (an opaque provider name) as a bundle", async () => {
    const seen = opened();
    await expect(createHttpApi(session).importCharacter({ fileName: "document", contentBase64: "UEsDBBQAAAA" }))
      .rejects.toMatchObject({ message: "it looks like a Borealis bundle, so it opened in the Open bundle window for review." });
    expect(calls.map((c) => c.path)).toEqual(["/api/rpc/stage_bundle"]);
    expect(seen).toHaveLength(1);
  });

  it("sends every other file to import_character as before", async () => {
    const seen = opened();
    await createHttpApi(session).importCharacter({ fileName: "Hero.dnd5e", contentBase64: "eyJ9" });
    expect(calls.map((c) => c.path)).toEqual(["/api/rpc/import_character"]);
    expect(seen).toEqual([]);
  });
});

describe("pickPath on a server", () => {
  const input = () => document.querySelector<HTMLInputElement>("input[type=file]");

  it("bundle-file: the browser's chooser without a filter, then an upload, and no path back", async () => {
    const api = createHttpApi(session);
    const picked = api.pickPath({ purpose: "bundle-file" });
    const el = input();
    expect(el).not.toBeNull();
    expect(el!.hasAttribute("accept")).toBe(false);
    Object.defineProperty(el, "files", { value: [new File([new Uint8Array([0x50, 0x4b, 3, 4])], "Hero.borealis")] });
    el!.dispatchEvent(new Event("change"));
    await expect(picked).resolves.toEqual([{ token: "tok-1", name: "Hero.borealis", path: "" }]);
    expect(calls).toEqual([{ path: "/api/rpc/stage_bundle", body: { fileName: "Hero.borealis", contentBase64: "UEsDBA==" } }]);
    expect(input()).toBeNull();
  });

  it("bundle-file: a cancelled chooser gives nothing, and a leftover input is removed next time", async () => {
    const api = createHttpApi(session);
    void api.pickPath({ purpose: "bundle-file" });
    const first = input();
    const again = api.pickPath({ purpose: "bundle-file" });
    expect(document.querySelectorAll("input[type=file]")).toHaveLength(1);
    expect(input()).not.toBe(first);
    input()!.dispatchEvent(new Event("cancel"));
    await expect(again).resolves.toEqual([]);
    expect(input()).toBeNull();
    expect(calls).toEqual([]);
  });

  it("other purposes still go to the server", async () => {
    await createHttpApi(session).pickPath({ purpose: "character-file" });
    expect(calls.map((c) => c.path)).toEqual(["/api/rpc/pick_path"]);
  });
});
