//! Spells and stat block actions (0.3.2): cast, resolve, apply, cancel, zones, standard actions.

use super::Cx;
use crate::command::Command;
use crate::error::{invalid, CombatError};

impl Cx<'_> {
    pub(super) fn spell_command(&mut self, cmd: &Command) -> Result<(), CombatError> {
        let _ = cmd;
        Err(invalid("Not implemented yet."))
    }
}
