import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  checkedAt: null as string | null,
  snapshot: null as null | { name: string },
  listeners: [] as ((p: { phase: string }) => void)[],
  asked: 0,
}));
const api = vi.hoisted(() => ({
  contentUpdateStatus: async () => { state.asked++; return { state: "done", message: "", updated: 0, failed: [], checkedAt: state.checkedAt }; },
  subscribe: (_: string, cb: (p: { phase: string }) => void) => { state.listeners.push(cb); return () => undefined; },
}));
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api }));
vi.mock("../../src/state/CharacterContext", () => ({ useCharacter: () => ({ snapshot: state.snapshot }) }));

const { lastCheckedLine, setStatus, useStatusMessage } = await import("./status");
const settle = () => act(() => new Promise((r) => setTimeout(r, 0)));

function Bar() {
  const message = useStatusMessage();
  return <p role="status">{message || "UPSTREAM IDLE LINE"}</p>;
}

beforeEach(() => {
  Object.assign(state, { checkedAt: null, snapshot: null, listeners: [], asked: 0 });
  setStatus("");
});

describe("the status bar's last content check (0.3.3)", () => {
  it("words the time, with the day when it was not today", () => {
    const now = new Date(2026, 10, 14, 15, 0);
    expect(lastCheckedLine(new Date(2026, 10, 14, 9, 5).toISOString(), now)).toMatch(/^Last checked for content updates at 0?9:05$/);
    const older = lastCheckedLine(new Date(2026, 10, 12, 9, 5).toISOString(), now)!;
    // The day in the browser's own words and language ("12 Nov", "Nov 12", …), then the time.
    expect(older).toMatch(/^Last checked for content updates at .*12.* 0?9:05$/);
    expect(lastCheckedLine(null, now)).toBeNull();
    expect(lastCheckedLine("not a date", now)).toBeNull();
  });

  it("shows the account's real last check, follows a new one, and keeps messages and a loaded character first", async () => {
    state.checkedAt = new Date().toISOString();
    render(<Bar />);
    await settle();
    expect(screen.getByRole("status").textContent).toMatch(/^Last checked for content updates at \d\d:\d\d$/);

    // A check ends: asked again.
    state.checkedAt = null;
    await act(async () => { state.listeners.forEach((l) => l({ phase: "done" })); });
    await settle();
    expect(state.asked).toBe(2);
    expect(screen.getByRole("status").textContent).toBe("UPSTREAM IDLE LINE");

    await act(async () => { setStatus("Saved."); });
    expect(screen.getByRole("status").textContent).toBe("Saved.");
  });

  it("leaves a loaded character's line to upstream", async () => {
    state.checkedAt = new Date().toISOString();
    state.snapshot = { name: "Ilsa" };
    render(<Bar />);
    await settle();
    expect(screen.getByRole("status").textContent).toBe("UPSTREAM IDLE LINE");
  });
});
