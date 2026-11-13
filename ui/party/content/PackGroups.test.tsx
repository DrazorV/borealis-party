import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ContentPack } from "../../src/api/contract";

const state = vi.hoisted(() => ({
  confirm: "confirm" as string,
  asked: [] as { title: string; body: string; confirmLabel: string; danger?: boolean }[],
  removed: [] as string[][],
  toasts: [] as string[],
}));
vi.mock("../../src/components/Confirm", () => ({
  useConfirm: () => async (q: { title: string; body: string; confirmLabel: string; danger?: boolean }) => { state.asked.push(q); return state.confirm; },
}));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (m: string) => state.toasts.push(m) }) }));
vi.mock("../api/packRemoval", () => ({
  removePacks: async (ids: string[]) => { state.removed.push(ids); return { removed: ids.length, skipped: [], packs: [] }; },
}));

const { PackGroups } = await import("./PackGroups");

const pack = (id: string, kind: ContentPack["kind"], redistributable?: boolean) =>
  ({ id, name: id.toUpperCase(), kind, redistributable, version: "1", rulesets: [], elementCount: 1, enabled: true, license: "", attribution: "", sourceIds: [] }) as ContentPack;
const PACKS = [
  pack("core", "bundled"),
  ...Array.from({ length: 109 }, (_, i) => pack(`fivetools-${i}`, "local-import", false)),
  pack("index-a", "downloaded"), pack("index-b", "downloaded"),
];
const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => { Object.assign(state, { confirm: "confirm", asked: [], removed: [], toasts: [] }); });

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
    expect(state.toasts).toEqual(["Removed 109 packs."]);
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
