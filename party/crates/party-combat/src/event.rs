//! What a command did: DM events (the DM-only log) and player events (only what the projection may show).

use crate::command::StandardActionKind;
use crate::damage::DamageResult;
use crate::model::{AbilityName, CastId, CombatantId, DamageType, EffectId, RowOutcome};
use crate::spell::{RulingButton, SummonInfo};
use crate::visibility::TurnPointer;
use party_dice::{D20Result, DiceResult, RollSource};
use serde::{Deserialize, Serialize};

/// What happened, for the DM. Every DM event goes to the DM-only log (D32). It may name hidden creatures, so it
/// must never reach a player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmEvent {
    /// The round it happened in.
    pub round: u32,
    /// The combatant it is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<CombatantId>,
    /// What happened.
    #[serde(flatten)]
    pub kind: DmEventKind,
    /// One line for the log ("Goblin 2 takes 7 slashing (resisted) · 0/7").
    pub text: String,
    /// The player's own business (0.3.3, the user's rule of 2026-11-14: "DM should never know players HP and
    /// resources"): a number of a player character or of a creature a player controls, or a player's cast with
    /// nothing the DM sees or resolves. Kept in the log, never shown to the DM
    /// ([`crate::visibility::dm_may_see`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub private: bool,
}

/// The kinds of [`DmEvent`], with the data a DM card shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DmEventKind {
    /// Added to the roster.
    Added,
    /// Removed from the roster.
    Removed,
    /// Renamed, notes or public name changed, settings changed.
    Edited,
    /// Revealed to players.
    Revealed,
    /// Hidden from players; `was_seen` warns that online players already saw it.
    Hidden {
        /// It had been visible.
        was_seen: bool,
    },
    /// An initiative was rolled or typed.
    Initiative {
        /// The roll.
        roll: D20Result,
    },
    /// An initiative was changed without a roll (cleared, swapped, linked, absent).
    OrderChanged,
    /// The combat started.
    Started,
    /// A new round started.
    RoundStarted,
    /// A turn started.
    TurnStarted,
    /// A 2014 surprised creature's first turn: no actions.
    Surprised,
    /// The combat ended.
    Ended,
    /// Action, bonus action or reaction changed.
    Economy,
    /// Damage was computed (and applied, for creatures).
    Damage {
        /// The breakdown, clauses included.
        result: DamageResult,
        /// For a PC: a prompt was sent to the player instead of changing HP (D7).
        prompted: bool,
    },
    /// Healing.
    Healed {
        /// HP regained.
        amount: i32,
        /// For a PC: a prompt was sent instead.
        prompted: bool,
    },
    /// HP set exactly or synced.
    HpSet,
    /// Temporary HP granted.
    TempHp,
    /// Dropped to 0 HP and is dying.
    Dying,
    /// Died.
    Died,
    /// Back above 0 HP.
    Revived,
    /// A death save was made.
    DeathSave {
        /// The roll; none for an outcome pressed at the table (0.3.5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roll: Option<D20Result>,
        /// The outcome pressed at the table, when there was no number (0.3.5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<party_dice::TableOutcome>,
    },
    /// Became stable.
    Stabilized,
    /// A concentration save is needed.
    ConcentrationCheck {
        /// The DC.
        dc: u32,
    },
    /// A concentration save was made.
    ConcentrationSave {
        /// The roll; none for an outcome pressed at the table (0.3.5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roll: Option<D20Result>,
        /// The outcome pressed at the table, when there was no number (0.3.5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<party_dice::TableOutcome>,
        /// Kept concentrating.
        kept: bool,
    },
    /// Concentration started.
    ConcentrationStarted,
    /// Concentration ended.
    ConcentrationEnded {
        /// On what.
        name: String,
    },
    /// An effect or condition was added.
    EffectAdded {
        /// Which.
        effect: EffectId,
    },
    /// An effect or condition ended.
    EffectEnded {
        /// Which.
        effect: EffectId,
        /// Why ("duration expired", "concentration ended", "dispelled").
        reason: String,
    },
    /// A charge was used.
    ChargeUsed {
        /// Which effect.
        effect: EffectId,
        /// Charges left.
        remaining: u32,
    },
    /// A repeated save is due.
    SaveDue {
        /// Which effect.
        effect: EffectId,
    },
    /// A repeated save was made.
    EffectSave {
        /// Which effect.
        effect: EffectId,
        /// The roll; none for an outcome pressed at the table (0.3.5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roll: Option<D20Result>,
        /// The outcome pressed at the table, when there was no number (0.3.5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<party_dice::TableOutcome>,
        /// It succeeded.
        success: bool,
    },
    /// The condition was not added: the target is immune.
    ConditionImmune,
    /// Exhaustion changed.
    Exhaustion {
        /// The new level.
        level: u8,
    },
    /// An ability was used.
    AbilityUsed {
        /// The ability key.
        ability: String,
    },
    /// A recharge d6 was rolled or typed.
    Recharge {
        /// The ability key.
        ability: String,
        /// The d6.
        roll: DiceResult,
        /// It is ready now.
        ready: bool,
    },
    /// A recharge is due and the DM types the d6 (auto recharge off).
    RechargeDue {
        /// The ability key.
        ability: String,
    },
    /// A recharge was set by hand.
    RechargeSet,
    /// A legendary pool refilled.
    LegendaryReset {
        /// The pool id.
        pool: String,
    },
    /// Legendary actions, Legendary Resistance, slots or resources changed.
    Resources,
    /// A mount's link to its rider ended (the rider went down).
    Unlinked {
        /// The former leader.
        from: CombatantId,
    },
    /// An encounter rule ended or was switched.
    Rule {
        /// The rule id.
        rule: String,
        /// Active now.
        active: bool,
    },
    /// A roll was made with [`crate::Command::Roll`].
    Rolled {
        /// What it was.
        label: String,
        /// The result.
        result: RollOutcome,
    },
    /// An attack roll against the target's effective AC.
    Attack {
        /// The target.
        target: CombatantId,
        /// The roll.
        roll: D20Result,
        /// The target's effective AC (`None` when unknown).
        target_ac: Option<i32>,
        /// Hit (`None` when the AC is unknown: the DM decides).
        hit: Option<bool>,
        /// A critical hit.
        critical: bool,
    },
    /// HP rolled from a formula.
    HpRolled {
        /// The roll.
        roll: DiceResult,
    },
    /// A controlled creature was added.
    Summoned {
        /// Its owner.
        owner: CombatantId,
    },
    /// A controlled creature was dismissed.
    Dismissed,
    /// A controlled creature's owner went down. The DM chooses: keep it after the owner, dismiss it, or give it its
    /// own initiative. Nothing changes automatically. DM only.
    OwnerDefeated {
        /// The owner.
        owner: CombatantId,
    },
    /// A spell was cast (or a stat block action used, or a zone applied): its rows are open until resolved.
    CastStarted {
        /// The cast.
        cast: CastId,
        /// The spell or action name.
        spell: String,
        /// The slot spent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<u8>,
        /// It waits for rolls (otherwise it applied at once).
        pending: bool,
        /// The definition's reminders.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        notes: Vec<String>,
    },
    /// One row of a cast was resolved.
    CastRow {
        /// The cast.
        cast: CastId,
        /// The target.
        target: CombatantId,
        /// The outcome.
        outcome: RowOutcome,
        /// The roll, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roll: Option<D20Result>,
    },
    /// A cast's amounts and effects were applied.
    CastApplied {
        /// The cast.
        cast: CastId,
        /// The amounts rolled or typed, in order.
        amounts: Vec<DiceResult>,
    },
    /// A pending cast was dropped.
    CastCancelled {
        /// The cast.
        cast: CastId,
        /// Its slot came back.
        refunded: bool,
    },
    /// A summon or conjure spell was cast: the options for the DM's Add creature dialog (nothing spawns).
    SummonOptions {
        /// The caster (the "Controlled by" preset).
        caster: CombatantId,
        /// The spell.
        spell: String,
        /// The slot.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<u8>,
        /// The options.
        info: SummonInfo,
        /// The caster concentrates on it: offer to bind the creatures (default on).
        concentration: bool,
    },
    /// A ruling row: the DM taps one of the buttons.
    RulingNeeded {
        /// The cast.
        cast: CastId,
        /// The question.
        question: String,
        /// The buttons.
        buttons: Vec<RulingButton>,
    },
    /// Dodge, Dash, Disengage or Help.
    StandardAction {
        /// Which.
        action: StandardActionKind,
    },
    /// The last change was undone.
    Undone {
        /// The undone command's label.
        label: String,
    },
}

/// What players may learn. Player events carry ids of visible combatants and PCs only, and never a creature's
/// numbers: the projection (part D) decides what each player sees, and these say only what to refetch or prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PlayerEvent {
    /// The combat started.
    Started,
    /// The combat ended.
    Ended,
    /// The visible turn pointer moved.
    Turn {
        /// Where it is now.
        pointer: TurnPointer,
    },
    /// The visible turn order changed (an initiative, a link, an absence).
    Order,
    /// A creature appeared to players.
    Appeared {
        /// Who.
        id: CombatantId,
    },
    /// A creature disappeared from players' view (hidden or removed).
    Disappeared {
        /// Who.
        id: CombatantId,
    },
    /// Something players can see about a visible combatant changed (band, conditions, name, life).
    Changed {
        /// Who.
        id: CombatantId,
    },
    /// "Take 7 slashing?": the owner applies it on their own Play tab (D7).
    DamagePrompt {
        /// The PC.
        id: CombatantId,
        /// The owner's account.
        owner: String,
        /// The total after immunity, resistance and vulnerability.
        amount: i32,
        /// Each part.
        parts: Vec<PromptPart>,
        /// A critical hit.
        critical_hit: bool,
    },
    /// "Regain 8 HP?" (healing, or a natural 20 on a death save).
    HealPrompt {
        /// The PC.
        id: CombatantId,
        /// The owner's account.
        owner: String,
        /// HP to regain.
        amount: i32,
    },
    /// A concentration save is needed.
    ConcentrationCheck {
        /// The PC.
        id: CombatantId,
        /// The owner's account.
        owner: String,
        /// The DC.
        dc: u32,
    },
    /// The PC's turn started while it is dying: roll a death save.
    DeathSaveDue {
        /// The PC.
        id: CombatantId,
        /// The owner's account.
        owner: String,
    },
    /// The result of a roll for the player's own PC or creature (in the app or typed): initiative, a death save, a
    /// save, an attack, a recharge, HP or any other roll. Sent to that player only.
    RollResult {
        /// The PC or creature.
        id: CombatantId,
        /// The player's account.
        owner: String,
        /// What it was ("Initiative", "Death save", "Longsword").
        label: String,
        /// The total.
        total: i32,
        /// The natural d20, for d20 tests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        natural: Option<u32>,
        /// Rolled in the app or typed.
        source: RollSource,
        /// `d20 [14] +5 = 19`.
        breakdown: String,
        /// For an attack: hit or miss (`None` when unknown).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hit: Option<bool>,
        /// A critical hit.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        critical: bool,
    },
    /// A save against a spell or a stat block action: "DEX save DC 15 from an unseen creature". Sent to the PC's
    /// player only; the player rolls at the table and types it (or the DM does), with `resolve_cast_row`.
    SaveRequest {
        /// The PC (or its creature).
        id: CombatantId,
        /// The player's account.
        owner: String,
        /// The cast to answer.
        cast: CastId,
        /// The ray.
        #[serde(default)]
        ray: u32,
        /// The save.
        ability: AbilityName,
        /// The DC, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dc: Option<u32>,
        /// The spell's name; `None` when the caster is hidden.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spell: Option<String>,
        /// The caster; `None` when hidden ("an unseen creature").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<CombatantId>,
    },
    /// The turn of the player's PC, or of a creature it controls, started ("Your turn" and a vibration).
    YourTurn {
        /// The PC.
        id: CombatantId,
        /// The owner's account.
        owner: String,
    },
}

/// One part of a damage prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptPart {
    /// The damage.
    pub amount: i32,
    /// Its type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage_type: Option<DamageType>,
}

/// Everything a command produced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Events {
    /// For the DM (and the DM-only log).
    pub dm: Vec<DmEvent>,
    /// For players.
    pub players: Vec<PlayerEvent>,
}

/// The result of [`crate::Command::Roll`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollOutcome {
    /// A d20 test.
    D20(D20Result),
    /// A dice expression.
    Dice(DiceResult),
}
