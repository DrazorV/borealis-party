import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { combatDeps, resetCombatDeps } from "../api/overrides/combat";
import type { Creature } from "../api/overrides/encounters";
import { saveSession } from "../api/session";
import { mockCombatServer } from "../combat/dm/mockServer";
import type { Precast } from "../combat/dm/spellApi";
import { memoryStorage } from "../test/memoryStorage";
import { PrecastDialog, SpellsSection, type SpellsValue } from "./EncounterSpells";

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

function Section({ onChange }: { onChange: (v: SpellsValue) => void }) {
  return <SpellsSection ruleset="dnd2024" value={{ spells: [], slots: {}, dc: "", attack: "" }} onChange={onChange} />;
}

describe("the Spells section of the override dialog", () => {
  it("adds a real spell with ONE copy per spell per edition, the creature's edition first", async () => {
    const changes: SpellsValue[] = [];
    render(<Section onChange={(v) => changes.push(v)} />);
    fireEvent.click(screen.getByRole("button", { name: "Add spell…" }));
    const d = await screen.findByRole("dialog", { name: "Add a spell" });
    fireEvent.change(within(d).getByLabelText("Search spells"), { target: { value: "fireball" } });
    await waitFor(() => expect(within(d).getAllByRole("button", { name: /^Fireball/ })).toHaveLength(2));
    const [first, second] = within(d).getAllByRole("button", { name: /^Fireball/ });
    expect(first!.textContent).toContain("2024");
    expect(first!.textContent).toContain("also in");
    expect(second!.textContent).toContain("2014");
    fireEvent.click(first!);
    expect(changes.at(-1)!.spells).toEqual([expect.objectContaining({ key: "fireball", ruleset: "2024", level: 3, frequency: "slot" })]);
  });
});

describe("Precast spell", () => {
  const mage = creature("m1", "Mage", { overrides: { spells: [
    { key: "mage_armor", name: "Mage Armor", level: 1, ruleset: "2024", frequency: "slot" },
    { key: "greater_invisibility", name: "Greater Invisibility", level: 4, ruleset: "2024", frequency: "slot", concentration: true },
  ] } });
  const knight = creature("k1", "Knight");

  it("picks caster, spell, slot and targets, and saves them", async () => {
    const saved: unknown[] = [];
    render(<PrecastDialog creatures={[mage, knight]} precasts={[]} onSave={async (f) => { saved.push(f); }} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("radio", { name: "Mage Armor" }));
    expect(within(screen.getByRole("radiogroup", { name: "Slot" })).getAllByRole("radio")).toHaveLength(9);
    fireEvent.click(screen.getByRole("radio", { name: "L2" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Targets" })).getByRole("button", { name: "Mage" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saved).toEqual([{ casterCreatureId: "m1", spellKey: "mage_armor", spellName: "Mage Armor", level: 1, concentration: false, slotLevel: 2, targets: ["m1"] }]));
  });

  it("warns on a second concentration spell for one caster", () => {
    const held: Precast = { id: "p1", casterCreatureId: "m1", spellKey: "fly", spellName: "Fly", level: 3, concentration: true, slotLevel: 3, targets: [] };
    render(<PrecastDialog creatures={[mage, knight]} precasts={[held]} onSave={async () => {}} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("radio", { name: "Greater Invisibility" }));
    expect(screen.getByText(/already concentrates on Fly/)).toBeTruthy();
  });
});
