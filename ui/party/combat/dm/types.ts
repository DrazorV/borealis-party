/**
 * The DM's combat state: the JSON of `party_combat::CombatState`, its commands and its DM events, as the combat
 * server sends them (party/docs/combat-engine.md). Players never get these; they get the server's projection.
 */
import type {
  AbilityName, Condition, D20Input, D20Result, DamageType, DiceInput, DiceResult, Hp,
} from "./common";

export type Edition = "2014" | "2024";

export interface DamageClause {
  types: DamageType[];
  text?: string;
}

export interface Defenses {
  immunities?: DamageClause[];
  resistances?: DamageClause[];
  vulnerabilities?: DamageClause[];
  condition_immunities?: Condition[];
}

export type Life =
  | { state: "alive" }
  | { state: "dying"; successes: number; failures: number }
  | { state: "stable" }
  | { state: "dead" };

export interface Speeds {
  walk: number;
  fly?: number;
  swim?: number;
  climb?: number;
  burrow?: number;
  hover?: boolean;
}

export interface Economy {
  action: boolean;
  bonus_action: boolean;
  reaction: boolean;
}

export interface Pool {
  max: number;
  remaining: number;
}

export type Reset = "turn_start" | "round_start" | "never";

export interface Resource {
  label: string;
  pool: Pool;
  reset?: Reset;
}

export type AbilityKind = "action" | "bonus_action" | "reaction" | "legendary" | "free" | "passive";

export interface Recharge {
  min: number;
  ready: boolean;
}

export interface Ability {
  key: string;
  name: string;
  kind: AbilityKind;
  text?: string;
  legendary_cost?: number;
  recharge?: Recharge;
  uses?: string;
  group_limit?: string;
  requires_effect?: string;
  /** The spell this entry casts (0.3.2). DM only. */
  spell?: SpellLink;
  /** What it does, compiled on the server: a spell, or a stat block attack or save (0.3.2). DM only. */
  mech?: CastSpec;
}

/** Ability modifiers and saves, STR..CHA order (0.3.2). DM only. */
export interface CombatStats {
  mods: [number, number, number, number, number, number];
  saves: [number, number, number, number, number, number];
  magic_resistance?: boolean;
  /** Lowercase ("humanoid"), for target filters. */
  creature_type?: string;
  /** What the printed AC comes from ("leather armor"): wearing armor blocks Mage Armor. */
  armor?: string;
}

export interface Casting {
  ability: AbilityName;
  dc?: number;
  attack?: number;
  caster_level?: number;
  cantrip_tier?: number;
}

export interface SpellTurn {
  round: number;
  turn: string;
  slot_spent?: boolean;
  bonus_spell?: boolean;
  leveled_action_spell?: boolean;
}

export interface ConcentrationCheck {
  dc: number;
  damage: number;
}

export interface Combatant {
  id: string;
  kind: "creature" | "character";
  /** For a PC: the owning account. */
  owner?: string;
  /** The DM's label. */
  label: string;
  /** What players see once revealed. */
  public_name: string;
  visible: boolean;
  ever_revealed?: boolean;
  removed?: boolean;

  initiative?: number | null;
  initiative_bonus?: number;
  initiative_advantage?: boolean;
  tiebreak?: number;
  absent?: boolean;
  linked_to?: string;
  surprised?: boolean;
  /** A controlled creature's owner (a summon, pet, familiar or companion of a PC or a creature). */
  owner_id?: string;
  /** Who gives its orders: the DM, or the player of a PC. */
  controller?: { kind: "dm" } | { kind: "player"; pc: string };
  /** A controlled creature acts right after its owner (default) or on its own initiative. */
  initiative_mode?: "after_owner" | "own";

  hp: Hp;
  hp_authority?: "engine" | "player";
  zero_hp?: "dies" | "death_saves";
  life?: Life;
  /** Null when the stat block prints none (a thin Aurora companion, D17 b). */
  ac: number | null;
  speed?: Speeds;
  defenses?: Defenses;
  exhaustion?: number;

  concentration?: { name: string };
  pending_concentration?: ConcentrationCheck[];

  economy?: Economy;
  abilities?: Ability[];
  legendary_resistance?: Pool;
  /** By slot level ("1".."9"). */
  spell_slots?: Record<string, Pool>;
  resources?: Record<string, Resource>;
  group?: string;
  notes?: string;
  /** 0.3.2, DM only. */
  stats?: CombatStats;
  casting?: Casting[];
  spell_turn?: SpellTurn;
}

export type Boundary = "start" | "end";

export type Duration =
  | { kind: "manual" }
  | { kind: "source_turns"; remaining: number; at: Boundary }
  | { kind: "target_turns"; remaining: number; at: Boundary };

export type Cover = "half" | "three_quarters";

export type Modifier =
  | { kind: "ac"; value: number }
  | { kind: "speed_add"; value: number }
  | { kind: "speed_multiply"; value: number }
  | { kind: "speed_halve" }
  | { kind: "speed_zero" }
  | { kind: "cover"; value: Cover }
  // 0.3.2
  | { kind: "ac_base"; value: { base: number; add_dex: boolean } }
  | { kind: "ac_min"; value: number }
  | { kind: "roll_dice"; value: { on: RollOn; ability?: AbilityName; term: DiceTerm } }
  | { kind: "roll_mode"; value: { on: ModeOn; ability?: AbilityName; mode: AdvDis } }
  | { kind: "defense"; value: { defense: DefenseKind; damage_type: DamageType } }
  | { kind: "max_hp"; value: number }
  | { kind: "speed_set_mode"; value: { mode: SpeedMode; ft: number } };

export interface RepeatSave {
  ability: AbilityName;
  dc: number;
  at: Boundary;
}

export interface EffectSpec {
  name: string;
  source?: string;
  target: string;
  detail?: string;
  duration?: Duration;
  charges?: number;
  concentration?: boolean;
  conditions?: Condition[];
  modifiers?: Modifier[];
  repeat_save?: RepeatSave;
  ends_on_defeat_of?: string[];
  follow_up?: EffectSpec;
  /** The spell it comes from (0.3.2). DM only. */
  spell?: SpellTag;
  /** Its target is dismissed when it ends (a creature bound to the caster's concentration). */
  dismiss_on_end?: boolean;
  /** It ends when its target takes damage. */
  ends_on_damage?: boolean;
  /** It ends when its target attacks or casts (Invisibility): the "It attacked: end" reminder, never automatic. */
  ends_on_attack?: boolean;
}

/** `derive::effective_ac` (0.3.2 breakdown): "15 = Mage Armor 13 + DEX 2". */
export interface AcBreakdown {
  base: number | null;
  effects: number;
  cover: number;
  total: number | null;
  base_effect?: { label: string; value: number };
  min?: number;
  parts?: { label: string; value: number }[];
  text: string;
}

export interface Effect extends EffectSpec {
  id: number;
  origin?: "command" | "dying";
}

/** A legendary pool (C16): one per legendary creature; several members only when the DM linked them. */
export interface LegendaryPool {
  max: number;
  remaining: number;
  members: string[];
  /** The stat block printed no count: `max` is the default 3, a guess the DM can change. */
  guessed?: boolean;
}

export type RuleTarget = { kind: "combatant"; id: string } | { kind: "group"; id: string };

export interface RuleFlag {
  target: RuleTarget;
  ability: string;
  while_active: boolean;
}

export interface EncounterRule {
  id: string;
  name: string;
  text_active?: string;
  text_ended?: string;
  linked?: string;
  active: boolean;
  flags?: RuleFlag[];
}

export interface GroupLimit {
  per_round: number;
  used: number;
}

export interface Settings {
  show_enemy_turn: boolean;
  auto_recharge: boolean;
}

export interface CombatState {
  edition: Edition;
  settings?: Settings;
  status: "setup" | "active" | "ended";
  round: number;
  current?: string;
  last_visible?: { id: string; round: number };
  combatants: Combatant[];
  effects: Effect[];
  legendary_pools: Record<string, LegendaryPool>;
  /** Pools are per creature (false only in a combat 0.3.0 saved; the server brings it forward on load). */
  legendary_per_creature?: boolean;
  rules: EncounterRule[];
  group_limits: Record<string, GroupLimit>;
  next_effect?: number;
  next_tiebreak?: number;
  /** Casts waiting for rolls (0.3.2). DM only. */
  casts?: PendingCast[];
  next_cast?: number;
}

// --- Spells (0.3.2): party_combat::spell ---

/** A number or dice expression over slot, slot_above, mod and cantrip_tier ("8d6+(slot_above)d6"). */
export type Formula = string;
export type Tier = "auto" | "partial" | "utility" | "ruling" | "uncurated";
export type CastTime = "action" | "bonus" | "reaction" | "minute";
export type TargetFilter =
  | { kind: "any" } | { kind: "willing" } | { kind: "ally" } | { kind: "enemy" }
  | { kind: "type"; types: string[] };
export type AreaShape = "sphere" | "cube" | "cone" | "line" | "cylinder" | "emanation";
export type Targeting =
  | { kind: "self" }
  | { kind: "creatures"; max: Formula; filter?: TargetFilter; include_self?: boolean }
  | { kind: "area"; shape: AreaShape; size_ft: number };
export type SaveSuccess = "half" | "none";
export type Delivery =
  | { kind: "auto" }
  | { kind: "attack"; attack: "melee" | "ranged"; rays?: Formula }
  | { kind: "save"; ability: AbilityName; success: SaveSuccess };
export type RollOn = "attacks" | "saves" | "checks";
export type ModeOn = "attacks_by" | "attacks_against" | "saves" | "checks";
export type AdvDis = "advantage" | "disadvantage";
export interface DiceTerm { count: number; sides: number; sign?: "plus" | "minus" }
export type DefenseKind = "resist" | "immune" | "vuln";
export type SpeedMode = "walk" | "fly" | "swim" | "climb" | "burrow";
export type SpeedChange =
  | { op: "add"; ft: number } | { op: "mult"; n: number } | { op: "halve" }
  | { op: "set_mode"; mode: SpeedMode; ft: number } | { op: "zero" };
export interface ZoneSpec {
  save?: { ability: AbilityName; success: SaveSuccess };
  damage?: Formula;
  damage_type?: DamageType;
  choice?: DamageType[];
  conditions?: Condition[];
}
/** A summon's options: info for Add creature only, nothing spawns in 0.3.2. */
export interface SummonInfo {
  creature_types?: string[];
  max_cr?: string;
  by_cr?: { cr: string; count: number }[];
  per_slot?: { slot: number; times: number }[];
  initiative?: "after_owner" | "own_group";
}
export type RulingAction =
  | { kind: "log" }
  | { kind: "cancel_target_cast"; refund: boolean }
  | { kind: "roll_check"; dc?: Formula }
  | { kind: "ask_save"; ability: AbilityName };
export interface RulingButton { label: string; action?: RulingAction }
export type SpellDuration =
  | { kind: "instant" }
  | { kind: "source_turns"; n: number; at: Boundary }
  | { kind: "target_turns"; n: number; at: Boundary }
  | { kind: "minutes"; n: number }
  | { kind: "long" }
  | { kind: "until_dispelled" };
export type Primitive =
  | { kind: "damage"; dice: Formula; damage_type?: DamageType; choice?: DamageType[] }
  | { kind: "heal"; dice: Formula; add_mod?: boolean }
  | { kind: "temp_hp"; dice: Formula; add_mod?: boolean }
  | { kind: "condition"; condition: Condition; repeat_save?: { ability: AbilityName; at: Boundary }; ends_on_damage?: boolean; ends_on_attack?: boolean }
  | { kind: "ac_base"; base: number; add_dex?: boolean }
  | { kind: "ac_bonus"; n: number }
  | { kind: "ac_min"; n: number }
  | { kind: "roll_dice"; on: RollOn; ability?: AbilityName; term: DiceTerm }
  | { kind: "roll_mode"; on: ModeOn; ability?: AbilityName; mode: AdvDis }
  | { kind: "defense"; defense: DefenseKind; types: DamageType[] }
  | { kind: "max_hp"; n: Formula }
  | { kind: "speed"; change: SpeedChange }
  | { kind: "zone"; zone: ZoneSpec }
  | { kind: "summon"; info: SummonInfo }
  | { kind: "follow_up"; effect: { name: string; duration?: SpellDuration; apply?: Primitive[] } }
  | { kind: "end_effects"; max_level: Formula }
  | { kind: "ruling"; question: string; buttons: RulingButton[] }
  | { kind: "note"; text: string };
/** One spell in one edition (closed, v 1). */
export interface SpellMech {
  v: 1;
  key: string;
  edition: Edition;
  name: string;
  rev?: number;
  level: number;
  concentration?: boolean;
  cast: CastTime;
  targeting: Targeting;
  delivery: Delivery;
  on_land?: Primitive[];
  on_resist?: Primitive[];
  always?: Primitive[];
  duration?: SpellDuration;
  tier: Tier;
  notes?: string[];
}
export type MechOrigin =
  | { kind: "srd"; key: string; rev: number }
  | { kind: "def"; id: string; rev: number }
  | { kind: "statblock" }
  | { kind: "ruling" };
export interface CastChoices { damage_type?: DamageType }
/** A SpellMech compiled on the server for one caster; embedded in cast_spell. */
export interface CastSpec {
  v: 1;
  origin: MechOrigin;
  mech: SpellMech;
  dc?: number;
  attack?: number;
  mod?: number;
  cantrip_tier?: number;
  baked_in?: boolean;
}
export interface SpellLink {
  key?: string;
  level: number;
  frequency?: "slot" | "at_will" | "per_day";
  fixed_slot?: number;
  baked_in?: boolean;
}
export interface FormulaVars { slot: number; slot_above: number; mod: number; cantrip_tier: number }
export interface SpellTag {
  key: string;
  level: number;
  slot?: number;
  cast?: string;
  dc?: number;
  zone?: ZoneSpec;
  vars?: FormulaVars;
  baked_in?: boolean;
}
export type RowOutcome = "failed" | "saved" | "hit" | "crit" | "miss" | "lands";
export interface CastRow {
  target: string;
  ray?: number;
  outcome?: RowOutcome;
  roll?: D20Result;
  used_lr?: boolean;
}
export interface PendingCast {
  id: string;
  caster: string;
  ability?: string;
  spec: CastSpec;
  slot?: number;
  vars?: FormulaVars;
  choices?: CastChoices;
  rows: CastRow[];
  round?: number;
  zone_of?: number;
  statblock?: boolean;
}
/** Typed first (D21); "app" rolls with the save bonus, Bless dice and advantage the engine derives. */
export type RowRoll =
  | { kind: "typed"; total: number; natural?: number }
  | { kind: "app" }
  | { kind: "outcome"; outcome: RowOutcome };
export interface RowInput { target: string; ray?: number; roll: RowRoll; use_lr?: boolean }
/** An amount (damage, healing, temp HP), in the order of the definition's amount primitives. */
export type AmountRoll = { kind: "typed"; total: number } | { kind: "app" };
export type StandardActionKind = "dodge" | "dash" | "disengage" | "help";

export interface DamagePart {
  amount: DiceInput;
  damage_type?: DamageType;
}

/** Every change to a combat (`party_combat::Command`, tagged by `command`). */
export type Command =
  | { command: "add_combatant"; combatant: Combatant; roll_initiative?: boolean }
  | { command: "remove_combatant"; id: string }
  | { command: "rename"; id: string; label: string }
  | { command: "set_public_name"; id: string; name: string }
  | { command: "reveal"; id: string }
  | { command: "hide"; id: string }
  | { command: "set_absent"; id: string; absent: boolean }
  | { command: "set_notes"; id: string; notes: string }
  | { command: "set_settings"; settings: Settings }
  | { command: "roll_initiative"; ids?: string[]; only_missing?: boolean }
  | { command: "set_initiative"; id: string; roll: D20Input }
  | { command: "clear_initiative"; id: string }
  | { command: "swap_tie"; a: string; b: string }
  | { command: "link"; id: string; to?: string }
  | { command: "start" }
  | { command: "next_turn" }
  | { command: "previous_turn" }
  | { command: "end" }
  | { command: "set_economy"; id: string; action?: boolean; bonus_action?: boolean; reaction?: boolean }
  | { command: "damage"; target: string; parts: DamagePart[]; magical?: boolean; apply_qualified?: boolean; critical_hit?: boolean }
  | { command: "heal"; target: string; amount: DiceInput }
  | { command: "set_hp"; id: string; current: number; max?: number; temp?: number }
  | { command: "grant_temp_hp"; id: string; amount: number }
  | { command: "death_save"; id: string; roll: D20Input }
  | { command: "stabilize"; id: string }
  | { command: "set_death_saves"; id: string; successes: number; failures: number }
  | { command: "add_condition"; effect: EffectSpec }
  | { command: "remove_condition"; target: string; condition: Condition }
  | { command: "set_exhaustion"; id: string; level: number }
  | { command: "set_concentration"; id: string; name: string }
  | { command: "end_concentration"; id: string }
  | { command: "resolve_concentration"; id: string; roll: D20Input }
  | { command: "add_effect"; effect: EffectSpec }
  | { command: "remove_effect"; effect: number; skip_follow_up?: boolean }
  | { command: "use_charge"; effect: number }
  | { command: "resolve_effect_save"; effect: number; roll: D20Input }
  | { command: "use_ability"; id: string; ability: string; slot?: number }
  | { command: "roll_recharge"; id: string; ability: string; roll: DiceInput }
  | { command: "set_recharge"; id: string; ability: string; ready: boolean }
  | { command: "set_legendary_pool"; pool: string; value: LegendaryPool }
  | { command: "adjust_legendary"; pool: string; delta: number }
  | { command: "adjust_legendary_resistance"; id: string; delta: number }
  | { command: "adjust_slot"; id: string; level: number; delta: number }
  | { command: "set_slots"; id: string; level: number; pool: Pool }
  | { command: "adjust_resource"; id: string; key: string; delta: number }
  | { command: "set_rule"; rule: EncounterRule }
  | { command: "set_rule_active"; id: string; active: boolean }
  | { command: "remove_rule"; id: string }
  | { command: "set_group_limit"; group: string; per_round: number }
  | ({ command: "summon"; owner: string; initiative?: D20Input; visible?: boolean }
      & ({ combatant: Combatant } | { copyOf: string } | { companion: string } | { elementId: string }))
  | { command: "dismiss"; id: string }
  | { command: "set_initiative_mode"; id: string; mode: "after_owner" | "own"; roll?: D20Input }
  // 0.3.2: spells and stat block actions
  | { command: "cast_spell"; caster: string; ability?: string; spec?: CastSpec; slot?: number; targets?: string[];
      rows?: RowInput[]; amounts?: AmountRoll[]; crit_amounts?: AmountRoll[]; choices?: CastChoices;
      precast?: boolean; force?: boolean; id?: string }
  | { command: "resolve_cast_row"; cast: string; target: string; ray?: number; roll: RowRoll; use_lr?: boolean }
  | { command: "apply_cast"; cast: string; amounts?: AmountRoll[]; crit_amounts?: AmountRoll[]; force?: boolean }
  | { command: "cancel_cast"; cast: string; refund?: boolean }
  | { command: "apply_zone"; effect: number; targets: string[]; rows?: RowInput[]; amounts?: AmountRoll[]; id?: string }
  | { command: "perform_statblock"; id: string; ability: string; spec?: CastSpec; targets?: string[];
      rows?: RowInput[]; amounts?: AmountRoll[]; crit_amounts?: AmountRoll[]; force?: boolean; cast_id?: string; as_reaction?: boolean }
  | { command: "standard_action"; id: string; action: StandardActionKind; target?: string; bonus?: boolean }
  | { command: "bind_to_concentration"; id: string };

export type CommandName = Command["command"];

export interface ClauseNote {
  kind: "immunity" | "resistance" | "vulnerability";
  text: string;
  scope: "always" | "nonmagical" | "qualified";
  applied: boolean;
}

export interface DamageResult {
  parts: { damage_type?: DamageType; roll: DiceResult; amount: number; immune: boolean; resisted: boolean; vulnerable: boolean; clauses?: ClauseNote[] }[];
  total: number;
}

/** A DM-only log line (`DmEvent`): names hidden creatures, so it never reaches players. */
export interface DmEvent {
  round: number;
  subject?: string;
  event: string;
  text: string;
  roll?: D20Result | DiceResult;
  result?: DamageResult;
  prompted?: boolean;
  dc?: number;
  ready?: boolean;
  was_seen?: boolean;
  /*
   * 0.3.2 events: cast_started {cast, spell, slot?, pending, notes?}, cast_row {cast, target, outcome, roll?},
   * cast_applied {cast, amounts}, cast_cancelled {cast, refunded}, summon_options {caster, spell, slot?, info,
   * concentration}, ruling_needed {cast, question, buttons}, standard_action {action}.
   */
  [key: string]: unknown;
}

// --- The stat block snapshot (upstream's `MonsterData`, the fields the tracker reads) ---

export type Usage =
  | { recharge: { min: number; max: number } }
  | { per_day: number }
  | { per_day_in_lair: { normal: number; lair: number } }
  | { on_rest: "short_or_long" | "long" }
  | string;

export interface MonsterAttack {
  kind: string;
  to_hit: number;
  reach?: number;
  range?: number;
  long_range?: number;
  damage?: { average: number; dice?: string; type: DamageType }[];
}

export interface MonsterSpellList {
  frequency: "at_will" | { per_day: { uses: number; each: boolean } };
  spells: { name: string; id?: string; note?: string }[];
}

export interface MonsterFeature {
  name: string;
  text: string;
  usage?: Usage;
  cost?: number;
  condition?: string;
  attack?: MonsterAttack;
  save?: { ability: AbilityName; dc: number; damage?: unknown[]; half_on_success?: boolean };
  spellcasting?: { ability?: AbilityName; save_dc?: number; attack_bonus?: number; lists: MonsterSpellList[] };
}

export interface StatBlock {
  ruleset?: string;
  size?: string[];
  creature_type?: unknown;
  alignment?: string;
  ac?: { value: number; source?: string; condition?: string }[];
  hp?: { average: number; formula?: string };
  speed?: Partial<Speeds>;
  initiative?: { modifier: number; score: number };
  abilities?: Record<AbilityName, number>;
  saves?: Partial<Record<AbilityName, number>>;
  skills?: Record<string, number>;
  damage_vulnerabilities?: DamageClause[];
  damage_resistances?: DamageClause[];
  damage_immunities?: DamageClause[];
  condition_immunities?: string[];
  senses?: unknown;
  languages?: string[];
  cr?: unknown;
  proficiency_bonus?: number;
  traits?: MonsterFeature[];
  actions?: MonsterFeature[];
  bonus_actions?: MonsterFeature[];
  reactions?: MonsterFeature[];
  legendary?: { per_round?: number; preamble?: string; actions: MonsterFeature[] };
  lair_actions?: { preamble?: string; options?: unknown[] };
}

/** A stat block with the monster's name (the tracker's fixtures and the mock server). */
export interface StatBlockSnapshot {
  name: string;
  data: StatBlock;
}

/** A damage or heal prompt waiting for a player (D7 a). */
export interface PlayerPrompt {
  id: string;
  combatantId: string;
  kind: "damage" | "heal";
  amount: number;
  parts?: { amount: number; damage_type?: string }[];
  criticalHit?: boolean;
  at?: number;
}

/** What the combat server sends the DM for one combat (party/docs/combat-api.md, "The DM view"). */
export interface DmCombat {
  id: string;
  partyId: string;
  encounter: { id: string; name: string; publicName?: string } | null;
  /** The write counter (CAS): sent back as `expectSeq` where a write must not land on a newer state. */
  seq: number;
  status?: "setup" | "active" | "ended";
  state: CombatState;
  /** Per creature: the stat block it fights with (overrides applied). PCs have none. */
  creatures: Record<string, {
    encounterCreatureId: string | null;
    statBlock: StatBlock;
    /** The element's name (DM only; null for a typed summon). */
    name?: string | null;
    /** Where it came from: an encounter, a copy, an element of the DM's content, a PC's companion, or a typed summon. */
    origin?: "encounter" | "copy" | "element" | "companion" | "summon" | null;
  }>;
  /** Per PC: its owner and character; `member: false` once it left the party mid-fight. */
  characters?: Record<string, { ownerId: string; characterId: string; member: boolean; online?: boolean }>;
  /** Open damage and heal prompts. */
  prompts?: PlayerPrompt[];
  /** Each PC's sheet companions not yet in the fight, for Summon. */
  offers?: CompanionOffer[];
  /** What undo would revert, newest first (the engine's labels). */
  undo: string[];
  startedAt?: number;
  endedAt?: number | null;
  /** A retried requestId: the write had already been done. */
  replayed?: boolean;
}

/** A companion on a PC's character sheet that can join the fight (`summon {owner, companion: key}`). */
export interface CompanionOffer {
  pc: string;
  key: string;
  name: string;
  hp: number;
  ac: number;
  initiative: number;
  /** The companion element it is, if any. */
  elementId?: string | null;
}

/** One row of the DM log (`GET /api/combats/{id}/log`). */
export interface LogEntry {
  seq: number;
  at: number;
  author: "dm" | "player" | "system";
  event: DmEvent;
}
