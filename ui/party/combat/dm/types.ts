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
  ac: number;
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
  | { kind: "cover"; value: Cover };

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
}

export interface Effect extends EffectSpec {
  id: number;
  origin?: "command" | "dying";
}

export interface LegendaryPool {
  max: number;
  remaining: number;
  members: string[];
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
  rules: EncounterRule[];
  group_limits: Record<string, GroupLimit>;
  next_effect?: number;
  next_tiebreak?: number;
}

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
  | { command: "set_initiative_mode"; id: string; mode: "after_owner" | "own"; roll?: D20Input };

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
  creatures: Record<string, { encounterCreatureId: string | null; statBlock: StatBlock; name?: string }>;
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
}

/** One row of the DM log (`GET /api/combats/{id}/log`). */
export interface LogEntry {
  seq: number;
  at: number;
  author: "dm" | "player" | "system";
  event: DmEvent;
}
