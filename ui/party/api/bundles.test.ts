import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { onBundleRequest, type BundleRequest } from "../../src/pages/transfer/openBundle";
import { createHttpApi } from "./http";
import type { ServerSession } from "./session";

const session: ServerSession = {
  baseUrl: "http://server", token: "t",
  user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
};

let calls: { path: string; body: Record<string, unknown> }[];
const hero = { id: "hero-1", name: "Hero", build: "Elf Wizard 3", level: 3 };
const preview = {
  bundleId: "b-1", fileName: "Hero.borealis", sourceDevice: "Borealis on Windows PC",
  preview: { totalBytes: 10, items: [
    { kind: "character", key: "hero-1", label: "Hero", action: "new", size: 5, missingPacks: ["far-realms"] },
    { kind: "pack", key: "homebrew", label: "homebrew", action: "new", size: 5, missingPacks: [] },
  ] },
};

beforeEach(() => {
  calls = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
    const path = url.replace(session.baseUrl, "");
    calls.push({ path, body: JSON.parse(String(init.body ?? "{}")) as Record<string, unknown> });
    const reply = path === "/api/rpc/stage_bundle" ? { token: "tok-1", name: "Hero.borealis" }
      : path === "/api/rpc/bundle_open" ? preview
      : path === "/api/rpc/list_characters" ? [{ id: "other", name: "Other" }, hero]
      : path === "/api/rpc/bundle_accept" ? { added: 2, replaced: 0, keptLocal: 0, identical: 0 }
      : { summary: { name: "Hero" } };
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
  /** BundleHost's part: open the requested preview, then press Accept or Cancel. */
  const window_ = (api: ReturnType<typeof createHttpApi>, press: "accept" | "cancel") => {
    const off = onBundleRequest((r) => {
      void api.bundleOpen({ token: r.token }).then(async (b) => {
        if (press === "accept") await api.bundleAccept({ bundleId: b.bundleId });
        else await api.bundleDiscard({ bundleId: b.bundleId });
      });
    });
    onTestFinished(off);
  };

  it("checks a .borealis file, waits for Accept in the Open bundle window, then reports the character", async () => {
    const seen = opened();
    const api = createHttpApi(session);
    window_(api, "accept");
    const report = await api.importCharacter({ fileName: "Hero.BOREALIS", contentBase64: "AAAA" });
    expect(report.summary).toEqual(hero);
    expect(report.missingContent).toEqual([]);
    expect(report.warnings).toEqual(["Content it needs that was not in the bundle: far-realms."]);
    // The window got the preview the import made: the token is read once.
    expect(calls.map((c) => c.path)).toEqual(["/api/rpc/stage_bundle", "/api/rpc/bundle_open", "/api/rpc/bundle_accept", "/api/rpc/list_characters"]);
    expect(calls[1]?.body).toEqual({ token: "tok-1" });
    expect(seen).toEqual([{ token: "tok-1", name: "Hero.borealis" }]);
  });

  it("says nothing changed when the Open bundle window is cancelled", async () => {
    const api = createHttpApi(session);
    window_(api, "cancel");
    await expect(api.importCharacter({ fileName: "Hero.borealis", contentBase64: "AAAA" }))
      .rejects.toMatchObject({ message: "it is a Borealis bundle. The Open bundle window was cancelled, so nothing was changed." });
    expect(calls.map((c) => c.path)).toEqual(["/api/rpc/stage_bundle", "/api/rpc/bundle_open", "/api/rpc/bundle_discard"]);
  });

  it("gives a refused bundle only the import report's message, with no window", async () => {
    const seen = opened();
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      const path = url.replace(session.baseUrl, "");
      calls.push({ path, body: {} });
      return path === "/api/rpc/stage_bundle"
        ? new Response(JSON.stringify({ token: "tok-1", name: "Hero.borealis" }), { status: 200 })
        : new Response(JSON.stringify({ code: "invalid_argument", message: "That is not a Borealis bundle." }), { status: 422 });
    }));
    await expect(createHttpApi(session).importCharacter({ fileName: "Hero.borealis", contentBase64: "AAAA" }))
      .rejects.toMatchObject({ message: "That is not a Borealis bundle." });
    expect(seen).toEqual([]);
  });

  it("opens a bundle the Open bundle menu picked from the server as before", async () => {
    await createHttpApi(session).bundleOpen({ token: "tok-9" });
    expect(calls).toEqual([{ path: "/api/rpc/bundle_open", body: { token: "tok-9" } }]);
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
    const api = createHttpApi(session);
    window_(api, "accept");
    await expect(api.importCharacter({ fileName: "document", contentBase64: "UEsDBBQAAAA" })).resolves.toMatchObject({ summary: hero });
    expect(calls[0]?.path).toEqual("/api/rpc/stage_bundle");
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

describe("Export to bundle", () => {
  it("asks the server for the file, saves it as a download and names it as the path", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
      calls.push({ path: url.replace(session.baseUrl, ""), body: JSON.parse(String(init.body ?? "{}")) as Record<string, unknown> });
      return new Response(JSON.stringify({ fileName: "Borealis content.borealis", characters: 0, packs: 3, bytes: 2, dataBase64: "UEs=" }), { status: 200 });
    }));
    const saved: string[] = [];
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) { saved.push(this.download); });
    onTestFinished(() => click.mockRestore());
    URL.createObjectURL ??= () => "blob:x";
    URL.revokeObjectURL ??= () => {};

    const api = createHttpApi(session);
    const selection = { characterIds: [], packs: "all" as const };
    const r = await api.bundleExport({ selection });

    expect(calls).toEqual([{ path: "/api/rpc/bundle_export", body: { selection } }]);
    expect(saved).toEqual(["Borealis content.borealis"]);
    expect(r).toEqual({ fileName: "Borealis content.borealis", path: "Borealis content.borealis", characters: 0, packs: 3, bytes: 2 });
  });
});
