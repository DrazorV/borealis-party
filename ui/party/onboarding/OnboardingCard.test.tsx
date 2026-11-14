import { act, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The card when the server is slow (0.3.6 release run): a new account's first content work held `list_packs` for over
 * 30 s, and the card waited for every fact, so it never showed. Now it shows after PATIENCE with what it knows.
 */

const never = <T,>() => new Promise<T>(() => undefined);
let packs: () => Promise<unknown[]>;

const api = {
  listPacks: () => packs(),
  listIndexes: async () => [],
  listCharacters: async () => [{ id: "c1" }],
};
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api }));
vi.mock("../../src/state/SettingsContext", () => ({ useSettings: () => ({ settings: { onboardingDismissed: false }, loaded: true, update: async () => undefined }) }));
vi.mock("../api/connectivity", () => ({ isOffline: () => false }));
vi.mock("../api/dmRole", () => ({ useDmRole: () => false }));
vi.mock("../api/session", () => ({ loadSession: () => ({ user: { id: "u1" } }) }));
vi.mock("../api/overrides/parties", () => ({ listParties: async () => ({ dm: false, parties: [] }), onPartiesChanged: () => () => undefined }));
vi.mock("../api/overrides/encounters", () => ({ listEncounters: async () => null }));
vi.mock("../pages/AiApps", () => ({ listTokens: async () => [], listApps: async () => [] }));

const { OnboardingCard, PATIENCE } = await import("./OnboardingCard");

const draw = () => render(<MemoryRouter><OnboardingCard /></MemoryRouter>);
const flush = () => act(async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); });

describe("OnboardingCard", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("shows at once when every fact is read", async () => {
    packs = async () => [{ kind: "bundled" }];
    draw();
    await flush();
    expect(screen.getByRole("region", { name: "Get started" })).toBeTruthy();
    expect(screen.getByText("1 of 3 done")).toBeTruthy();
  });

  it("a fact the server doesn't answer doesn't hold the card: it shows after a while, that step not done", async () => {
    packs = never;
    draw();
    await flush();
    expect(screen.queryByRole("region", { name: "Get started" })).toBeNull();
    await act(async () => { vi.advanceTimersByTime(PATIENCE); });
    await flush();
    const card = screen.getByRole("region", { name: "Get started" });
    expect(card.querySelector('[data-step="character"]')!.getAttribute("data-done")).toBe("true");
    expect(card.querySelector('[data-step="content"]')!.getAttribute("data-done")).toBe("false");
  });
});
