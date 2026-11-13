import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Api } from "../../src/api/contract";
import { classLine, partiesDeps, resetParties, viewMember, watchParties, type PartyEvent } from "../api/overrides/parties";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { charId, fakePartiesServer, stats, type FakeServer } from "../test/partiesServer";

/**
 * The live roster and the DM's read-only view (0.2.1): party events refresh what is shown, each viewer sees the
 * server's projection of a card (plan D6), and the view reads the character again as the player plays.
 */

let api: Api;
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => "confirm" }));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: () => {} }) }));

const { PartiesPage } = await import("./PartiesPage");
const { PartyCharacterView } = await import("./PartyCharacterView");

const ARIA = charId(2);
const BRAM = charId(3);
const FIGHTER = [{ name: "Fighter", level: 3 }];

let server: FakeServer;

function accounts() {
  return [
    { id: "dm1", username: "dana", dm: true, characters: [] },
    { id: "pl1", username: "alice", dm: false, online: true, characters: [{ id: ARIA, name: "Aria", level: 3, classes: FIGHTER, stats: stats(28, 28, { concentration: "Bless" }) }] },
    { id: "pl2", username: "bob", dm: false, online: false, characters: [{ id: BRAM, name: "Bram", level: 2, classes: [{ name: "Wizard", level: 2 }], stats: stats(12, 12, { spellDcs: [{ class: "Wizard", dc: 13 }] }) }] },
  ];
}

function signIn(uid: string) {
  server.me = uid;
  const a = server.accounts.find((x) => x.id === uid)!;
  saveSession({ baseUrl: "http://server", token: "t", user: { id: a.id, username: a.username, email: null, admin: false, mustChangePassword: false } });
  api = { listCharacters: async () => a.characters.map((c) => ({ id: c.id, name: c.name, build: "", level: c.level })) } as unknown as Api;
}

function page(path = "/parties") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/parties" element={<PartiesPage />} />
        <Route path="/parties/:party/view/:owner/:character" element={<PartyCharacterView />} />
      </Routes>
    </MemoryRouter>,
  );
}

const aria = () => server.accounts[1]!.characters[0]!;
/** The player plays: the server's card changes and it sends the DM (and the players, when they see it) an event. */
function hurt(hp: number, players: boolean) {
  aria().stats = { ...aria().stats!, hp };
  const dm = (server.pseq.get("p0") ?? 0) + 1;
  server.pseq.set("p0", dm);
  if (server.me === "dm1" || players) server.emit({ event: "party://party", payload: { party: "p0", pseq: dm, kind: "card", id: ARIA } });
}
const gets = () => server.calls.filter((c) => c.method === "GET" && c.path === "/api/parties/p0").length;

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  server = fakePartiesServer(accounts(), "dm1");
  partiesDeps.fetch = server.fetch;
  partiesDeps.events = server.listen;
  server.parties.push({ id: "p0", name: "Tuesday Party", dmId: "dm1", createdAt: 1 });
  server.members.push({ ownerId: "pl1", characterId: ARIA, partyId: "p0", joinedAt: 1 }, { ownerId: "pl2", characterId: BRAM, partyId: "p0", joinedAt: 2 });
});

afterEach(() => {
  resetParties();
  vi.unstubAllGlobals();
});

describe("the party events", () => {
  it("come as party, mine and resync events; a reopened stream is a resync; others are ignored", () => {
    const got: PartyEvent[] = [];
    const stop = watchParties((e) => got.push(e));
    server.emit({ event: "party://party", payload: { party: "p0", pseq: 4, kind: "card", id: ARIA } });
    server.emit({ event: "party://mine", payload: { kind: "parties" } });
    server.emit({ event: "party://resync", payload: {} });
    server.emit({ event: "party://characters-changed", payload: { ids: [] } });
    server.reopen();
    stop();
    server.emit({ event: "party://resync", payload: {} });
    expect(got).toEqual([
      { type: "party", party: "p0", pseq: 4, kind: "card", id: ARIA },
      { type: "mine", kind: "parties" },
      { type: "resync" },
      { type: "resync" },
    ]);
    expect(server.listening()).toBe(0);
  });

  it("class lines read as on a sheet", () => {
    expect(classLine([{ name: "Fighter", level: 3 }, { name: "Wizard", level: 2 }])).toBe("Fighter 3 / Wizard 2");
    expect(classLine(undefined)).toBe("");
  });
});

describe("the DM's live roster", () => {
  it("shows each member's numbers, and follows a player's damage as it happens", async () => {
    signIn("dm1");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    const aria = within(card).getByRole("listitem", { name: "Aria" });
    expect(within(aria).getByText("Fighter 3 · 2024 rules · alice")).toBeTruthy();
    expect(within(aria).getByRole("listitem", { name: "Hit points 28 of 28" })).toBeTruthy();
    expect(within(aria).getByRole("listitem", { name: "Armor class 16" })).toBeTruthy();
    expect(within(aria).getByRole("listitem", { name: "Passive Perception 13" })).toBeTruthy();
    expect(within(aria).getByText("Concentrating: Bless")).toBeTruthy();
    expect(within(aria).getByText("Unhurt")).toBeTruthy();
    // Bob doesn't have the app open: his card says so, with his spell DC.
    const bram = within(card).getByRole("listitem", { name: "Bram" });
    expect(within(bram).getByText("Offline")).toBeTruthy();
    expect(within(bram).getByRole("listitem", { name: "Spell save DC 13" })).toBeTruthy();
    expect(within(aria).queryByText("Offline")).toBeNull();

    const before = gets();
    act(() => hurt(9, false));
    await waitFor(() => expect(within(card).getByRole("listitem", { name: "Hit points 9 of 28" })).toBeTruthy());
    expect(within(within(card).getByRole("listitem", { name: "Aria" })).getByText("Bloodied")).toBeTruthy();
    expect(gets()).toBe(before + 1);
    // An event not newer than what is shown reads nothing.
    act(() => server.emit({ event: "party://party", payload: { party: "p0", pseq: 1, kind: "card", id: ARIA } }));
    await act(async () => { await Promise.resolve(); });
    expect(gets()).toBe(before + 1);
  });

  it("a resync or a reopened stream reloads; a change to the account's list too", async () => {
    signIn("dm1");
    page();
    await screen.findByRole("region", { name: "Tuesday Party" });
    const lists = () => server.calls.filter((c) => c.path === "/api/parties").length;
    const before = lists();
    act(() => server.reopen());
    await waitFor(() => expect(lists()).toBe(before + 1));
    act(() => server.emit({ event: "party://mine", payload: { kind: "parties" } }));
    await waitFor(() => expect(lists()).toBe(before + 2));
    // A party deleted elsewhere is gone after the reload.
    server.parties = [];
    act(() => server.emit({ event: "party://resync", payload: {} }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Tuesday Party" })).toBeNull());
  });

  it("opens a member's read-only sheet, which follows the player's play", async () => {
    signIn("dm1");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    fireEvent.click(within(card).getByRole("link", { name: "View Aria" }));
    const head = await screen.findByRole("region", { name: "Aria" });
    expect(within(head).getByRole("group", { name: "Hit points 28 of 28" })).toBeTruthy();
    expect(within(head).getByText(/alice's character/)).toBeTruthy();
    expect(screen.getByText("Read-only")).toBeTruthy();
    expect(screen.getByRole("group", { name: "STR" })).toBeTruthy();
    expect(screen.getByText("Longsword", { selector: "strong" })).toBeTruthy();
    // Nothing on the sheet changes the character.
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(server.calls.filter((c) => c.path.endsWith("/view/snapshot"))).toEqual([
      { method: "POST", path: `/api/parties/p0/members/pl1/${ARIA}/view/snapshot`, body: {} },
    ]);
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      act(() => hurt(0, false));
      await act(async () => { await vi.advanceTimersByTimeAsync(400); });
      await waitFor(() => expect(within(screen.getByRole("region", { name: "Aria" })).getByRole("group", { name: "Hit points 0 of 28" })).toBeTruthy());
      expect(within(screen.getByRole("region", { name: "Aria" })).getByText("Down")).toBeTruthy();
      // Another character's event reads nothing.
      const views = server.calls.filter((c) => c.path.endsWith("/view/snapshot")).length;
      act(() => server.emit({ event: "party://party", payload: { party: "p0", pseq: 99, kind: "card", id: BRAM } }));
      await act(async () => { await vi.advanceTimersByTimeAsync(400); });
      expect(server.calls.filter((c) => c.path.endsWith("/view/snapshot")).length).toBe(views);
    } finally { vi.useRealTimers(); }
  });

  it("a character no longer in the party says so", async () => {
    signIn("dm1");
    server.members = server.members.filter((m) => m.characterId !== ARIA);
    page(`/parties/p0/view/pl1/${ARIA}`);
    expect(await screen.findByText("This character isn't in your party any more.")).toBeTruthy();
    expect(await viewMember("p0", "pl1", ARIA)).toBeNull();
  });
});

describe("a player's view of the party", () => {
  it("other members show a name, class, level and health band only; their own card is whole", async () => {
    signIn("pl2");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    const aria = within(card).getByRole("listitem", { name: "Aria" });
    expect(within(aria).getByText("Fighter 3 · 2024 rules · alice")).toBeTruthy();
    expect(within(aria).getByText("Unhurt")).toBeTruthy();
    expect(within(aria).queryByRole("list", { name: "Numbers" })).toBeNull();
    expect(within(aria).queryByText(/Concentrating/)).toBeNull();
    expect(within(aria).queryByRole("link")).toBeNull();
    const own = within(card).getByRole("listitem", { name: "Bram" });
    expect(within(own).getByRole("listitem", { name: "Hit points 12 of 12" })).toBeTruthy();
    // No DM's view for a player, and no "offline" for anyone.
    expect(within(card).queryByRole("link", { name: /View/ })).toBeNull();
    expect(within(card).queryByText("Offline")).toBeNull();

    // A scratch moves nothing a player sees; bloodied does, through their own event.
    const before = gets();
    act(() => hurt(20, false));
    await act(async () => { await Promise.resolve(); });
    expect(gets()).toBe(before);
    ariaBloodied();
    await waitFor(() => expect(within(within(card).getByRole("listitem", { name: "Aria" })).getByText("Bloodied")).toBeTruthy());
  });
});

/** The players' sequence moves: the server tells them the band changed. */
function ariaBloodied() {
  aria().stats = { ...aria().stats!, hp: 10 };
  const players = (server.pseq.get("p0") ?? 0) + 1;
  server.pseq.set("p0", players);
  act(() => server.emit({ event: "party://party", payload: { party: "p0", pseq: players, kind: "card", id: ARIA } }));
}
