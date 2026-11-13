import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Api, CharacterSnapshot } from "../../src/api/contract";
import { createHttpApi } from "./http";
import { createOfflineApi } from "./offline";
import { UPSTREAM_DEFAULT_PLAYER } from "./playerName";
import { saveSession, type ServerSession } from "./session";
import { memoryStorage } from "../test/memoryStorage";

const session: ServerSession = {
  baseUrl: "http://party.test", token: "t",
  user: { id: "u1", username: "drazor", email: null, admin: false, mustChangePassword: false },
};

interface Stored { player: string; dirty: boolean; saved: string }

/** A server's library as the RPC routes see it: what each character's player name is now and on file. */
class FakeServer {
  chars = new Map<string, Stored>();
  settingsPlayer = UPSTREAM_DEFAULT_PLAYER;
  calls: string[] = [];
  next = 1;

  snapshot(id: string): CharacterSnapshot {
    const c = this.chars.get(id)!;
    return { id, dirty: c.dirty, details: { playerName: c.player }, slots: [], progress: null } as unknown as CharacterSnapshot;
  }

  answer(command: string, args: Record<string, unknown>): unknown {
    this.calls.push(command);
    const id = String(args.characterId ?? "");
    switch (command) {
      case "create_character": {
        const nid = `c${this.next++}`;
        // Upstream's builder: the player is Settings > Player Name, and the new character is on file.
        this.chars.set(nid, { player: this.settingsPlayer, dirty: false, saved: this.settingsPlayer });
        return this.snapshot(nid);
      }
      case "import_character": {
        const nid = `c${this.next++}`;
        const player = String(args.fileName).includes("empty") ? "" : "Imported Player";
        this.chars.set(nid, { player, dirty: false, saved: player });
        return { characterId: nid, name: "Imported", missing: [] };
      }
      case "load_character": return this.snapshot(id);
      case "update_details": {
        const c = this.chars.get(id)!;
        const patch = args.patch as { playerName?: string };
        if (patch.playerName !== undefined) c.player = patch.playerName;
        c.dirty = true;
        return this.snapshot(id);
      }
      case "save_character": {
        const c = this.chars.get(id)!;
        c.saved = c.player;
        c.dirty = false;
        return this.snapshot(id);
      }
      default: return null;
    }
  }
}

let server: FakeServer;
let api: Api;

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  vi.stubGlobal("sessionStorage", memoryStorage());
  saveSession(session);
  server = new FakeServer();
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
    const command = url.replace(/^.*\/api\/rpc\//, "");
    const body = server.answer(command, JSON.parse(String(init.body ?? "{}")) as Record<string, unknown>);
    return new Response(JSON.stringify(body), { status: 200 });
  }));
  api = createHttpApi(session);
});

afterEach(() => vi.unstubAllGlobals());

const request = { name: "Brannoc", ruleset: "dnd2014", startingLevel: 1, gender: "", abilityMethod: "manual", options: {} } as never;

describe("a new character's player name", () => {
  it("is the account's username, on file at once, so the character opens clean", async () => {
    const snap = await api.createCharacter({ request });
    expect(snap.details.playerName).toBe("drazor");
    expect(snap.dirty).toBe(false);
    expect(server.chars.get(snap.id)!.saved).toBe("drazor");
  });

  it("is the username when Settings > Player Name is empty", async () => {
    server.settingsPlayer = "";
    expect((await api.createCharacter({ request })).details.playerName).toBe("drazor");
  });

  it("keeps a Player Name the account chose in Settings", async () => {
    server.settingsPlayer = "Vagelis";
    const snap = await api.createCharacter({ request });
    expect(snap.details.playerName).toBe("Vagelis");
    expect(server.calls).not.toContain("update_details");
  });
});

describe("an opened character's player name", () => {
  it("empty: the username, as an edit that the next save writes", async () => {
    server.chars.set("old", { player: "", dirty: false, saved: "" });
    const snap = await api.loadCharacter({ characterId: "old" });
    expect(snap.details.playerName).toBe("drazor");
    expect(server.chars.get("old")!.saved).toBe("");
    await api.saveCharacter({ characterId: "old" });
    expect(server.chars.get("old")!.saved).toBe("drazor");
  });

  it("non-empty: never changed, upstream's default included", async () => {
    server.chars.set("a", { player: "Someone Else", dirty: false, saved: "Someone Else" });
    server.chars.set("b", { player: UPSTREAM_DEFAULT_PLAYER, dirty: false, saved: UPSTREAM_DEFAULT_PLAYER });
    expect((await api.loadCharacter({ characterId: "a" })).details.playerName).toBe("Someone Else");
    expect((await api.loadCharacter({ characterId: "b" })).details.playerName).toBe(UPSTREAM_DEFAULT_PLAYER);
    expect(server.calls).not.toContain("update_details");
  });

  it("edited by the user: kept, on every later open", async () => {
    const { id } = await api.createCharacter({ request });
    await api.updateDetails({ characterId: id, patch: { playerName: "Table Name" } });
    await api.saveCharacter({ characterId: id });
    expect((await api.loadCharacter({ characterId: id })).details.playerName).toBe("Table Name");
    expect(server.chars.get(id)!.saved).toBe("Table Name");
  });
});

describe("an imported character's player name", () => {
  it("is the one it came with", async () => {
    const report = (await api.importCharacter({ fileName: "hero.dnd5e", contentBase64: "" })) as unknown as { characterId: string };
    expect((await api.loadCharacter({ characterId: report.characterId })).details.playerName).toBe("Imported Player");
  });

  it("empty: the username, as for any opened character", async () => {
    const report = (await api.importCharacter({ fileName: "empty.dnd5e", contentBase64: "" })) as unknown as { characterId: string };
    expect((await api.loadCharacter({ characterId: report.characterId })).details.playerName).toBe("drazor");
  });
});

describe("offline", () => {
  it("an opened character with an empty player name gets the username too", async () => {
    let player = "";
    const local = {
      loadCharacter: async () => ({ id: "L1", details: { playerName: player }, slots: [], progress: null }),
      updateDetails: async ({ patch }: { patch: { playerName: string } }) => {
        player = patch.playerName;
        return { id: "L1", details: { playerName: player }, slots: [], progress: null };
      },
    } as unknown as Api;
    const offline = createOfflineApi(local, new Map([["S1", "L1"]]));
    const snap = await offline.loadCharacter({ characterId: "S1" });
    expect(snap.details.playerName).toBe("drazor");
    expect(snap.id).toBe("S1");
  });
});
