import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppProviders } from "../../src/AppProviders";
import { MockApi } from "../../src/api/mock";
import { D20Button } from "../../src/pages/play/rolls/RollButton";
import { setDieForTests } from "../../src/pages/play/rolls/rollStore";
import { useCharacter } from "../../src/state/CharacterContext";
import { seededId } from "../../src/test/utils";
import { saveSession } from "../api/session";
import { combatPlayerDeps, resetCombatPlayer } from "../combat/player/api";
import * as store from "../combat/player/store";
import { fakeCombatServer, mine, pc, projection, type FakeCombatServer } from "../test/combatServer";
import { memoryStorage } from "../test/memoryStorage";
import { RollHistoryButton } from "./rolls/RollResult";
import { RollProvider } from "./rolls/RollContext";
import { resetShareable } from "./rolls/shareable";

vi.mock("../../src/pages/play/rolls/RollContext", () => import("./rolls/RollContext"));

/**
 * Share a roll with the DM (0.3.5): during a live combat of the open character's party, a roll in the history goes to
 * the combat log as the player's own `roll` (its label and total). Nothing else is sent.
 */

let server: FakeCombatServer;
let hero: string;
let stop: () => void = () => {};

beforeEach(async () => {
  vi.stubGlobal("localStorage", memoryStorage());
  resetShareable();
  setDieForTests(null);
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "pl1", username: "alice", email: null, admin: false, mustChangePassword: false } });
  const api = new MockApi({ aiChunkDelayMs: 1, rng: () => 0.5 });
  api.presetSettings({ showImportNotice: false });
  hero = await seededId(api, "Seraphine");
  sessionStorage.setItem("lk.currentCharacter", hero);
  server = fakeCombatServer([{ id: "p1", name: "The Company", myCharacters: [hero] }]);
  server.combats.set("p1", projection("p1", [pc("me1", "Seraphine", true, { initiative: 12 })], [mine("me1", hero)]));
  combatPlayerDeps.fetch = server.fetch;
  combatPlayerDeps.events = server.listen;
  store.writeDeps.sleep = async () => {};
  render(<AppProviders api={api}><Sheet /></AppProviders>);
  await act(async () => {
    stop = store.startCombatWatch();
    await store.combatWatchReady();
  });
});

afterEach(() => {
  stop();
  store.resetCombatStore();
  resetCombatPlayer();
  vi.unstubAllGlobals();
});

function Sheet() {
  const { snapshot: s } = useCharacter();
  if (!s) return null;
  return (
    <RollProvider characterId={s.id}>
      <D20Button what="Perception check" modifier={3} />
      <RollHistoryButton characterId={s.id} />
    </RollProvider>
  );
}

describe("share a roll with the DM", () => {
  it("sends the roll's label and total as the player's own roll, once", async () => {
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /^Roll Perception check, \+3/ }));
    await user.type(within(await screen.findByRole("dialog", { name: "Perception check" })).getByRole("textbox"), "16{Enter}");
    await user.click(await screen.findByRole("button", { name: "Recent rolls (1)" }));
    const history = await screen.findByRole("list", { name: "Recent rolls" });
    await user.click(within(history).getByRole("button", { name: "Share Perception check 16 with the DM" }));
    await waitFor(() => expect(within(history).getByText("Shared")).toBeTruthy());
    const writes = server.calls.filter((c) => c.method === "POST");
    expect(writes).toHaveLength(1);
    expect(writes[0]!.path).toMatch(/\/roll$/);
    const args = (writes[0]!.body as { args: Record<string, unknown> }).args;
    expect(args).toEqual({ id: "me1", label: "Perception check", roll: { d20: { kind: "typed", total: 16 } } });
    expect(within(history).queryByRole("button", { name: /^Share/ })).toBeNull();
  });
});
