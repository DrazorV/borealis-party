//! A combat with its die, undo journal (C8) and DM-only event log (D32).

use crate::command::{Actor, Command};
use crate::engine::apply;
use crate::error::CombatError;
use crate::event::{DmEvent, DmEventKind, Events};
use crate::model::{CombatState, Edition};
use crate::visibility::PlayerSummary;
use party_dice::{DieRoller, SeededDie};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Undo keeps this many steps (C8).
pub const UNDO_LIMIT: usize = 50;

/// The server deletes an ended combat's event log this many days after it ended (D32). Enforced by the server.
pub const LOG_RETENTION_DAYS: u32 = 90;

/// Who wrote a log entry, without the account (the log is DM-only, but it is exported).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    /// The DM.
    Dm,
    /// A player.
    Player,
    /// The server.
    System,
}

impl From<&Actor> for Author {
    fn from(a: &Actor) -> Self {
        match a {
            Actor::Dm => Author::Dm,
            Actor::Player { .. } => Author::Player,
            Actor::System => Author::System,
        }
    }
}

/// One line of the DM-only log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// The write that produced it.
    pub seq: u64,
    /// Who.
    pub author: Author,
    /// What happened.
    pub event: DmEvent,
}

/// One undo step: the state before a write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// The write.
    pub seq: u64,
    /// Who made it.
    pub author: Author,
    /// The command's label.
    pub label: String,
    /// The state before it.
    pub before: CombatState,
}

/// A combat: the state plus everything around it. The server stores it whole; `seq` is the CAS counter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Combat {
    /// The state.
    pub state: CombatState,
    /// The server's die for creature rolls (D21). Not part of the state, so undo never replays a roll.
    pub die: SeededDie,
    /// Undo steps, oldest first.
    pub journal: VecDeque<JournalEntry>,
    /// The DM-only event log, oldest first.
    pub log: Vec<LogEntry>,
    /// Writes so far.
    pub seq: u64,
}

impl Combat {
    /// A new, empty combat. The server picks `seed` from the OS's random source.
    #[must_use]
    pub fn new(edition: Edition, seed: u32) -> Self {
        Self::from_state(CombatState::new(edition), seed)
    }

    /// A combat starting from `state` (an encounter's roster, or an import).
    #[must_use]
    pub fn from_state(state: CombatState, seed: u32) -> Self {
        Self {
            state,
            die: SeededDie::new(seed),
            journal: VecDeque::new(),
            log: Vec::new(),
            seq: 0,
        }
    }

    /// Applies a command with the combat's own die, journals the previous state and logs the DM events.
    pub fn execute(&mut self, actor: &Actor, cmd: &Command) -> Result<Events, CombatError> {
        let mut die = self.die;
        let out = self.execute_with(actor, cmd, &mut die)?;
        self.die = die;
        Ok(out)
    }

    /// [`Combat::execute`] with another die (tests, or a scripted replay).
    pub fn execute_with(
        &mut self,
        actor: &Actor,
        cmd: &Command,
        die: &mut dyn DieRoller,
    ) -> Result<Events, CombatError> {
        let outcome = apply(&self.state, actor, cmd, die)?;
        self.seq += 1;
        let before = std::mem::replace(&mut self.state, outcome.state);
        self.journal.push_back(JournalEntry {
            seq: self.seq,
            author: actor.into(),
            label: cmd.label().into(),
            before,
        });
        while self.journal.len() > UNDO_LIMIT {
            self.journal.pop_front();
        }
        self.push_log(actor.into(), &outcome.events.dm);
        Ok(outcome.events)
    }

    fn push_log(&mut self, author: Author, events: &[DmEvent]) {
        let seq = self.seq;
        self.log.extend(
            events
                .iter()
                .cloned()
                .map(|event| LogEntry { seq, author, event }),
        );
    }

    /// Reverts the last write: the state becomes exactly what it was before it. Only the DM undoes. If the last
    /// write was a player's, it needs `force`. Undo is a write too (it moves `seq`), but it isn't undoable itself.
    pub fn undo(&mut self, actor: &Actor, force: bool) -> Result<Events, CombatError> {
        if *actor != Actor::Dm {
            return Err(CombatError::NotAllowed);
        }
        let entry = self.journal.back().ok_or(CombatError::NothingToUndo)?;
        if entry.author == Author::Player && !force {
            return Err(CombatError::UndoWouldRevertPlayer);
        }
        let entry = self.journal.pop_back().expect("checked");
        let before = PlayerSummary::of(&self.state);
        self.state = entry.before;
        self.seq += 1;
        let players = before.diff(&PlayerSummary::of(&self.state));
        let dm = vec![DmEvent {
            round: self.state.round,
            subject: None,
            kind: DmEventKind::Undone {
                label: entry.label.clone(),
            },
            text: format!("Undid {}", entry.label),
            private: false,
        }];
        self.push_log(Author::Dm, &dm);
        Ok(Events { dm, players })
    }

    /// The labels of the undo steps, newest first.
    #[must_use]
    pub fn undo_labels(&self) -> Vec<&str> {
        self.journal
            .iter()
            .rev()
            .map(|e| e.label.as_str())
            .collect()
    }
}
