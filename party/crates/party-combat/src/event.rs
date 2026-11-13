//! What a command did: DM events (the DM-only log) and player events (only what the projection may show).

use crate::damage::DamageResult;
use crate::model::{CombatantId, DamageType, EffectId};
use crate::visibility::TurnPointer;
use party_dice::{D20Result, DiceResult};
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
        /// The roll.
        roll: D20Result,
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
        /// The roll.
        roll: D20Result,
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
        /// The roll.
        roll: D20Result,
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
    /// The PC's turn started ("Your turn" and a vibration).
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
