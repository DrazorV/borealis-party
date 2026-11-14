import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppProviders } from "../../src/AppProviders";
import { MockApi } from "../../src/api/mock";
import { sequenceDie } from "../../src/lib/dice";
import { D20Button } from "../../src/pages/play/rolls/RollButton";
import { setDieForTests } from "../../src/pages/play/rolls/rollStore";
import { useCharacter } from "../../src/state/CharacterContext";
import { seededId } from "../../src/test/utils";
import { memoryStorage } from "../test/memoryStorage";
import { applyDeathSave, resetDeathSaves } from "./DeathSaves";
import { concentrationDc, resetConcentrationChecks } from "./concentration";
import { HIDE_DICE_KEY, resetDicePrefs, setDigitalDiceHidden } from "./dicePrefs";
import { HitPointsCard } from "./HitPointsCard";
import { RestBar } from "./PlayTop";
import { RollHistoryButton } from "./rolls/RollResult";
import { RollProvider } from "./rolls/RollContext";
import { resetShareable } from "./rolls/shareable";
import { ConcentrationBanner } from "./SpellSlotsCard";
import { TRACKER_PRESETS, TrackersCard } from "./TrackersCard";

// The party build points the Play page's roll buttons at the party's RollProvider (vite.config.ts REPLACED); vitest
// runs upstream's config, so do it here for upstream's D20Button.
vi.mock("../../src/pages/play/rolls/RollContext", () => import("./rolls/RollContext"));

/**
 * 0.3.5 player quick wins on the Play tab: typed table rolls first on every d20 button, death saves outside combat,
 * the concentration save after damage, quick counters, and the roll history with typed rolls.
 */

let api: MockApi;
let id: string;

beforeEach(async () => {
  vi.stubGlobal("localStorage", memoryStorage());
  resetDicePrefs();
  resetDeathSaves();
  resetConcentrationChecks();
  resetShareable();
  setDieForTests(sequenceDie([14, 7, 3, 12, 18]));
  api = new MockApi({ aiChunkDelayMs: 1, rng: () => 0.5 });
  api.presetSettings({ showImportNotice: false });
  id = await seededId(api, "Seraphine");
  sessionStorage.setItem("lk.currentCharacter", id);
});

afterEach(() => {
  setDieForTests(null);
  vi.unstubAllGlobals();
});

/** The Play pieces this release changes, on the open character, the way the Play sub-tab stacks them. */
function Sheet() {
  const { snapshot: s } = useCharacter();
  if (!s) return null;
  return (
    <RollProvider characterId={s.id}>
      <RestBar hp={s.play.hitPoints} />
      <ConcentrationBanner concentration={s.play.concentration} />
      <HitPointsCard hp={s.play.hitPoints} />
      <TrackersCard trackers={s.play.trackers} />
      <D20Button what="Stealth check" modifier={5} />
      <RollHistoryButton characterId={s.id} />
    </RollProvider>
  );
}

async function open() {
  render(<AppProviders api={api}><Sheet /></AppProviders>);
  await screen.findByRole("status", { name: /^Hit points: / });
  return userEvent.setup();
}

const hp = () => Number(screen.getByRole("status", { name: /^Hit points: / }).textContent);

async function damage(user: ReturnType<typeof userEvent.setup>, n: number) {
  fireEvent.change(screen.getByRole("spinbutton", { name: "Hit point amount" }), { target: { value: String(n) } });
  await user.click(screen.getByRole("button", { name: "Take damage" }));
}

describe("type my d20", () => {
  it("asks for the table's total first; Send records it as typed", async () => {
    const user = await open();
    await user.click(screen.getByRole("button", { name: /^Roll Stealth check, \+5/ }));
    const sheet = await screen.findByRole("dialog", { name: "Stealth check" });
    expect(within(sheet).getByText("Roll a d20 and add +5, then type the total.")).toBeTruthy();
    const box = within(sheet).getByRole("textbox", { name: "Stealth check: my table roll" });
    await waitFor(() => expect(document.activeElement).toBe(box));
    await user.type(box, "17{Enter}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Stealth check" })).toBeNull());
    const card = await screen.findByTestId("roll-result");
    expect(within(card).getByText("17")).toBeTruthy();
    expect(within(card).getByText("rolled at the table")).toBeTruthy();
  });

  it("Roll for me rolls the app's dice, second", async () => {
    const user = await open();
    await user.click(screen.getByRole("button", { name: /^Roll Stealth check, \+5/ }));
    const sheet = await screen.findByRole("dialog", { name: "Stealth check" });
    await user.click(within(sheet).getByRole("button", { name: /Roll for me \(\+5\)/ }));
    const card = await screen.findByTestId("roll-result");
    expect(within(card).getByText("19")).toBeTruthy();
  });

  it("Hide Roll for me leaves only the box, and the history keeps typed rolls", async () => {
    setDigitalDiceHidden(true);
    expect(localStorage.getItem(HIDE_DICE_KEY)).toBe("true");
    const user = await open();
    await user.click(screen.getByRole("button", { name: /^Roll Stealth check, \+5/ }));
    const sheet = await screen.findByRole("dialog", { name: "Stealth check" });
    expect(within(sheet).queryByRole("button", { name: /Roll for me/ })).toBeNull();
    await user.type(within(sheet).getByRole("textbox"), "9{Enter}");
    await user.click(await screen.findByRole("button", { name: "Recent rolls (1)" }));
    const history = await screen.findByRole("list", { name: "Recent rolls" });
    expect(within(history).getByText("Stealth check")).toBeTruthy();
    // Outside a live combat there is no one to share with: no Share button, a note says when there is.
    expect(within(history).queryByRole("button", { name: /^Share/ })).toBeNull();
    expect(screen.getByText(/During a live combat, Share sends a roll to the DM/)).toBeTruthy();
  });
});

describe("concentration save after damage", () => {
  it("DC is 10 or half the damage", () => {
    expect(concentrationDc(1)).toBe(10);
    expect(concentrationDc(21)).toBe(10);
    expect(concentrationDc(22)).toBe(11);
    expect(concentrationDc(40)).toBe(20);
  });

  async function concentrating() {
    const s = await api.getSnapshot({ characterId: id });
    const spell = s.play.concentrationSpells[0];
    if (!spell) throw new Error("Seraphine knows no concentration spell");
    await api.setConcentration({ characterId: id, spellId: spell.id });
    return spell;
  }

  it("damage asks for the save; Failed ends concentration", async () => {
    const spell = await concentrating();
    const user = await open();
    await screen.findByText(spell.name);
    await damage(user, 4);
    const prompt = await screen.findByRole("alertdialog", { name: "Concentration save, DC 10" });
    expect(within(prompt).getByText("(4 damage)")).toBeTruthy();
    expect(within(prompt).queryByRole("textbox")).toBeNull();
    await user.click(within(prompt).getByRole("button", { name: "Failed" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Concentration" })).toBeNull());
    expect((await api.getSnapshot({ characterId: id })).play.concentration).toBeNull();
  });

  it("Saved keeps it", async () => {
    await concentrating();
    const user = await open();
    await damage(user, 2);
    const prompt = await screen.findByRole("alertdialog", { name: "Concentration save, DC 10" });
    await user.click(within(prompt).getByRole("button", { name: "Saved" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect((await api.getSnapshot({ characterId: id })).play.concentration).not.toBeNull();
  });

  it("0 hit points ends it without a save", async () => {
    await concentrating();
    const user = await open();
    await damage(user, hp());
    await waitFor(async () => expect((await api.getSnapshot({ characterId: id })).play.concentration).toBeNull());
    expect(screen.queryByRole("alertdialog", { name: /Concentration save/ })).toBeNull();
  });
});

describe("death saves outside combat", () => {
  it("follows the rules: 10+ success, 9- failure, a 1 two failures, a 20 back up", () => {
    const none = { successes: 0, failures: 0 };
    expect(applyDeathSave(none, 10)).toEqual({ saves: { successes: 1, failures: 0 }, revived: false });
    expect(applyDeathSave(none, 9)).toEqual({ saves: { successes: 0, failures: 1 }, revived: false });
    expect(applyDeathSave(none, 1)).toEqual({ saves: { successes: 0, failures: 2 }, revived: false });
    expect(applyDeathSave({ successes: 2, failures: 2 }, 20)).toEqual({ saves: none, revived: true });
  });

  it("appears at 0 hit points, counts outcome buttons to stable, and goes once healed", async () => {
    const user = await open();
    expect(screen.queryByRole("region", { name: "Death saves" })).toBeNull();
    await damage(user, hp());
    const card = await screen.findByRole("region", { name: "Death saves" });
    expect(within(card).getByText("Dying")).toBeTruthy();
    expect(within(card).queryByRole("textbox")).toBeNull();
    for (const outcome of ["Success", "Success", "Success"]) {
      await user.click(within(card).getByRole("button", { name: outcome }));
    }
    await waitFor(() => expect(within(card).getByText("Stable")).toBeTruthy());
    expect(within(card).getByRole("img", { name: "Successes: 3 of 3" })).toBeTruthy();
    expect(within(card).queryByRole("textbox")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Regain 1 hit point" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Death saves" })).toBeNull());
  });

  it("Natural 20 brings them back with 1 hit point; damage at 0 is a failure", async () => {
    const user = await open();
    await damage(user, hp());
    const card = await screen.findByRole("region", { name: "Death saves" });
    await user.click(within(card).getByRole("button", { name: "Critical hit" }));
    expect(within(card).getByRole("img", { name: "Failures: 2 of 3" })).toBeTruthy();
    await user.click(within(card).getByRole("button", { name: "Natural 20" }));
    await waitFor(() => expect(hp()).toBe(1));
    expect(screen.queryByRole("region", { name: "Death saves" })).toBeNull();
  });
});

describe("quick counters", () => {
  it("one tap fills a counter; Save keeps it on the character", async () => {
    const user = await open();
    const quick = await screen.findByRole("group", { name: "Quick counters" });
    expect(within(quick).getAllByRole("button").map((b) => b.textContent)).toEqual(TRACKER_PRESETS.map((p) => p.name));
    await user.click(within(quick).getByRole("button", { name: "Luck" }));
    const dialog = await screen.findByRole("dialog", { name: "Add tracker" });
    expect((within(dialog).getByRole("textbox", { name: "Name" }) as HTMLInputElement).value).toBe("Luck");
    expect((within(dialog).getByRole("spinbutton", { name: "Maximum" }) as HTMLInputElement).value).toBe("3");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(async () => expect((await api.getSnapshot({ characterId: id })).play.trackers.map((t) => [t.name, t.max, t.reset])).toEqual([["Luck", 3, "long_rest"]]));
    // The Add dialog offers the ones not added yet.
    await user.click(screen.getByRole("button", { name: "Add tracker" }));
    const again = await screen.findByRole("dialog", { name: "Add tracker" });
    expect(within(within(again).getByRole("group", { name: "Quick counters" })).queryByRole("button", { name: "Luck" })).toBeNull();
  });
});
