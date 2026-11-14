import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppProviders } from "../../src/AppProviders";
import type { CharacterSnapshot } from "../../src/api/contract";
import { MockApi } from "../../src/api/mock";
import { sequenceDie } from "../../src/lib/dice";
import { setDieForTests } from "../../src/pages/play/rolls/rollStore";
import { useCharacter } from "../../src/state/CharacterContext";
import { seededId } from "../../src/test/utils";
import type { PlayerCombat, PlayerCombatant } from "../combat/player/api";
import { memoryStorage } from "../test/memoryStorage";
import { companionAttacks } from "./companionAttacks";
import { companionCards, damageLocal, healLocal, localCompanion, tempLocal } from "./Companions";
import { HitPointsCard } from "./HitPointsWithCompanions";
import { RollProvider } from "./rolls/RollContext";

/**
 * The Play tab's Companions card (0.3.6): a card per companion under Hit Points, with AC, hit points and Damage / Heal /
 * Temp, conditions and its attacks as typed-first roll buttons. Outside combat everything stays on this device.
 */

let api: MockApi;
let id: string;

beforeEach(async () => {
  vi.stubGlobal("localStorage", memoryStorage());
  setDieForTests(sequenceDie([14, 5, 3, 2]));
  api = new MockApi({ aiChunkDelayMs: 1, rng: () => 0.5 });
  api.presetSettings({ showImportNotice: false });
  id = await seededId(api, "Seraphine");
  sessionStorage.setItem("lk.currentCharacter", id);
});

afterEach(() => {
  setDieForTests(null);
  vi.unstubAllGlobals();
});

function Sheet() {
  const { snapshot: s } = useCharacter();
  if (!s) return null;
  return <RollProvider characterId={s.id}><HitPointsCard hp={s.play.hitPoints} /></RollProvider>;
}

async function open() {
  render(<AppProviders api={api}><Sheet /></AppProviders>);
  await screen.findByRole("status", { name: /^Hit points: / });
  return userEvent.setup();
}

describe("attacks from the stat block", () => {
  it("reads 2014 and 2024 attack lines, and skips actions with no attack roll", () => {
    const attacks = companionAttacks([
      { name: "Multiattack", text: "The beast makes two attacks." },
      { name: "Bite", text: "Melee Weapon Attack: +4 to hit, reach 5 ft., one target. Hit: 7 (2d4 + 2) piercing damage." },
      { name: "Claw.", text: "Melee Attack Roll: +5, reach 5 ft. Hit: 9 (1d10 + 4) Slashing damage." },
      { name: "Talons", text: "Melee Weapon Attack: +3 to hit, reach 5 ft., one target. Hit: 1 slashing damage." },
    ]);
    expect(attacks.map((a) => [a.name, a.toHit, a.damageText])).toEqual([
      ["Bite", 4, "7 (2d4 + 2) piercing"],
      ["Claw", 5, "9 (1d10 + 4) slashing"],
      ["Talons", 3, "1 slashing"],
    ]);
    expect(attacks[0]!.damage?.terms).toEqual([{ kind: "dice", count: 2, sides: 4, sign: 1 }, { kind: "flat", value: 2 }]);
    expect(attacks[2]!.damage?.terms).toEqual([{ kind: "flat", value: 1 }]);
  });

  it("takes damage through temporary hit points first, never below 0", () => {
    const v = { damage: 0, temp: 3, conditions: [] };
    expect(damageLocal(v, 11, 5)).toEqual({ damage: 2, temp: 0, conditions: [] });
    expect(damageLocal(v, 11, 50)).toEqual({ damage: 11, temp: 0, conditions: [] });
    expect(healLocal({ damage: 4, temp: 0, conditions: [] }, 10).damage).toBe(0);
    expect(tempLocal({ damage: 0, temp: 5, conditions: [] }, 3).temp).toBe(5);
  });
});

describe("the card", () => {
  it("shows nothing without companions", async () => {
    await open();
    expect(screen.queryByRole("list", { name: "Companions" })).toBeNull();
  });

  it("shows a companion's AC and HP, takes damage, heal, temp and conditions on this device", async () => {
    await api.addCompanion({ characterId: id, creatureId: "ID_SRD51_MONSTER_WOLF", name: "Grip" });
    const user = await open();
    const card = within(await screen.findByRole("listitem", { name: "Grip" }));
    const hp = () => card.getByRole("status", { name: /^Grip hit points/ }).getAttribute("aria-label");
    expect(hp()).toBe("Grip hit points: 11 of 11");
    expect(card.getByText("AC 13")).toBeTruthy();
    fireEvent.change(card.getByRole("spinbutton", { name: "Grip amount" }), { target: { value: "4" } });
    await user.click(card.getByRole("button", { name: "Damage Grip" }));
    expect(hp()).toBe("Grip hit points: 7 of 11");
    fireEvent.change(card.getByRole("spinbutton", { name: "Grip amount" }), { target: { value: "3" } });
    await user.click(card.getByRole("button", { name: "Temporary hit points for Grip" }));
    expect(hp()).toBe("Grip hit points: 7 of 11, 3 temporary");
    fireEvent.change(card.getByRole("spinbutton", { name: "Grip amount" }), { target: { value: "2" } });
    await user.click(card.getByRole("button", { name: "Heal Grip" }));
    expect(hp()).toBe("Grip hit points: 9 of 11, 3 temporary");
    fireEvent.change(card.getByRole("combobox", { name: "Add a condition to Grip" }), { target: { value: "prone" } });
    expect(card.getByRole("list", { name: "Grip conditions" }).textContent).toContain("Prone");
    // Kept on this device, per character and companion.
    const companion = (await api.getSnapshot({ characterId: id }) as CharacterSnapshot).companions[0]!;
    expect(localCompanion(id, companion.id)).toEqual({ damage: 2, temp: 3, conditions: ["prone"] });
    expect(JSON.parse(localStorage.getItem(`party.companion.${id}.${companion.id}`)!)).toEqual({ damage: 2, temp: 3, conditions: ["prone"] });
    await user.click(card.getByRole("button", { name: "Remove prone" }));
    await user.click(card.getByRole("button", { name: "Grip: full hit points" }));
    expect(hp()).toBe("Grip hit points: 11 of 11");
    expect(localStorage.getItem(`party.companion.${id}.${companion.id}`)).toBeNull();
  });

  it("rolls an attack table-first, then its damage", async () => {
    await api.addCompanion({ characterId: id, creatureId: "ID_SRD51_MONSTER_WOLF", name: "Grip" });
    const user = await open();
    const card = within(await screen.findByRole("listitem", { name: "Grip" }));
    const attacks = card.getByRole("group", { name: "Grip attacks" });
    await user.click(within(attacks).getByRole("button", { name: "Grip: Bite, +4 to hit" }));
    const sheet = await screen.findByRole("dialog", { name: "Grip · Bite" });
    expect(within(sheet).getByRole("button", { name: /Roll for me \(\+4\)/ })).toBeTruthy();
    await user.type(within(sheet).getByRole("textbox", { name: "Grip · Bite: my table roll" }), "17{Enter}");
    // The result offers its damage, asked the same way.
    const card2 = await screen.findByTestId("roll-result");
    expect(within(card2).getByText("17")).toBeTruthy();
    await user.click(within(card2).getByRole("button", { name: "Damage" }));
    const damage = await screen.findByRole("dialog", { name: "Grip · Bite damage" });
    expect(within(damage).getByRole("button", { name: /Roll for me/ })).toBeTruthy();
  });

  it("lists several companions side by side in one grid", async () => {
    await api.addCompanion({ characterId: id, creatureId: "ID_SRD51_MONSTER_WOLF", name: "Grip" });
    await api.addCompanion({ characterId: id, creatureId: "ID_SRD51_MONSTER_OWL", name: "Hoot" });
    await open();
    const list = await screen.findByRole("list", { name: "Companions" });
    expect(list.className).toContain("bp-companions");
    expect(within(list).getAllByRole("listitem", { name: /^(Grip|Hoot)$/ }).map((li) => li.getAttribute("aria-label"))).toEqual(["Grip", "Hoot"]);
  });
});

describe("in a live combat", () => {
  const me: PlayerCombatant = { id: "pc1", kind: "character", name: "Ranger", mine: true, initiative: 14, conditions: [], inOrder: true };
  const creature = (cid: string, name: string, ownerId = "pc1"): PlayerCombatant => ({
    id: cid, kind: "creature", name, mine: true, initiative: null, conditions: [], inOrder: true, ownerId, hp: { current: 5, max: 11 }, ac: 13,
  });
  const combat = (combatants: PlayerCombatant[]) => ({ combatants } as unknown as PlayerCombat);
  const sheet = (names: string[]) => ({ companions: names.map((n, i) => ({ id: `companion-${i}`, name: n, actions: [] })) } as unknown as CharacterSnapshot);

  it("uses the fight's card for a companion in it, keeps the others local, and adds my summons", () => {
    const c = combat([me, creature("x1", "Grip"), creature("x2", "Spirit"), creature("x3", "Their Wolf", "pc2")]);
    const { sheet: rows, summons } = companionCards(sheet(["Grip", "Hoot"]), c, me);
    expect(rows.map((r) => [r.k.name, r.live?.id ?? null])).toEqual([["Grip", "x1"], ["Hoot", null]]);
    expect(summons.map((x) => x.name)).toEqual(["Spirit"]);
  });

  it("outside combat, every sheet companion is local and there are no summons", () => {
    const { sheet: rows, summons } = companionCards(sheet(["Grip"]), null, null);
    expect(rows[0]!.live).toBeNull();
    expect(summons).toEqual([]);
  });
});
