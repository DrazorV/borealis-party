import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createHttpApi } from "../http";
import { saveSession, type ServerSession } from "../session";
import { memoryStorage } from "../../test/memoryStorage";
import {
  charactersInParties, formatCode, joinLink, keepJoinCode, keptJoinCode, normaliseCode, onPartiesChanged, partiesDeps,
  removeMember, resetParties, takeJoinLink, type Party,
} from "./parties";

const session: ServerSession = { baseUrl: "http://server", token: "t", user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false } };

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession(session);
});
afterEach(() => {
  resetParties();
  vi.unstubAllGlobals();
});

describe("join codes", () => {
  it("read as typed or pasted: case, spaces and dashes don't matter, and a link gives its code", () => {
    expect(normaliseCode(" abcd-efgh ijkl ")).toBe("ABCDEFGHIJKL");
    expect(normaliseCode("https://party.example.com/#/join/abcd-efgh")).toBe("ABCDEFGH");
    expect(normaliseCode("https://party.example.com/join#abcd-efgh")).toBe("ABCDEFGH");
    expect(formatCode("abcdefghij")).toBe("ABCD-EFGH-IJ");
  });

  it("a link puts the code after the #, so it never reaches the server", () => {
    expect(joinLink("abcd-efgh", "https://party.example.com")).toBe("https://party.example.com/#/join/ABCDEFGH");
  });

  it("a join link is kept on the device for the sign-up or the Parties page, in both forms", () => {
    expect(takeJoinLink({ pathname: "/", hash: "#/join/abcd-efgh" })).toBe("ABCDEFGH");
    expect(keptJoinCode()).toBe("ABCDEFGH");
    keepJoinCode(null);
    expect(takeJoinLink({ pathname: "/join", hash: "#wxyz1234" })).toBe("WXYZ1234");
    expect(keptJoinCode()).toBe("WXYZ1234");
    expect(window.location.hash).toBe("#/parties");
  });

  it("any other address is left alone", () => {
    keepJoinCode(null);
    expect(takeJoinLink({ pathname: "/", hash: "#/characters" })).toBeNull();
    expect(takeJoinLink({ pathname: "/join", hash: "" })).toBeNull();
    expect(keptJoinCode()).toBe("");
  });
});

it("ids go into a path as one encoded segment each", async () => {
  const paths: string[] = [];
  partiesDeps.fetch = async (_s, _m, path) => { paths.push(path); return null; };
  await removeMember("p/1", "../u", "c_1?x");
  expect(paths).toEqual(["/api/parties/p%2F1/members/..%2Fu/c_1%3Fx"]);
});

it("the account's characters in parties come from the members marked as its own", () => {
  const party = (id: string, members: [string, boolean][]): Party => ({
    id, name: id, ruleset: "dnd2024", role: "player", dmName: "dm", createdAt: 0,
    members: members.map(([characterId, mine]) => ({ ownerId: "o", ownerName: "o", characterId, name: characterId, classLevel: "", joinedAt: 0, mine })),
  });
  const taken = charactersInParties([party("p1", [["c_a", true], ["c_b", false]]), party("p2", [["c_c", true]])]);
  expect([...taken].map(([c, p]) => [c, p.id])).toEqual([["c_a", "p1"], ["c_c", "p2"]]);
});

it("deleting a character tells the Parties page, after the server took it", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => new Response("null", { status: 200 })));
  const heard = vi.fn();
  onPartiesChanged(heard);
  await createHttpApi(session).deleteCharacter({ id: "c_1" } as never);
  expect(heard).toHaveBeenCalledOnce();
});

it("a refused delete tells nobody", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ code: "not_found", message: "Not found." }), { status: 404 })));
  const heard = vi.fn();
  onPartiesChanged(heard);
  await expect(createHttpApi(session).deleteCharacter({ id: "c_1" } as never)).rejects.toMatchObject({ code: "not_found" });
  expect(heard).not.toHaveBeenCalled();
});
