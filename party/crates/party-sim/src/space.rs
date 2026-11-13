//! Space as bands: every combatant stands at a whole number of 30-foot steps on one line (the party on the plus
//! side, the foes on the minus side, the middle at 0). Two combatants at the same step are within 5 feet: enemies
//! there are engaged. This is an abstraction (no grid, terrain, flanking or real area shapes) and the report says
//! so.

use std::collections::BTreeMap;

use party_combat::CombatantId;
use serde::{Deserialize, Serialize};

use crate::setup::{Band, Side};

/// Feet per step.
pub const STEP_FT: u32 = 30;

/// Where everyone is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub pos: BTreeMap<CombatantId, i32>,
    pub side: BTreeMap<CombatantId, Side>,
    /// Left the fight (fled past the far edge).
    pub withdrawn: Vec<CombatantId>,
}

/// How a move is asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Move {
    /// Move next to this enemy (engage).
    Engage(CombatantId),
    /// One step from the nearest enemy.
    Near,
    /// Two steps from the nearest enemy.
    Far,
    /// As far back as movement allows (leaves the fight past three steps).
    Back,
    Hold,
}

impl Move {
    /// From the wire form: `engage:cN`, `near`, `far`, `back`, `hold`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "near" => Move::Near,
            "far" => Move::Far,
            "back" => Move::Back,
            "hold" | "" => Move::Hold,
            t => Move::Engage(CombatantId::new(t.strip_prefix("engage:")?)),
        })
    }
}

impl Space {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn place(&mut self, id: &CombatantId, side: Side, band: Band) {
        let p = band.steps();
        let p = if side == Side::Party { p } else { -p };
        self.pos.insert(id.clone(), p);
        self.side.insert(id.clone(), side);
    }

    #[must_use]
    pub fn at(&self, id: &CombatantId) -> i32 {
        self.pos.get(id).copied().unwrap_or(0)
    }

    #[must_use]
    pub fn side_of(&self, id: &CombatantId) -> Side {
        self.side.get(id).copied().unwrap_or(Side::Foes)
    }

    #[must_use]
    pub fn present(&self, id: &CombatantId) -> bool {
        !self.withdrawn.contains(id)
    }

    /// Distance in steps.
    #[must_use]
    pub fn steps(&self, a: &CombatantId, b: &CombatantId) -> i32 {
        (self.at(a) - self.at(b)).abs()
    }

    /// Distance in feet (5 when at the same step).
    #[must_use]
    pub fn feet(&self, a: &CombatantId, b: &CombatantId) -> u32 {
        match self.steps(a, b) {
            0 => 5,
            n => u32::try_from(n).unwrap_or(0) * STEP_FT,
        }
    }

    /// Enemies at the same step, among `alive`.
    #[must_use]
    pub fn engaged_with(&self, id: &CombatantId, alive: &[CombatantId]) -> Vec<CombatantId> {
        let side = self.side_of(id);
        alive
            .iter()
            .filter(|o| {
                *o != id && self.present(o) && self.side_of(o) != side && self.steps(id, o) == 0
            })
            .cloned()
            .collect()
    }

    /// The nearest enemy among `alive` (ties: listed order).
    #[must_use]
    pub fn nearest_enemy(&self, id: &CombatantId, alive: &[CombatantId]) -> Option<CombatantId> {
        let side = self.side_of(id);
        alive
            .iter()
            .filter(|o| self.present(o) && self.side_of(o) != side)
            .min_by_key(|o| self.steps(id, o))
            .cloned()
    }

    /// The band a combatant shows: its distance to the nearest enemy.
    #[must_use]
    pub fn band(&self, id: &CombatantId, alive: &[CombatantId]) -> Band {
        self.nearest_enemy(id, alive)
            .map_or(Band::Distant, |e| Band::of(self.steps(id, &e)))
    }

    /// Where `id` ends up after `mv` with `budget` steps, and whether it leaves the fight.
    #[must_use]
    pub fn target_pos(
        &self,
        id: &CombatantId,
        mv: &Move,
        budget: i32,
        alive: &[CombatantId],
    ) -> (i32, bool) {
        let me = self.at(id);
        let away = if self.side_of(id) == Side::Party {
            1
        } else {
            -1
        };
        let toward = |goal: i32| -> i32 {
            if (goal - me).abs() <= budget {
                goal
            } else {
                me + (goal - me).signum() * budget
            }
        };
        let near_goal = |gap: i32| -> i32 {
            match self.nearest_enemy(id, alive) {
                Some(e) => {
                    let ep = self.at(&e);
                    // Stand `gap` steps from it on our own side.
                    toward(ep + away * gap)
                }
                None => me,
            }
        };
        match mv {
            Move::Hold => (me, false),
            Move::Engage(t) => (toward(self.at(t)), false),
            Move::Near => (near_goal(1), false),
            Move::Far => (near_goal(2), false),
            Move::Back => {
                let p = me + away * budget;
                let leaves = self
                    .nearest_enemy(id, alive)
                    .is_none_or(|e| (p - self.at(&e)).abs() > 3);
                (p, leaves)
            }
        }
    }

    pub fn set(&mut self, id: &CombatantId, p: i32) {
        self.pos.insert(id.clone(), p);
    }

    pub fn withdraw(&mut self, id: &CombatantId) {
        if !self.withdrawn.contains(id) {
            self.withdrawn.push(id.clone());
        }
    }
}

/// Steps a speed allows in one move.
#[must_use]
pub fn budget(speed_ft: u32) -> i32 {
    i32::try_from(speed_ft / STEP_FT).unwrap_or(1).max(1)
}

/// The assumptions the band model makes, for the report.
#[must_use]
pub fn assumptions() -> Vec<&'static str> {
    vec![
        "Space is modelled as 30-foot steps on one line; combatants on the same step are within 5 feet. No grid, terrain, cover, flanking or chokepoints.",
        "Movement is speed divided by 30 feet in whole steps (at least one); Dash doubles it.",
        "Leaving a step with an engaged enemy without Disengage provokes one opportunity attack from each such enemy that has its reaction and a parsed melee attack.",
        "Reach longer than 5 feet is treated as ordinary melee.",
        "Areas catch at most a fixed number of targets on one step (cone: size/5, sphere or radius: size/5, line: size/20, at most 8).",
        "Ranged attacks are checked against step distance; beyond normal range they have disadvantage.",
        "Opportunity attacks and death saves are rolled automatically.",
        "A prone combatant stands up at the start of its turn for half its movement.",
        "Damage is not magical unless the stat block says so; resistances to nonmagical damage apply.",
    ]
}
