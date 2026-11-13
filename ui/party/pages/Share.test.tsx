import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Api } from "../../src/api/contract";

const calls = vi.hoisted(() => [] as { name: string; args: unknown }[]);
const state = vi.hoisted(() => ({
  confirm: "confirm" as string,
  list: { sent: [] as unknown[], received: [] as unknown[], blocked: [] as unknown[], limits: { openPerSender: 20, waitingPerSender: 8, days: 7, maxBytes: 95 * 1024 * 1024, diskBytes: 190 * 1024 * 1024, heldBytes: 0 } },
  redeemed: null as unknown,
  refuse: null as string | null,
  outcome: null as ((id: string, outcome: "accepted" | "cancelled") => void) | null,
  opened: [] as unknown[],
}));

const api = {
  listCharacters: async () => [{ id: "c1", name: "Ana", build: "Elf Wizard 3" }, { id: "c2", name: "Bo", build: "" }],
  updateIndexes: async () => { calls.push({ name: "updateIndexes", args: {} }); return { updated: 1 }; },
} as unknown as Api;

vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async (o: unknown) => { calls.push({ name: "confirm", args: o }); return state.confirm; } }));
vi.mock("../../src/pages/transfer/openBundle", () => ({ openBundleFile: (r: unknown) => state.opened.push(r) }));
vi.mock("../api/overrides/shares", () => {
  const rec = (name: string, answer: (args: unknown[]) => unknown) => async (...args: unknown[]) => {
    calls.push({ name, args });
    if (state.refuse) throw { code: "not_found", message: state.refuse };
    return answer(args);
  };
  return {
    listShares: async () => state.list,
    createShare: rec("createShare", () => ({
      share: { id: "s1", fileName: "Ana.borealis", characters: 1, packs: 0, links: [], bytes: 2048, createdAt: 1, expiresAt: 2_000_000_000, status: "pending", forUsername: null, claimedBy: null },
      code: "ABCD-EFGH-JKMN-PQRS", link: "https://party.example.com/#/share/receive/ABCD-EFGH-JKMN-PQRS", leftOut: ["Some book"],
    })),
    cancelShare: rec("cancelShare", () => ({})),
    unblockSender: rec("unblockSender", () => ({})),
    redeemShare: rec("redeemShare", () => state.redeemed),
    reopenShare: rec("reopenShare", () => state.redeemed),
    declineShare: rec("declineShare", () => ({})),
    acceptShare: rec("acceptShare", () => ({})),
    addShareLinks: rec("addShareLinks", () => ({ indexes: [{ url: "https://content.example/a.index", packs: ["A"], added: true }], fivetools: [] })),
    onShareOutcome: (l: typeof state.outcome) => { state.outcome = l; return () => { state.outcome = null; }; },
  };
});

const { SharePage } = await import("./SharePage");
const { ShareReceivePage } = await import("./ShareReceivePage");

function at(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/share" element={<SharePage />} />
        <Route path="/share/receive/:code?" element={<ShareReceivePage />} />
        <Route path="/content" element={<p>Additional Content</p>} />
      </Routes>
    </MemoryRouter>,
  );
}

const names = () => calls.map((c) => c.name);

beforeEach(() => {
  calls.length = 0;
  state.confirm = "confirm";
  state.refuse = null;
  state.opened = [];
  state.list = { ...state.list, sent: [], received: [], blocked: [] };
  state.redeemed = {
    share: { id: "r1", from: "alice", fromId: "a", fileName: "Ana.borealis", characters: 1, packs: 1, links: [], bytes: 4096, expiresAt: 2_000_000_000, status: "claimed" },
    bundle: { token: "tok", name: "Ana.borealis" },
  };
});

describe("Share with a code", () => {
  it("makes a code of the chosen characters and shows it with its link", async () => {
    at("/share");
    fireEvent.click(await screen.findByRole("checkbox", { name: /Ana/ }));
    fireEvent.change(screen.getByLabelText("Only for (optional)"), { target: { value: " bob " } });
    fireEvent.click(screen.getByRole("button", { name: "Make a share code" }));
    expect((await screen.findByTestId("share-code")).textContent).toContain("ABCD-EFGH-JKMN-PQRS");
    expect(calls.find((c) => c.name === "createShare")?.args).toEqual([{ characterIds: ["c1"], packs: "required", includeFiles: false, forUsername: "bob" }]);
    expect((screen.getByLabelText("Share link") as HTMLInputElement).value).toBe("https://party.example.com/#/share/receive/ABCD-EFGH-JKMN-PQRS");
    expect(screen.getByText(/Left out, as they can only travel as files: Some book/)).toBeTruthy();
  });

  it("nothing chosen and no content packs: nothing to make", async () => {
    at("/share");
    await screen.findByRole("checkbox", { name: /Ana/ });
    fireEvent.change(screen.getByLabelText("Content packs"), { target: { value: "none" } });
    expect((screen.getByRole("button", { name: "Make a share code" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("including the files asks about the licence first; Cancel leaves them out", async () => {
    at("/share");
    const files = await screen.findByRole("checkbox", { name: /Include the files/ });
    state.confirm = "cancel";
    fireEvent.click(files);
    await waitFor(() => expect(names()).toContain("confirm"));
    expect((files as HTMLInputElement).checked).toBe(false);
    state.confirm = "confirm";
    fireEvent.click(files);
    await waitFor(() => expect((files as HTMLInputElement).checked).toBe(true));
    fireEvent.click(screen.getByRole("checkbox", { name: /Bo/ }));
    fireEvent.click(screen.getByRole("button", { name: "Make a share code" }));
    await screen.findByTestId("share-code");
    expect(calls.find((c) => c.name === "createShare")?.args).toEqual([{ characterIds: ["c2"], packs: "required", includeFiles: true, forUsername: undefined }]);
  });

  it("lists this account's codes, with Cancel on unused ones, and the blocked players", async () => {
    state.list = {
      ...state.list,
      sent: [
        { id: "p", characters: 1, packs: 0, links: [], status: "pending", expiresAt: 2_000_000_000, createdAt: 1, forUsername: null, claimedBy: null },
        { id: "d", characters: 0, packs: 2, links: [], status: "done", expiresAt: 1, createdAt: 1, forUsername: null, claimedBy: "bob" },
      ],
      blocked: [{ id: "x", username: "pest" }],
    };
    at("/share");
    const list = await screen.findByRole("list", { name: "Your share codes" });
    expect(within(list).getByText(/Accepted by bob/)).toBeTruthy();
    fireEvent.click(within(list).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(calls.find((c) => c.name === "cancelShare")?.args).toEqual(["p"]));
    fireEvent.click(within(screen.getByRole("list", { name: "Blocked players" })).getByRole("button", { name: "Unblock" }));
    await waitFor(() => expect(calls.find((c) => c.name === "unblockSender")?.args).toEqual(["x"]));
  });
});

describe("Enter a share code", () => {
  it("opens the share in the Open bundle window and says when it was accepted", async () => {
    at("/share/receive");
    fireEvent.change(screen.getByLabelText("Share code"), { target: { value: "abcd efgh jkmn pqrs" } });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(await screen.findByText("From alice")).toBeTruthy();
    expect(calls.find((c) => c.name === "redeemShare")?.args).toEqual(["abcd efgh jkmn pqrs"]);
    expect(state.opened).toEqual([{ token: "tok", name: "Ana.borealis" }]);
    act(() => state.outcome?.("r1", "accepted"));
    expect(await screen.findByText(/Accepted\. Anything you already had/)).toBeTruthy();
  });

  it("a share link redeems its code once, as if typed", async () => {
    at("/share/receive/ABCD-EFGH-JKMN-PQRS");
    expect(await screen.findByText("From alice")).toBeTruthy();
    expect(calls.filter((c) => c.name === "redeemShare")).toHaveLength(1);
  });

  it("a wrong code shows the server's words", async () => {
    state.refuse = "That code doesn't work.";
    at("/share/receive");
    fireEvent.change(screen.getByLabelText("Share code"), { target: { value: "nope" } });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect((await screen.findByRole("alert")).textContent).toContain("That code doesn't work.");
  });

  it("decline and block asks first", async () => {
    at("/share/receive/ABCD");
    await screen.findByText("From alice");
    state.confirm = "cancel";
    fireEvent.click(screen.getByRole("button", { name: "Decline and block" }));
    await waitFor(() => expect(names()).toContain("confirm"));
    expect(names()).not.toContain("declineShare");
    state.confirm = "confirm";
    fireEvent.click(screen.getByRole("button", { name: "Decline and block" }));
    await waitFor(() => expect(calls.find((c) => c.name === "declineShare")?.args).toEqual(["r1", true]));
    expect(await screen.findByText(/Declined: nothing came in/)).toBeTruthy();
  });

  it("a share of links only adds them at once and downloads the indexes", async () => {
    state.redeemed = {
      share: { id: "l1", from: "alice", fromId: "a", fileName: "x", characters: 0, packs: 0, bytes: 0, expiresAt: 2_000_000_000, status: "claimed",
        links: [{ kind: "index", url: "https://content.example/a.index", packs: ["A"] }, { kind: "5etools", url: "https://github.com/o/r", packs: ["B"] }] },
      bundle: null,
    };
    at("/share/receive/ABCD");
    expect(await screen.findByText("Added.")).toBeTruthy();
    await waitFor(() => expect(names()).toEqual(expect.arrayContaining(["addShareLinks", "updateIndexes", "acceptShare"])));
    expect(state.opened).toEqual([]);
    expect(screen.getByText(/5etools data: https:\/\/github.com\/o\/r/)).toBeTruthy();
  });

  it("shares waiting from before can be opened again", async () => {
    state.list = { ...state.list, received: [{ id: "w1", from: "carol", fromId: "c", characters: 2, packs: 0, links: [], bytes: 1, expiresAt: 2_000_000_000, status: "claimed" }] };
    at("/share/receive");
    const list = await screen.findByRole("list", { name: "Shares waiting for you" });
    fireEvent.click(within(list).getByRole("button", { name: "Open" }));
    await waitFor(() => expect(calls.find((c) => c.name === "reopenShare")?.args).toEqual(["w1"]));
    expect(state.opened).toHaveLength(1);
  });
});
