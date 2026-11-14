import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { UpdateAll, UpdateInfo } from "../api/contentUpdate";

const CHECKED = "2026-11-14T09:00:00Z";

const state = vi.hoisted(() => ({
  info: null as unknown as UpdateInfo,
  result: null as unknown as UpdateAll,
  runs: 0,
  saved: [] as unknown[],
  cleared: 0,
  confirm: "confirm" as string,
  asked: [] as { title: string; body: string }[],
  fail: "",
}));
vi.mock("../api/contentUpdate", async (actual) => ({
  ...(await actual<typeof import("../api/contentUpdate")>()),
  updateInfo: async () => state.info,
  updateAll: async () => {
    state.runs++;
    if (state.fail) throw { code: "conflict", message: state.fail };
    state.info = { ...state.info, last: state.result };
    return state.result;
  },
}));
vi.mock("../../src/components/Confirm", () => ({
  useConfirm: () => async (q: { title: string; body: string }) => { state.asked.push(q); return state.confirm; },
}));
const api = vi.hoisted(() => ({
  saveSettings: async (a: unknown) => { state.saved.push(a); return {}; },
  clearDownloadedContent: async () => { state.cleared++; return []; },
  subscribe: () => () => undefined,
}));
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api }));

const { UpdateAllCard } = await import("./UpdateAllCard");
const { resetContentSync } = await import("./contentSync");
const settle = () => new Promise((r) => setTimeout(r, 0));

const result = (over: Partial<UpdateAll> = {}): UpdateAll => ({
  indexes: 3, sources: 1, updated: 2, failed: [], message: "3 index files and 1 5etools source checked · 2 updated",
  checkedAt: CHECKED, auto: false, ...over,
});

beforeEach(() => {
  resetContentSync();
  Object.assign(state, {
    info: { auto: true, everyHours: 24, indexes: 3, sources: 1, last: null },
    result: result(),
    runs: 0, saved: [], cleared: 0, confirm: "confirm", asked: [], fail: "",
  });
});

describe("Update content (0.3.3)", () => {
  it("updates everything with one button and says what was checked", async () => {
    render(<UpdateAllCard />);
    await settle();
    expect(screen.getByRole("region", { name: "Update content" })).toBeTruthy();
    expect(screen.getByRole("status").textContent).toBe("Not checked yet.");
    fireEvent.click(screen.getByRole("button", { name: "Update all content" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("3 index files and 1 5etools source checked · 2 updated"));
    expect(screen.getByRole("status").textContent).toMatch(/^Last checked /);
    expect(state.runs).toBe(1);
    // Only one Update all on the card.
    expect(screen.getAllByRole("button", { name: /update/i })).toHaveLength(1);
  });

  it("lists what could not be updated, and shows a failure in the line", async () => {
    state.result = result({ updated: 0, failed: ["https://data.example.com/5e: the server did not answer"], message: "3 index files and 1 5etools source checked · 0 updated · 1 could not be updated" });
    render(<UpdateAllCard />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Update all content" }));
    await waitFor(() => expect(screen.getByRole("list", { name: "Not updated" }).textContent).toContain("the server did not answer"));
    state.fail = "Other downloads keep running on this account.";
    fireEvent.click(screen.getByRole("button", { name: "Update all content" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Other downloads keep running"));
  });

  it("shows the last automatic check and switches it off for the account", async () => {
    state.info = { ...state.info, last: result({ auto: true }) };
    render(<UpdateAllCard />);
    await settle();
    expect(screen.getByRole("status").textContent).toContain("· 2 updated");
    const box = screen.getByRole("checkbox", { name: /Update content automatically/ }) as HTMLInputElement;
    expect(box.checked).toBe(true);
    expect(screen.getByText(/at most once every 24 hours: index files and 5etools sources/)).toBeTruthy();
    fireEvent.click(box);
    await settle();
    expect(state.saved).toEqual([{ patch: { autoContentUpdate: false } }]);
  });

  it("has nothing to update without sources, and asks before clearing downloads", async () => {
    state.info = { ...state.info, indexes: 0, sources: 0 };
    const { unmount } = render(<UpdateAllCard />);
    await settle();
    expect((screen.getByRole("button", { name: "Update all content" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole("status").textContent).toContain("Add an index file or a 5etools source");
    unmount();

    state.info = { ...state.info, indexes: 2 };
    render(<UpdateAllCard />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Clear downloaded content" }));
    await settle();
    expect(state.asked[0]!.body).toContain("downloaded from your index files");
    expect(state.cleared).toBe(1);
  });
});
