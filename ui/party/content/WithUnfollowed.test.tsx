import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ContentIndex, ContentPack } from "../../src/api/contract";
import type { PackSource } from "../api/packRemoval";

const LEGACY = "https://raw.githubusercontent.com/o/legacy/master/Legacy.index";
const THIRD = "https://raw.githubusercontent.com/o/elements/master/third-party.index";

const index = (url: string, name: string, extra: Partial<ContentIndex> = {}): ContentIndex =>
  ({ url, name, lastChecked: null, fileCount: 0, updatesAvailable: 0, requiresNewerApp: false, verified: false, problem: null, ...extra }) as ContentIndex;
const pack = (id: string, name: string) =>
  ({ id, name, kind: "downloaded", version: "1", rulesets: [], elementCount: 1, enabled: true, license: "", attribution: "", sourceIds: [] }) as unknown as ContentPack;

const state = vi.hoisted(() => ({
  confirm: "confirm" as string,
  asked: [] as { title: string; body: string }[],
  added: [] as string[],
  toasts: [] as string[],
  sources: [] as PackSource[],
  indexes: [] as ContentIndex[],
  packs: [] as ContentPack[],
}));
vi.mock("../../src/components/Confirm", () => ({
  useConfirm: () => async (q: { title: string; body: string }) => { state.asked.push(q); return state.confirm; },
}));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (m: string) => state.toasts.push(m) }) }));
vi.mock("../api/packRemoval", () => ({
  packSources: async () => state.sources,
  removePacks: async () => ({ removed: 0, skipped: [], packs: [] }),
  packsFrom: () => [],
}));
/** Upstream's list, as the server answers it: names come from the server (index_subs.rs). */
vi.mock("../../src/pages/content/IndexesTab", () => ({
  sourceConsentText: (host: string) => `consent ${host}`,
  IndexesTab: () => <ul aria-label="Indexes">{state.indexes.map((i) => <li key={i.url}>{i.name}</li>)}</ul>,
}));
/** One api object, as the app's provider gives one. */
const api = vi.hoisted(() => ({
  listIndexes: async () => state.indexes,
  listPacks: async () => state.packs,
  addIndex: async ({ url }: { url: string }) => {
    state.added.push(url);
    state.indexes = [...state.indexes, { ...state.indexes[0]!, url, name: "Third Party" }];
    return state.indexes;
  },
  subscribe: () => () => undefined,
}));
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api, ApiProvider: ({ children }: { children: unknown }) => children }));

const { IndexesTab, unfollowed } = await import("./WithUnfollowed");
const { resetContentSync } = await import("./contentSync");
const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  resetContentSync();
  Object.assign(state, {
    confirm: "confirm", asked: [], added: [], toasts: [],
    sources: [
      { id: "dl-legacy", indexUrl: LEGACY, fiveToolsUrl: null },
      { id: "dl-third-party", indexUrl: THIRD, fiveToolsUrl: null },
      { id: "fivetools-x", indexUrl: null, fiveToolsUrl: "https://example.com/data" },
    ],
    indexes: [index(LEGACY, "Aurora Legacy")],
    packs: [pack("dl-legacy", "Aurora Legacy"), pack("dl-third-party", "Third Party")],
  });
});

describe("unfollowed", () => {
  it("lists the indexes packs came from that the account does not follow, by pack name", () => {
    expect(unfollowed(state.sources, state.indexes, state.packs)).toEqual([{ url: THIRD, packs: ["Third Party"] }]);
  });

  it("matches addresses with or without a trailing slash and falls back to the pack id", () => {
    const sources = [{ id: "dl-a", indexUrl: `${LEGACY}/`, fiveToolsUrl: null }, { id: "dl-b", indexUrl: THIRD, fiveToolsUrl: null }];
    expect(unfollowed(sources, state.indexes, [])).toEqual([{ url: THIRD, packs: ["dl-b"] }]);
  });

  it("is empty when every index is followed", () => {
    expect(unfollowed(state.sources, [...state.indexes, index(THIRD, "Third Party")], state.packs)).toEqual([]);
  });
});

describe("IndexesTab with Packs without their index", () => {
  it("shows the pack whose index is missing, with Add its index, above the named index list", async () => {
    render(<IndexesTab />);
    await settle();
    const list = screen.getByRole("list", { name: "Packs without their index" });
    expect(within(list).getByText("Third Party")).toBeTruthy();
    expect(within(list).getByText(THIRD)).toBeTruthy();
    expect(within(screen.getByRole("list", { name: "Indexes" })).getByText("Aurora Legacy")).toBeTruthy();
  });

  it("asks first, adds the index and redraws the list", async () => {
    render(<IndexesTab />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Add the index of Third Party" }));
    await settle();
    await settle();
    expect(state.asked).toEqual([{ title: "Add content source", body: "consent raw.githubusercontent.com", confirmLabel: "Add source" }]);
    expect(state.added).toEqual([THIRD]);
    // Both lists are fetched again once the content-change settles (contentSync.ts), with nothing mounted again.
    await waitFor(() => expect(screen.queryByRole("list", { name: "Packs without their index" })).toBeNull());
    const names = within(screen.getByRole("list", { name: "Indexes" })).getAllByRole("listitem").map((li) => li.textContent);
    expect(names).toEqual(["Aurora Legacy", "Third Party"]);
    expect(state.toasts[0]).toMatch(/Press Update/);
  });

  it("adds nothing when the question is cancelled", async () => {
    state.confirm = "cancel";
    render(<IndexesTab />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Add the index of Third Party" }));
    await settle();
    expect(state.added).toEqual([]);
    expect(screen.getByRole("list", { name: "Packs without their index" })).toBeTruthy();
  });

  it("shows no section when nothing is missing", async () => {
    state.indexes = [index(LEGACY, "Aurora Legacy"), index(THIRD, "Third Party")];
    render(<IndexesTab />);
    await settle();
    expect(screen.queryByRole("list", { name: "Packs without their index" })).toBeNull();
  });
});
