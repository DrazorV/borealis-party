//! What each kind of viewer may see: the visible turn order and the turn pointer (D5). Part D's projection
//! (`view_for`) builds on these; it must not read hidden combatants any other way.

use crate::derive::{conditions, turn_order};
use crate::model::{CombatState, CombatantId, Condition, HealthBand, Life, Status};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The DM, or a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerKind {
    /// Sees everything.
    Dm,
    /// Sees PCs and revealed creatures only.
    Player,
}

/// Whose turn it is, for a viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnPointer {
    /// Not started.
    NotStarted,
    /// Ended.
    Ended,
    /// This combatant's turn, in this round.
    Combatant {
        /// Whose turn.
        id: CombatantId,
        /// The round of that turn.
        round: u32,
    },
    /// Hidden creatures are acting and the encounter shows an "Enemy turn" slot (D5 opt-in).
    EnemyTurn {
        /// The current round.
        round: u32,
    },
    /// Hidden creatures act before any visible combatant has (players see no pointer yet).
    Waiting {
        /// The round players last saw.
        round: u32,
    },
}

/// The combatant is visible to the viewer: the DM sees everyone not removed; players see PCs and revealed
/// creatures that aren't removed.
#[must_use]
pub fn is_visible(state: &CombatState, id: &CombatantId, viewer: ViewerKind) -> bool {
    state
        .combatant(id)
        .is_some_and(|c| !c.removed && (viewer == ViewerKind::Dm || c.visible || c.is_character()))
}

/// The turn order as the viewer may see it. For players, hidden creatures don't exist.
#[must_use]
pub fn visible_order(state: &CombatState, viewer: ViewerKind) -> Vec<CombatantId> {
    turn_order(state)
        .into_iter()
        .filter(|id| is_visible(state, id, viewer))
        .collect()
}

/// Whose turn it is for the viewer. For players the pointer stays on the last visible combatant while hidden
/// creatures act (D5), and its round is the round of that turn, so players can't count hidden turns.
#[must_use]
pub fn turn_pointer(state: &CombatState, viewer: ViewerKind) -> TurnPointer {
    match state.status {
        Status::Setup => return TurnPointer::NotStarted,
        Status::Ended => return TurnPointer::Ended,
        Status::Active => {}
    }
    let Some(current) = &state.current else {
        return TurnPointer::NotStarted;
    };
    if is_visible(state, current, viewer) {
        return TurnPointer::Combatant {
            id: current.clone(),
            round: state.round,
        };
    }
    if state.settings.show_enemy_turn {
        return TurnPointer::EnemyTurn { round: state.round };
    }
    match &state.last_visible {
        Some(mark) if is_visible(state, &mark.id, viewer) => TurnPointer::Combatant {
            id: mark.id.clone(),
            round: mark.round,
        },
        Some(mark) => TurnPointer::Waiting { round: mark.round },
        None => TurnPointer::Waiting { round: 1 },
    }
}

/// What players can see of one combatant, coarsely, to decide whether a change is player-relevant.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    name: String,
    band: Option<HealthBand>,
    life: Life,
    conditions: Vec<Condition>,
    exhaustion: u8,
    concentration: Option<String>,
    pending_concentration: usize,
}

/// A coarse summary of everything players may see. Two states with the same summary are the same to players, so
/// a command whose before and after summaries match produces no player events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlayerSummary {
    status: Status,
    pointer: TurnPointer,
    order: Vec<CombatantId>,
    seen: BTreeMap<CombatantId, Seen>,
}

impl PlayerSummary {
    pub(crate) fn of(state: &CombatState) -> Self {
        let seen = state
            .combatants
            .iter()
            .filter(|c| is_visible(state, &c.id, ViewerKind::Player))
            .map(|c| {
                let pc = c.is_character();
                let seen = Seen {
                    name: c.public_name.clone(),
                    band: c.hp.band(),
                    life: c.life,
                    conditions: conditions(state, &c.id),
                    exhaustion: c.exhaustion,
                    concentration: if pc {
                        c.concentration.as_ref().map(|k| k.name.clone())
                    } else {
                        None
                    },
                    pending_concentration: if pc { c.pending_concentration.len() } else { 0 },
                };
                (c.id.clone(), seen)
            })
            .collect();
        Self {
            status: state.status,
            pointer: turn_pointer(state, ViewerKind::Player),
            order: visible_order(state, ViewerKind::Player),
            seen,
        }
    }

    /// The player events that turn `self` into `after`.
    pub(crate) fn diff(&self, after: &Self) -> Vec<crate::event::PlayerEvent> {
        use crate::event::PlayerEvent as P;
        let mut out = Vec::new();
        if self.status != after.status {
            match after.status {
                Status::Active => out.push(P::Started),
                Status::Ended => out.push(P::Ended),
                Status::Setup => {}
            }
        }
        for id in self.seen.keys().filter(|id| !after.seen.contains_key(*id)) {
            out.push(P::Disappeared { id: id.clone() });
        }
        for (id, seen) in &after.seen {
            match self.seen.get(id) {
                None => out.push(P::Appeared { id: id.clone() }),
                Some(before) if before != seen => out.push(P::Changed { id: id.clone() }),
                Some(_) => {}
            }
        }
        if self.order != after.order {
            out.push(P::Order);
        }
        if self.pointer != after.pointer {
            out.push(P::Turn {
                pointer: after.pointer.clone(),
            });
        }
        out
    }
}
