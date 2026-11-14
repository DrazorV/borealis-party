import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { combatDeps, resetCombatDeps } from "../api/overrides/combat";
import type { Creature, Precast, PrecastFields } from "../api/overrides/encounters";
import { saveSession } from "../api/session";
import { mockCombatServer } from "../combat/dm/mockServer";
import { memoryStorage } from "../test/memoryStorage";
import { overrideSpells, PrecastDialog, SpellsSection, type SpellsValue } from "./EncounterSpells";

/** 0.3.2 in the encounter builder: the override dialog's Spells section and [Precast spell]. SRD names only. */

const creature = (id: string, label: string, extra: Partial<Creature> = {}): Creature => ({
  id, sort: 0, kind: "monster", dmLabel: label, publicName: label, hidden: true, revealedAt: null, ownerId: null, initMode: "after_owner",
  elementId: `ID_${id}`, packId: "srd", packRev: "1", name: label, source: "SRD 5.2.1", ruleset: "dnd2024", mapsTo: null, cr: "6", type: "humanoid",
  size: ["M"], hp: { average: 40 }, hpMode: "encounter", hpMax: null, ac: 12, printed: { hp: null, ac: null, saves: { str: null, dex: null, con: null, int: null, wis: null, cha: null } },
  overrides: {}, modified: false, statBlock: {} as Creature["statBlock"], ...extra,
});

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "dm1", username: "dana", email: null, admin: false, mustChangePassword: false } });
  Object.assign(combatDeps, mockCombatServer().deps);
});
afterEach(() => { resetCombatDeps(); vi.unstubAllGlobals(); });

describe("the Spells section of the override dialog", () => {
  it("adds a real spell, ONE copy per spell per edition, the creature's edition first, at its slot level", async () => {
    const changes: SpellsValue[] = [];
    render(<SpellsSection ruleset="dnd2024" value={{ spells: [], slots: { "3": "2" }, dc: "", attack: "" }} onChange={(v) => changes.push(v)} />);
    fireEvent.click(screen.getByRole("button", { name: "Add spell…" }));
    const d = await screen.findByRole("dialog", { name: "Add a spell" });
    fireEvent.change(within(d).getByLabelText("Search spells"), { target: { value: "fireball" } });
    await waitFor(() => expect(within(d).getAllByRole("button", { name: /^Fireball/ })).toHaveLength(2));
    const [first, second] = within(d).getAllByRole("button", { name: /^Fireball/ });
    expect(first!.textContent).toContain("2024");
    expect(first!.textContent).toContain("also in a 5etools copy");
    expect(second!.textContent).toContain("2014");
    fireEvent.click(first!);
    const listed = changes.at(-1)!.spells;
    expect(overrideSpells(listed)).toEqual([{ id: "ID_SRD_SPELL_FIREBALL", name: "Fireball", frequency: { kind: "slots", level: 3, slots: 2 } }]);
  });
});

describe("Precast spell", () => {
  const mage = creature("m1", "Mage", { overrides: { spells: { spells: [
    { id: "ID_SRD_SPELL_MAGE_ARMOR", name: "Mage Armor", frequency: { kind: "slots", level: 1, slots: 4 } },
    { id: "ID_SRD_SPELL_GREATER_INVISIBILITY", name: "Greater Invisibility", frequency: { kind: "slots", level: 4, slots: 3 } },
  ] } } });
  const knight = creature("k1", "Knight");

  it("picks caster, spell, slot and targets, and saves them", async () => {
    const saved: PrecastFields[] = [];
    render(<PrecastDialog creatures={[mage, knight]} precasts={[]} onSave={async (f) => { saved.push(f); }} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("radio", { name: "Mage Armor" }));
    expect(within(screen.getByRole("radiogroup", { name: "Slot" })).getAllByRole("radio")).toHaveLength(9);
    fireEvent.click(screen.getByRole("radio", { name: "L2" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Targets" })).getByRole("button", { name: "Mage" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saved).toEqual([{ casterId: "m1", spellKey: "ID_SRD_SPELL_MAGE_ARMOR", slotLevel: 2, targets: ["m1"] }]));
  });

  it("with no target it is cast on the caster itself", async () => {
    const saved: PrecastFields[] = [];
    render(<PrecastDialog creatures={[mage, knight]} precasts={[]} onSave={async (f) => { saved.push(f); }} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("radio", { name: "Mage Armor" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saved[0]).toMatchObject({ slotLevel: 1, targets: ["self"] }));
  });

  it("warns on a second concentration spell for one caster", async () => {
    const held: Precast = { id: "p1", casterId: "m1", sort: 0, spellKey: "ID_SRD_SPELL_FLY", name: "Fly", slotLevel: 3, targets: ["self"], choices: {} };
    render(<PrecastDialog creatures={[mage, knight]} precasts={[held]} onSave={async () => {}} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("radio", { name: "Greater Invisibility" }));
    expect(await screen.findByText(/already concentrates on Fly/)).toBeTruthy();
  });
});
