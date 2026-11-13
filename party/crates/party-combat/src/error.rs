//! Why a command was refused. A refused command leaves the state unchanged.

use crate::model::{CombatantId, EffectId};
use party_dice::DiceError;
use serde::{Deserialize, Serialize};

/// Why an ability can't be used now (C14). The messages are the ones the DM card shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "reason", content = "detail", rename_all = "snake_case")]
pub enum Unavailable {
    /// Dead or removed.
    #[error("Defeated")]
    Defeated,
    /// At 0 HP.
    #[error("Down")]
    Down,
    /// Incapacitated (or a condition that includes it).
    #[error("Incapacitated")]
    Incapacitated,
    /// A trait.
    #[error("Passive")]
    Passive,
    /// The action is spent.
    #[error("Action spent")]
    ActionSpent,
    /// The bonus action is spent.
    #[error("Bonus action spent")]
    BonusActionSpent,
    /// The reaction is spent.
    #[error("Reaction spent")]
    ReactionSpent,
    /// Waiting for a recharge.
    #[error("Needs recharge")]
    NeedsRecharge,
    /// No uses left.
    #[error("No uses left: {0}")]
    NoUses(String),
    /// No slot of that level left.
    #[error("No level {0} slot left")]
    NoSlot(u8),
    /// The group's per-round limit is used.
    #[error("Group limit used this round: {0}")]
    GroupLimit(String),
    /// An encounter rule switches it off.
    #[error("Switched off by {0}")]
    Rule(String),
    /// Needs an effect that isn't active.
    #[error("Requires {0} active")]
    Requires(String),
    /// Not in a legendary pool.
    #[error("No legendary actions")]
    NoLegendaryPool,
    /// Not enough legendary actions.
    #[error("Not enough legendary actions")]
    NotEnoughLegendary,
    /// Legendary actions are taken on other creatures' turns.
    #[error("Not on its own turn")]
    OwnTurn,
}

/// Why a command was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CombatError {
    /// No such combatant (or it was removed).
    #[error("No combatant {0}.")]
    UnknownCombatant(CombatantId),
    /// The id is taken.
    #[error("Combatant {0} already exists.")]
    DuplicateCombatant(CombatantId),
    /// No such effect.
    #[error("No effect {}.", .0 .0)]
    UnknownEffect(EffectId),
    /// No such ability on the combatant.
    #[error("No ability {0}.")]
    UnknownAbility(String),
    /// No such legendary pool.
    #[error("No legendary pool {0}.")]
    UnknownPool(String),
    /// No such resource.
    #[error("No resource {0}.")]
    UnknownResource(String),
    /// No such rule.
    #[error("No rule {0}.")]
    UnknownRule(String),
    /// A player issued a DM command, or a command for someone else's PC.
    #[error("Players can't do that.")]
    NotAllowed,
    /// The combat isn't running.
    #[error("The combat isn't running.")]
    NotActive,
    /// The combat already started.
    #[error("The combat already started.")]
    AlreadyStarted,
    /// The combat ended.
    #[error("The combat has ended.")]
    Ended,
    /// Nobody has an initiative.
    #[error("Nobody has an initiative yet.")]
    NoInitiative,
    /// The ability can't be used now.
    #[error("Unavailable: {0}")]
    Unavailable(Unavailable),
    /// A roll failed.
    #[error(transparent)]
    Dice(#[from] DiceError),
    /// A value out of range, or a command that makes no sense for this combatant.
    #[error("{0}")]
    Invalid(String),
    /// Not dying.
    #[error("Not making death saves.")]
    NotDying,
    /// Healing can't bring back the dead (set HP to revive).
    #[error("Dead: set HP to revive.")]
    Dead,
    /// No concentration check is waiting.
    #[error("No concentration check is waiting.")]
    NoPendingCheck,
    /// The effect has no repeated save.
    #[error("The effect has no repeated save.")]
    NoSave,
    /// Nothing to undo.
    #[error("Nothing to undo.")]
    NothingToUndo,
    /// The last change was a player's: undo it only with `force`.
    #[error("The last change was a player's; undo it anyway?")]
    UndoWouldRevertPlayer,
    /// An export couldn't be read.
    #[error("Can't import: {0}")]
    Import(String),
}

pub(crate) fn invalid(msg: impl Into<String>) -> CombatError {
    CombatError::Invalid(msg.into())
}
