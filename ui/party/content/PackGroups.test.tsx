import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ContentPack } from "../../src/api/contract";

const state = vi.hoisted(() => ({
  confirm: "confirm" as string,
  asked: [] as { title: string; body: string; confirmLabel: string; danger?: boolean }[],
  removed: [] as string[][],
  toasts: [] as string[],
  refuse: null as string | null,
  /** The server's answer waits for this. */
  hold: null as Promise<void> | null,
}));
vi.mock("../../src/components/Confirm", () => ({
  useConfirm: () => async (q: { title: string; body: string; confirmLabel: string; danger?: boolean }) => { state.asked.push(q); return state.confirm; },
}));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (m: string) => state.toasts.push(m) }) }));
vi.mock("../api/packRemoval", () => ({
  removePacks: async (ids: string[]) => {
    state.removed.push(ids);
    if (state.hold) await state.hold;
    if (state.refuse) throw { code: "internal", message: state.refuse };
    return { removed: ids.length, skipped: [], packs: [] };
  },
}));

const { PackGroups } = await import("./PackGroups");
const { contentSync, resetContentSync } = await import("./contentSync");

const pack = (id: string, kind: ContentPack["kind"], redistributable?: boolean) =>
  ({ id, name: id.toUpperCase(), kind, redistributable, version: "1", rulesets: [], elementCount: 1, enabled: true, license: "", attribution: "", sourceIds: [] }) as ContentPack;
const PACKS = [
  pack("core", "bundled"),
  ...Array.from({ length: 109 }, (_, i) => pack(`fivetools-${i}`, "local-import", false)),
  pack("index-a", "downloaded"), pack("index-b", "downloaded"),
];
const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => { resetContentSync(); Object.assign(state, { confirm: "confirm", asked: [], removed: [], toasts: [], refuse: null, hold: null }); });

describe("PackGroups with Remove all", () => {
  it("puts Remove all on every group but the built-in one", () => {
    render(<PackGroups packs={PACKS} row={(p) => <li key={p.id}>{p.name}</li>} />);
    expect(screen.getByRole("button", { name: "Remove all 5etools-format packs" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove all Aurora index packs" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Remove all Built-in/ })).toBeNull();
    expect(screen.getByRole("button", { name: "5etools-format (109)" })).toBeTruthy();
  });

  it("asks with the exact count and removes the group in one request", async () => {
    render(<PackGroups packs={PACKS} row={(p) => <li key={p.id}>{p.name}</li>} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove all 5etools-format packs" }));
    await settle();
    expect(state.asked).toHaveLength(1);
    expect(state.asked[0]!.body).toMatch(/^Remove all 109 5etools-format packs\?/);
    expect(state.asked[0]!.body).toMatch(/show it as missing content/);
    expect(state.asked[0]!).toMatchObject({ confirmLabel: "Remove 109", danger: true });
    expect(state.removed).toHaveLength(1);
    expect(state.removed[0]).toHaveLength(109);
    expect(state.removed[0]).not.toContain("core");
    expect(contentSync.get().activity.map((a) => a.message)).toEqual(["Removed 109 packs."]);
  });

  it("hides the group's packs at once, as Removing…, before the server answers", async () => {
    let answer = () => {};
    state.hold = new Promise((r) => { answer = r; });
    render(<PackGroups packs={PACKS} row={(p) => <li key={p.id}>{p.name}</li>} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove all Aurora index packs" }));
    await Promise.resolve();
    await Promise.resolve();
    expect(contentSync.get().activity).toMatchObject([{ area: "packs", state: "working", label: "Removing 2 Aurora index packs…" }]);
    expect(contentSync.packs(PACKS).map((p) => p.id)).not.toContain("index-a");
    expect(contentSync.packs(PACKS)).toHaveLength(PACKS.length - 2);
    answer();
    await settle();
    expect(contentSync.get().activity).toMatchObject([{ state: "done", message: "Removed 2 packs." }]);
    expect(contentSync.packs(PACKS)).toHaveLength(PACKS.length - 2);
  });

  it("brings the packs back with the server's message when it refuses", async () => {
    state.refuse = "The disk is full.";
    render(<PackGroups packs={PACKS} row={(p) => <li key={p.id}>{p.name}</li>} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove all Aurora index packs" }));
    await settle();
    expect(contentSync.packs(PACKS)).toHaveLength(PACKS.length);
    expect(contentSync.get().activity).toMatchObject([{ state: "failed", message: "The 2 Aurora index packs weren't removed: The disk is full." }]);
  });

  it("removes nothing when the question is cancelled", async () => {
    state.confirm = "cancel";
    render(<PackGroups packs={PACKS} row={(p) => <li key={p.id}>{p.name}</li>} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove all Aurora index packs" }));
    await settle();
    expect(state.asked[0]!.body).toMatch(/^Remove all 2 Aurora index packs\?/);
    expect(state.removed).toEqual([]);
  });

  it("keeps each group folded with its own packs", () => {
    render(<PackGroups packs={PACKS} row={(p) => <li key={p.id}>{p.name}</li>} />);
    const toggle = screen.getByRole("button", { name: "Aurora index (2)" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(within(screen.getByRole("list", { name: "Aurora index packs" })).getAllByRole("listitem")).toHaveLength(2);
  });
});
