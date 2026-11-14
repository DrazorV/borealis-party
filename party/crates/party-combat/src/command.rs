//! Commands: every change to a combat is one [`Command`], issued by an [`Actor`].

use crate::model::{
    CastId, Combatant, CombatantId, DamageType, EffectId, EffectSpec, EncounterRule, LegendaryPool,
    Pool, RowOutcome, Settings,
};
use crate::spell::{CastChoices, CastSpec};
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

    // --- Spells and stat block actions (0.3.2) ---
    /// Casts a spell: checks the slot, the economy and the edition rules (`force` overrides the economy, the
    /// edition rules, target filters and the armor check, never a missing slot), spends them, starts concentration
    /// and opens a [`crate::PendingCast`] with a row per target. When every row and every amount is given (typed
    /// first, D21, or `app`), it resolves and applies at once: one undo step. A summon spell's options come back as
    /// a `summon_options` DM event; nothing spawns.
    CastSpell {
        /// Who casts.
        caster: CombatantId,
        /// The stat block entry it uses (its `mech` is the spec when `spec` is absent; its uses are spent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ability: Option<String>,
        /// The compiled definition, embedded by the server (`None`: the ability's `mech`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spec: Option<Box<CastSpec>>,
        /// The slot level spent (`None`: a cantrip, at will or per day).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<u8>,
        /// The targets, in order (a target listed twice takes two rays). Empty for a self spell.
        #[serde(default)]
        targets: Vec<CombatantId>,
        /// Rolls or outcomes for the rows, if known now.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rows: Vec<RowInput>,
        /// The amounts (damage, healing, temp HP) in the order of the definition's amount primitives, if known now.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        amounts: Vec<AmountRoll>,
        /// Critical-hit amounts for crit rows (typed); `app` or absent rolls the doubled dice in the app.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        crit_amounts: Vec<AmountRoll>,
        /// Choices (a damage type).
        #[serde(default)]
        choices: CastChoices,
        /// Cast before the fight (a precast starting effect): no economy or edition rules, the slot is spent.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        precast: bool,
        /// The DM's one-tap override.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
        /// The cast's id (the server's random id); the engine numbers it when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<CastId>,
    },
    /// Resolves one row of a pending cast: a typed save or attack (first), a roll in the app (save bonus, Bless,
    /// advantage and automatic failure applied), or an outcome. `use_lr` spends Legendary Resistance to turn a
    /// failed save into a success. A player may resolve their own PC's row.
    ResolveCastRow {
        /// The cast.
        cast: CastId,
        /// The target.
        target: CombatantId,
        /// The ray (0 unless the target takes several).
        #[serde(default)]
        ray: u32,
        /// The roll or outcome.
        roll: RowRoll,
        /// Spend Legendary Resistance on a failure.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        use_lr: bool,
    },
    /// Applies a pending cast: one amount roll for everyone, halved for savers, critical dice for crit rows,
    /// through damage (resistances, concentration checks), healing and the effects. Unresolved rows are refused
    /// unless `force` (they are skipped).
    ApplyCast {
        /// The cast.
        cast: CastId,
        /// The amounts, as in [`Command::CastSpell`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        amounts: Vec<AmountRoll>,
        /// Critical amounts.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        crit_amounts: Vec<AmountRoll>,
        /// Skip unresolved rows.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    /// Drops a pending cast (countered, or a mistake). With `refund` its slot and per-day use come back; the
    /// economy and concentration stay as they are.
    CancelCast {
        /// The cast.
        cast: CastId,
        /// Give the slot back.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        refund: bool,
    },
    /// Applies a spell effect's zone (its [Apply to…] button) to the creatures the DM taps: the zone's save and
    /// damage, as a pending cast that spends nothing. With every roll given it applies at once.
    ApplyZone {
        /// The effect carrying the zone.
        effect: EffectId,
        /// Who is in it.
        targets: Vec<CombatantId>,
        /// Rolls or outcomes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rows: Vec<RowInput>,
        /// The damage.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        amounts: Vec<AmountRoll>,
        /// The pending cast's id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<CastId>,
    },
    /// Uses a stat block attack or save ability (its `mech`, or `spec`) through the cast pipeline: spends its
    /// economy, recharge, uses, legendary actions and group limit, then rows and amounts as a cast.
    PerformStatblock {
        /// Who.
        id: CombatantId,
        /// The ability key.
        ability: String,
        /// The definition, when the ability has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spec: Option<Box<CastSpec>>,
        /// The targets.
        #[serde(default)]
        targets: Vec<CombatantId>,
        /// Rolls or outcomes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rows: Vec<RowInput>,
        /// The amounts.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        amounts: Vec<AmountRoll>,
        /// Critical amounts.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        crit_amounts: Vec<AmountRoll>,
        /// The DM's override (target filters).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
        /// The pending cast's id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cast_id: Option<CastId>,
    },
    /// Dodge, Dash, Disengage or Help: spends the action (or the bonus action) and adds the marker effect.
    StandardAction {
        /// Who.
        id: CombatantId,
        /// Which.
        action: StandardActionKind,
        /// Help's ally.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<CombatantId>,
        /// Spend the bonus action instead (Cunning Action and similar).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        bonus: bool,
    },
    /// Binds a controlled creature to its owner's current concentration: it is dismissed when that concentration
    /// ends (a conjured beast added with Add creature). Undo brings it back.
    BindToConcentration {
        /// The controlled creature.
        id: CombatantId,
    },
}

/// A row's roll: typed first (D21), in the app, or an outcome the DM sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RowRoll {
    /// The total read off physical dice, everything included (Bless's d4 too).
    Typed {
        /// The total.
        total: i32,
        /// The natural d20 (needed for a natural 20 or 1 on attacks).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        natural: Option<u32>,
    },
    /// Rolled in the app: the save bonus (or attack bonus), roll dice and advantage from the effects.
    App,
    /// The DM's verdict without a roll.
    Outcome {
        /// The outcome.
        outcome: RowOutcome,
    },
}

/// A row's input inside a cast command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowInput {
    /// The target.
    pub target: CombatantId,
    /// The ray.
    #[serde(default)]
    pub ray: u32,
    /// The roll.
    pub roll: RowRoll,
    /// Spend Legendary Resistance on a failure.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_lr: bool,
}

/// An amount: typed (D21 first) or rolled in the app from the definition's formula at the cast's slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AmountRoll {
    /// The total read off physical dice (modifier included).
    Typed {
        /// The total.
        total: i32,
    },
    /// Rolled in the app.
    App,
}

/// A standard action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandardActionKind {
    /// Attacks against it have disadvantage and it has advantage on DEX saves, until its next turn starts.
    Dodge,
    /// Extra movement this turn.
    Dash,
    /// No opportunity attacks this turn.
    Disengage,
    /// An ally's next check or attack has advantage.
    Help,
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
                | Command::ResolveCastRow { .. }
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
            Command::CastSpell { .. } => "cast spell",
            Command::ResolveCastRow { .. } => "cast roll",
            Command::ApplyCast { .. } => "apply cast",
            Command::CancelCast { .. } => "cancel cast",
            Command::ApplyZone { .. } => "apply zone",
            Command::PerformStatblock { .. } => "stat block action",
            Command::StandardAction { .. } => "standard action",
            Command::BindToConcentration { .. } => "bind to concentration",
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
