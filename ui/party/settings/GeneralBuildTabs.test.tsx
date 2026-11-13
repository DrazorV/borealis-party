import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { followSettings, SKIP_KEY } from "../api/personality";
import { SKIP_TITLE, SkipPersonalityCheck } from "./GeneralBuildTabs";

const deps = vi.hoisted(() => ({
  saveSettings: vi.fn(),
  replace: vi.fn(),
  mutate: vi.fn(),
  getSnapshot: vi.fn(),
  snapshot: { id: "c1" } as unknown,
}));
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => ({ saveSettings: deps.saveSettings, getSnapshot: deps.getSnapshot }) }));
vi.mock("../../src/state/SettingsContext", () => ({ useSettings: () => ({ replace: deps.replace }) }));
vi.mock("../../src/state/CharacterContext", () => ({ useCharacter: () => ({ snapshot: deps.snapshot, mutate: deps.mutate }) }));

afterEach(() => { followSettings({}); vi.clearAllMocks(); document.body.innerHTML = ""; });

const box = () => screen.getByRole("checkbox", { name: SKIP_TITLE });

it("shows the account's choice, saves a change at once and reads the open character again, both ways", async () => {
  deps.saveSettings.mockImplementation(async ({ patch }: { patch: Record<string, unknown> }) => {
    const saved = { accentColor: "blue", ...patch };
    followSettings(saved); // what the API override does with the answer
    return saved;
  });
  deps.mutate.mockImplementation(async (fn: (api: unknown, id: string) => unknown) => fn({ getSnapshot: deps.getSnapshot }, "c1"));
  render(<SkipPersonalityCheck />);
  expect((box() as HTMLInputElement).checked).toBe(true);

  fireEvent.click(box());
  await waitFor(() => expect(deps.saveSettings).toHaveBeenCalledWith({ patch: { [SKIP_KEY]: false } }));
  await waitFor(() => expect((box() as HTMLInputElement).checked).toBe(false));
  expect(deps.replace).toHaveBeenCalledWith({ accentColor: "blue", [SKIP_KEY]: false });
  expect(deps.getSnapshot).toHaveBeenCalledWith({ characterId: "c1" });

  fireEvent.click(box());
  await waitFor(() => expect(deps.saveSettings).toHaveBeenLastCalledWith({ patch: { [SKIP_KEY]: true } }));
  await waitFor(() => expect((box() as HTMLInputElement).checked).toBe(true));
});

it("a refused save puts the box back", async () => {
  deps.saveSettings.mockRejectedValue({ code: "network", message: "The server can't be reached." });
  render(<SkipPersonalityCheck />);
  fireEvent.click(box());
  await waitFor(() => expect(deps.saveSettings).toHaveBeenCalled());
  await waitFor(() => expect((box() as HTMLInputElement).checked).toBe(true));
  expect(deps.mutate).not.toHaveBeenCalled();
});
