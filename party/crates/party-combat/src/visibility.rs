//! What each kind of viewer may see: the visible turn order and the turn pointer (D5). Part D's projection
//! (`view_for`) builds on these; it must not read hidden combatants any other way.

use crate::derive::{conditions, turn_order};
use crate::model::{CombatState, CombatantId, Controller, Status};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// The combatant is visible to the viewer. The DM sees everyone not removed. Players see PCs, creatures a player
/// controls (their summons and pets), and revealed creatures, if not removed. A DM creature's pet has its own
/// `visible` flag: it stays hidden while hidden, whatever its owner's state (revealing an owner reveals no pets).
#[must_use]
pub fn is_visible(state: &CombatState, id: &CombatantId, viewer: ViewerKind) -> bool {
    state.combatant(id).is_some_and(|c| {
        !c.removed
            && (viewer == ViewerKind::Dm
                || c.visible
                || c.is_character()
                || matches!(c.controller, Controller::Player { .. }))
    })
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

/// Everything players may see of one combatant, or `None` when it is hidden from them. A revealed creature shows
/// its full stat block (D20 b): exact HP, conditions, effects, concentration, resources, and its abilities' text,
/// legendary actions included. DM-only data is left out: legendary-action and Legendary Resistance counts (never
/// shown to players), the DM label, notes, tie-break, effect ids, follow-ups, and any reference to a hidden
/// combatant (an effect's hidden source and its duration, which counts that source's turns; a hidden owner or
/// link). Part D's projection may narrow it further per viewer (D6: other players' PCs show a health band only).
#[must_use]
pub fn player_view_of(state: &CombatState, id: &CombatantId) -> Option<Value> {
    let c = state.combatant(id)?;
    if !is_visible(state, id, ViewerKind::Player) {
        return None;
    }
    let seen = |other: &CombatantId| is_visible(state, other, ViewerKind::Player);
    let mut v = serde_json::to_value(c).ok()?;
    let o = v.as_object_mut()?;
    for dm_only in [
        "label",
        "notes",
        "tiebreak",
        "ever_revealed",
        "visible",
        "removed",
        // Legendary counts are DM only (user, 2026-11-13): a pool shared with a hidden creature would show its uses.
        "legendary_resistance",
    ] {
        o.remove(dm_only);
    }
    for (field, target) in [("owner_id", &c.owner_id), ("linked_to", &c.linked_to)] {
        if target.as_ref().is_some_and(|t| !seen(t)) {
            o.remove(field);
        }
    }
    if let Controller::Player { pc } = &c.controller {
        if !seen(pc) {
            o.remove("controller");
        }
    }
    let effects: Vec<Value> = state
        .effects
        .iter()
        .filter(|e| &e.spec.target == id)
        .map(|e| {
            let s = &e.spec;
            let source_seen = s.source.as_ref().is_none_or(seen);
            let mut fx = serde_json::json!({
                "name": s.name,
                "detail": s.detail,
                "concentration": s.concentration,
                "conditions": s.conditions,
                "modifiers": s.modifiers,
                "charges": s.charges,
                "repeat_save": s.repeat_save,
            });
            if source_seen {
                fx["source"] = serde_json::to_value(&s.source).unwrap_or(Value::Null);
                fx["duration"] = serde_json::to_value(s.duration).unwrap_or(Value::Null);
            }
            fx
        })
        .collect();
    o.insert("effects".into(), Value::Array(effects));
    o.insert(
        "conditions".into(),
        serde_json::to_value(conditions(state, id)).ok()?,
    );
    Some(v)
}

/// The open cast rows a player answers: saves their own PCs (and its creatures) make against pending casts, each
/// `{cast, target, ray, ability, dc, spell?, from?, resolved}`. The spell and caster are left out when the
/// caster is hidden ("DEX save DC 15 from an unseen creature"). Nothing else of a cast reaches players.
#[must_use]
pub fn player_casts(state: &CombatState, account: &str) -> Vec<Value> {
    let _ = (state, account);
    Vec::new()
}

/// A coarse summary of everything players may see. Two states with the same summary are the same to players, so
/// a command whose before and after summaries match produces no player events.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlayerSummary {
    status: Status,
    pointer: TurnPointer,
    order: Vec<CombatantId>,
    seen: BTreeMap<CombatantId, Value>,
}

impl PlayerSummary {
    pub(crate) fn of(state: &CombatState) -> Self {
        let seen = state
            .combatants
            .iter()
            .filter_map(|c| player_view_of(state, &c.id).map(|v| (c.id.clone(), v)))
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
