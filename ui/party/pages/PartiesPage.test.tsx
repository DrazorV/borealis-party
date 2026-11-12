import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Api } from "../../src/api/contract";
import { partiesDeps, resetParties } from "../api/overrides/parties";
import { saveSession, loadSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { fakePartiesServer, type FakeServer } from "../test/partiesServer";

const state = vi.hoisted(() => ({ confirm: "confirm" as string, toasts: [] as { text: string; tone?: string }[] }));
let api: Api;
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => state.confirm }));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (text: string, tone?: string) => state.toasts.push({ text, tone }) }) }));

const { PartiesPage, JoinRoute } = await import("./PartiesPage");
const { DungeonMasterCard } = await import("./PartiesDm");

const ACCOUNTS = () => [
  { id: "dm1", username: "dana", dm: true, characters: [{ id: "c_dm", name: "Dana's Cleric", classLevel: "Cleric 3" }] },
  { id: "pl1", username: "alice", dm: false, characters: [
    { id: "c_a1", name: "Aria", classLevel: "Fighter 3" },
    { id: "c_a2", name: "Bram", classLevel: "Wizard 2" },
  ] },
];

let server: FakeServer;

function signIn(uid: string) {
  server.me = uid;
  const a = server.accounts.find((x) => x.id === uid)!;
  saveSession({ baseUrl: "http://server", token: "t", user: { id: a.id, username: a.username, email: null, admin: false, mustChangePassword: false, dm: a.dm } as never });
  api = { listCharacters: async () => a.characters.map((c) => ({ id: c.id, name: c.name, build: c.classLevel, level: 3 })) } as unknown as Api;
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

/** The DM makes a party and a join code; answers the code. */
async function dmMakesCode(name = "Tuesday Party", uses = 1): Promise<string> {
  signIn("dm1");
  const view = page();
  await loaded();
  fireEvent.click(screen.getByRole("button", { name: /New party/ }));
  fireEvent.change(within(dialog()).getByLabelText("Party name"), { target: { value: name } });
  fireEvent.click(within(dialog()).getByRole("button", { name: "Create" }));
  const card = await screen.findByRole("region", { name });
  fireEvent.click(within(card).getByRole("button", { name: /Invite/ }));
  fireEvent.click(within(dialog()).getByRole("tab", { name: "Join code" }));
  if (uses !== 1) fireEvent.change(within(dialog()).getByRole("spinbutton", { name: "Number of uses" }), { target: { value: String(uses) } });
  fireEvent.click(within(dialog()).getByRole("button", { name: "Make a join code" }));
  const out = await within(dialog()).findByLabelText("Join code");
  const code = out.textContent!.replace(/-/g, "");
  view.unmount();
  return code;
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  server = fakePartiesServer(ACCOUNTS(), "pl1");
  partiesDeps.fetch = server.fetch;
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

  it("on: the account switch asks the server, keeps the answer, and shows the way to the parties", async () => {
    signIn("pl1");
    page("/account");
    const toggle = screen.getByRole("switch", { name: "Dungeon Master tools" });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle.getAttribute("aria-checked")).toBe("true"));
    expect(server.calls).toContainEqual({ method: "PUT", path: "/api/me/roles/dm", body: { on: true } });
    expect((loadSession()?.user as { dm?: boolean }).dm).toBe(true);
    expect(screen.getByRole("link", { name: "Manage your parties" })).toBeTruthy();
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle.getAttribute("aria-checked")).toBe("false"));
    expect((loadSession()?.user as { dm?: boolean }).dm).toBe(false);
  });

  it("a refused switch keeps its place and says why", async () => {
    signIn("pl1");
    partiesDeps.fetch = async () => { throw { code: "forbidden", message: "Confirm your email address first." }; };
    page("/account");
    const toggle = screen.getByRole("switch", { name: "Dungeon Master tools" });
    fireEvent.click(toggle);
    await waitFor(() => expect(toastTexts()).toContain("Confirm your email address first."));
    expect(toggle.getAttribute("aria-checked")).toBe("false");
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
    fireEvent.click(within(dialog()).getByRole("button", { name: "Create" }));
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    expect(within(card).getByText("DM")).toBeTruthy();
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/parties", body: { name: "Tuesday Party" } });

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
    server.parties.push({ id: "p0", name: "Keep", dmId: "dm1", createdAt: 1 });
    state.confirm = "cancel";
    page();
    const card = await screen.findByRole("region", { name: "Keep" });
    fireEvent.click(within(card).getByRole("button", { name: /Delete/ }));
    await act(async () => { await Promise.resolve(); });
    expect(server.parties).toHaveLength(1);
  });

  it("invites by username: always 'sent', and the invite is listed until withdrawn", async () => {
    signIn("dm1");
    server.parties.push({ id: "p0", name: "Tuesday Party", dmId: "dm1", createdAt: 1 });
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    fireEvent.click(within(card).getByRole("button", { name: /Invite/ }));
    fireEvent.change(within(dialog()).getByLabelText("Username"), { target: { value: " nobody-here " } });
    fireEvent.click(within(dialog()).getByRole("button", { name: "Send invite" }));
    expect(await within(dialog()).findByText(/Invite sent to nobody-here/)).toBeTruthy();
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/parties/p0/invites", body: { username: "nobody-here" } });
    fireEvent.click(within(dialog()).getByRole("button", { name: "Done" }));
    const listed = await within(card).findByText("nobody-here");
    fireEvent.click(within(listed.closest("li")!).getByRole("button", { name: "Withdraw" }));
    await waitFor(() => expect(within(card).queryByText("nobody-here")).toBeNull());
  });

  it("makes a join code with a number of uses, shown once with a link to copy", async () => {
    const code = await dmMakesCode("Tuesday Party", 3);
    expect(code).toMatch(/^CODE/);
    expect(server.calls).toContainEqual({ method: "POST", path: `/api/parties/${server.parties[0]!.id}/invites`, body: { uses: 3, expiresInDays: 7 } });
  });

  it("sees members' read-only cards and removes a member", async () => {
    signIn("dm1");
    server.parties.push({ id: "p0", name: "Tuesday Party", dmId: "dm1", createdAt: 1 });
    server.members.push({ ownerId: "pl1", characterId: "c_a1", partyId: "p0", joinedAt: 1 });
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    const member = within(card).getByRole("listitem", { name: "Aria" });
    expect(within(member).getByText(/Fighter 3 · alice/)).toBeTruthy();
    expect(within(member).getByText("AC")).toBeTruthy();
    expect(within(member).getByText("20/24")).toBeTruthy();
    expect(within(member).getByText("Passive Perception")).toBeTruthy();
    expect(within(member).queryByText("Spell DC")).toBeNull();
    fireEvent.click(within(member).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(within(card).queryByRole("listitem", { name: "Aria" })).toBeNull());
    expect(server.calls).toContainEqual({ method: "DELETE", path: "/api/parties/p0/members/pl1/c_a1" });
    expect(toastTexts()).toContain("Aria was removed.");
  });

  it("adds their own character to a party they run (D4)", async () => {
    signIn("dm1");
    server.parties.push({ id: "p0", name: "Tuesday Party", dmId: "dm1", createdAt: 1 });
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    fireEvent.click(within(card).getByRole("button", { name: /Add my character/ }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Join with this character" }));
    const member = await within(card).findByRole("listitem", { name: "Dana's Cleric" });
    expect(within(member).getByText(/your character/)).toBeTruthy();
    expect(within(member).getByRole("button", { name: "Leave" })).toBeTruthy();
    expect(within(member).queryByRole("button", { name: "Remove" })).toBeNull();
  });
});

describe("as a player", () => {
  it("joins with a code: the party's name, then a character to pick", async () => {
    const code = await dmMakesCode();
    signIn("pl1");
    page();
    await loaded();
    expect(screen.getByText("None of your characters is in a party yet.")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Join code"), { target: { value: code.toLowerCase().replace(/(.{4})/, "$1-") } });
    fireEvent.click(screen.getByRole("button", { name: "Join" }));
    expect(await screen.findByRole("dialog", { name: "Join Tuesday Party" })).toBeTruthy();
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/join", body: { code } });
    fireEvent.click(within(dialog()).getByRole("radio", { name: /Bram/ }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Join with this character" }));
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    expect(within(card).getByText("Player")).toBeTruthy();
    expect(within(card).getByText("DM: dana")).toBeTruthy();
    expect(within(card).getByRole("listitem", { name: "Bram" })).toBeTruthy();
    expect(toastTexts()).toContain("You joined Tuesday Party.");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("a code that has run out is refused, in the server's words", async () => {
    const code = await dmMakesCode();
    server.invites.find((i) => i.code === code)!.usesLeft = 0;
    signIn("pl1");
    page();
    await loaded();
    fireEvent.change(screen.getByLabelText("Join code"), { target: { value: code } });
    fireEvent.click(screen.getByRole("button", { name: "Join" }));
    await waitFor(() => expect(toastTexts()).toContain("That code isn't valid, or it has run out."));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("accepts a username invite, or declines one", async () => {
    server.parties.push({ id: "p0", name: "Tuesday Party", dmId: "dm1", createdAt: 1 }, { id: "p1", name: "Other", dmId: "dm1", createdAt: 1 });
    server.invites.push({ id: "i0", partyId: "p0", username: "alice", usesLeft: 1, expiresAt: Date.now() + 1e9 },
      { id: "i1", partyId: "p1", username: "alice", usesLeft: 1, expiresAt: Date.now() + 1e9 });
    signIn("pl1");
    page();
    const declined = await screen.findByRole("listitem", { name: "Invite to Other" });
    fireEvent.click(within(declined).getByRole("button", { name: "Decline" }));
    await waitFor(() => expect(screen.queryByRole("listitem", { name: "Invite to Other" })).toBeNull());
    const invite = screen.getByRole("listitem", { name: "Invite to Tuesday Party" });
    fireEvent.click(within(invite).getByRole("button", { name: "Accept" }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Join with this character" }));
    expect(await screen.findByRole("region", { name: "Tuesday Party" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Invitations" })).toBeNull();
  });

  it("a character already in a party can't be picked (D19)", async () => {
    server.parties.push({ id: "p0", name: "Tuesday Party", dmId: "dm1", createdAt: 1 }, { id: "p1", name: "Other", dmId: "dm1", createdAt: 1 });
    server.members.push({ ownerId: "pl1", characterId: "c_a1", partyId: "p0", joinedAt: 1 });
    server.invites.push({ id: "i1", partyId: "p1", username: "alice", usesLeft: 1, expiresAt: Date.now() + 1e9 });
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
    server.parties.push({ id: "p0", name: "Tuesday Party", dmId: "dm1", createdAt: 1 }, { id: "p1", name: "Other", dmId: "dm1", createdAt: 1 });
    server.invites.push({ id: "i1", partyId: "p1", username: "alice", usesLeft: 1, expiresAt: Date.now() + 1e9 });
    signIn("pl1");
    page();
    const invite = await screen.findByRole("listitem", { name: "Invite to Other" });
    fireEvent.click(within(invite).getByRole("button", { name: "Accept" }));
    // Another device put Aria in a party meanwhile.
    server.members.push({ ownerId: "pl1", characterId: "c_a1", partyId: "p0", joinedAt: 1 });
    fireEvent.click(within(dialog()).getByRole("button", { name: "Join with this character" }));
    expect(await within(dialog()).findByText("That character is already in a party.")).toBeTruthy();
    expect(await screen.findByRole("region", { name: "Tuesday Party" })).toBeTruthy();
  });

  it("with every character in a party, says so and offers nothing to join with", async () => {
    server.parties.push({ id: "p0", name: "Tuesday Party", dmId: "dm1", createdAt: 1 }, { id: "p1", name: "Other", dmId: "dm1", createdAt: 1 });
    server.members.push({ ownerId: "pl1", characterId: "c_a1", partyId: "p0", joinedAt: 1 }, { ownerId: "pl1", characterId: "c_a2", partyId: "p0", joinedAt: 1 });
    server.invites.push({ id: "i1", partyId: "p1", username: "alice", usesLeft: 1, expiresAt: Date.now() + 1e9 });
    signIn("pl1");
    page();
    fireEvent.click(within(await screen.findByRole("listitem", { name: "Invite to Other" })).getByRole("button", { name: "Accept" }));
    expect(within(dialog()).getByText(/already in a party/)).toBeTruthy();
    expect((within(dialog()).getByRole("button", { name: "Join with this character" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("leaves a party: the character stays theirs and can join another", async () => {
    server.parties.push({ id: "p0", name: "Tuesday Party", dmId: "dm1", createdAt: 1 });
    server.members.push({ ownerId: "pl1", characterId: "c_a1", partyId: "p0", joinedAt: 1 }, { ownerId: "dm1", characterId: "c_dm", partyId: "p0", joinedAt: 1 });
    signIn("pl1");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    // Another member's character: name and class only, no card, nothing to press.
    const other = within(card).getByRole("listitem", { name: "Dana's Cleric" });
    expect(within(other).queryByText("AC")).toBeNull();
    expect(within(other).queryByRole("button")).toBeNull();
    const mine = within(card).getByRole("listitem", { name: "Aria" });
    expect(within(mine).getByText("AC")).toBeTruthy();
    expect(within(card).queryByRole("button", { name: /Invite|Rename|Delete/ })).toBeNull();
    fireEvent.click(within(mine).getByRole("button", { name: "Leave" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Tuesday Party" })).toBeNull());
    expect(server.calls).toContainEqual({ method: "DELETE", path: "/api/parties/p0/members/pl1/c_a1" });
    expect(screen.getByText("None of your characters is in a party yet.")).toBeTruthy();
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
