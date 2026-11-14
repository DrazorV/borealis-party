import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ServerEvent } from "../api/http";
import type { Api } from "../../src/api/contract";
import { watchEncounters } from "../api/overrides/encounters";
import { editionToSwitchTo, newestEdition, partiesDeps, resetParties, type Party, type PartyMember, type Ruleset } from "../api/overrides/parties";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { charId, fakePartiesServer, type FakeServer } from "../test/partiesServer";

const state = vi.hoisted(() => ({ confirm: "confirm" as string, asked: [] as { title: string }[], toasts: [] as string[] }));
let api: Api;
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async (o: { title: string }) => { state.asked.push(o); return state.confirm; } }));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (text: string) => state.toasts.push(text) }) }));

const { PartiesPage } = await import("./PartiesPage");
const { EditionSwitch, PartyEditionRow } = await import("./PartyEdition");

const OLD = charId(1);
const NEW = charId(2);
const A = charId(3);
const B = charId(4);
const C = charId(5);

let server: FakeServer;

function accounts(dmCharacters: { id: string; name: string; level: number; ruleset?: Ruleset; updatedAt?: string }[] = []) {
  return [
    { id: "dm1", username: "dana", dm: true, characters: dmCharacters },
    {
      id: "pl1", username: "alice", dm: false, characters: [
        { id: A, name: "Aria", level: 3, ruleset: "dnd2014" as Ruleset },
        { id: B, name: "Bram", level: 3, ruleset: "dnd2014" as Ruleset },
        { id: C, name: "Cora", level: 3, ruleset: "dnd2024" as Ruleset },
      ],
    },
  ];
}

function signIn(uid: string) {
  server.me = uid;
  const a = server.accounts.find((x) => x.id === uid)!;
  saveSession({ baseUrl: "http://server", token: "t", user: { id: a.id, username: a.username, email: null, admin: false, mustChangePassword: false } });
  api = {
    listCharacters: async () => a.characters.map((c) => ({ id: c.id, name: c.name, build: "", level: c.level, ruleset: c.ruleset, updatedAt: c.updatedAt })),
  } as unknown as Api;
}

function start(dmCharacters?: Parameters<typeof accounts>[0]) {
  server = fakePartiesServer(accounts(dmCharacters), "dm1");
  partiesDeps.fetch = server.fetch;
  partiesDeps.events = server.listen;
}

/** Party p1, run by dm1, with `members` of alice's (Aria and Bram use 2014 rules, Cora 2024). */
function party(ruleset: Ruleset, members: string[]) {
  server.parties.push({ id: "p1", name: "Tuesday Party", dmId: "dm1", createdAt: 1, ruleset });
  for (const m of members) server.members.push({ ownerId: "pl1", characterId: m, partyId: "p1", joinedAt: 1 });
}

const patches = () => server.calls.filter((c) => c.method === "PATCH");
const dialog = () => screen.getByRole("dialog");

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  state.confirm = "confirm";
  state.asked = [];
  state.toasts = [];
  start();
});

afterEach(() => {
  resetParties();
  vi.unstubAllGlobals();
});

describe("the edition rules", () => {
  const member = (ruleset: Ruleset | null) => ({ ruleset }) as PartyMember;
  const of = (role: "dm" | "player", ruleset: Ruleset, members: (Ruleset | null)[]) =>
    ({ role, ruleset, members: members.map(member) }) as Pick<Party, "role" | "ruleset" | "members">;

  it("offers the other edition when most of the characters use it", () => {
    expect(editionToSwitchTo(of("dm", "dnd2024", ["dnd2014", "dnd2014", "dnd2024"]))).toBe("dnd2014");
    expect(editionToSwitchTo(of("dm", "dnd2014", ["dnd2024"]))).toBe("dnd2024");
  });

  it("not on a tie, when the party matches, or with no readable characters", () => {
    expect(editionToSwitchTo(of("dm", "dnd2024", ["dnd2014", "dnd2024"]))).toBeNull();
    expect(editionToSwitchTo(of("dm", "dnd2024", ["dnd2024", "dnd2024", "dnd2014"]))).toBeNull();
    expect(editionToSwitchTo(of("dm", "dnd2024", []))).toBeNull();
    expect(editionToSwitchTo(of("dm", "dnd2024", [null, null]))).toBeNull();
  });

  it("an unreadable character counts for neither edition", () => {
    expect(editionToSwitchTo(of("dm", "dnd2024", ["dnd2014", null, null]))).toBe("dnd2014");
  });

  it("never for a player", () => {
    expect(editionToSwitchTo(of("player", "dnd2024", ["dnd2014", "dnd2014"]))).toBeNull();
  });

  it("pre-selects the edition of the most recently changed character, or none", () => {
    expect(newestEdition([])).toBeNull();
    expect(newestEdition([{ ruleset: "dnd2024", updatedAt: "2026-01-01T00:00:00Z" }, { ruleset: "dnd2014", updatedAt: "2026-11-01T00:00:00Z" }])).toBe("dnd2014");
    expect(newestEdition([{ ruleset: "dnd2014", updatedAt: "2026-11-01T00:00:00Z" }, { ruleset: "dnd2024", updatedAt: "2026-11-02T00:00:00Z" }])).toBe("dnd2024");
    expect(newestEdition([{ ruleset: "nonsense", updatedAt: "2026-11-02T00:00:00Z" }])).toBeNull();
  });
});

function page() {
  return render(
    <MemoryRouter initialEntries={["/parties"]}>
      <Routes><Route path="/parties" element={<PartiesPage />} /></Routes>
    </MemoryRouter>,
  );
}

describe("New party", () => {
  it("has no edition picked when the DM has no characters: Create waits for one, and it is sent", async () => {
    signIn("dm1");
    page();
    fireEvent.click(await screen.findByRole("button", { name: /New party/ }));
    const d = dialog();
    fireEvent.change(within(d).getByLabelText("Party name"), { target: { value: "Friday" } });
    const radios = within(d).getAllByRole("radio");
    expect(radios.map((r) => [r.textContent, r.getAttribute("aria-checked")])).toEqual([["2014 rules", "false"], ["2024 rules", "false"]]);
    const create = within(d).getByRole("button", { name: "Create" }) as HTMLButtonElement;
    expect(create.disabled).toBe(true);
    fireEvent.click(within(d).getByRole("radio", { name: "2014 rules" }));
    expect(create.disabled).toBe(false);
    fireEvent.click(create);
    const card = await screen.findByRole("region", { name: "Friday" });
    expect(server.calls).toContainEqual({ method: "POST", path: "/api/parties", body: { name: "Friday", ruleset: "dnd2014" } });
    expect(within(card).getByText("2014 rules")).toBeTruthy();
  });

  it("pre-selects the edition of the DM's most recent character", async () => {
    start([
      { id: OLD, name: "Old", level: 1, ruleset: "dnd2024", updatedAt: "2026-01-01T00:00:00Z" },
      { id: NEW, name: "New", level: 1, ruleset: "dnd2014", updatedAt: "2026-11-01T00:00:00Z" },
    ]);
    signIn("dm1");
    page();
    fireEvent.click(await screen.findByRole("button", { name: /New party/ }));
    expect(within(dialog()).getByRole("radio", { name: "2014 rules" }).getAttribute("aria-checked")).toBe("true");
    expect(within(dialog()).getByRole("radio", { name: "2024 rules" }).getAttribute("aria-checked")).toBe("false");
  });
});

describe("Change edition on the party card", () => {
  it("the DM changes it with the existing PATCH; the dialog says what follows it", async () => {
    party("dnd2024", [A, B, C]);
    signIn("dm1");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    expect(within(card).getByText("2024 rules")).toBeTruthy();
    expect(within(card).getByText("Most of its characters use 2014 rules.")).toBeTruthy();
    fireEvent.click(within(card).getByRole("button", { name: "Change edition" }));
    const d = screen.getByRole("dialog", { name: "Change edition" });
    expect(d.textContent).toContain("A combat that is running keeps the rules it started with.");
    expect(d.textContent).toContain("Its characters: 2 use 2014 rules, 1 uses 2024 rules.");
    fireEvent.click(within(d).getByRole("button", { name: "Switch to 2014 rules" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(patches()).toEqual([{ method: "PATCH", path: "/api/parties/p1", body: { ruleset: "dnd2014" } }]);
    await waitFor(() => expect(within(screen.getByRole("region", { name: "Tuesday Party" })).getByText("2014 rules")).toBeTruthy());
    expect(within(screen.getByRole("region", { name: "Tuesday Party" })).queryByText(/Most of its characters/)).toBeNull();
  });

  it("a player sees the edition but can't change it", async () => {
    party("dnd2024", [A]);
    signIn("pl1");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    expect(within(card).getByText("2024 rules")).toBeTruthy();
    expect(within(card).queryByRole("button", { name: "Change edition" })).toBeNull();
    expect(within(card).queryByText(/Most of its characters/)).toBeNull();
  });
});

describe("the party card's layout (0.3.3: one header row, one toolbar)", () => {
  it("the DM's card: name, edition with Change edition inline and the DM badge in one header; Encounters to Delete in one toolbar", async () => {
    party("dnd2024", [A]);
    signIn("dm1");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    const head = card.querySelector(".party-card-head")!;
    expect(within(head as HTMLElement).getByRole("heading", { name: "Tuesday Party" })).toBeTruthy();
    expect(head.textContent).toMatch(/Edition:\s*2024 rules\s*Change edition.*DM/);
    const bar = within(card).getByRole("toolbar", { name: "Tuesday Party actions" });
    const labels = [...bar.children].map((b) => b.textContent?.trim());
    expect(labels).toEqual(["Encounters", "Invite", "Add my character", "Rename", "Delete"]);
    expect(bar.lastElementChild!.classList.contains("party-actions-end")).toBe(true);
    // The DM keeps the numbers outside combat, with View on the same row as the chips.
    const member = within(card).getByRole("listitem", { name: "Aria" });
    const body = member.querySelector(".party-member-body")!;
    expect(within(body as HTMLElement).getByRole("link", { name: "View Aria" })).toBeTruthy();
  });

  it("a player's card has no toolbar, and says who the DM is under the header", async () => {
    party("dnd2024", [A]);
    signIn("pl1");
    page();
    const card = await screen.findByRole("region", { name: "Tuesday Party" });
    expect(within(card).queryByRole("toolbar")).toBeNull();
    expect(card.querySelector(".party-card-meta")!.textContent).toContain("DM: dana");
    expect(card.querySelector(".party-card-head")!.textContent).toContain("Player");
  });
});

describe("the edition notes' switch", () => {
  it("DM, most characters use 2014: one tap, a confirm, then the PATCH", async () => {
    party("dnd2024", [A, B, C]);
    signIn("dm1");
    const changed = vi.fn();
    render(<EditionSwitch partyId="p1" onChanged={changed} />);
    const button = await screen.findByRole("button", { name: "Switch party to 2014 rules" });
    fireEvent.click(button);
    await waitFor(() => expect(changed).toHaveBeenCalledTimes(1));
    expect(state.asked.map((a) => a.title)).toEqual(["Switch Tuesday Party to 2014 rules?"]);
    expect(patches()).toEqual([{ method: "PATCH", path: "/api/parties/p1", body: { ruleset: "dnd2014" } }]);
    expect(state.toasts).toContain("Tuesday Party now uses 2014 rules.");
  });

  it("cancelling the confirm changes nothing", async () => {
    party("dnd2024", [A, B]);
    signIn("dm1");
    state.confirm = "cancel";
    const changed = vi.fn();
    render(<EditionSwitch partyId="p1" onChanged={changed} />);
    fireEvent.click(await screen.findByRole("button", { name: "Switch party to 2014 rules" }));
    await waitFor(() => expect(state.asked).toHaveLength(1));
    expect(patches()).toEqual([]);
    expect(changed).not.toHaveBeenCalled();
  });

  it("nothing when the party matches most characters, or on a tie", async () => {
    party("dnd2014", [A, B, C]);
    signIn("dm1");
    const view = render(<EditionSwitch partyId="p1" />);
    await waitFor(() => expect(server.calls.some((c) => c.path === "/api/parties/p1")).toBe(true));
    expect(view.container.textContent).toBe("");
    view.unmount();
    server.parties[0]!.ruleset = "dnd2024";
    server.members = server.members.filter((m) => m.characterId !== B);
    const tie = render(<EditionSwitch partyId="p1" />);
    await waitFor(() => expect(server.calls.filter((c) => c.path === "/api/parties/p1")).toHaveLength(2));
    expect(tie.container.textContent).toBe("");
  });

  it("never for a player", async () => {
    party("dnd2024", [A, B]);
    signIn("pl1");
    const view = render(<EditionSwitch partyId="p1" />);
    await waitFor(() => expect(server.calls.some((c) => c.path === "/api/parties/p1")).toBe(true));
    expect(view.container.textContent).toBe("");
  });
});

describe("the encounter's Setup", () => {
  it("shows the party's edition and changes it from there", async () => {
    party("dnd2024", [A]);
    signIn("dm1");
    const changed = vi.fn();
    render(<PartyEditionRow partyId="p1" ruleset="dnd2024" onChanged={changed} />);
    expect(screen.getByText(/The party's edition:/).textContent).toBe("The party's edition: 2024 rules");
    fireEvent.click(screen.getByRole("button", { name: "Change edition" }));
    const d = await screen.findByRole("dialog", { name: "Change edition" });
    fireEvent.click(await within(d).findByRole("button", { name: "Switch to 2014 rules" }));
    await waitFor(() => expect(changed).toHaveBeenCalledTimes(1));
    expect(patches()).toEqual([{ method: "PATCH", path: "/api/parties/p1", body: { ruleset: "dnd2014" } }]);
  });

  it("open encounter screens read again when the party changes (its SSE event)", () => {
    const seen: (string | null)[] = [];
    const stop = watchEncounters("p1", (id) => seen.push(id));
    const send = (e: ServerEvent) => server.emit(e);
    send({ event: "party://party", payload: { party: "p1", pseq: 2, kind: "party", id: null } } as unknown as ServerEvent);
    send({ event: "party://party", payload: { party: "p2", pseq: 2, kind: "party", id: null } } as unknown as ServerEvent);
    stop();
    expect(seen).toEqual([null]);
  });
});
