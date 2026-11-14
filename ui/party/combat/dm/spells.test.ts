import { describe, expect, it } from "vitest";
import { byId, effectiveAc } from "./derive";
import { buildMech, fieldsFrom, guessPreset, numberChips, presetFields } from "./EffectBuilder";
import { sampleCombat } from "./fixtures";
import { evalCount, evalFormula, preview } from "./formula";
import { execute, predict } from "./mockEngine";
import { castAmounts, castsOf, mechOf, openCasts } from "./spellTypes";
import { summonOptions } from "./SummonPanel";
import { turnButtons } from "./turn";
import type { Command } from "./types";

const cmd = (c: Command) => c;
const typed = (total: number) => ({ kind: "typed" as const, total });

function started() {
  const m = sampleCombat();
  for (const [id, n] of [["knight", 18], ["dragon", 12], ["priest", 10], ["cultist1", 9], ["cultist2", 8], ["mage", 5], ["druid", 4]] as const) {
    execute(m, { command: "set_initiative", id, roll: typed(n) });
  }
  execute(m, { command: "start" });
  return m;
}

describe("formula previews", () => {
  it("evaluates slot variables into dice", () => {
    expect(evalFormula("8d6+(slot_above)d6", { slot: 4, slot_above: 1 })).toBe("9d6");
    expect(evalFormula("8d6+(slot_above)d6", { slot: 3, slot_above: 0 })).toBe("8d6");
    expect(evalFormula("2d8+(2*slot_above)d8+mod", { slot_above: 2, mod: 3 })).toBe("6d8+3");
    expect(evalFormula("(cantrip_tier)d10", { cantrip_tier: 2 })).toBe("2d10");
    expect(evalCount("1+slot_above", { slot_above: 2 })).toBe(3);
    expect(evalFormula("weird(", {})).toBe("weird(");
  });

  it("previews a cast the way the sheet shows it", () => {
    const fireball = mechOf(byId(sampleCombat().view.state, "mage")!.abilities!.find((a) => a.key === "fireball")!)!;
    expect(preview(fireball, 4)).toBe("9d6 fire at L4 · DEX save, half on a success · 20-ft sphere");
    const hold = mechOf(byId(sampleCombat().view.state, "mage")!.abilities!.find((a) => a.key === "hold_person")!)!;
    expect(preview(hold, 3)).toContain("up to 2 targets");
    const cure = mechOf(byId(sampleCombat().view.state, "priest")!.abilities!.find((a) => a.key === "cure_wounds")!)!;
    expect(preview(cure, 2, 1, 3)).toBe("heal 4d8+3 at L2 · up to 1 target");
  });
});

describe("AC with spells", () => {
  it("Mage Armor sets 13 + DEX and says so; Shield adds 5 on top; ending it goes back", () => {
    const m = started();
    const s = () => m.view.state;
    expect(effectiveAc(s(), byId(s(), "mage")!).total).toBe(12);
    execute(m, cmd({ command: "cast_spell", caster: "mage", ability: "mage_armor", slot: 1, targets: ["mage"] }));
    const ac = effectiveAc(s(), byId(s(), "mage")!);
    expect(ac.total).toBe(15);
    expect(ac.text).toBe("15 = Mage Armor 13 + DEX 2");
    execute(m, { command: "set_economy", id: "mage", reaction: false });
    execute(m, cmd({ command: "cast_spell", caster: "mage", ability: "shield", slot: 1, targets: [] }));
    expect(effectiveAc(s(), byId(s(), "mage")!).total).toBe(20);
    expect(effectiveAc(s(), byId(s(), "mage")!).text).toBe("20 = Mage Armor 13 + DEX 2 + Shield 5");
    expect(byId(s(), "mage")!.spell_slots!["1"]!.remaining).toBe(2);
  });

  it("a printed AC higher than the spell's base stays", () => {
    const m = started();
    execute(m, { command: "add_effect", effect: { name: "Mage Armor", target: "knight", modifiers: [{ kind: "ac_base", value: { base: 13, add_dex: true } }] } });
    expect(effectiveAc(m.view.state, byId(m.view.state, "knight")!).total).toBe(18);
  });
});

describe("turn buttons", () => {
  it("labels every action with its numbers and groups them by what they spend", () => {
    const m = started();
    const s = m.view.state;
    const dragon = turnButtons(s, byId(s, "dragon")!, m.view.statblocks.dragon!.data);
    expect(dragon.find((b) => b.key === "fire_breath")).toMatchObject({ label: "Fire Breath 5–6", kind: "save", group: "action", detail: "DC 21 DEX · 18d6" });
    const knight = turnButtons(s, byId(s, "knight")!, m.view.statblocks.knight!.data);
    expect(knight.find((b) => b.key === "greatsword")).toMatchObject({ label: "Greatsword", detail: "+5 · 2d6+3", kind: "attack" });
    expect(knight.filter((b) => b.kind === "standard").map((b) => b.label)).toEqual(["Dodge", "Dash", "Disengage", "Help"]);
    expect(knight.find((b) => b.label === "Opportunity attack")?.group).toBe("reaction");
    const mage = turnButtons(s, byId(s, "mage")!, m.view.statblocks.mage!.data);
    expect(mage.find((b) => b.key === "fireball")).toMatchObject({ label: "Fireball L3+", detail: "8d6 · 7 slots", coverage: "auto", group: "action" });
    expect(mage.find((b) => b.key === "fire_bolt")).toMatchObject({ detail: "2d10 · cantrip" });
    expect(mage.find((b) => b.key === "shield")?.group).toBe("reaction");
    expect(mage.find((b) => b.key === "cone_of_cold")?.coverage).toBe("ruling");
    expect(turnButtons(s, byId(s, "mage")!, undefined, new Set(["ID_SRD_SPELL_CONE_OF_COLD"])).find((b) => b.key === "cone_of_cold")?.coverage).toBe("suggested");
    expect(mage.find((b) => b.key === "arcane_word")?.coverage).toBe("unlinked");
  });

  it("a disabled button says why", () => {
    const m = started();
    execute(m, cmd({ command: "standard_action", id: "knight", action: "dodge" }));
    const s = m.view.state;
    const knight = turnButtons(s, byId(s, "knight")!, m.view.statblocks.knight!.data);
    expect(knight.find((b) => b.key === "greatsword")!.reason).toBe("Action spent");
    expect(knight.find((b) => b.key === "std-dash")!.reason).toBe("Action spent");
    expect(s.effects.find((e) => e.name === "Dodge" && e.target === "knight")).toBeTruthy();
    for (const l of ["3", "4", "5"]) execute(m, { command: "set_slots", id: "mage", level: Number(l), pool: { max: 3, remaining: 0 } });
    const mage = turnButtons(m.view.state, byId(m.view.state, "mage")!, undefined);
    expect(mage.find((b) => b.key === "fireball")!.reason).toBe("No slot of level 3+ left");
  });
});

describe("casts in the mock engine (typed rolls first)", () => {
  it("Fireball at L4: typed saves, one damage roll, half for those who saved", () => {
    const m = started();
    const s = () => m.view.state;
    execute(m, cmd({ command: "cast_spell", caster: "mage", ability: "fireball", slot: 4, targets: ["knight", "cultist1"] }));
    const k = openCasts(s())[0]!;
    expect(k).toMatchObject({ slot: 4, spec: { dc: 14, mech: { name: "Fireball" } } });
    expect(castAmounts(k)).toEqual([{ kind: "damage", dice: "9d6", type: "fire" }]);
    expect(byId(s(), "mage")!.economy!.action).toBe(true);
    expect(() => execute(m, cmd({ command: "apply_cast", cast: k.id, amounts: [typed(30)] }))).toThrow(/Waiting on 2 rolls/);
    execute(m, cmd({ command: "resolve_cast_row", cast: k.id, target: "knight", roll: typed(15) }));
    execute(m, cmd({ command: "resolve_cast_row", cast: k.id, target: "cultist1", roll: { kind: "outcome", outcome: "failed" } }));
    expect(castsOf(s())[0]!.rows.map((r) => r.outcome)).toEqual(["saved", "failed"]);
    execute(m, cmd({ command: "apply_cast", cast: k.id, amounts: [typed(30)] }));
    expect(byId(s(), "knight")!.hp.current).toBe(52 - 15);
    expect(byId(s(), "cultist1")!.life?.state).toBe("dead");
    expect(openCasts(s())).toHaveLength(0);
  });

  it("Hold Person paralyzes on a failed save, held by concentration; ending it ends the condition", () => {
    const m = started();
    execute(m, cmd({ command: "cast_spell", caster: "mage", ability: "hold_person", slot: 2, targets: ["cultist2"] }));
    const k = openCasts(m.view.state)[0]!;
    execute(m, cmd({ command: "resolve_cast_row", cast: k.id, target: "cultist2", roll: typed(9) }));
    execute(m, cmd({ command: "apply_cast", cast: k.id }));
    const fx = m.view.state.effects.find((e) => e.target === "cultist2")!;
    expect(fx).toMatchObject({ name: "Hold Person", conditions: ["paralyzed"], concentration: true, repeat_save: { ability: "wis", dc: 14, at: "end" } });
    execute(m, { command: "end_concentration", id: "mage" });
    expect(m.view.state.effects.find((e) => e.target === "cultist2")).toBeUndefined();
  });

  it("refuses too many targets and a slot below the spell's level", () => {
    const m = started();
    expect(() => execute(m, cmd({ command: "cast_spell", caster: "mage", ability: "hold_person", slot: 2, targets: ["cultist1", "cultist2"] }))).toThrow(/up to 1 target/);
    expect(() => execute(m, cmd({ command: "cast_spell", caster: "mage", ability: "fireball", slot: 2, targets: ["knight"] }))).toThrow(/level 3 slot/);
  });

  it("Legendary Resistance turns a failed save into a success", () => {
    const m = started();
    execute(m, cmd({ command: "cast_spell", caster: "mage", ability: "fireball", slot: 3, targets: ["dragon"] }));
    const k = openCasts(m.view.state)[0]!;
    execute(m, cmd({ command: "resolve_cast_row", cast: k.id, target: "dragon", roll: { kind: "outcome", outcome: "failed" } }));
    execute(m, cmd({ command: "resolve_cast_row", cast: k.id, target: "dragon", roll: { kind: "outcome", outcome: "saved" }, use_lr: true }));
    expect(openCasts(m.view.state)[0]!.rows[0]).toMatchObject({ outcome: "saved", used_lr: true });
    expect(byId(m.view.state, "dragon")!.legendary_resistance!.remaining).toBe(2);
  });

  it("a statblock breath weapon opens saves with the dice from its text; cancel can refund", () => {
    const m = started();
    execute(m, { command: "next_turn" }); execute(m, { command: "next_turn" }); execute(m, { command: "next_turn" });
    execute(m, cmd({ command: "perform_statblock", id: "dragon", ability: "fire_breath", targets: ["knight", "pc-aria"] }));
    const k = openCasts(m.view.state)[0]!;
    expect(k).toMatchObject({ statblock: true, spec: { dc: 21, origin: { kind: "statblock" } } });
    expect(castAmounts(k)).toEqual([{ kind: "damage", dice: "18d6", type: "fire" }]);
    expect(byId(m.view.state, "dragon")!.abilities!.find((a) => a.key === "fire_breath")!.recharge!.ready).toBe(false);
    execute(m, cmd({ command: "cancel_cast", cast: k.id }));
    expect(openCasts(m.view.state)).toHaveLength(0);
  });

  it("predict leaves a cast to the server when it would roll", () => {
    const m = started();
    expect(predict(m.view.state, cmd({ command: "cast_spell", caster: "mage", ability: "mage_armor", slot: 1, targets: ["mage"] }))).not.toBeNull();
  });
});

describe("summon spells (no spawning)", () => {
  it("lists the options, times the slot's multiplier", () => {
    const druid = byId(sampleCombat().view.state, "druid")!;
    const mech = mechOf(druid.abilities!.find((a) => a.key === "conjure_animals")!)!;
    expect(summonOptions(mech, 3)!.options).toEqual([{ cr: "2", count: 1 }, { cr: "1", count: 2 }, { cr: "1/2", count: 4 }, { cr: "1/4", count: 8 }]);
    expect(summonOptions(mech, 5)!.options[0]).toEqual({ cr: "2", count: 2 });
    expect(summonOptions(mech, 7)!.options[3]).toEqual({ cr: "1/4", count: 24 });
  });

  it("casting it takes concentration and adds nobody", () => {
    const m = started();
    const before = m.view.state.combatants.length;
    execute(m, cmd({ command: "cast_spell", caster: "druid", ability: "conjure_animals", slot: 3, targets: [] }));
    expect(m.view.state.combatants).toHaveLength(before);
    expect(byId(m.view.state, "druid")!.concentration?.name).toBe("Conjure Animals");
    expect(m.view.log.find((e) => e.event === "summon_options")).toMatchObject({ caster: "druid", concentration: true });
  });
});

describe("the effect builder", () => {
  it("builds a closed mechanic from the preset fields and reads it back", () => {
    const f = { ...presetFields("damage_save", fieldsFrom({}, "damage_save", 1)), dice: "2d6", damageType: "psychic" as const, saveAbility: "wis" as const, condition: "frightened", repeatAt: "end" as const, target: "creatures" as const };
    const mech = buildMech("ID_ARCANE_WORD", "Arcane Word", "2024", f);
    expect(mech).toMatchObject({ v: 1, edition: "2024", level: 1, delivery: { kind: "save", ability: "wis", success: "half" }, targeting: { kind: "creatures", max: "1" }, tier: "partial" });
    expect(mech.on_land).toEqual([{ kind: "damage", dice: "2d6", damage_type: "psychic" }, { kind: "condition", condition: "frightened", repeat_save: { ability: "wis", at: "end" } }]);
    expect(guessPreset(mech)).toBe("damage_save");
    expect(fieldsFrom(mech, "damage_save", 1)).toMatchObject({ dice: "2d6", condition: "frightened", saveAbility: "wis" });
  });

  it("offers number chips from the user's own text", () => {
    const chips = numberChips("One creature within 60 feet makes a Wisdom save or takes 2d6 psychic damage and is frightened.");
    expect(chips.map((c) => c.label)).toEqual(["2d6", "60 ft", "Wisdom", "frightened"]);
  });
});
