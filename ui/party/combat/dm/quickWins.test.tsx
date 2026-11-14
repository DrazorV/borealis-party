import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AddMemberDialog } from "./AddMemberDialog";
import { byId } from "./derive";
import { sampleCombat } from "./fixtures";
import { execute } from "./mockEngine";
import { MultiDamageSheet, multiDamageCommand } from "./MultiDamageSheet";
import type { Command } from "./types";

/** 0.3.5 combat quick wins on the DM's side: multi-target damage (D13) and a late-joining party member (A2). */

describe("multi-target damage (D13)", () => {
  it("builds one command: every target needs its save when there is one, a typed total or dice", () => {
    const base = { amount: "28", type: "fire" as const, magical: false, save: true, ability: "dex" as const, dc: "15", half: true, picked: ["a", "b"] };
    expect(multiDamageCommand({ ...base, saves: { a: { kind: "typed", text: "12" } } }), "b has no save yet").toBeNull();
    expect(multiDamageCommand({ ...base, dc: "", saves: { a: { kind: "saved" }, b: { kind: "app" } } }), "no DC").toBeNull();
    expect(multiDamageCommand({ ...base, saves: { a: { kind: "typed", text: "12" }, b: { kind: "app" } } })).toEqual({
      command: "multi_damage", amount: { kind: "typed", total: 28 }, damage_type: "fire", save: { ability: "dex", dc: 15, half: true },
      targets: [{ id: "a", save: { kind: "typed", total: 12 } }, { id: "b", save: { kind: "app" } }],
    });
    expect(multiDamageCommand({ ...base, amount: "8d6", save: false, type: null, saves: {} })).toEqual({
      command: "multi_damage", amount: { kind: "app", notation: "8d6" }, targets: [{ id: "a" }, { id: "b" }],
    });
  });

  it("the mock engine: one roll, full on a fail, half on a save, and a PC is asked (D7)", () => {
    const m = sampleCombat();
    const s = m.view.state;
    const cultist = byId(s, "cultist1")!.hp.current;
    const priest = byId(s, "priest")!.hp.current;
    const pc = s.combatants.find((c) => c.kind === "character")!;
    execute(m, {
      command: "multi_damage", amount: { kind: "typed", total: 9 }, damage_type: "fire", save: { ability: "dex", dc: 13, half: true },
      targets: [{ id: "cultist1", save: { kind: "typed", total: 4 } }, { id: "priest", save: { kind: "outcome", outcome: "saved" } }, { id: pc.id, save: { kind: "outcome", outcome: "failed" } }],
    });
    expect(byId(m.view.state, "cultist1")!.hp.current).toBe(Math.max(0, cultist - 9));
    expect(byId(m.view.state, "priest")!.hp.current).toBe(priest - 4);
    expect(m.playerEvents.filter((e) => e.event === "damage_prompt").map((e) => [e.id, e.amount])).toEqual([[pc.id, 9]]);
  });

  it("the sheet: typed saves first, Saved/Failed, Roll creature saves, then Apply sends one command", () => {
    const m = sampleCombat();
    const send = vi.fn<(c: Command) => void>();
    const close = vi.fn();
    render(<MultiDamageSheet state={m.view.state} first="cultist1" blockOf={() => undefined} send={send} onClose={close} />);
    const sheet = screen.getByTestId("multi-damage");
    fireEvent.change(within(sheet).getByLabelText("Damage: a total, or dice to roll once"), { target: { value: "8d6" } });
    fireEvent.change(within(sheet).getByLabelText("Save DC"), { target: { value: "15" } });
    fireEvent.click(within(sheet).getByLabelText("Target Cultist 2"));
    const pc = m.view.state.combatants.find((c) => c.kind === "character")!;
    fireEvent.click(within(sheet).getByLabelText(`Target ${pc.label}`));
    expect(screen.getByRole("button", { name: /Apply to 3 targets/ }).hasAttribute("disabled")).toBe(true);
    fireEvent.change(within(sheet).getByLabelText(`${pc.label}: DEX save, typed`), { target: { value: "17" } });
    fireEvent.click(within(sheet).getByRole("button", { name: "Roll creature saves (2)" }));
    fireEvent.click(screen.getByRole("button", { name: /Apply to 3 targets/ }));
    expect(send).toHaveBeenCalledWith({
      command: "multi_damage", amount: { kind: "app", notation: "8d6" }, save: { ability: "dex", dc: 15, half: true },
      targets: [{ id: "cultist1", save: { kind: "app" } }, { id: "cultist2", save: { kind: "app" } }, { id: pc.id, save: { kind: "typed", total: 17 } }],
    });
    expect(close).toHaveBeenCalled();
  });
});

describe("a late-joining party member (A2)", () => {
  it("picks the member, typed initiative first, or Roll for me", () => {
    const add = vi.fn();
    render(<AddMemberDialog joinable={[{ characterId: "c_1", name: "Bram" }, { characterId: "c_2", name: "Ilse" }]} onAdd={add} onClose={() => {}} />);
    expect(screen.getByRole("button", { name: "Add to the fight" }).hasAttribute("disabled")).toBe(true);
    fireEvent.click(screen.getByRole("radio", { name: "Ilse" }));
    fireEvent.change(screen.getByLabelText("Initiative: typed result"), { target: { value: "14" } });
    fireEvent.click(screen.getByRole("button", { name: "Add to the fight" }));
    expect(add).toHaveBeenLastCalledWith("c_2", { kind: "typed", total: 14 });
    fireEvent.click(screen.getByRole("button", { name: "Roll for me" }));
    expect(add).toHaveBeenLastCalledWith("c_2", { kind: "app", modifier: 0, mode: "normal" });
  });
});
