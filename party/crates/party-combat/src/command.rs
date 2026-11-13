//! Commands: every change to a combat is one [`Command`], issued by an [`Actor`].

use crate::model::{
    Combatant, CombatantId, DamageType, EffectId, EffectSpec, EncounterRule, LegendaryPool, Pool,
    Settings,
};
use party_dice::{D20Input, DiceInput};
use serde::{Deserialize, Serialize};

/// Who issues a command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    /// The party's DM.
    Dm,
    /// A player, by account id: only for their own PC and the creatures it controls (see
    /// [`crate::engine::player_may`]).
    Player {
        /// The account.
        account: String,
    },
    /// The server itself (HP sync from the player's character file).
    System,
}

/// One part of a hit: an amount and its damage type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DamagePart {
    /// Typed (D43) or rolled.
    pub amount: DiceInput,
    /// The damage type; `None` for untyped damage (never resisted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage_type: Option<DamageType>,
}

impl DamagePart {
    /// A typed amount of one type.
    #[must_use]
    pub fn typed(amount: i32, damage_type: Option<DamageType>) -> Self {
        Self {
            amount: DiceInput::typed(amount),
            damage_type,
        }
    }
}

/// Damage the player's character took, reported by the server after the player's own `take_damage` (D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DamageTaken {
    /// The damage after immunity, resistance and vulnerability, temp HP included.
    pub amount: i32,
    /// From a critical hit (two death-save failures at 0 HP).
    #[serde(default)]
    pub critical_hit: bool,
}

/// Every change to a combat. Commands are plain data, so the server can journal and replay them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    // --- Roster (C1, C7) ---
    /// Adds a combatant, before or during the fight. With `roll_initiative`, a creature rolls d20 + its modifier.
    AddCombatant {
        /// The combatant.
        combatant: Box<Combatant>,
        /// Roll its initiative now.
        #[serde(default)]
        roll_initiative: bool,
    },
    /// Removes a combatant (kept as a tombstone; undo restores it).
    RemoveCombatant {
        /// Who.
        id: CombatantId,
    },
    /// Changes the DM's label.
    Rename {
        /// Who.
        id: CombatantId,
        /// The new label.
        label: String,
    },
    /// Changes the name players see.
    SetPublicName {
        /// Who.
        id: CombatantId,
        /// The new name.
        name: String,
    },
    /// Shows a creature to players.
    Reveal {
        /// Who.
        id: CombatantId,
    },
    /// Hides a creature from players again.
    Hide {
        /// Who.
        id: CombatantId,
    },
    /// Marks a PC as not in this fight, or back in (C1).
    SetAbsent {
        /// Who.
        id: CombatantId,
        /// Absent.
        absent: bool,
    },
    /// Sets the DM's notes.
    SetNotes {
        /// Who.
        id: CombatantId,
        /// The notes.
        notes: String,
    },
    /// Changes the settings.
    SetSettings {
        /// The new settings.
        settings: Settings,
    },

    // --- Initiative and turns (C2–C4) ---
    /// Rolls initiative for creatures: the listed ones, or every creature when `ids` is empty. With
    /// `only_missing`, combatants that already have one keep it. PCs are rolled only when listed.
    RollInitiative {
        /// Who; empty for every creature.
        #[serde(default)]
        ids: Vec<CombatantId>,
        /// Keep existing values.
        #[serde(default)]
        only_missing: bool,
    },
    /// Sets one combatant's initiative: a typed total (D43) or a roll. Players may set their own PC's.
    SetInitiative {
        /// Who.
        id: CombatantId,
        /// Typed or rolled.
        roll: D20Input,
    },
    /// Clears an initiative (a PC with none is not in this fight).
    ClearInitiative {
        /// Who.
        id: CombatantId,
    },
    /// Swaps two tied combatants (same initiative).
    SwapTie {
        /// One.
        a: CombatantId,
        /// The other.
        b: CombatantId,
    },
    /// Links a combatant to act right after another (C3), or unlinks it (`to: None`).
    Link {
        /// The follower (a mount).
        id: CombatantId,
        /// The leader (its rider).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<CombatantId>,
    },
    /// Starts round 1 with the first combatant in the order.
    Start,
    /// Ends the current turn and starts the next one (C4).
    NextTurn,
    /// Steps back one turn without undoing anything (undo reverts changes).
    PreviousTurn,
    /// Ends the combat.
    End,

    // --- Action economy (C5) ---
    /// Marks the action, bonus action or reaction as spent or ready.
    SetEconomy {
        /// Who.
        id: CombatantId,
        /// Action spent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<bool>,
        /// Bonus action spent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bonus_action: Option<bool>,
        /// Reaction spent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reaction: Option<bool>,
    },

    // --- HP (C6) ---
    /// Damage: immunity, resistance, then vulnerability per part, then temp HP, then HP. For a PC the result is a
    /// prompt to the player instead (D7).
    Damage {
        /// Who.
        target: CombatantId,
        /// The parts of the hit.
        parts: Vec<DamagePart>,
        /// From a magical source (clauses "from nonmagical attacks" don't apply).
        #[serde(default)]
        magical: bool,
        /// Apply clauses with other qualifier text (the DM has read them).
        #[serde(default)]
        apply_qualified: bool,
        /// A critical hit (two death-save failures at 0 HP).
        #[serde(default)]
        critical_hit: bool,
    },
    /// Healing, up to max HP. For a PC, a prompt to the player.
    Heal {
        /// Who.
        target: CombatantId,
        /// Typed or rolled.
        amount: DiceInput,
    },
    /// Sets HP exactly (and optionally max and temp HP). Revives a dead creature when above 0.
    SetHp {
        /// Who.
        id: CombatantId,
        /// Current HP.
        current: i32,
        /// Max HP.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<i32>,
        /// Temp HP.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        temp: Option<i32>,
    },
    /// Grants temporary HP (they don't stack: the higher value is kept).
    GrantTempHp {
        /// Who.
        id: CombatantId,
        /// The amount.
        amount: i32,
    },
    /// The server reports a PC's HP after the player's own write (D7), with the damage taken if any.
    SyncPlayerHp {
        /// Who.
        id: CombatantId,
        /// Current HP.
        current: i32,
        /// Max HP.
        max: i32,
        /// Temp HP.
        #[serde(default)]
        temp: i32,
        /// The damage that caused the change, for concentration and death saves.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        damage: Option<DamageTaken>,
    },

    // --- Death saves (D18) ---
    /// A death save: typed (D43) or rolled. Players may make their own PC's.
    DeathSave {
        /// Who.
        id: CombatantId,
        /// The roll. A typed total without its natural die counts 20 as a natural 20 and 1 as a natural 1.
        roll: D20Input,
    },
    /// Stabilizes a dying creature.
    Stabilize {
        /// Who.
        id: CombatantId,
    },
    /// Sets death-save counts exactly (DM correction).
    SetDeathSaves {
        /// Who.
        id: CombatantId,
        /// Successes (0..=2).
        successes: u8,
        /// Failures (0..=2).
        failures: u8,
    },

    // --- Conditions (C10) ---
    /// Adds a condition as an effect. Incapacitating conditions end the target's concentration.
    AddCondition {
        /// The effect carrying the condition; its name defaults to the condition's.
        effect: Box<EffectSpec>,
    },
    /// Removes a condition from every effect on the target that gives it.
    RemoveCondition {
        /// Who.
        target: CombatantId,
        /// The condition.
        condition: crate::model::Condition,
    },
    /// Sets exhaustion (0..=6; 6 is death).
    SetExhaustion {
        /// Who.
        id: CombatantId,
        /// The level.
        level: u8,
    },

    // --- Concentration (C11) ---
    /// Starts concentrating (ending any earlier concentration and its effects).
    SetConcentration {
        /// Who.
        id: CombatantId,
        /// On what.
        name: String,
    },
    /// Ends concentration and every effect it holds.
    EndConcentration {
        /// Who.
        id: CombatantId,
    },
    /// Resolves the oldest pending concentration check: a typed total (D43) or a rolled CON save.
    ResolveConcentration {
        /// Who.
        id: CombatantId,
        /// The save.
        roll: D20Input,
    },

    // --- Effects (C12, C13) ---
    /// Adds an active effect.
    AddEffect {
        /// The effect.
        effect: Box<EffectSpec>,
    },
    /// Removes an effect (dispelled or ended by the DM).
    RemoveEffect {
        /// Which.
        effect: EffectId,
        /// Don't add its follow-up effect.
        #[serde(default)]
        skip_follow_up: bool,
    },
    /// Spends one charge of an effect; the last one ends it.
    UseCharge {
        /// Which.
        effect: EffectId,
    },
    /// Resolves an effect's repeated save: a typed total or a roll. A success ends it.
    ResolveEffectSave {
        /// Which.
        effect: EffectId,
        /// The save.
        roll: D20Input,
    },

    // --- Abilities and resources (C14–C18, C21) ---
    /// Uses an ability: checks and spends the economy, recharge, uses, legendary actions, group limit and slot.
    UseAbility {
        /// Who.
        id: CombatantId,
        /// The ability key.
        ability: String,
        /// The spell slot level spent, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<u8>,
    },
    /// Rolls or types the d6 for a recharge (C15).
    RollRecharge {
        /// Who.
        id: CombatantId,
        /// The ability key.
        ability: String,
        /// Typed or rolled (`1d6`).
        roll: DiceInput,
    },
    /// Marks a recharge ready or spent.
    SetRecharge {
        /// Who.
        id: CombatantId,
        /// The ability key.
        ability: String,
        /// Ready.
        ready: bool,
    },
    /// Creates or replaces a legendary pool and its members (C16). Each legendary creature has its own pool; a
    /// pool with several members is the DM's link (one multi-headed creature), opt-in only. Its members leave
    /// the pools they were in, and a pool left empty goes.
    SetLegendaryPool {
        /// The pool id.
        pool: String,
        /// The pool.
        value: LegendaryPool,
    },
    /// Spends or restores legendary actions directly.
    AdjustLegendary {
        /// The pool id.
        pool: String,
        /// Negative spends, positive restores.
        delta: i32,
    },
    /// Spends (negative) or restores (positive) Legendary Resistance (C17).
    AdjustLegendaryResistance {
        /// Who.
        id: CombatantId,
        /// The change.
        delta: i32,
    },
    /// Changes a spell slot level's remaining slots (C18).
    AdjustSlot {
        /// Who.
        id: CombatantId,
        /// The slot level.
        level: u8,
        /// The change.
        delta: i32,
    },
    /// Sets a spell slot level's pool.
    SetSlots {
        /// Who.
        id: CombatantId,
        /// The slot level.
        level: u8,
        /// The pool.
        pool: Pool,
    },
    /// Changes a resource's remaining uses.
    AdjustResource {
        /// Who.
        id: CombatantId,
        /// The resource key.
        key: String,
        /// The change.
        delta: i32,
    },

    // --- Encounter rules (C20, C21) ---
    /// Adds or replaces an encounter rule (by id).
    SetRule {
        /// The rule.
        rule: EncounterRule,
    },
    /// Switches a rule on or off by hand.
    SetRuleActive {
        /// The rule id.
        id: String,
        /// Active.
        active: bool,
    },
    /// Removes a rule.
    RemoveRule {
        /// The rule id.
        id: String,
    },
    /// Sets a group's per-round limit.
    SetGroupLimit {
        /// The group tag.
        group: String,
        /// Uses per round; 0 removes the limit.
        per_round: u32,
    },

    // --- Rolls (D21: in the app or typed, for everyone) ---
    /// A roll to log and show: an ability check, a save, a skill, anything without a rule of its own. In the app or
    /// typed. Players may roll for their own PC and the creatures it controls.
    Roll {
        /// Who rolls.
        id: CombatantId,
        /// What it is ("Dexterity save", "Perception").
        label: String,
        /// A d20 test or a dice expression.
        roll: RollInput,
    },
    /// An attack roll against a target's effective AC: a natural 20 always hits, a natural 1 always misses. Damage
    /// follows as its own `damage` command. Players may attack with their own PC or its creatures, at a target
    /// they can see.
    Attack {
        /// Who attacks.
        attacker: CombatantId,
        /// The target.
        target: CombatantId,
        /// What it is ("Longsword").
        #[serde(default)]
        label: String,
        /// The attack roll.
        roll: D20Input,
    },
    /// Rolls (or types) a creature's HP from its formula (D22): max and current HP become the result (at least 1).
    RollHp {
        /// Who.
        id: CombatantId,
        /// The formula in the app, or a typed total.
        roll: DiceInput,
    },

    // --- Controlled creatures: summons, pets, companions ---
    /// Adds a creature controlled by `owner`. A PC's creature is controlled by that PC's player and is always
    /// visible; any other owner's is the DM's, and is visible only if `visible` says so (when absent: the owner's
    /// current visibility). With `initiative` it acts on its own roll; without, right after its owner.
    Summon {
        /// The owner: a PC, a creature, or another controlled creature.
        owner: CombatantId,
        /// The creature, built from a stat block snapshot (a Monster or a Companion element) or typed by hand.
        combatant: Box<Combatant>,
        /// Its own initiative (in the app or typed). Absent: it acts right after its owner.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initiative: Option<D20Input>,
        /// For a DM creature: visible to players. Absent: as its owner is now.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        visible: Option<bool>,
    },
    /// Dismisses a controlled creature (kept as a tombstone; undo brings it back).
    Dismiss {
        /// Who.
        id: CombatantId,
    },
    /// Switches a controlled creature between "right after its owner" and its own initiative (with a roll in the
    /// app or typed, when it has none yet).
    SetInitiativeMode {
        /// Who.
        id: CombatantId,
        /// The mode.
        mode: crate::model::InitiativeMode,
        /// An initiative for `own`, if it has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roll: Option<D20Input>,
    },
}

impl Command {
    /// Command kinds a player may ever issue. Whether a given one is allowed also depends on whose combatant it is
    /// about: see [`crate::engine::player_may`].
    #[must_use]
    pub fn player_allowed(&self) -> bool {
        matches!(
            self,
            Command::SetInitiative { .. }
                | Command::DeathSave { .. }
                | Command::SetConcentration { .. }
                | Command::EndConcentration { .. }
                | Command::ResolveConcentration { .. }
                | Command::ResolveEffectSave { .. }
                | Command::Roll { .. }
                | Command::Attack { .. }
                | Command::Summon { .. }
                | Command::Dismiss { .. }
                | Command::SetInitiativeMode { .. }
                | Command::Damage { .. }
                | Command::Heal { .. }
                | Command::SetHp { .. }
                | Command::GrantTempHp { .. }
                | Command::RollHp { .. }
                | Command::AddCondition { .. }
                | Command::RemoveCondition { .. }
                | Command::SetExhaustion { .. }
                | Command::AddEffect { .. }
                | Command::RemoveEffect { .. }
                | Command::UseCharge { .. }
                | Command::UseAbility { .. }
                | Command::SetEconomy { .. }
                | Command::RollRecharge { .. }
                | Command::SetRecharge { .. }
                | Command::AdjustSlot { .. }
                | Command::AdjustResource { .. }
        )
    }

    /// A short label for the undo list.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Command::AddCombatant { .. } => "add combatant",
            Command::RemoveCombatant { .. } => "remove combatant",
            Command::Rename { .. } => "rename",
            Command::SetPublicName { .. } => "public name",
            Command::Reveal { .. } => "reveal",
            Command::Hide { .. } => "hide",
            Command::SetAbsent { .. } => "absent",
            Command::SetNotes { .. } => "notes",
            Command::SetSettings { .. } => "settings",
            Command::RollInitiative { .. } => "roll initiative",
            Command::SetInitiative { .. } => "set initiative",
            Command::ClearInitiative { .. } => "clear initiative",
            Command::SwapTie { .. } => "swap tie",
            Command::Link { .. } => "link",
            Command::Start => "start",
            Command::NextTurn => "next turn",
            Command::PreviousTurn => "previous turn",
            Command::End => "end",
            Command::SetEconomy { .. } => "economy",
            Command::Damage { .. } => "damage",
            Command::Heal { .. } => "heal",
            Command::SetHp { .. } => "set HP",
            Command::GrantTempHp { .. } => "temp HP",
            Command::SyncPlayerHp { .. } => "player HP",
            Command::DeathSave { .. } => "death save",
            Command::Stabilize { .. } => "stabilize",
            Command::SetDeathSaves { .. } => "set death saves",
            Command::AddCondition { .. } => "add condition",
            Command::RemoveCondition { .. } => "remove condition",
            Command::SetExhaustion { .. } => "exhaustion",
            Command::SetConcentration { .. } => "concentration",
            Command::EndConcentration { .. } => "end concentration",
            Command::ResolveConcentration { .. } => "concentration save",
            Command::AddEffect { .. } => "add effect",
            Command::RemoveEffect { .. } => "remove effect",
            Command::UseCharge { .. } => "use charge",
            Command::ResolveEffectSave { .. } => "effect save",
            Command::UseAbility { .. } => "use ability",
            Command::RollRecharge { .. } => "recharge",
            Command::SetRecharge { .. } => "set recharge",
            Command::SetLegendaryPool { .. } => "legendary pool",
            Command::AdjustLegendary { .. } => "legendary actions",
            Command::AdjustLegendaryResistance { .. } => "legendary resistance",
            Command::AdjustSlot { .. } => "spell slot",
            Command::SetSlots { .. } => "spell slots",
            Command::AdjustResource { .. } => "resource",
            Command::SetRule { .. } => "rule",
            Command::SetRuleActive { .. } => "rule on/off",
            Command::RemoveRule { .. } => "remove rule",
            Command::SetGroupLimit { .. } => "group limit",
            Command::Roll { .. } => "roll",
            Command::Attack { .. } => "attack",
            Command::RollHp { .. } => "roll HP",
            Command::Summon { .. } => "summon",
            Command::Dismiss { .. } => "dismiss",
            Command::SetInitiativeMode { .. } => "initiative mode",
        }
    }
}

/// A roll for [`Command::Roll`]: a d20 test or a dice expression, each in the app or typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollInput {
    /// `{"d20": {"kind": "app", "modifier": 4}}`.
    D20(D20Input),
    /// `{"dice": {"kind": "typed", "total": 9}}`.
    Dice(DiceInput),
}
