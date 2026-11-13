import { describe, expect, it } from "vitest";
import { abilityStatus, byId, effectiveAc, effectiveSpeed, rosterOrder, turnOrder } from "./derive";
import { sampleCombat } from "./fixtures";
import { execute, exportCombat, importCombat, undo, type MockCombat } from "./mockEngine";
import type { Command } from "./types";

/**
 * The mock server's rules, which the tracker's screens are tested against: they follow party/docs/combat-engine.md
 * so the UI tests mean the same thing against the real server.
 */

const run = (m: MockCombat, ...cmds: Command[]) => { for (const c of cmds) execute(m, c); return m.view.state; };
const typed = (total: number) => ({ kind: "typed" as const, total });

function ready(): MockCombat {
  const m = sampleCombat();
  run(m,
    { command: "set_initiative", id: "knight", roll: typed(18) },
    { command: "set_initiative", id: "dragon", roll: typed(12) },
    { command: "set_initiative", id: "priest", roll: typed(10) },
    { command: "set_initiative", id: "cultist1", roll: typed(9) },
    { command: "set_initiative", id: "cultist2", roll: typed(8) },
    { command: "set_initiative", id: "mage", roll: typed(5) },
  );
  return m;
}

describe("turn order", () => {
  it("sorts by initiative with PCs, puts the mount right after its rider, and leaves blank PCs out (C1, C3)", () => {
    const s = ready().view.state;
    expect(turnOrder(s)).toEqual(["knight", "warhorse", "pc-aria", "dragon", "priest", "cultist1", "cultist2", "mage"]);
    expect(rosterOrder(s).map((c) => c.id).at(-1)).toBe("pc-bram");
  });

  it("skips the defeated, counts rounds and never restarts the round for a defeated active creature (C4)", () => {
    const m = ready();
    run(m, { command: "start" });
    expect(m.view.state.current).toBe("knight");
    run(m, { command: "next_turn" }, { command: "next_turn" }, { command: "next_turn" });
    expect(m.view.state.current).toBe("dragon");
    // The dragon dies on its own turn: the next turn is the priest's, still round 1.
    run(m, { command: "set_hp", id: "dragon", current: 0 }, { command: "next_turn" });
    expect(m.view.state.current).toBe("priest");
    expect(m.view.state.round).toBe(1);
    run(m, { command: "next_turn" }, { command: "next_turn" }, { command: "next_turn" }, { command: "next_turn" });
    expect(m.view.state.round).toBe(2);
    expect(m.view.state.current).toBe("knight");
  });

  it("a creature whose block prints no AC (a thin companion) has no AC, with or without effects (D17 b)", () => {
    const m = ready();
    const knight = byId(m.view.state, "knight")!;
    const thin = { ...knight, id: "wisp", ac: null };
    const state = { ...m.view.state, combatants: [...m.view.state.combatants, thin] };
    expect(effectiveAc(state, thin)).toEqual({ base: null, effects: 0, cover: 0, total: null });
    const shielded = { ...state, effects: [{ id: 99, name: "Shield", target: "wisp", modifiers: [{ kind: "ac" as const, value: 5 }] }] };
    expect(effectiveAc(shielded, thin).total).toBeNull();
  });

  it("unlinks the mount when its rider goes down, and the rider loses the mount's cover (C3, C22)", () => {
    const m = ready();
    run(m, { command: "add_effect", effect: { name: "Mounted", target: "knight", modifiers: [{ kind: "cover", value: "half" }], ends_on_defeat_of: ["warhorse"] } });
    expect(effectiveAc(m.view.state, byId(m.view.state, "knight")!).total).toBe(20);
    run(m, { command: "damage", target: "warhorse", parts: [{ amount: typed(30) }] });
    expect(effectiveAc(m.view.state, byId(m.view.state, "knight")!).total).toBe(18);
    run(m, { command: "set_hp", id: "knight", current: 0 });
    expect(byId(m.view.state, "warhorse")!.linked_to).toBeUndefined();
  });
});

describe("rolls: in the app or typed (D21, D43)", () => {
  it("a typed initiative is kept as typed; a roll uses the creature's modifier", () => {
    const m = sampleCombat();
    run(m, { command: "set_initiative", id: "mage", roll: typed(17) });
    expect(byId(m.view.state, "mage")!.initiative).toBe(17);
    run(m, { command: "set_initiative", id: "mage", roll: { kind: "app" } });
    const ev = m.view.log.at(-1)!;
    expect(ev.event).toBe("initiative");
    expect((ev.roll as { source: string }).source).toBe("app");
    expect(byId(m.view.state, "mage")!.initiative).toBe((ev.roll as { total: number }).total);
  });

  it("a recharge takes a typed d6 or rolls one, and refuses a typed 7 (C15)", () => {
    const m = ready();
    run(m, { command: "start" }, { command: "next_turn" }, { command: "next_turn" }, { command: "next_turn" });
    run(m, { command: "use_ability", id: "dragon", ability: "fire_breath" });
    expect(abilityStatus(m.view.state, byId(m.view.state, "dragon")!, byId(m.view.state, "dragon")!.abilities![1]!)).not.toBeNull();
    expect(() => execute(m, { command: "roll_recharge", id: "dragon", ability: "fire_breath", roll: typed(7) })).toThrow(/1 to 6/);
    run(m, { command: "roll_recharge", id: "dragon", ability: "fire_breath", roll: typed(4) });
    expect(byId(m.view.state, "dragon")!.abilities![1]!.recharge!.ready).toBe(false);
    run(m, { command: "roll_recharge", id: "dragon", ability: "fire_breath", roll: typed(5) });
    expect(byId(m.view.state, "dragon")!.abilities![1]!.recharge!.ready).toBe(true);
  });

  it("damage dice are rolled by the server; typed totals are applied as typed", () => {
    const m = ready();
    run(m, { command: "damage", target: "mage", parts: [{ amount: typed(7), damage_type: "slashing" }] });
    expect(byId(m.view.state, "mage")!.hp.current).toBe(33);
    run(m, { command: "damage", target: "mage", parts: [{ amount: { kind: "app", notation: "1d4" } }] });
    const hp = byId(m.view.state, "mage")!.hp.current;
    expect(hp).toBeGreaterThanOrEqual(29);
    expect(hp).toBeLessThanOrEqual(32);
  });
});

describe("HP and damage (C6, D7)", () => {
  it("applies immunity and keeps temp HP first", () => {
    const m = ready();
    run(m, { command: "damage", target: "dragon", parts: [{ amount: typed(20), damage_type: "fire" }, { amount: typed(5), damage_type: "piercing" }] });
    expect(byId(m.view.state, "dragon")!.hp.current).toBe(251);
    run(m, { command: "grant_temp_hp", id: "mage", amount: 5 }, { command: "damage", target: "mage", parts: [{ amount: typed(8) }] });
    expect(byId(m.view.state, "mage")!.hp).toMatchObject({ current: 37, temp: 0 });
  });

  it("damage to a PC is a prompt to its player and leaves the PC's HP alone (D7)", () => {
    const m = ready();
    run(m, { command: "damage", target: "pc-aria", parts: [{ amount: typed(7), damage_type: "slashing" }] });
    expect(byId(m.view.state, "pc-aria")!.hp.current).toBe(28);
    expect(m.playerEvents.at(-1)).toMatchObject({ event: "damage_prompt", id: "pc-aria", owner: "pl1", amount: 7 });
  });
});

describe("concentration and effects (C11, C12)", () => {
  it("damage asks for a CON save at max(10, half); ending concentration ends every effect it holds, with follow-ups", () => {
    const m = ready();
    run(m,
      { command: "add_effect", effect: { name: "Haste", source: "mage", target: "knight", concentration: true, duration: { kind: "source_turns", remaining: 10, at: "start" }, modifiers: [{ kind: "ac", value: 2 }, { kind: "speed_multiply", value: 2 }], follow_up: { name: "Lethargy", target: "knight", duration: { kind: "target_turns", remaining: 1, at: "end" }, modifiers: [{ kind: "speed_zero" }] } } },
      { command: "add_effect", effect: { name: "Haste", source: "mage", target: "dragon", concentration: true, modifiers: [{ kind: "ac", value: 2 }] } },
    );
    const s1 = m.view.state;
    expect(byId(s1, "mage")!.concentration?.name).toBe("Haste");
    expect(effectiveAc(s1, byId(s1, "knight")!).total).toBe(20);
    expect(effectiveSpeed(s1, byId(s1, "knight")!).walk).toBe(60);
    run(m, { command: "damage", target: "mage", parts: [{ amount: typed(24) }] });
    expect(byId(m.view.state, "mage")!.pending_concentration).toEqual([{ dc: 12, damage: 24 }]);
    run(m, { command: "resolve_concentration", id: "mage", roll: typed(9) });
    const s2 = m.view.state;
    expect(byId(s2, "mage")!.concentration).toBeUndefined();
    expect(s2.effects.map((e) => `${e.name}@${e.target}`)).toEqual(["Lethargy@knight"]);
    expect(effectiveSpeed(s2, byId(s2, "knight")!).walk).toBe(0);
  });

  it("an incapacitating condition ends concentration (C10)", () => {
    const m = ready();
    run(m, { command: "set_concentration", id: "priest", name: "Bless" }, { command: "add_condition", effect: { name: "Stunned", target: "priest", conditions: ["stunned"] } });
    expect(byId(m.view.state, "priest")!.concentration).toBeUndefined();
  });

  it("counts a duration in the source's turns and spends charges", () => {
    const m = ready();
    run(m, { command: "add_effect", effect: { name: "Ward", source: "knight", target: "priest", charges: 2, duration: { kind: "source_turns", remaining: 2, at: "start" } } });
    const ward = m.view.state.effects[0]!.id;
    run(m, { command: "use_charge", effect: ward });
    expect(m.view.state.effects[0]!.charges).toBe(1);
    run(m, { command: "start" });
    expect(m.view.state.effects[0]!.duration).toMatchObject({ remaining: 1 });
    run(m, { command: "use_charge", effect: ward });
    expect(m.view.state.effects).toHaveLength(0);
  });
});

describe("legendary actions, rules and group limits (C16, C20, C21)", () => {
  it("two creatures share one pool; it isn't usable on a member's own turn and refills at the first member's turn", () => {
    const m = ready();
    run(m, { command: "start" });
    const s = () => m.view.state;
    // The knight's own turn: its legendary option is refused, the dragon's is not.
    expect(abilityStatus(s(), byId(s(), "knight")!, byId(s(), "knight")!.abilities!.find((a) => a.key === "charge")!)).toBe("Not on its own turn");
    run(m, { command: "use_ability", id: "dragon", ability: "wing" });
    expect(s().legendary_pools.court!.remaining).toBe(1);
    run(m, { command: "next_turn" }, { command: "use_ability", id: "knight", ability: "charge" });
    expect(s().legendary_pools.court!.remaining).toBe(0);
    expect(abilityStatus(s(), byId(s(), "dragon")!, byId(s(), "dragon")!.abilities!.find((a) => a.key === "tail")!)).toBe("Not enough legendary actions");
    // The pool resets at the start of the dragon's turn (the first member listed).
    run(m, { command: "next_turn" }, { command: "next_turn" });
    expect(s().current).toBe("dragon");
    expect(s().legendary_pools.court!.remaining).toBe(3);
  });

  it("a rule tied to the priest switches the cultists' gift off for good when the priest drops", () => {
    const m = ready();
    run(m, { command: "start" });
    const gift = (id: string) => abilityStatus(m.view.state, byId(m.view.state, id)!, byId(m.view.state, id)!.abilities!.find((a) => a.key === "dark_gift")!);
    expect(gift("cultist1")).toBeNull();
    run(m, { command: "use_ability", id: "cultist1", ability: "dark_gift" });
    expect(gift("cultist2")).toBe("Group limit used this round: cultists");
    run(m, { command: "damage", target: "priest", parts: [{ amount: typed(40) }] });
    expect(m.view.state.rules[0]!.active).toBe(false);
    expect(gift("cultist2")).toBe("Switched off by The priest's gift");
  });
});

describe("reveal, undo and export (D20 b, C8, D45)", () => {
  it("reveal and hide tell players; hiding after a reveal says they saw it", () => {
    const m = ready();
    run(m, { command: "reveal", id: "dragon" });
    expect(m.playerEvents.at(-1)).toEqual({ event: "appeared", id: "dragon" });
    run(m, { command: "hide", id: "dragon" });
    expect(m.view.log.at(-1)).toMatchObject({ event: "hidden", was_seen: true });
    expect(m.playerEvents.at(-1)).toEqual({ event: "disappeared", id: "dragon" });
  });

  it("undo goes back to the exact earlier state, and a player's write needs force", () => {
    const m = ready();
    const before = JSON.stringify(m.view.state);
    run(m, { command: "damage", target: "mage", parts: [{ amount: typed(10) }] });
    undo(m);
    expect(JSON.stringify(m.view.state)).toBe(before);
    execute(m, { command: "set_initiative", id: "pc-bram", roll: typed(11) }, undefined, "player");
    expect(() => undo(m)).toThrow(/player's/);
    undo(m, undefined, true);
    expect(byId(m.view.state, "pc-bram")!.initiative).toBeNull();
  });

  it("refuses a stale seq (CAS)", () => {
    const m = ready();
    expect(() => execute(m, { command: "start" }, m.view.seq - 1)).toThrow(/changed/);
  });

  it("an export imports as a new combat with seq 0, and other files are refused", () => {
    const m = ready();
    run(m, { command: "start" });
    const copy = importCombat(exportCombat(m), "c2", "p1");
    expect(copy.view.id).toBe("c2");
    expect(copy.view.seq).toBe(0);
    expect(copy.view.state).toEqual(m.view.state);
    expect(() => importCombat("{\"format\":\"other\"}", "c3", "p1")).toThrow(/not a combat export/);
    expect(() => importCombat(JSON.stringify({ format: "borealis-party-combat", schema_version: 99, state: m.view.state }), "c3", "p1")).toThrow(/newer/);
  });
});

describe("controlled creatures: summons and pets (plan 4.4)", () => {
  const wolf = (id: string) => ({ id, kind: "creature" as const, label: "Wolf", public_name: "Wolf", visible: false, hp: { current: 11, max: 11 }, ac: 13, initiative_bonus: 2 });

  it("act right after their owner, nested to any depth, and roll no initiative of their own", () => {
    const m = ready();
    run(m,
      { command: "summon", owner: "pc-aria", combatant: wolf("wolf-a") },
      { command: "summon", owner: "wolf-a", combatant: { ...wolf("pup"), label: "Pup" } },
      { command: "summon", owner: "mage", combatant: { ...wolf("imp"), label: "Imp" } },
      { command: "roll_initiative" },
    );
    const s = m.view.state;
    const order = turnOrder(s);
    expect(order.slice(order.indexOf("pc-aria"), order.indexOf("pc-aria") + 3)).toEqual(["pc-aria", "wolf-a", "pup"]);
    expect(order[order.indexOf("mage") + 1]).toBe("imp");
    // A PC's summon is its player's and visible; the mage's is the DM's and as hidden as the mage.
    expect(byId(s, "wolf-a")).toMatchObject({ controller: { kind: "player", pc: "pc-aria" }, visible: true });
    expect(byId(s, "imp")).toMatchObject({ controller: { kind: "dm" }, visible: false });
    expect(byId(s, "wolf-a")!.initiative).toBeNull();
  });

  it("can switch to its own initiative, typed or rolled", () => {
    const m = ready();
    run(m, { command: "summon", owner: "mage", combatant: wolf("imp"), initiative: typed(19) });
    expect(turnOrder(m.view.state)[0]).toBe("imp");
    run(m, { command: "set_initiative_mode", id: "imp", mode: "after_owner" });
    expect(turnOrder(m.view.state).at(-1)).toBe("imp");
  });

  it("an owner going down asks the DM (nothing changes on its own), and dismiss removes it", () => {
    const m = ready();
    run(m, { command: "summon", owner: "mage", combatant: wolf("imp") }, { command: "set_hp", id: "mage", current: 0 });
    expect(m.view.log.some((e) => e.event === "owner_defeated" && e.subject === "imp")).toBe(true);
    expect(byId(m.view.state, "imp")!.removed).toBeFalsy();
    run(m, { command: "dismiss", id: "imp" });
    expect(byId(m.view.state, "imp")!.removed).toBe(true);
    expect(turnOrder(m.view.state)).not.toContain("imp");
  });
});
