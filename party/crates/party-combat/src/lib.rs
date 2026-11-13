//! The Borealis Party combat engine: pure, serializable combat state and the rules that change it.
//!
//! - [`CombatState`] is plain data: the roster, the turn, effects, legendary pools and encounter rules.
//! - [`apply`] takes a state, an [`Actor`], a [`Command`] and a die, and returns the new state plus [`Events`]:
//!   [`DmEvent`]s for the DM-only log and [`PlayerEvent`]s that are safe for players (visible ids only, never a
//!   creature's numbers). A refused command returns `Err` and changes nothing.
//! - [`Combat`] wraps a state with the server's seeded die (D21), the undo journal (C8) and the log (D32).
//! - [`visible_order`] and [`turn_pointer`] are what the player projection is built from (D5).
//! - [`export_json`] / [`import_json`] are the versioned export (D45).
//!
//! Every roll accepts a typed result from physical dice (D43); automatic rolls are optional. Both 2014 (SRD 5.1)
//! and 2024 (SRD 5.2.1) rules are supported. No I/O: the same state, command and die always give the same result.
//!
//! See `party/docs/combat-engine.md` for the design and the list of commands.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod combat;
pub mod command;
pub mod damage;
pub mod derive;
pub mod engine;
pub mod error;
pub mod event;
pub mod export;
pub mod model;
pub mod visibility;

pub use combat::{Combat, LOG_RETENTION_DAYS, UNDO_LIMIT};
pub use command::{Actor, Command, DamagePart, DamageTaken, RollInput};
pub use engine::{account_of, apply, player_may, Outcome};
pub use error::{CombatError, Unavailable};
pub use event::{DmEvent, DmEventKind, Events, PlayerEvent, RollOutcome};
pub use export::{export_json, import_json, SCHEMA_VERSION};
pub use model::*;
pub use party_dice;
pub use visibility::{
    is_visible, player_view_of, turn_pointer, visible_order, TurnPointer, ViewerKind,
};
