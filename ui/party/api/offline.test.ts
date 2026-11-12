import { expect, it } from "vitest";
import type { Api } from "../../src/api/contract";
import { createOfflineApi, OFFLINE_MESSAGE, swapIds } from "./offline";

/** The app's own backend: records each command with its arguments and answers with `answers[name]`. */
function fakeLocal(answers: Record<string, (args: Record<string, unknown>) => unknown> = {}) {
  const calls: { name: string; args: unknown }[] = [];
  const handlers = new Map<string, (p: unknown) => void>();
  const api = new Proxy({} as Record<string, unknown>, {
    get(_, name: string) {
      if (name === "kind") return "tauri";
      if (name === "subscribe") return (event: string, h: (p: unknown) => void) => { handlers.set(event, h); return () => {}; };
      return async (args: Record<string, unknown>) => {
        calls.push({ name, args });
        return answers[name]?.(args) ?? null;
      };
    },
  }) as unknown as Api;
  return { api, calls, handlers };
}

const ids = new Map([["c_server1", "c_local1"], ["c_server2", "c_local2"]]);

it("what needs the server is refused with the offline message, and never reaches the device", async () => {
  const local = fakeLocal();
  const api = createOfflineApi(local.api, ids);
  const refused = [
    api.createCharacter({ request: {} as never }),
    api.duplicateCharacter({ characterId: "c_server1" }),
    api.saveCharacterAs({ characterId: "c_server1" } as never),
    api.deleteCharacter({ characterId: "c_server1" }),
    api.importCharacter({ fileName: "a.json", contentBase64: "" }),
    api.applyConvertTo2024({ characterId: "c_server1" } as never),
    api.importPack({ request: {} as never }),
    api.removePack({ packId: "p" }),
    api.updateIndexes(),
    api.bundleOpen({} as never),
    api.pickPath({ purpose: "bundle-file" }),
    api.setDocumentsDir({}),
  ];
  for (const r of refused) await expect(r).rejects.toEqual({ code: "offline", message: OFFLINE_MESSAGE });
  // What the server does not offer either gets the same answer as online.
  await expect(api.linkStatus({} as never)).rejects.toMatchObject({ code: "not_available" });
  await expect(api.aiStatus()).rejects.toMatchObject({ code: "not_available" });
  await expect(api.takeOpenedFiles()).resolves.toEqual([]);
  await expect(api.takeOpenedBundles()).resolves.toEqual([]);
  expect(local.calls).toEqual([]);
});

it("characters keep the server's ids: swapped on the way in and out", async () => {
  const local = fakeLocal({
    loadCharacter: ({ characterId }) => ({ id: characterId, name: "Ana", companions: [{ owner: characterId }] }),
    listCharacters: () => [{ id: "c_local1" }, { id: "c_local2" }, { id: "c_spare" }],
  });
  const api = createOfflineApi(local.api, ids);
  const snapshot = await api.loadCharacter({ characterId: "c_server1" });
  expect(local.calls[0]).toEqual({ name: "loadCharacter", args: { characterId: "c_local1" } });
  expect(snapshot).toEqual({ id: "c_server1", name: "Ana", companions: [{ owner: "c_server1" }] });
  await api.takeDamage({ characterId: "c_server2", amount: 3 } as never);
  expect(local.calls[1]).toEqual({ name: "takeDamage", args: { characterId: "c_local2", amount: 3 } });
  // A spare copy (in no entry) is not listed.
  expect((await api.listCharacters()).map((c) => c.id)).toEqual(["c_server1", "c_server2"]);
  // A character that is not on the device goes through as it is (and is not found there).
  await api.getSnapshot({ characterId: "c_elsewhere" });
  expect(local.calls.at(-1)?.args).toEqual({ characterId: "c_elsewhere" });
});

it("events carry the server's ids", () => {
  const local = fakeLocal();
  const api = createOfflineApi(local.api, ids);
  const seen: unknown[] = [];
  api.subscribe("content:reloaded", (p) => seen.push(p));
  local.handlers.get("content:reloaded")?.({ reason: "c_local1" });
  expect(seen).toEqual([{ reason: "c_server1" }]);
});

it("open characters are tracked and saved on the device before the page goes online", async () => {
  const local = fakeLocal();
  const api = createOfflineApi(local.api, ids);
  await api.loadCharacter({ characterId: "c_server1" });
  await api.loadCharacter({ characterId: "c_server2" });
  await api.closeCharacter({ characterId: "c_server2", discard: false });
  expect(api.openCharacters()).toEqual(["c_server1"]);
  local.calls.length = 0;
  await api.closeAll();
  expect(local.calls).toEqual([{ name: "closeCharacter", args: { characterId: "c_local1", discard: false } }]);
  expect(api.openCharacters()).toEqual([]);
});

it("closing to discard (upstream's delete does it first) saves instead, so a refused delete loses no edit", async () => {
  const local = fakeLocal();
  const api = createOfflineApi(local.api, ids);
  await api.loadCharacter({ characterId: "c_server1" });
  await api.closeCharacter({ characterId: "c_server1", discard: true });
  await expect(api.deleteCharacter({ characterId: "c_server1" })).rejects.toMatchObject({ code: "offline" });
  expect(local.calls.slice(1)).toEqual([{ name: "closeCharacter", args: { characterId: "c_local1", discard: false } }]);
  expect(api.openCharacters()).toEqual([]);
});

it("settings saved offline never move the device's folders", async () => {
  const local = fakeLocal();
  const api = createOfflineApi(local.api, ids);
  await api.saveSettings({ patch: { theme: "dark", documentsDir: "/elsewhere" } as never });
  expect(local.calls).toEqual([{ name: "saveSettings", args: { patch: { theme: "dark" } } }]);
});

it("swapIds replaces whole strings only, at any depth", () => {
  expect(swapIds({ a: ["c_local1", "x c_local1", 3, null, { b: "c_local1" }] }, new Map([["c_local1", "S"]])))
    .toEqual({ a: ["S", "x c_local1", 3, null, { b: "S" }] });
});
