import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Api } from "../../src/api/contract";
import { partiesDeps, resetParties } from "../api/overrides/parties";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { charId, fakePartiesServer, type FakeServer } from "../test/partiesServer";

const state = vi.hoisted(() => ({ confirm: "confirm" as string, toasts: [] as { text: string; tone?: string }[] }));
let api: Api;
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => state.confirm }));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (text: string, tone?: string) => state.toasts.push({ text, tone }) }) }));

const { PartiesPage, JoinRoute } = await import("./PartiesPage");
const { DungeonMasterCard } = await import("./PartiesDm");

const CLERIC = charId(1);
const ARIA = charId(2);
const BRAM = charId(3);
const ACCOUNTS = () => [
  { id: "dm1", username: "dana", dm: true, characters: [{ id: CLERIC, name: "Dana's Cleric", level: 4 }] },
  { id: "pl1", username: "alice", dm: false, characters: [{ id: ARIA, name: "Aria", level: 3 }, { id: BRAM, name: "Bram", level: 2 }] },
];
const LATER = () => Math.floor(Date.now() / 1000) + 86_400;

let server: FakeServer;

function signIn(uid: string) {
  server.me = uid;
  const a = server.accounts.find((x) => x.id === uid)!;
  saveSession({ baseUrl: "http://server", token: "t", user: { id: a.id, username: a.username, email: null, admin: false, mustChangePassword: false } });
  api = {
    listCharacters: async () => a.characters.map((c) => ({ id: c.id, name: c.name, build: `Fighter ${c.level}`, level: c.level, ruleset: c.ruleset, updatedAt: c.updatedAt })),
  } as unknown as Api;
}

function page(path = "/parties") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/parties" element={<PartiesPage />} />
        <Route path="/join/:code" element={<JoinRoute />} />
        <Route path="/account" element={<DungeonMasterCard />} />
      </Routes>
    </MemoryRouter>,
  );
}

const loaded = () => screen.findByRole("heading", { name: "My parties" });
const dialog = () => screen.getByRole("dialog");
const toastTexts = () => state.toasts.map((t) => t.text);
const party = (id: string, name: string, dmId = "dm1") => server.parties.push({ id, name, dmId, createdAt: 1 });
const member = (partyId: string, ownerId: string, characterId: string) => server.members.push({ ownerId, characterId, partyId, joinedAt: 1 });
const usernameInvite = (id: string, partyId: string, username: string) =>
  server.invites.push({ id, partyId, username, usesLeft: 1, expiresAt: LATER() });

/** The DM makes a party and a join code; answers the code as shown. */
async function dmMakesCode(name = "Tuesday Party", uses = 1): Promise<string> {
  signIn("dm1");
  const view = page();
  await loaded();
  fireEvent.click(screen.getByRole("button", { name: /New party/ }));
  fireEvent.change(within(dialog()).getByLabelText("Party name"), { target: { value: name } });
  fireEvent.click(within(dialog()).getByRole("radio", { name: "2024 rules" }));
  fireEvent.click(within(dialog()).getByRole("button", { name: "Create" }));
  const card = await screen.findByRole("region", { name });
  fireEvent.click(within(card).getByRole("button", { name: /Invite/ }));
  fireEvent.click(within(dialog()).getByRole("tab", { name: "Join code" }));
  if (uses !== 1) fireEvent.change(within(dialog()).getByRole("spinbutton", { name: "Number of uses" }), { target: { value: String(uses) } });
  fireEvent.click(within(dialog()).getByRole("button", { name: "Make a join code" }));
  const out = await within(dialog()).findByLabelText("Join code");
  const code = out.textContent!;
  view.unmount();
  return code;
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  server = fakePartiesServer(ACCOUNTS(), "pl1");
  partiesDeps.fetch = server.fetch;
  partiesDeps.events = server.listen;
  state.confirm = "confirm";
  state.toasts = [];
});

afterEach(() => {
  resetParties();
  vi.unstubAllGlobals();
});

describe("Dungeon Master tools", () => {
  it("off: no parties to run, and the way to turn them on", async () => {
    signIn("pl1");
    page();
    await loaded();
    expect(screen.queryByRole("heading", { name: "Parties I run" })).toBeNull();
    expect(screen.queryByRole("button", { name: /New party/ })).toBeNull();
    expect(screen.getByRole("link", { name: "Account" }).getAttribute("href")).toBe("/account");
    expect(screen.getByLabelText("Join code")).toBeTruthy();
  });

  it("the account switch: on and off on the server, with the way to the parties while on", async () => {
    signIn("pl1");
    page("/account");
    const toggle = screen.getByRole("switch", { name: "Dungeon Master tools" });
    await waitFor(() => expect((toggle as HTMLButtonElement).disabled).toBe(false));
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle.getAttribute("aria-checked")).toBe("true"));
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/me/roles/dm", body: {} });
    expect(server.accounts[1]!.dm).toBe(true);
    expect(screen.getByRole("link", { name: "Manage your parties" })).toBeTruthy();
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle.getAttribute("aria-checked")).toBe("false"));
    expect(server.calls.at(-1)).toEqual({ method: "DELETE", path: "/api/me/roles/dm" });
  });

  it("off is refused while the account runs parties, in the server's words", async () => {
    signIn("dm1");
    party("p0", "Tuesday Party");
    page("/account");
    const toggle = screen.getByRole("switch", { name: "Dungeon Master tools" });
    await waitFor(() => expect(toggle.getAttribute("aria-checked")).toBe("true"));
    fireEvent.click(toggle);
    await waitFor(() => expect(toastTexts()).toContain("Delete the parties you run first."));
    expect(toggle.getAttribute("aria-checked")).toBe("true");
  });

  it("a server with parties switched off shows no switch, and the page says so", async () => {
    signIn("pl1");
    server.off = true;
    const view = page("/account");
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByRole("switch")).toBeNull();
    view.unmount();
    page();
    expect(await screen.findByText("Parties are switched off on this server.")).toBeTruthy();
    expect(screen.queryByLabelText("Join code")).toBeNull();
  });
});

describe("as a DM", () => {
  it("creates a party, renames it and deletes it", async () => {
    signIn("dm1");
    page();
    await loaded();
    expect(screen.getByText("No parties yet")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /New party/ }));
    fireEvent.change(within(dialog()).getByLabelText("Party name"), { target: { value: "Tuesday Party" } });
    fireEvent.click(within(dialog()).getByRole("radio", { name: "2024 rules" }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Create" }));
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    expect(within(card).getByText("DM")).toBeTruthy();
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/parties", body: { name: "Tuesday Party", ruleset: "dnd2024" } });

    fireEvent.click(within(card).getByRole("button", { name: /Rename/ }));
    fireEvent.change(within(dialog()).getByLabelText("Party name"), { target: { value: "Friday Party" } });
    fireEvent.click(within(dialog()).getByRole("button", { name: "Rename" }));
    const renamed = await screen.findByRole("region", { name: "Friday Party" });

    fireEvent.click(within(renamed).getByRole("button", { name: /Delete/ }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Friday Party" })).toBeNull());
    expect(server.parties).toEqual([]);
  });

  it("keeps a party when the delete is cancelled", async () => {
    signIn("dm1");
    party("p0", "Keep");
    state.confirm = "cancel";
    page();
    const card = await screen.findByRole("region", { name: "Keep" });
    fireEvent.click(within(card).getByRole("button", { name: /Delete/ }));
    await act(async () => { await Promise.resolve(); });
    expect(server.parties).toHaveLength(1);
    expect(server.calls.some((c) => c.method === "DELETE")).toBe(false);
  });

  it("invites by username: always 'sent', and the invite is listed until withdrawn", async () => {
    signIn("dm1");
    party("p0", "Tuesday Party");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    fireEvent.click(within(card).getByRole("button", { name: /Invite/ }));
    fireEvent.change(within(dialog()).getByLabelText("Username"), { target: { value: " nobody-here " } });
    fireEvent.click(within(dialog()).getByRole("button", { name: "Send invite" }));
    expect(await within(dialog()).findByText(/Invite sent to nobody-here/)).toBeTruthy();
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/parties/p0/invites", body: { kind: "username", username: "nobody-here" } });
    fireEvent.click(within(dialog()).getByRole("button", { name: "Done" }));
    const listed = await within(card).findByText("nobody-here");
    fireEvent.click(within(listed.closest("li")!).getByRole("button", { name: "Withdraw" }));
    await waitFor(() => expect(within(card).queryByText("nobody-here")).toBeNull());
  });

  it("makes a join code with a number of uses and days, shown once; the party lists it with its uses", async () => {
    const code = await dmMakesCode("Tuesday Party", 3);
    expect(code).toMatch(/^K7QM-3XTR-9BWD-\d{4}$/);
    expect(server.calls).toContainEqual({ method: "POST", path: `/api/parties/${server.parties[0]!.id}/invites`, body: { kind: "code", uses: 3, days: 7 } });
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    expect(within(card).getByText(/3 uses left/)).toBeTruthy();
  });

  it("sees members' read-only cards and removes a member", async () => {
    signIn("dm1");
    party("p0", "Tuesday Party");
    member("p0", "pl1", ARIA);
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    const row = within(card).getByRole("listitem", { name: "Aria" });
    expect(within(row).getByText("Level 3 · 2024 rules · alice")).toBeTruthy();
    // Read-only: nothing on the card edits the character.
    expect(within(row).getAllByRole("button").map((b) => b.textContent)).toEqual(["Remove"]);
    fireEvent.click(within(row).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(within(card).queryByRole("listitem", { name: "Aria" })).toBeNull());
    expect(server.calls).toContainEqual({ method: "DELETE", path: `/api/parties/p0/members/pl1/${ARIA}` });
    expect(toastTexts()).toContain("Aria was removed.");
  });

  it("a member whose file is gone shows as missing", async () => {
    signIn("dm1");
    party("p0", "Tuesday Party");
    member("p0", "pl1", charId(99));
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    const row = within(card).getByRole("listitem", { name: "A deleted character" });
    expect(within(row).getByText("Missing")).toBeTruthy();
  });

  it("adds their own character to a party they run (D4), and it shows as theirs", async () => {
    signIn("dm1");
    party("p0", "Tuesday Party");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    fireEvent.click(within(card).getByRole("button", { name: /Add my character/ }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Join with this character" }));
    const row = await within(card).findByRole("listitem", { name: "Dana's Cleric" });
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/parties/p0/members", body: { characterId: CLERIC } });
    expect(within(row).getByText(/your character/)).toBeTruthy();
    expect(within(row).getByRole("button", { name: "Leave" })).toBeTruthy();
    expect(within(row).queryByRole("button", { name: "Remove" })).toBeNull();
    // Still the DM's view of it (plan 3.1).
    expect(within(card).getByText("DM")).toBeTruthy();
  });
});

describe("as a player", () => {
  it("joins with a code: the party's name, then a character to pick", async () => {
    const code = await dmMakesCode();
    signIn("pl1");
    page();
    await loaded();
    expect(screen.getByText("None of your characters is in a party yet.")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Join code"), { target: { value: code.toLowerCase() } });
    fireEvent.click(screen.getByRole("button", { name: "Join" }));
    expect(await screen.findByRole("dialog", { name: "Join Tuesday Party" })).toBeTruthy();
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/join", body: { code: code.replace(/-/g, "") } });
    fireEvent.click(within(dialog()).getByRole("radio", { name: /Bram/ }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Join with this character" }));
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    expect(within(card).getByText("Player")).toBeTruthy();
    expect(within(card).getByText(/DM: dana/)).toBeTruthy();
    expect(within(card).getByRole("listitem", { name: "Bram" })).toBeTruthy();
    expect(toastTexts()).toContain("You joined Tuesday Party.");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("a code that has run out is refused", async () => {
    const code = await dmMakesCode();
    server.invites.find((i) => i.code === code.replace(/-/g, ""))!.usesLeft = 0;
    signIn("pl1");
    page();
    await loaded();
    fireEvent.change(screen.getByLabelText("Join code"), { target: { value: code } });
    fireEvent.click(screen.getByRole("button", { name: "Join" }));
    await waitFor(() => expect(toastTexts()).toContain("That code isn't valid, or it has expired or run out."));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("a code entered but not used yet waits under Invitations", async () => {
    const code = await dmMakesCode();
    signIn("pl1");
    page();
    await loaded();
    fireEvent.change(screen.getByLabelText("Join code"), { target: { value: code } });
    fireEvent.click(screen.getByRole("button", { name: "Join" }));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Cancel" }));
    const invite = await screen.findByRole("listitem", { name: "Invite to Tuesday Party" });
    expect(within(invite).getByText(/your join code/)).toBeTruthy();
  });

  it("accepts a username invite, or declines one", async () => {
    party("p0", "Tuesday Party");
    party("p1", "Other");
    usernameInvite("i0", "p0", "alice");
    usernameInvite("i1", "p1", "alice");
    signIn("pl1");
    page();
    const declined = await screen.findByRole("listitem", { name: "Invite to Other" });
    fireEvent.click(within(declined).getByRole("button", { name: "Decline" }));
    await waitFor(() => expect(screen.queryByRole("listitem", { name: "Invite to Other" })).toBeNull());
    const invite = screen.getByRole("listitem", { name: "Invite to Tuesday Party" });
    expect(within(invite).getByText(/sent to you/)).toBeTruthy();
    fireEvent.click(within(invite).getByRole("button", { name: "Accept" }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Join with this character" }));
    expect(await screen.findByRole("region", { name: "Tuesday Party" })).toBeTruthy();
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/invites/i0/accept", body: { characterId: ARIA } });
    expect(screen.queryByRole("heading", { name: "Invitations" })).toBeNull();
  });

  it("a character already in a party can't be picked (D19)", async () => {
    party("p0", "Tuesday Party");
    party("p1", "Other");
    member("p0", "pl1", ARIA);
    usernameInvite("i1", "p1", "alice");
    signIn("pl1");
    page();
    const invite = await screen.findByRole("listitem", { name: "Invite to Other" });
    fireEvent.click(within(invite).getByRole("button", { name: "Accept" }));
    const aria = within(dialog()).getByRole("radio", { name: /Aria/ }) as HTMLInputElement;
    expect(aria.disabled).toBe(true);
    expect(within(dialog()).getByText(/in Tuesday Party/)).toBeTruthy();
    expect((within(dialog()).getByRole("radio", { name: /Bram/ }) as HTMLInputElement).checked).toBe(true);
  });

  it("the server's refusal of a character in a party shows in the dialog, and the page catches up", async () => {
    party("p0", "Tuesday Party");
    party("p1", "Other");
    usernameInvite("i1", "p1", "alice");
    signIn("pl1");
    page();
    const invite = await screen.findByRole("listitem", { name: "Invite to Other" });
    fireEvent.click(within(invite).getByRole("button", { name: "Accept" }));
    // Another device put Aria in a party meanwhile.
    member("p0", "pl1", ARIA);
    fireEvent.click(within(dialog()).getByRole("button", { name: "Join with this character" }));
    expect(await within(dialog()).findByText("That character is already in a party.")).toBeTruthy();
    expect(await screen.findByRole("region", { name: "Tuesday Party" })).toBeTruthy();
  });

  it("with every character in a party, says so and offers nothing to join with", async () => {
    party("p0", "Tuesday Party");
    party("p1", "Other");
    member("p0", "pl1", ARIA);
    member("p0", "pl1", BRAM);
    usernameInvite("i1", "p1", "alice");
    signIn("pl1");
    page();
    fireEvent.click(within(await screen.findByRole("listitem", { name: "Invite to Other" })).getByRole("button", { name: "Accept" }));
    expect(within(dialog()).getByText(/already in a party/)).toBeTruthy();
    expect((within(dialog()).getByRole("button", { name: "Join with this character" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("leaves a party: the character stays theirs; other members show name and level only", async () => {
    party("p0", "Tuesday Party");
    member("p0", "pl1", ARIA);
    member("p0", "dm1", CLERIC);
    signIn("pl1");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    const other = within(card).getByRole("listitem", { name: "Dana's Cleric" });
    expect(within(other).getByText(/the DM's character \(dana\)/)).toBeTruthy();
    expect(within(other).queryByRole("button")).toBeNull();
    expect(within(card).queryByRole("button", { name: /Invite|Rename|Delete/ })).toBeNull();
    const mine = within(card).getByRole("listitem", { name: "Aria" });
    fireEvent.click(within(mine).getByRole("button", { name: "Leave" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Tuesday Party" })).toBeNull());
    expect(server.calls).toContainEqual({ method: "DELETE", path: `/api/parties/p0/members/pl1/${ARIA}` });
    expect(screen.getByText("None of your characters is in a party yet.")).toBeTruthy();
  });

  it("a removed member's party is gone on the next load", async () => {
    party("p0", "Tuesday Party");
    member("p0", "pl1", ARIA);
    signIn("pl1");
    page();
    await screen.findByRole("region", { name: "Tuesday Party" });
    server.members = [];
    await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
    await waitFor(() => expect(screen.queryByRole("region", { name: "Tuesday Party" })).toBeNull());
  });

  it("a join link fills in the code", async () => {
    signIn("pl1");
    page("/join/ABCD-EFGH");
    await loaded();
    expect((screen.getByLabelText("Join code") as HTMLInputElement).value).toBe("ABCDEFGH");
  });

  it("a page that can't load says why", async () => {
    signIn("pl1");
    partiesDeps.fetch = async () => { throw { code: "network", message: "The server can't be reached. Check your connection." }; };
    page();
    expect(await screen.findByText("The server can't be reached. Check your connection.")).toBeTruthy();
  });
});
