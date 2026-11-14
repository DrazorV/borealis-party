//! Encounter simulation for Borealis Connect (0.3.1, D26): one pure runner that owns a whole fight on a **copy** of
//! an encounter and pauses at every decision point for an oracle.
//!
//! * Oracles: an MCP agent (through the `sim_*` tools: the client's own model), [`basic`] (the deterministic
//!   heuristic), or a transcript (replay). The server never calls a model itself (0.3.3).
//! * The engine (`party_combat::apply` with one seeded die) produces every number. Oracles only pick ids from
//!   engine-built menus ([`menu`]); text-only actions are adjudicated with numbers by index ([`adjudicate`]).
//! * No I/O, no clock, no hash-ordered collections: the same setup, seed and transcript give the same report.
//! * D16: reports describe what happened; there is no XP, CR, difficulty or rating anywhere. A batch gives plain
//!   counts out of N.
//! * D25: nothing here can reach a live combat; the server hands in copies and keeps the session in memory.
//!
//! Modules: [`setup`] (inputs), [`sheet`] (what the engine lacks: saves, attacks, save effects, multiattack),
//! [`rules`] (the advantage table), [`space`] (bands), [`menu`] (cards), [`runner`] (the loop and upkeep),
//! [`basic`], [`adjudicate`], [`report`], [`spells`] (casts through the tracker's `CastSpell`, 0.3.3).

#![forbid(unsafe_code)]

pub mod adjudicate;
pub mod basic;
pub mod menu;
pub mod report;
pub mod rules;
pub mod runner;
pub mod setup;
pub mod sheet;
pub mod space;
pub mod spells;

pub use menu::Decision;
pub use report::{batch, report, BatchReport, Report};
pub use runner::{replay, By, Outcome, Pending, Sim, Until};
pub use setup::{CreatureInput, PcInput, SimOpts, SimSetup};
