/**
 * An SRD-only sample fight for tests, the rig and the mock server: a knight on a warhorse (linked initiative and
 * cover, C3/C22), a dragon and the knight sharing one legendary pool (C16), a priest whose life holds an encounter
 * rule for the cultists (C20) with a group limit (C21), a mage with spell slots and concentration (C18), and two PCs.
 */
import { emptyState, newCombat, type MockCombat } from "./mockEngine";
import type { Ability, Combatant, CombatState, StatBlockSnapshot } from "./types";

function creature(id: string, label: string, hp: number, ac: number, init: number, extra: Partial<Combatant> = {}): Combatant {
  return {
    id, kind: "creature", label, public_name: label, visible: false, ever_revealed: false, removed: false,
    initiative: null, initiative_bonus: init, tiebreak: 0, absent: false,
    hp: { current: hp, max: hp, temp: 0 }, hp_authority: "engine", zero_hp: "dies", life: { state: "alive" },
    ac, speed: { walk: 30 }, economy: { action: false, bonus_action: false, reaction: false }, ...extra,
  };
}

export function character(id: string, owner: string, name: string, hp: number, ac: number, init: number, initiative: number | null): Combatant {
  return {
    ...creature(id, name, hp, ac, init), kind: "character", owner, visible: true, ever_revealed: true,
    hp_authority: "player", zero_hp: "death_saves", initiative, speed: { walk: 30 },
  };
}

const act = (key: string, name: string, kind: Ability["kind"], text: string, more: Partial<Ability> = {}): Ability => ({ key, name, kind, text, ...more });

export const SAMPLE_STATBLOCKS: Record<string, StatBlockSnapshot> = {
  knight: {
    name: "Knight",
    data: {
      ac: [{ value: 18, source: "plate armor" }], hp: { average: 52, formula: "8d8+16" }, speed: { walk: 30 },
      abilities: { str: 16, dex: 11, con: 14, int: 11, wis: 11, cha: 15 }, saves: { con: 4, wis: 2 },
      actions: [
        { name: "Multiattack", text: "The knight makes two melee attacks." },
        { name: "Greatsword", text: "Melee Weapon Attack: +5 to hit, reach 5 ft. Hit: 10 (2d6 + 3) slashing damage.", attack: { kind: "melee_weapon", to_hit: 5, reach: 5, damage: [{ average: 10, dice: "2d6+3", type: "slashing" }] } },
        { name: "Heavy Crossbow", text: "Ranged Weapon Attack: +2 to hit, range 100/400 ft. Hit: 5 (1d10) piercing damage.", attack: { kind: "ranged_weapon", to_hit: 2, range: 100, long_range: 400, damage: [{ average: 5, dice: "1d10", type: "piercing" }] } },
      ],
      reactions: [{ name: "Parry", text: "The knight adds 2 to its AC against one melee attack that would hit it." }],
      traits: [{ name: "Brave", text: "The knight has advantage on saving throws against being frightened." }],
    },
  },
  warhorse: {
    name: "Warhorse",
    data: {
      ac: [{ value: 11 }], hp: { average: 19, formula: "3d10+3" }, speed: { walk: 60 },
      abilities: { str: 18, dex: 12, con: 13, int: 2, wis: 12, cha: 7 },
      actions: [{ name: "Hooves", text: "Melee Weapon Attack: +6 to hit, reach 5 ft. Hit: 11 (2d6 + 4) bludgeoning damage.", attack: { kind: "melee_weapon", to_hit: 6, reach: 5, damage: [{ average: 11, dice: "2d6+4", type: "bludgeoning" }] } }],
    },
  },
  dragon: {
    name: "Adult Red Dragon",
    data: {
      ac: [{ value: 19, source: "natural armor" }], hp: { average: 256, formula: "19d12+133" }, speed: { walk: 40, climb: 40, fly: 80 },
      abilities: { str: 27, dex: 10, con: 25, int: 16, wis: 13, cha: 21 }, saves: { dex: 6, con: 13, wis: 7, cha: 11 },
      damage_immunities: [{ types: ["fire"] }],
      traits: [{ name: "Legendary Resistance", text: "If the dragon fails a saving throw, it can choose to succeed instead.", usage: { per_day: 3 } }],
      actions: [
        { name: "Multiattack", text: "The dragon can use its Frightful Presence. It then makes three attacks: one with its bite and two with its claws." },
        { name: "Bite", text: "Melee Weapon Attack: +14 to hit, reach 10 ft. Hit: 19 (2d10 + 8) piercing damage plus 7 (2d6) fire damage.", attack: { kind: "melee_weapon", to_hit: 14, reach: 10, damage: [{ average: 19, dice: "2d10+8", type: "piercing" }, { average: 7, dice: "2d6", type: "fire" }] } },
        { name: "Fire Breath", text: "The dragon exhales fire in a 60-foot cone. Each creature in that area must make a DC 21 Dexterity saving throw, taking 63 (18d6) fire damage on a failed save, or half as much damage on a successful one.", usage: { recharge: { min: 5, max: 6 } }, save: { ability: "dex", dc: 21, half_on_success: true } },
        { name: "Frightful Presence", text: "Each creature of the dragon's choice within 120 feet must succeed on a DC 19 Wisdom saving throw or become frightened for 1 minute.", save: { ability: "wis", dc: 19 } },
      ],
      legendary: { per_round: 3, actions: [
        { name: "Detect", text: "The dragon makes a Wisdom (Perception) check." },
        { name: "Tail Attack", text: "The dragon makes a tail attack." },
        { name: "Wing Attack", text: "The dragon beats its wings. Each creature within 10 feet must succeed on a DC 22 Dexterity saving throw.", cost: 2, save: { ability: "dex", dc: 22 } },
      ] },
    },
  },
  priest: {
    name: "Priest",
    data: {
      ac: [{ value: 13, source: "chain shirt" }], hp: { average: 27, formula: "5d8+5" }, speed: { walk: 25 },
      abilities: { str: 10, dex: 10, con: 12, int: 13, wis: 16, cha: 13 },
      traits: [{ name: "Spellcasting", text: "The priest is a 5th-level spellcaster. Its spellcasting ability is Wisdom (spell save DC 13, +5 to hit with spell attacks).", spellcasting: { ability: "wis", save_dc: 13, attack_bonus: 5, lists: [{ frequency: "at_will", spells: [{ name: "Light" }, { name: "Sacred Flame" }] }] } }],
      actions: [{ name: "Mace", text: "Melee Weapon Attack: +2 to hit, reach 5 ft. Hit: 3 (1d6) bludgeoning damage.", attack: { kind: "melee_weapon", to_hit: 2, reach: 5, damage: [{ average: 3, dice: "1d6", type: "bludgeoning" }] } }],
    },
  },
  cultist: {
    name: "Cultist",
    data: {
      ac: [{ value: 12, source: "leather armor" }], hp: { average: 9, formula: "2d8" }, speed: { walk: 30 },
      abilities: { str: 11, dex: 12, con: 10, int: 10, wis: 11, cha: 10 },
      actions: [{ name: "Scimitar", text: "Melee Weapon Attack: +3 to hit, reach 5 ft. Hit: 4 (1d6 + 1) slashing damage.", attack: { kind: "melee_weapon", to_hit: 3, reach: 5, damage: [{ average: 4, dice: "1d6+1", type: "slashing" }] } }],
    },
  },
  mage: {
    name: "Mage",
    data: {
      ac: [{ value: 12 }], hp: { average: 40, formula: "9d8" }, speed: { walk: 30 },
      abilities: { str: 9, dex: 14, con: 11, int: 17, wis: 12, cha: 11 }, saves: { int: 6, wis: 4 },
      traits: [{ name: "Spellcasting", text: "The mage is a 9th-level spellcaster (spell save DC 14, +6 to hit with spell attacks).", spellcasting: { ability: "int", save_dc: 14, attack_bonus: 6, lists: [{ frequency: "at_will", spells: [{ name: "Fire Bolt" }] }] } }],
      actions: [{ name: "Dagger", text: "Melee or Ranged Weapon Attack: +5 to hit. Hit: 4 (1d4 + 2) piercing damage.", attack: { kind: "melee_or_ranged_weapon", to_hit: 5, reach: 5, range: 20, damage: [{ average: 4, dice: "1d4+2", type: "piercing" }] } }],
    },
  },
};

/** The sample state: creatures hidden until revealed, PCs with the initiative their players typed (one absent). */
export function sampleState(): CombatState {
  const s = emptyState("2024");
  s.combatants = [
    creature("knight", "Knight", 52, 18, 0, {
      abilities: [
        act("multiattack", "Multiattack", "action", "Two melee attacks."),
        act("greatsword", "Greatsword", "action", "+5 to hit, 2d6+3 slashing."),
        act("parry", "Parry", "reaction", "+2 AC against one melee attack."),
        act("charge", "Lance Charge", "legendary", "The knight moves and makes one attack.", { legendary_cost: 1 }),
      ],
    }),
    creature("warhorse", "Warhorse", 19, 11, 1, { linked_to: "knight", speed: { walk: 60 }, abilities: [act("hooves", "Hooves", "action", "+6 to hit, 2d6+4 bludgeoning.")] }),
    creature("dragon", "Adult Red Dragon", 256, 19, 0, {
      speed: { walk: 40, climb: 40, fly: 80 }, defenses: { immunities: [{ types: ["fire"] }] },
      legendary_resistance: { max: 3, remaining: 3 },
      abilities: [
        act("multiattack", "Multiattack", "action", "Frightful Presence, then bite and two claws."),
        act("fire_breath", "Fire Breath", "action", "60-ft cone, DC 21 DEX, 18d6 fire, half on a success.", { recharge: { min: 5, ready: true } }),
        act("tail", "Tail Attack", "legendary", "The dragon makes a tail attack.", { legendary_cost: 1 }),
        act("wing", "Wing Attack", "legendary", "DC 22 DEX; the dragon flies up to half its speed.", { legendary_cost: 2 }),
      ],
    }),
    creature("priest", "Priest", 27, 13, 0, {
      spell_slots: { "1": { max: 4, remaining: 4 }, "2": { max: 3, remaining: 3 }, "3": { max: 2, remaining: 2 } },
      abilities: [
        act("mace", "Mace", "action", "+2 to hit, 1d6 bludgeoning."),
        act("bless", "Bless", "action", "Up to three creatures add 1d4 to attacks and saves. Concentration, 1 minute."),
        act("spirit_guardians", "Spirit Guardians", "action", "15-ft aura, DC 13 WIS, 3d8 radiant. Concentration, 10 minutes."),
      ],
    }),
    creature("cultist1", "Cultist 1", 9, 12, 1, { group: "cultists", abilities: [act("scimitar", "Scimitar", "action", "+3 to hit, 1d6+1 slashing."), act("dark_gift", "Dark Gift", "bonus_action", "Once per round for the whole group, while the priest lives.", { group_limit: "cultists" })] }),
    creature("cultist2", "Cultist 2", 9, 12, 1, { group: "cultists", abilities: [act("scimitar", "Scimitar", "action", "+3 to hit, 1d6+1 slashing."), act("dark_gift", "Dark Gift", "bonus_action", "Once per round for the whole group, while the priest lives.", { group_limit: "cultists" })] }),
    creature("mage", "Mage", 40, 12, 2, {
      spell_slots: { "1": { max: 4, remaining: 4 }, "2": { max: 3, remaining: 3 }, "3": { max: 3, remaining: 3 }, "4": { max: 3, remaining: 3 }, "5": { max: 1, remaining: 1 } },
      resources: { misty: { label: "Misty Step 1/day", pool: { max: 1, remaining: 1 }, reset: "never" } },
      abilities: [
        act("dagger", "Dagger", "action", "+5 to hit, 1d4+2 piercing."),
        act("fire_bolt", "Fire Bolt", "action", "+6 to hit, 2d10 fire."),
        act("haste", "Haste", "action", "One willing creature: speed doubled, +2 AC. Concentration, 1 minute."),
        act("misty_step", "Misty Step", "bonus_action", "Teleport up to 30 ft.", { uses: "misty" }),
        act("counterspell", "Counterspell", "reaction", "Interrupt a spell."),
      ],
    }),
    character("pc-aria", "pl1", "Aria", 28, 16, 2, 15),
    character("pc-bram", "pl2", "Bram", 12, 12, 1, null),
  ];
  s.combatants.forEach((c, i) => { c.tiebreak = i; });
  s.next_tiebreak = s.combatants.length;
  s.legendary_pools = { court: { max: 3, remaining: 3, members: ["dragon", "knight"] } };
  s.group_limits = { cultists: { per_round: 1, used: 0 } };
  s.rules = [{
    id: "dark-gift", name: "The priest's gift", linked: "priest", active: true,
    text_active: "Cultists may use Dark Gift (one per round for the group).", text_ended: "The priest is down: no more Dark Gift.",
    flags: [{ target: { kind: "group", id: "cultists" }, ability: "dark_gift", while_active: true }],
  }];
  return s;
}

export const SAMPLE_BLOCK_OF: Record<string, keyof typeof SAMPLE_STATBLOCKS> = {
  knight: "knight", warhorse: "warhorse", dragon: "dragon", priest: "priest", cultist1: "cultist", cultist2: "cultist", mage: "mage",
};

export function sampleStatblocks(): Record<string, StatBlockSnapshot> {
  return Object.fromEntries(Object.entries(SAMPLE_BLOCK_OF).map(([id, key]) => [id, SAMPLE_STATBLOCKS[key]!]));
}

export function sampleCombat(id = "c1", partyId = "p1", seed = 7): MockCombat {
  const m = newCombat(id, partyId, sampleState(), sampleStatblocks(), seed);
  m.view.encounterId = "e1";
  return m;
}
