//! The runner: one fight on a copy, every change through `party_combat::apply` with one seeded die. It pauses at
//! each decision point for an oracle and runs all the upkeep itself (death saves, concentration, repeated saves,
//! recharge, opportunity attacks, legendary windows, Legendary Resistance, the end, the round cap).

use std::collections::{BTreeMap, BTreeSet};

use party_combat::command::RollInput;
use party_combat::party_dice::{D20Input, DiceInput, RollMode, SeededDie};
use party_combat::{
    apply, derive, AbilityKind, Actor, Boundary, CombatState, CombatantId, Command, Condition,
    DamagePart, DmEvent, DmEventKind, Duration, EffectSpec, Life, RepeatSave, RollOutcome,
    Settings, Status,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adjudicate::{self, Adj, Resolved};
use crate::menu::{Decision, Opt, OptKind};
use crate::rules::{self, Context};
use crate::setup::{pc_combatant, sim_id, AgentControl, Side, SimOpts, SimSetup};
use crate::sheet::{self, ActionDef, AttackDef, Def, Rider, SaveDef, SimSheet};
use crate::space::{self, Move, Space};

/// Which decisions are open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Window {
    Turn,
    /// Creatures with legendary actions left, after another combatant's turn.
    Legendary {
        queue: Vec<CombatantId>,
    },
}

/// The turn in progress.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnState {
    pub actor: Option<CombatantId>,
    pub steps_left: i32,
    pub disengaged: bool,
    pub decisions: u32,
}

/// Who decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    Agent,
    Assistant,
    Basic,
    Fallback,
}

/// How the fight ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    PartyStanding,
    CreaturesStanding,
    BothWithdrew,
    RoundCap,
}

/// One log line: the DM's (engine) text and a fog-safe one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub step: u32,
    pub round: u32,
    pub dm: String,
    pub fog: String,
}

/// One decision, for the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRec {
    pub step: u32,
    pub round: u32,
    pub actor: CombatantId,
    pub by: By,
    pub option: String,
    pub targets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// One transcript entry: a decision an oracle made (Basic turns are recomputed on replay).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub step: u32,
    pub by: By,
    pub d: Decision,
}

/// What the report counts per combatant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    pub start_hp: i32,
    pub lowest_hp: i32,
    pub damage_taken: i32,
    pub damage_dealt: i32,
    pub taken_by: BTreeMap<String, i32>,
    pub slots_spent: BTreeMap<u8, u32>,
    pub resources_spent: BTreeMap<String, u32>,
    pub abilities_used: BTreeMap<String, u32>,
    pub conditions: BTreeSet<String>,
    pub concentration_broken: u32,
    pub recharges: u32,
    pub legendary_resistance_spent: u32,
    pub down_round: Option<u32>,
    pub died_round: Option<u32>,
    pub withdrew_round: Option<u32>,
    pub rounds_at_zero: BTreeSet<u32>,
    pub death_saves: (u32, u32),
    /// First round each ability was used.
    pub first_used: BTreeMap<String, u32>,
}

/// Static facts about a combatant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub name: String,
    pub side: Side,
    pub agent: bool,
    /// The encounter creature id, or the PC id.
    pub source_id: String,
    pub element_id: String,
}

/// A refused answer: a template text (ids only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal(pub String);

/// The next thing the runner needs.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    Decide { actor: CombatantId, card: Value },
    Ended(Outcome),
}

/// A replay oracle's source.
pub trait Oracle {
    /// A decision for `actor` at `step`, or `None` to let Basic play it.
    fn decide(&mut self, sim: &Sim, actor: &CombatantId, card: &Value) -> Option<(Decision, By)>;
}

/// The simulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sim {
    pub seed: u32,
    pub state: CombatState,
    pub die: SeededDie,
    pub sheets: BTreeMap<CombatantId, SimSheet>,
    pub space: Space,
    pub info: BTreeMap<CombatantId, Info>,
    pub opts: SimOpts,
    pub window: Window,
    pub turn: TurnState,
    pub step: u32,
    pub log: Vec<Line>,
    pub decisions: Vec<DecisionRec>,
    pub transcript: Vec<Entry>,
    pub tallies: BTreeMap<CombatantId, Tally>,
    pub timeline: Vec<String>,
    pub memos: BTreeMap<CombatantId, String>,
    pub sticky: BTreeMap<CombatantId, CombatantId>,
    pub adjudications: Vec<AdjRec>,
    pub adj_cache: BTreeMap<String, Adj>,
    pub outcome: Option<Outcome>,
    pub all_basic: bool,
    pub fallback_streak: u32,
    pub coverage: Coverage,
    pub fingerprints: BTreeMap<String, String>,
    /// The source of damage being dealt now (for the tallies).
    #[serde(skip)]
    cur: Option<(CombatantId, String, bool)>,
}

/// One adjudication, for the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjRec {
    pub step: u32,
    pub actor: CombatantId,
    pub option: String,
    pub by: By,
    pub adj: Adj,
    pub numbers: Vec<adjudicate::Number>,
    /// The text, quoted as data.
    pub untrusted_stat_block_text: String,
}

/// What a run automates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub automated: Vec<String>,
    pub adjudicable: Vec<String>,
    pub not_simulated: Vec<String>,
}

fn d20(modifier: i32, mode: RollMode) -> D20Input {
    D20Input::app(modifier, mode)
}

impl Sim {
    /// Build the copy: PCs `c1..`, then creatures, initiative rolled by the engine, and the first turn started.
    ///
    /// # Errors
    /// A copy the engine refuses.
    pub fn new(setup: &SimSetup) -> Result<Self, String> {
        let mut state = CombatState::new(setup.edition);
        state.settings = Settings {
            show_enemy_turn: false,
            auto_recharge: true,
        };
        let mut die = SeededDie::new(setup.seed);
        let mut sheets = BTreeMap::new();
        let mut info = BTreeMap::new();
        let mut space = Space::new();
        let mut fingerprints = BTreeMap::new();
        let mut coverage = Coverage::default();
        let mut by_source: BTreeMap<String, CombatantId> = BTreeMap::new();
        let agent_of = |side: Side, id: &CombatantId, src: &str| match &setup.opts.control.agent {
            AgentControl::Side(s) => match s.as_str() {
                "all" => true,
                "party" => side == Side::Party,
                "foes" => side == Side::Foes,
                _ => false,
            },
            AgentControl::Ids(list) => list.iter().any(|x| x == &id.0 || x == src),
        };
        let mut n = 0;
        let mut add = |state: &mut CombatState, c: party_combat::Combatant| -> Result<(), String> {
            let cmd = Command::AddCombatant {
                combatant: Box::new(c),
                roll_initiative: false,
            };
            *state = apply(state, &Actor::System, &cmd, &mut die)
                .map_err(|e| format!("a combatant: {e}"))?
                .state;
            Ok(())
        };
        for pc in &setup.pcs {
            let id = sim_id(n);
            n += 1;
            let (mut c, keys) = pc_combatant(&id.0, pc, setup.opts.pc_hp);
            c.id = id.clone();
            let sheet = sheet::pc_sheet(pc, &keys, setup.edition);
            for a in &sheet.actions {
                let label = format!("{}: {}", pc.name, a.name);
                match a.coverage {
                    sheet::Coverage::Automated => coverage.automated.push(label),
                    sheet::Coverage::Adjudicable => coverage.adjudicable.push(label),
                    sheet::Coverage::NotSimulated => coverage.not_simulated.push(label),
                }
            }
            if pc.card_only {
                coverage.not_simulated.push(format!(
                    "{}: sheet unreadable, built from its party card (HP, AC, initiative only)",
                    pc.name
                ));
            }
            fingerprints.insert(format!("pc:{}", pc.id), pc.fingerprint());
            space.place(&id, Side::Party, setup.opts.start.party_band);
            info.insert(
                id.clone(),
                Info {
                    name: c.label.clone(),
                    side: Side::Party,
                    agent: agent_of(Side::Party, &id, &pc.id),
                    source_id: pc.id.clone(),
                    element_id: String::new(),
                },
            );
            by_source.insert(pc.id.clone(), id.clone());
            sheets.insert(id, sheet);
            add(&mut state, c)?;
        }
        for cr in &setup.creatures {
            let id = sim_id(n);
            n += 1;
            let f = party_encounter::combatant::fighter_from_snapshot(&cr.snapshot, &cr.overrides);
            let mut c =
                party_encounter::combatant::creature_of(&id.0, &cr.label, &cr.label, &f, f.average);
            c.visible = true;
            let sheet = sheet::creature_sheet(&c, f.data.as_ref());
            for a in &sheet.actions {
                let label = format!("{}: {}", cr.label, a.name);
                match a.coverage {
                    sheet::Coverage::Automated => coverage.automated.push(label),
                    sheet::Coverage::Adjudicable => coverage.adjudicable.push(label),
                    sheet::Coverage::NotSimulated => coverage.not_simulated.push(label),
                }
            }
            if let Some(d) = &f.data {
                if d.lair_actions.is_some() {
                    coverage
                        .not_simulated
                        .push(format!("{}: lair actions", cr.label));
                }
            }
            if sheet.traits.regeneration.is_some() {
                coverage.adjudicable.push(format!(
                    "{}: Regeneration (healed every turn; what stops it is not simulated)",
                    cr.label
                ));
            }
            fingerprints.insert(format!("creature:{}", cr.id), cr.snapshot.rev());
            space.place(&id, Side::Foes, setup.opts.start.foes_band);
            info.insert(
                id.clone(),
                Info {
                    name: cr.label.clone(),
                    side: Side::Foes,
                    agent: agent_of(Side::Foes, &id, &cr.id),
                    source_id: cr.id.clone(),
                    element_id: cr.element_id.clone(),
                },
            );
            by_source.insert(cr.id.clone(), id.clone());
            sheets.insert(id.clone(), sheet);
            let pool = party_encounter::combatant::legendary_pool(&id.0, f.data.as_ref());
            add(&mut state, c)?;
            if let Some(p) = pool {
                state.legendary_pools.insert(id.0.clone(), p);
            }
        }
        fingerprints.insert("party-sim".into(), env!("CARGO_PKG_VERSION").into());
        // Rules name encounter creatures; here they name the copy's combatants.
        for r in &setup.rules {
            let mut r = r.clone();
            r.linked = r.linked.and_then(|l| by_source.get(&l.0).cloned());
            for f in &mut r.flags {
                if let party_combat::RuleTarget::Combatant(id) = &f.target {
                    if let Some(to) = by_source.get(&id.0) {
                        f.target = party_combat::RuleTarget::Combatant(to.clone());
                    }
                }
            }
            if let Ok(o) = apply(
                &state,
                &Actor::System,
                &Command::SetRule { rule: r },
                &mut die,
            ) {
                state = o.state;
            }
        }
        for e in &setup.start_effects {
            let mut e = e.clone();
            let Some(t) = by_source
                .get(&e.target.0)
                .cloned()
                .or_else(|| state.combatant(&e.target).map(|c| c.id.clone()))
            else {
                continue;
            };
            e.target = t;
            e.source = e.source.and_then(|s| by_source.get(&s.0).cloned());
            if let Ok(o) = apply(
                &state,
                &Actor::System,
                &Command::AddEffect {
                    effect: Box::new(e),
                },
                &mut die,
            ) {
                state = o.state;
            }
        }
        for s in &setup.opts.start.surprise {
            let id = by_source
                .get(s)
                .cloned()
                .unwrap_or_else(|| CombatantId::new(s.clone()));
            if let Some(c) = state.combatants.iter_mut().find(|c| c.id == id) {
                c.surprised = true;
            }
        }
        let mut sim = Self {
            seed: setup.seed,
            state,
            die,
            sheets,
            space,
            info,
            opts: setup.opts.clone(),
            window: Window::Turn,
            turn: TurnState::default(),
            step: 0,
            log: Vec::new(),
            decisions: Vec::new(),
            transcript: Vec::new(),
            tallies: BTreeMap::new(),
            timeline: Vec::new(),
            memos: BTreeMap::new(),
            sticky: BTreeMap::new(),
            adjudications: Vec::new(),
            adj_cache: BTreeMap::new(),
            outcome: None,
            all_basic: false,
            fallback_streak: 0,
            coverage,
            fingerprints,
            cur: None,
        };
        for c in &sim.state.combatants {
            sim.tallies.insert(
                c.id.clone(),
                Tally {
                    start_hp: c.hp.current,
                    lowest_hp: c.hp.current,
                    ..Tally::default()
                },
            );
        }
        // Everyone rolls with the engine, PCs included (with their own bonus and advantage), in roster order.
        let ids: Vec<CombatantId> = sim.state.combatants.iter().map(|c| c.id.clone()).collect();
        sim.exec(&Command::RollInitiative {
            ids,
            only_missing: false,
        })
        .map_err(|e| e.0)?;
        sim.exec(&Command::Start).map_err(|e| e.0)?;
        sim.turn_upkeep();
        Ok(sim)
    }

    // ----- Queries -----

    /// Combatants not removed, not dead and still in the fight, in roster order.
    #[must_use]
    pub fn alive_ids(&self) -> Vec<CombatantId> {
        self.state
            .combatants
            .iter()
            .filter(|c| !c.removed && !c.life.is_dead() && self.space.present(&c.id))
            .map(|c| c.id.clone())
            .collect()
    }

    /// Up and fighting.
    #[must_use]
    pub fn standing(&self, id: &CombatantId) -> bool {
        self.state
            .combatant(id)
            .is_some_and(|c| !c.removed && c.life == Life::Alive && self.space.present(id))
    }

    #[must_use]
    pub fn name(&self, id: &CombatantId) -> String {
        self.info
            .get(id)
            .map_or_else(|| id.0.clone(), |i| i.name.clone())
    }

    #[must_use]
    pub fn side(&self, id: &CombatantId) -> Side {
        self.space.side_of(id)
    }

    /// An action of `owner`'s sheet by key (multiattack steps).
    #[must_use]
    pub fn action_by_key(&self, owner: &CombatantId, key: &str) -> Option<&ActionDef> {
        self.sheets.get(owner).and_then(|s| s.action(key))
    }

    /// The step a multiattack names, looked up on whichever sheet holds `a`.
    #[must_use]
    pub fn sheet_of_key(&self, a: &ActionDef, key: &str) -> Option<&ActionDef> {
        self.sheets
            .values()
            .find(|s| s.actions.iter().any(|x| std::ptr::eq(x, a)))
            .and_then(|s| s.action(key))
    }

    /// Who decides now.
    #[must_use]
    pub fn decider(&self) -> Option<CombatantId> {
        match &self.window {
            Window::Turn => self.state.current.clone(),
            Window::Legendary { queue } => queue.first().cloned(),
        }
    }

    /// The table context of an attack.
    #[must_use]
    pub fn context(
        &self,
        actor: &CombatantId,
        target: &CombatantId,
        ranged: bool,
        x: &AttackDef,
    ) -> Context {
        let alive = self.alive_ids();
        let engaged = self
            .space
            .engaged_with(actor, &alive)
            .iter()
            .any(|e| self.standing(e) && !derive::incapacitated(&self.state, e));
        let side = self.side(actor);
        let ally_adjacent = alive.iter().any(|o| {
            o != actor
                && self.side(o) == side
                && self.standing(o)
                && !derive::incapacitated(&self.state, o)
                && self.space.steps(o, target) == 0
        });
        Context {
            ranged,
            engaged,
            long_range: ranged && self.space.feet(actor, target) > x.range.max(5),
            ally_adjacent,
            pack_tactics: self
                .sheets
                .get(actor)
                .is_some_and(|s| s.traits.pack_tactics),
            adjacent: self.space.steps(actor, target) == 0,
        }
    }

    /// The last `n` log lines a card shows (fog-safe when fog is on).
    #[must_use]
    pub fn recent_lines(&self, n: usize) -> Vec<String> {
        let start = self.log.len().saturating_sub(n);
        self.log[start..]
            .iter()
            .map(|l| {
                if self.opts.fog {
                    l.fog.clone()
                } else {
                    l.dm.clone()
                }
            })
            .collect()
    }

    // ----- Engine writes -----

    /// Apply one command and its automatic follow-ups (repeated saves, concentration checks).
    fn exec(&mut self, cmd: &Command) -> Result<Vec<DmEvent>, Refusal> {
        let out = apply(&self.state, &Actor::Dm, cmd, &mut self.die)
            .map_err(|e| Refusal(e.to_string()))?;
        self.state = out.state;
        let events = out.events.dm;
        self.note(&events);
        let mut all = events.clone();
        for e in &events {
            if let DmEventKind::SaveDue { effect } = &e.kind {
                if let Some(ev) = self.state.effect(*effect).cloned() {
                    if let Some(rs) = ev.spec.repeat_save {
                        let bonus = self
                            .sheets
                            .get(&ev.spec.target)
                            .map_or(0, |s| s.save(rs.ability));
                        let m = rules::save_mode(
                            &self.state,
                            &ev.spec.target,
                            rs.ability,
                            false,
                            false,
                        );
                        let roll = if m.auto_fail {
                            D20Input::typed_natural(1 + bonus, 1)
                        } else {
                            d20(bonus, m.mode)
                        };
                        if let Ok(more) = self.exec(&Command::ResolveEffectSave {
                            effect: *effect,
                            roll,
                        }) {
                            all.extend(more);
                        }
                    }
                }
            }
        }
        // Concentration checks after damage.
        loop {
            let Some(c) = self
                .state
                .combatants
                .iter()
                .find(|c| !c.pending_concentration.is_empty())
                .map(|c| c.id.clone())
            else {
                break;
            };
            let bonus = self.sheets.get(&c).map_or(0, |s| s.con_save);
            match self.exec(&Command::ResolveConcentration {
                id: c.clone(),
                roll: d20(bonus, RollMode::Normal),
            }) {
                Ok(more) => all.extend(more),
                Err(_) => {
                    if let Some(x) = self.state.combatants.iter_mut().find(|x| x.id == c) {
                        x.pending_concentration.clear();
                    }
                }
            }
        }
        Ok(all)
    }

    /// Log lines and tallies of events.
    fn note(&mut self, events: &[DmEvent]) {
        let round = self.state.round;
        for e in events {
            if matches!(e.kind, DmEventKind::Economy | DmEventKind::TurnStarted)
                && !e.text.contains("surprised")
            {
                continue;
            }
            let subject = e.subject.clone();
            let name = subject.as_ref().map(|s| self.name(s)).unwrap_or_default();
            let fog = match &e.kind {
                DmEventKind::Damage { result, .. } => {
                    let types: Vec<String> = result
                        .parts
                        .iter()
                        .filter_map(|p| p.damage_type.map(|t| t.as_str().to_string()))
                        .collect();
                    format!("{name} takes {} {}", result.total, types.join("+"))
                }
                DmEventKind::Healed { .. } => format!("{name} regains hit points"),
                DmEventKind::HpSet | DmEventKind::TempHp | DmEventKind::HpRolled { .. } => {
                    String::new()
                }
                DmEventKind::Dying => format!("{name} drops"),
                DmEventKind::Died => format!("{name} dies"),
                _ => e.text.clone(),
            };
            if !fog.is_empty() || !e.text.is_empty() {
                self.log.push(Line {
                    step: self.step,
                    round,
                    dm: e.text.clone(),
                    fog,
                });
            }
            let Some(s) = subject else { continue };
            match &e.kind {
                DmEventKind::Damage { result, .. } => {
                    let amount = result.total;
                    let src = self.cur.clone();
                    let t = self.tallies.entry(s.clone()).or_default();
                    t.damage_taken += amount;
                    if let Some((src_id, label, _)) = &src {
                        *t.taken_by.entry(label.clone()).or_default() += amount;
                        let src_id = src_id.clone();
                        self.tallies.entry(src_id).or_default().damage_dealt += amount;
                    }
                    let hp = self.state.combatant(&s).map_or(0, |c| c.hp.current);
                    let t = self.tallies.entry(s.clone()).or_default();
                    t.lowest_hp = t.lowest_hp.min(hp);
                    if hp <= 0 {
                        t.rounds_at_zero.insert(round);
                    }
                }
                DmEventKind::Dying => {
                    let t = self.tallies.entry(s.clone()).or_default();
                    t.down_round.get_or_insert(round);
                    t.rounds_at_zero.insert(round);
                    let by = self
                        .cur
                        .as_ref()
                        .map(|(_, l, crit)| {
                            format!(" ({l}{})", if *crit { ", critical hit" } else { "" })
                        })
                        .unwrap_or_default();
                    self.timeline
                        .push(format!("R{round} {name} drops to 0{by}"));
                }
                DmEventKind::Died => {
                    let t = self.tallies.entry(s.clone()).or_default();
                    t.died_round.get_or_insert(round);
                    t.down_round.get_or_insert(round);
                    let by = self
                        .cur
                        .as_ref()
                        .map(|(_, l, _)| format!(" ({l})"))
                        .unwrap_or_default();
                    let word = if self.side(&s) == Side::Party {
                        "dies"
                    } else {
                        "is defeated"
                    };
                    self.timeline.push(format!("R{round} {name} {word}{by}"));
                }
                DmEventKind::DeathSave { roll } => {
                    let t = self.tallies.entry(s.clone()).or_default();
                    if roll.total >= 10 {
                        t.death_saves.0 += 1;
                    } else {
                        t.death_saves.1 += 1;
                    }
                }
                DmEventKind::Revived => self.timeline.push(format!("R{round} {name} is back up")),
                DmEventKind::Stabilized => self.timeline.push(format!("R{round} {name} is stable")),
                DmEventKind::ConcentrationSave { kept: false, .. } => {
                    self.tallies
                        .entry(s.clone())
                        .or_default()
                        .concentration_broken += 1;
                    self.timeline
                        .push(format!("R{round} {name} loses concentration"));
                }
                DmEventKind::Recharge { ready: true, .. } => {
                    self.tallies.entry(s.clone()).or_default().recharges += 1;
                }
                DmEventKind::EffectAdded { effect } => {
                    if let Some(ef) = self.state.effect(*effect) {
                        let conds: Vec<String> = ef
                            .spec
                            .conditions
                            .iter()
                            .map(|c| c.as_str().to_string())
                            .collect();
                        let target = ef.spec.target.clone();
                        self.tallies
                            .entry(target)
                            .or_default()
                            .conditions
                            .extend(conds);
                    }
                }
                _ => {}
            }
        }
        for c in &self.state.combatants {
            if let Some(t) = self.tallies.get_mut(&c.id) {
                t.lowest_hp = t.lowest_hp.min(c.hp.current);
            }
        }
    }

    fn line(&mut self, text: String) {
        self.log.push(Line {
            step: self.step,
            round: self.state.round,
            dm: text.clone(),
            fog: text,
        });
    }

    fn rolled_total(events: &[DmEvent]) -> Option<i32> {
        events.iter().rev().find_map(|e| match &e.kind {
            DmEventKind::Rolled {
                result: RollOutcome::D20(r),
                ..
            } => Some(r.total),
            DmEventKind::Rolled {
                result: RollOutcome::Dice(r),
                ..
            } => Some(r.total),
            _ => None,
        })
    }

    // ----- Turns -----

    #[allow(clippy::never_loop)] // one pass; each `return` is a way out
    fn turn_upkeep(&mut self) {
        loop {
            if self.check_end() {
                return;
            }
            let Some(id) = self.state.current.clone() else {
                return;
            };
            self.window = Window::Turn;
            let speed = self
                .state
                .combatant(&id)
                .map_or(30, |c| derive::effective_speed(&self.state, c).walk);
            self.turn = TurnState {
                actor: Some(id.clone()),
                steps_left: space::budget(speed),
                disengaged: false,
                decisions: 0,
            };
            if speed == 0 {
                self.turn.steps_left = 0;
            }
            let Some(c) = self.state.combatant(&id).cloned() else {
                return;
            };
            if !self.space.present(&id) {
                self.end_turn();
                return;
            }
            if let Some(n) = self.sheets.get(&id).and_then(|s| s.traits.regeneration) {
                if c.life == Life::Alive && c.hp.current > 0 && c.hp.current < c.hp.max {
                    let _ = self.exec(&Command::Heal {
                        target: id.clone(),
                        amount: DiceInput::typed(n),
                    });
                }
            }
            match c.life {
                Life::Dying { .. } => {
                    self.cur = None;
                    let _ = self.exec(&Command::DeathSave {
                        id: id.clone(),
                        roll: d20(0, RollMode::Normal),
                    });
                    self.end_turn();
                    return;
                }
                Life::Stable | Life::Dead => {
                    self.end_turn();
                    return;
                }
                Life::Alive => {}
            }
            if derive::incapacitated(&self.state, &id) {
                let n = self.name(&id);
                self.line(format!("{n} can't act"));
                self.end_turn();
                return;
            }
            if derive::has_condition(&self.state, &id, Condition::Prone)
                && self.turn.steps_left > 0
                && self
                    .exec(&Command::RemoveCondition {
                        target: id.clone(),
                        condition: Condition::Prone,
                    })
                    .is_ok()
            {
                self.turn.steps_left /= 2;
                let n = self.name(&id);
                self.line(format!("{n} stands up"));
            }
            if c.economy.action && c.economy.bonus_action {
                // Surprised (2014): no actions this turn.
                self.end_turn();
                return;
            }
            return;
        }
    }

    /// End the current turn: open the legendary window, or move on.
    fn end_turn(&mut self) {
        let just = self.state.current.clone();
        let queue: Vec<CombatantId> = self
            .state
            .combatants
            .iter()
            .filter(|c| Some(&c.id) != just.as_ref())
            .filter(|c| self.standing(&c.id) && !derive::incapacitated(&self.state, &c.id))
            .filter(|c| derive::pool_of(&self.state, &c.id).is_some_and(|(_, p)| p.remaining > 0))
            .filter(|c| {
                self.sheets.get(&c.id).is_some_and(|s| {
                    s.actions.iter().any(|a| {
                        a.kind == AbilityKind::Legendary
                            && (a.coverage == sheet::Coverage::Automated
                                || (a.coverage == sheet::Coverage::Adjudicable
                                    && self.opts.adjudicate))
                    })
                })
            })
            .map(|c| c.id.clone())
            .collect();
        if !queue.is_empty() && self.outcome.is_none() {
            self.window = Window::Legendary { queue };
            // Only offer it if the creature has a usable option besides passing.
            self.skip_empty_legendary();
            if matches!(self.window, Window::Legendary { .. }) {
                return;
            }
        }
        self.next_turn();
    }

    fn skip_empty_legendary(&mut self) {
        while let Window::Legendary { queue } = &self.window {
            let Some(first) = queue.first().cloned() else {
                self.window = Window::Turn;
                return;
            };
            if self.options(&first).iter().any(|o| o.kind != OptKind::Pass) {
                return;
            }
            if let Window::Legendary { queue } = &mut self.window {
                queue.remove(0);
                if queue.is_empty() {
                    self.window = Window::Turn;
                }
            }
        }
    }

    fn next_turn(&mut self) {
        self.window = Window::Turn;
        if self.check_end() {
            return;
        }
        let round = self.state.round;
        if self.exec(&Command::NextTurn).is_err() {
            self.finish(Outcome::RoundCap);
            return;
        }
        if self.state.round > round && self.state.round > self.opts.max_rounds {
            self.finish(Outcome::RoundCap);
            return;
        }
        self.turn_upkeep();
    }

    fn finish(&mut self, o: Outcome) {
        if self.outcome.is_none() {
            self.outcome = Some(o);
            let _ = apply(&self.state, &Actor::Dm, &Command::End, &mut self.die)
                .map(|out| self.state = out.state);
        }
    }

    /// Whether the fight is over (and records how).
    pub fn check_end(&mut self) -> bool {
        if self.outcome.is_some() {
            return true;
        }
        let up = |s: &Self, side: Side| {
            s.state
                .combatants
                .iter()
                .any(|c| s.side(&c.id) == side && s.standing(&c.id))
        };
        let gone = |s: &Self, side: Side| {
            s.state
                .combatants
                .iter()
                .any(|c| s.side(&c.id) == side && !s.space.present(&c.id))
        };
        let party = up(self, Side::Party);
        let foes = up(self, Side::Foes);
        let o = match (party, foes) {
            (true, true) => return false,
            (true, false) => Outcome::PartyStanding,
            (false, true) => Outcome::CreaturesStanding,
            (false, false) if gone(self, Side::Party) && gone(self, Side::Foes) => {
                Outcome::BothWithdrew
            }
            (false, false) => Outcome::CreaturesStanding,
        };
        self.finish(o);
        true
    }

    // ----- The loop -----

    /// Run Basic for everyone it plays until an oracle must decide, or the end.
    pub fn advance(&mut self) -> Pending {
        loop {
            if let Some(o) = self.outcome {
                return Pending::Ended(o);
            }
            if self.step >= self.opts.max_steps {
                self.finish(Outcome::RoundCap);
                continue;
            }
            let Some(actor) = self.decider() else {
                self.finish(Outcome::RoundCap);
                continue;
            };
            let agent = self.info.get(&actor).is_some_and(|i| i.agent) && !self.all_basic;
            if agent {
                let card = self.card(&actor);
                return Pending::Decide { actor, card };
            }
            let d = crate::basic::decide(self, &actor);
            if self.answer(d, By::Basic).is_err() {
                let end = if matches!(self.window, Window::Legendary { .. }) {
                    "pass"
                } else {
                    "end"
                };
                let _ = self.answer(
                    Decision {
                        o: end.into(),
                        ..Decision::default()
                    },
                    By::Basic,
                );
            }
        }
    }

    /// Run until the end, asking `oracle` at every agent decision point.
    pub fn run_with(&mut self, oracle: &mut dyn Oracle) -> Outcome {
        loop {
            match self.advance() {
                Pending::Ended(o) => return o,
                Pending::Decide { actor, card } => match oracle.decide(self, &actor, &card) {
                    Some((d, by)) => {
                        if self.answer(d, by).is_err() {
                            let b = crate::basic::decide(self, &actor);
                            if self.answer(b, By::Fallback).is_err() {
                                self.force_end();
                            }
                        }
                    }
                    None => {
                        let b = crate::basic::decide(self, &actor);
                        if self.answer(b, By::Basic).is_err() {
                            self.force_end();
                        }
                    }
                },
            }
        }
    }

    /// End the open decision point (end turn or pass), or the run when even that is refused.
    pub fn force_end(&mut self) {
        let end = if matches!(self.window, Window::Legendary { .. }) {
            "pass"
        } else {
            "end"
        };
        if self
            .answer(
                Decision {
                    o: end.into(),
                    ..Decision::default()
                },
                By::Fallback,
            )
            .is_err()
        {
            self.finish(Outcome::RoundCap);
        }
    }

    /// Let Basic play the agent's combatants too, until `until`.
    pub fn auto(&mut self, until: Until) -> Pending {
        let round = self.state.round;
        let turn_of = self.state.current.clone();
        loop {
            match self.advance() {
                Pending::Ended(o) => return Pending::Ended(o),
                Pending::Decide { actor, card } => {
                    let stop = match until {
                        Until::Turn => self.state.current != turn_of && self.step > 0,
                        Until::Round => self.state.round > round,
                        Until::End => false,
                    };
                    if stop {
                        return Pending::Decide { actor, card };
                    }
                    let d = crate::basic::decide(self, &actor);
                    let rec = Decision {
                        o: "basic".into(),
                        ..Decision::default()
                    };
                    self.transcript.push(Entry {
                        step: self.step,
                        by: By::Basic,
                        d: rec,
                    });
                    if self.apply_decision(d, By::Basic, false).is_err() {
                        self.force_end();
                    }
                }
            }
        }
    }

    /// Answer the open decision point. Agent and assistant decisions go to the transcript.
    ///
    /// # Errors
    /// A template text (ids only); nothing changed.
    pub fn answer(&mut self, d: Decision, by: By) -> Result<Vec<String>, Refusal> {
        if d.o == "basic" {
            let actor = self
                .decider()
                .ok_or_else(|| Refusal("nothing to decide".into()))?;
            let b = crate::basic::decide(self, &actor);
            self.transcript.push(Entry {
                step: self.step,
                by,
                d: d.clone(),
            });
            return self.apply_decision(b, By::Basic, false);
        }
        let record = by != By::Basic;
        let entry = Entry {
            step: self.step,
            by,
            d: d.clone(),
        };
        let out = self.apply_decision(d, by, record)?;
        if record {
            self.transcript.push(entry);
        }
        Ok(out)
    }

    fn apply_decision(
        &mut self,
        mut d: Decision,
        by: By,
        _record: bool,
    ) -> Result<Vec<String>, Refusal> {
        let actor = self
            .decider()
            .ok_or_else(|| Refusal("nothing to decide".into()))?;
        if d.o == "repeat" {
            let last = self
                .decisions
                .iter()
                .rev()
                .find(|r| r.actor == actor && r.option != "end" && r.option != "pass")
                .ok_or_else(|| Refusal("no earlier decision of this combatant to repeat".into()))?;
            d = Decision {
                o: last.option.clone(),
                t: last.targets.clone(),
                ..Decision::default()
            };
        }
        let options = self.options(&actor);
        let Some(opt) = options.iter().find(|o| o.o == d.o).cloned() else {
            let ids: Vec<&str> = options.iter().map(|o| o.o.as_str()).collect();
            return Err(Refusal(format!(
                "option {} is not on the card; choose one of: {}",
                clean(&d.o),
                ids.join(", ")
            )));
        };
        let mv = match &d.mv {
            Some(m) => Some(
                Move::parse(m)
                    .ok_or_else(|| Refusal("mv is engage:cN, near, far, back or hold".into()))?,
            ),
            None => None,
        };
        if let Some(Move::Engage(t)) = &mv {
            if !self.alive_ids().contains(t) || self.side(t) == self.side(&actor) {
                return Err(Refusal(format!(
                    "engage target {} is not an enemy in the fight",
                    clean(&t.0)
                )));
            }
        }
        let mut targets: Vec<CombatantId> = Vec::new();
        for t in &d.t {
            let id = CombatantId::new(t.clone());
            if !opt.targets.contains(&id) && opt.kind != OptKind::Adjudicate {
                let ids: Vec<&str> = opt.targets.iter().map(|x| x.0.as_str()).collect();
                return Err(Refusal(format!(
                    "target {} is not legal for {}; legal: {}",
                    clean(t),
                    opt.o,
                    ids.join(", ")
                )));
            }
            if opt.kind == OptKind::Adjudicate && !self.alive_ids().contains(&id) {
                return Err(Refusal(format!("target {} is not in the fight", clean(t))));
            }
            if !targets.contains(&id) {
                targets.push(id);
            }
        }
        let needs_target = matches!(
            opt.kind,
            OptKind::Attack | OptKind::Multiattack | OptKind::Save | OptKind::Heal
        );
        if needs_target && targets.is_empty() {
            return Err(Refusal(format!("{} needs targets from its list", opt.o)));
        }
        if targets.len() > usize::try_from(opt.max_targets.max(1)).unwrap_or(1) && needs_target {
            return Err(Refusal(format!(
                "{} takes at most {} targets",
                opt.o, opt.max_targets
            )));
        }
        let adj_resolved = if opt.kind == OptKind::Adjudicate {
            let a = opt
                .action
                .and_then(|i| self.sheets.get(&actor).map(|s| s.actions[i].clone()))
                .ok_or_else(|| Refusal("unknown option".into()))?;
            let cache_key = format!(
                "{}\u{0}{}",
                self.info.get(&actor).map_or("", |i| i.element_id.as_str()),
                a.key
            );
            let adj = match d
                .adj
                .clone()
                .or_else(|| self.adj_cache.get(&cache_key).cloned())
            {
                Some(x) => x,
                None => {
                    return Err(Refusal(format!(
                        "{} is text only: send adj with numbers by index",
                        opt.o
                    )))
                }
            };
            let numbers = adjudicate::numbers_of(self, &actor, &a);
            let r = adjudicate::resolve(&adj, &numbers).map_err(Refusal)?;
            if targets.is_empty() {
                for t in &adj.targets {
                    let id = CombatantId::new(t.clone());
                    if self.alive_ids().contains(&id) && !targets.contains(&id) {
                        targets.push(id);
                    }
                }
            }
            if !matches!(r, Resolved::NoEffect | Resolved::TempHp(_))
                && targets.is_empty()
                && !matches!(&r, Resolved::Run(Def::Heal { .. }))
            {
                return Err(Refusal(format!("{} needs targets", opt.o)));
            }
            self.adj_cache.insert(cache_key, adj.clone());
            self.adjudications.push(AdjRec {
                step: self.step,
                actor: actor.clone(),
                option: a.name.clone(),
                by,
                adj,
                numbers,
                untrusted_stat_block_text: a.text.clone(),
            });
            Some((r, a))
        } else {
            None
        };
        // Accepted: from here on it runs.
        let before = self.log.len();
        if let Some(m) = &d.memo {
            self.memos.insert(actor.clone(), clean_text(m, 200));
        }
        self.decisions.push(DecisionRec {
            step: self.step,
            round: self.state.round,
            actor: actor.clone(),
            by,
            option: opt.o.clone(),
            targets: targets.iter().map(|t| t.0.clone()).collect(),
            why: d.why.as_deref().map(|w| clean_text(w, 160)),
        });
        match by {
            By::Fallback => self.fallback_streak += 1,
            By::Agent | By::Assistant => self.fallback_streak = 0,
            By::Basic => {}
        }
        if self.fallback_streak >= 3 {
            self.all_basic = true;
        }
        if let Some(t) = targets.first() {
            if self.side(t) != self.side(&actor) {
                self.sticky.insert(actor.clone(), t.clone());
            }
        }
        self.step += 1;
        self.turn.decisions += 1;
        self.run_option(&actor, &opt, &targets, mv, adj_resolved);
        let lines: Vec<String> = self.log[before..]
            .iter()
            .map(|l| {
                if self.opts.fog {
                    l.fog.clone()
                } else {
                    l.dm.clone()
                }
            })
            .collect();
        self.after_decision(&actor, &opt);
        Ok(lines)
    }

    fn after_decision(&mut self, actor: &CombatantId, opt: &Opt) {
        if self.check_end() {
            return;
        }
        match &mut self.window {
            Window::Legendary { queue } => {
                if queue.first() == Some(actor) {
                    queue.remove(0);
                }
                if queue.is_empty() {
                    self.next_turn();
                } else {
                    self.skip_empty_legendary();
                    if matches!(self.window, Window::Turn) {
                        self.next_turn();
                    }
                }
            }
            Window::Turn => {
                let done = matches!(opt.kind, OptKind::End | OptKind::Yield)
                    || !self.standing(actor)
                    || self.turn.decisions >= 4
                    || !self.options(actor).iter().any(|o| {
                        !matches!(
                            o.kind,
                            OptKind::End
                                | OptKind::Dodge
                                | OptKind::Dash
                                | OptKind::Disengage
                                | OptKind::Yield
                        )
                    }) && self
                        .state
                        .combatant(actor)
                        .is_some_and(|c| c.economy.action);
                if done {
                    self.end_turn();
                }
            }
        }
    }

    // ----- Doing things -----

    fn spend(&mut self, actor: &CombatantId, field: &str) {
        let cmd = match field {
            "action" => Command::SetEconomy {
                id: actor.clone(),
                action: Some(true),
                bonus_action: None,
                reaction: None,
            },
            _ => Command::SetEconomy {
                id: actor.clone(),
                action: None,
                bonus_action: None,
                reaction: Some(true),
            },
        };
        let _ = self.exec(&cmd);
    }

    fn run_option(
        &mut self,
        actor: &CombatantId,
        opt: &Opt,
        targets: &[CombatantId],
        mv: Option<Move>,
        adj: Option<(Resolved, ActionDef)>,
    ) {
        match opt.kind {
            OptKind::Dash => {
                self.spend(actor, "action");
                let speed = self
                    .state
                    .combatant(actor)
                    .map_or(30, |c| derive::effective_speed(&self.state, c).walk);
                if speed > 0 {
                    self.turn.steps_left += space::budget(speed);
                }
                let n = self.name(actor);
                self.line(format!("{n} dashes"));
                if let Some(m) = mv {
                    self.do_move(actor, &m);
                }
                return;
            }
            OptKind::Disengage => {
                self.spend(actor, "action");
                self.turn.disengaged = true;
                let n = self.name(actor);
                self.line(format!("{n} disengages"));
                if let Some(m) = mv {
                    self.do_move(actor, &m);
                }
                return;
            }
            _ => {}
        }
        if let Some(m) = &mv {
            self.do_move(actor, m);
            if !self.standing(actor) {
                return;
            }
        }
        match opt.kind {
            OptKind::End | OptKind::Pass => {}
            OptKind::Dodge => {
                self.spend(actor, "action");
                let mut e = EffectSpec::new(rules::DODGE, actor.clone());
                e.source = Some(actor.clone());
                e.duration = Duration::TargetTurns {
                    remaining: 1,
                    at: Boundary::Start,
                };
                let _ = self.exec(&Command::AddEffect {
                    effect: Box::new(e),
                });
            }
            OptKind::Yield => {
                self.spend(actor, "action");
                let n = self.name(actor);
                self.line(format!("{n} surrenders"));
                self.space.withdraw(actor);
                let round = self.state.round;
                self.tallies
                    .entry(actor.clone())
                    .or_default()
                    .withdrew_round = Some(round);
                self.timeline.push(format!("R{round} {n} surrenders"));
            }
            OptKind::Dash | OptKind::Disengage => {}
            _ => {
                let Some(i) = opt.action else { return };
                let Some(a) = self.sheets.get(actor).map(|s| s.actions[i].clone()) else {
                    return;
                };
                if self
                    .exec(&Command::UseAbility {
                        id: actor.clone(),
                        ability: a.key.clone(),
                        slot: a.slot,
                    })
                    .is_err()
                {
                    return;
                }
                self.count_use(actor, &a);
                if let Some((r, src)) = adj {
                    self.run_adjudicated(actor, &src, r, targets);
                } else if matches!(a.def, Def::Multiattack { moves: true, .. })
                    && matches!(self.window, Window::Legendary { .. })
                {
                    // A legendary action that moves: a turn of its own for the move, then back.
                    let steps = self.legendary_steps(actor);
                    let turn = std::mem::replace(
                        &mut self.turn,
                        TurnState {
                            actor: Some(actor.clone()),
                            steps_left: steps,
                            disengaged: false,
                            decisions: 0,
                        },
                    );
                    let def = a.def.clone();
                    self.run_def(actor, &a, &def, targets);
                    self.turn = turn;
                } else {
                    let def = a.def.clone();
                    self.run_def(actor, &a, &def, targets);
                }
            }
        }
    }

    fn count_use(&mut self, actor: &CombatantId, a: &ActionDef) {
        let round = self.state.round;
        let t = self.tallies.entry(actor.clone()).or_default();
        *t.abilities_used.entry(a.name.clone()).or_default() += 1;
        t.first_used.entry(a.name.clone()).or_insert(round);
        if let Some(l) = a.slot {
            *t.slots_spent.entry(l).or_default() += 1;
        }
        if let Some(r) = &a.resource {
            *t.resources_spent
                .entry(r.trim_start_matches("res_").to_string())
                .or_default() += 1;
        }
    }

    fn run_def(&mut self, actor: &CombatantId, a: &ActionDef, def: &Def, targets: &[CombatantId]) {
        match def {
            Def::Attack(x) => {
                let mut k = 0;
                for _ in 0..x.count.max(1) {
                    let Some(t) = self.pick_target(actor, x, targets, &mut k) else {
                        break;
                    };
                    self.attack_once(actor, &t, x, &a.name);
                    if !self.standing(actor) {
                        break;
                    }
                }
            }
            Def::Multiattack { steps, .. } => {
                let mut k = 0;
                for st in steps {
                    let Some(step) = self.action_by_key(actor, &st.key).cloned() else {
                        continue;
                    };
                    for _ in 0..st.count {
                        match &step.def {
                            Def::Attack(x) => {
                                let Some(t) = self.pick_target(actor, x, targets, &mut k) else {
                                    break;
                                };
                                self.attack_once(actor, &t, x, &step.name);
                            }
                            Def::Save(s) => {
                                let t: Vec<CombatantId> = targets.to_vec();
                                self.save_effect(actor, s, &t, &step.name);
                            }
                            _ => {}
                        }
                        if !self.standing(actor) || self.check_end() {
                            return;
                        }
                    }
                }
            }
            Def::Save(s) => self.save_effect(actor, s, targets, &a.name),
            Def::Heal { dice } => {
                if let Some(t) = targets.first() {
                    let _ = self.exec(&Command::Heal {
                        target: t.clone(),
                        amount: DiceInput::app(dice.clone()),
                    });
                }
            }
            Def::Spell(_) | Def::Text => {}
        }
    }

    fn run_adjudicated(
        &mut self,
        actor: &CombatantId,
        a: &ActionDef,
        r: Resolved,
        targets: &[CombatantId],
    ) {
        let concentration = self
            .adjudications
            .last()
            .is_some_and(|x| x.adj.concentration)
            || matches!(&a.def, Def::Spell(s) if s.concentration);
        if concentration {
            let _ = self.exec(&Command::SetConcentration {
                id: actor.clone(),
                name: a.name.clone(),
            });
        }
        match r {
            Resolved::NoEffect => {
                let n = self.name(actor);
                self.line(format!(
                    "{n} uses {} (no mechanical effect simulated)",
                    a.name
                ));
            }
            Resolved::TempHp(dice) => {
                let ev = self
                    .exec(&Command::Roll {
                        id: actor.clone(),
                        label: format!("{} temp HP", a.name),
                        roll: RollInput::Dice(DiceInput::app(dice)),
                    })
                    .unwrap_or_default();
                let amount = Self::rolled_total(&ev).unwrap_or(0).max(0);
                let who = targets.first().cloned().unwrap_or_else(|| actor.clone());
                let _ = self.exec(&Command::GrantTempHp { id: who, amount });
            }
            Resolved::Condition { rider, turns } => {
                for t in targets.iter().take(6) {
                    self.apply_rider(actor, t, &rider, None, turns, concentration, &a.name);
                }
            }
            Resolved::Run(def) => {
                if let Def::Heal { dice } = &def {
                    let t = targets.first().cloned().unwrap_or_else(|| actor.clone());
                    let _ = self.exec(&Command::Heal {
                        target: t,
                        amount: DiceInput::app(dice.clone()),
                    });
                    return;
                }
                let mut def = def;
                if let Def::Save(s) = &mut def {
                    if targets.len() > 1 {
                        s.area = Some(sheet::Area {
                            shape: "adjudicated".into(),
                            size: 0,
                            max_targets: 6,
                        });
                    }
                }
                self.run_def(actor, a, &def, targets);
            }
        }
    }

    /// The next target of an attack: the chosen ones in order, skipping those down or out of reach; when none are
    /// left, the nearest standing enemy in reach (listed order on ties).
    fn pick_target(
        &mut self,
        actor: &CombatantId,
        x: &AttackDef,
        chosen: &[CombatantId],
        k: &mut usize,
    ) -> Option<CombatantId> {
        let alive = self.alive_ids();
        let enemies: Vec<CombatantId> = alive
            .iter()
            .filter(|o| self.side(o) != self.side(actor))
            .cloned()
            .collect();
        let reachable = self.attack_targets(actor, x, &enemies);
        while *k < chosen.len() {
            let t = chosen[*k].clone();
            if self.standing(&t) && reachable.contains(&t) {
                // The same target again for the next attack unless it drops.
                return Some(t);
            }
            *k += 1;
        }
        reachable
            .iter()
            .filter(|e| self.standing(e))
            .min_by_key(|e| self.space.steps(actor, e))
            .cloned()
    }

    /// One attack roll, damage on a hit, and its rider.
    fn attack_once(
        &mut self,
        actor: &CombatantId,
        target: &CombatantId,
        x: &AttackDef,
        label: &str,
    ) -> bool {
        let d = self.space.steps(actor, target);
        if x.melee
            && d > 0
            && (!x.ranged || d * 30 > i32::try_from(x.long.max(x.range)).unwrap_or(0))
        {
            self.do_move(actor, &Move::Engage(target.clone()));
            if !self.standing(actor) {
                return false;
            }
        } else if !x.melee && d * 30 > i32::try_from(x.long.max(x.range)).unwrap_or(0) {
            self.do_move(actor, &Move::Near);
        }
        let d = self.space.steps(actor, target);
        let melee = x.melee && d == 0;
        let ranged = !melee;
        if ranged && (!x.ranged || self.space.feet(actor, target) > x.long.max(x.range)) {
            let (a, t) = (self.name(actor), self.name(target));
            self.line(format!("{a} can't reach {t} with {label}"));
            return false;
        }
        let cx = self.context(actor, target, ranged, x);
        let mode = rules::attack_mode(&self.state, actor, target, cx);
        let roll = D20Input::App {
            modifier: x.to_hit,
            mode: mode.mode,
            crit_range: x.crit_range,
        };
        let Ok(ev) = self.exec(&Command::Attack {
            attacker: actor.clone(),
            target: target.clone(),
            label: label.to_string(),
            roll,
        }) else {
            return false;
        };
        let (hit, crit) = ev
            .iter()
            .find_map(|e| match &e.kind {
                DmEventKind::Attack { hit, critical, .. } => {
                    Some((hit.unwrap_or(false), *critical))
                }
                _ => None,
            })
            .unwrap_or((false, false));
        if !hit {
            return false;
        }
        let crit = crit || mode.auto_crit;
        let parts: Vec<DamagePart> = x
            .parts
            .iter()
            .map(|p| DamagePart {
                amount: DiceInput::App {
                    notation: p.dice.clone(),
                    critical: crit,
                    floor: None,
                },
                damage_type: p.damage_type,
            })
            .collect();
        self.cur = Some((actor.clone(), format!("{} {label}", self.name(actor)), crit));
        let _ = self.exec(&Command::Damage {
            target: target.clone(),
            parts,
            magical: false,
            apply_qualified: false,
            critical_hit: crit,
        });
        if let Some(r) = &x.rider {
            if self
                .state
                .combatant(target)
                .is_some_and(|c| !c.life.is_dead())
            {
                self.apply_rider(actor, target, r, None, None, false, label);
            }
        }
        self.cur = None;
        true
    }

    /// Roll a save: `(success, total)`. Legendary Resistance turns a failure that would apply a condition (or cost
    /// at least half its HP) into a success.
    fn roll_save(
        &mut self,
        target: &CombatantId,
        ability: party_combat::AbilityName,
        dc: i32,
        spell: bool,
        stakes: (bool, i32),
    ) -> bool {
        let sheet = self.sheets.get(target);
        let bonus = sheet.map_or(0, |s| s.save(ability));
        let mr = sheet.is_some_and(|s| s.traits.magic_resistance);
        let m = rules::save_mode(&self.state, target, ability, spell, mr);
        let success = if m.auto_fail {
            let n = self.name(target);
            self.line(format!(
                "{n} fails the {} save automatically",
                ability.as_str().to_uppercase()
            ));
            false
        } else {
            let ev = self
                .exec(&Command::Roll {
                    id: target.clone(),
                    label: format!("{} save DC {dc}", ability.as_str().to_uppercase()),
                    roll: RollInput::D20(d20(bonus, m.mode)),
                })
                .unwrap_or_default();
            Self::rolled_total(&ev).unwrap_or(0) >= dc
        };
        if success {
            return true;
        }
        let (condition, dmg) = stakes;
        let c = self.state.combatant(target);
        let lr_left = c
            .and_then(|c| c.legendary_resistance)
            .is_some_and(|p| p.remaining > 0);
        let big = c.is_some_and(|c| dmg * 2 >= c.hp.current && c.hp.current > 0);
        if lr_left
            && (condition || big)
            && self
                .exec(&Command::AdjustLegendaryResistance {
                    id: target.clone(),
                    delta: -1,
                })
                .is_ok()
        {
            let n = self.name(target);
            let round = self.state.round;
            self.line(format!("{n} uses Legendary Resistance to succeed"));
            self.timeline
                .push(format!("R{round} {n} spends Legendary Resistance"));
            self.tallies
                .entry(target.clone())
                .or_default()
                .legendary_resistance_spent += 1;
            return true;
        }
        false
    }

    fn save_effect(
        &mut self,
        actor: &CombatantId,
        s: &SaveDef,
        chosen: &[CombatantId],
        label: &str,
    ) {
        let mut targets: Vec<CombatantId> = chosen
            .iter()
            .filter(|t| self.alive_ids().contains(t))
            .cloned()
            .collect();
        let max = usize::try_from(s.area.as_ref().map_or(1, |a| a.max_targets)).unwrap_or(1);
        if let Some(first) = targets.first().cloned() {
            if s.area.is_some() {
                targets.retain(|t| self.space.steps(t, &first) == 0);
            }
        }
        targets.truncate(max);
        let Some(first) = targets.first().cloned() else {
            return;
        };
        // Close in if the effect needs it.
        if s.range > 0 && self.space.feet(actor, &first) > s.range.max(5) {
            self.do_move(actor, &Move::Engage(first.clone()));
        }
        let mut totals: Vec<i32> = Vec::new();
        for p in &s.parts {
            let ev = self
                .exec(&Command::Roll {
                    id: actor.clone(),
                    label: format!("{label} damage"),
                    roll: RollInput::Dice(DiceInput::app(p.dice.clone())),
                })
                .unwrap_or_default();
            totals.push(Self::rolled_total(&ev).unwrap_or(0).max(0));
        }
        let dmg: i32 = totals.iter().sum();
        self.cur = Some((
            actor.clone(),
            format!("{} {label}", self.name(actor)),
            false,
        ));
        for t in &targets {
            if !self.alive_ids().contains(t) {
                continue;
            }
            let ok = self.roll_save(t, s.ability, s.dc, s.spell, (s.rider.is_some(), dmg));
            if !s.parts.is_empty() {
                let parts: Vec<DamagePart> = s
                    .parts
                    .iter()
                    .zip(&totals)
                    .map(|(p, total)| {
                        let v = if ok {
                            if s.half {
                                total / 2
                            } else {
                                0
                            }
                        } else {
                            *total
                        };
                        DamagePart::typed(v, p.damage_type)
                    })
                    .filter(|p| !matches!(p.amount, DiceInput::Typed { total: 0 }))
                    .collect();
                if !parts.is_empty() {
                    let _ = self.exec(&Command::Damage {
                        target: t.clone(),
                        parts,
                        magical: s.spell,
                        apply_qualified: false,
                        critical_hit: false,
                    });
                }
            }
            if !ok {
                if let Some(r) = &s.rider {
                    if self.state.combatant(t).is_some_and(|c| !c.life.is_dead()) {
                        self.apply_rider(actor, t, r, Some(false), None, false, label);
                    }
                }
            }
        }
        self.cur = None;
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_rider(
        &mut self,
        source: &CombatantId,
        target: &CombatantId,
        r: &Rider,
        saved: Option<bool>,
        turns: Option<u32>,
        concentration: bool,
        label: &str,
    ) {
        let ok = match (saved, r.save) {
            (Some(v), _) => v,
            (None, Some((ability, dc))) => self.roll_save(target, ability, dc, false, (true, 0)),
            (None, None) => false,
        };
        if ok {
            return;
        }
        let mut e = EffectSpec::new(format!("{label}: {}", r.condition), target.clone());
        e.source = Some(source.clone());
        e.conditions = vec![r.condition];
        e.concentration = concentration;
        e.duration = match turns {
            Some(n) if n > 0 => Duration::TargetTurns {
                remaining: n,
                at: Boundary::End,
            },
            _ if r.one_turn => Duration::TargetTurns {
                remaining: 1,
                at: Boundary::End,
            },
            _ => Duration::Manual,
        };
        if r.repeat {
            if let Some((ability, dc)) = r.save {
                e.repeat_save = Some(RepeatSave {
                    ability,
                    dc: u32::try_from(dc.max(1)).unwrap_or(10),
                    at: Boundary::End,
                });
            }
        }
        if matches!(r.condition, Condition::Grappled | Condition::Restrained) {
            e.ends_on_defeat_of = vec![source.clone()];
        }
        let _ = self.exec(&Command::AddCondition {
            effect: Box::new(e),
        });
    }

    /// Move with opportunity attacks from engaged enemies (unless disengaged).
    fn do_move(&mut self, actor: &CombatantId, mv: &Move) {
        if matches!(mv, Move::Hold) || self.turn.actor.as_ref() != Some(actor) {
            return;
        }
        let alive = self.alive_ids();
        let from = self.space.at(actor);
        let (to, leaves) = self
            .space
            .target_pos(actor, mv, self.turn.steps_left.max(0), &alive);
        if to == from && !leaves {
            return;
        }
        if !self.turn.disengaged {
            let engagers: Vec<CombatantId> = self
                .space
                .engaged_with(actor, &alive)
                .into_iter()
                .filter(|e| self.standing(e) && !derive::incapacitated(&self.state, e))
                .filter(|e| self.state.combatant(e).is_some_and(|c| !c.economy.reaction))
                .collect();
            for e in engagers {
                let Some(best) = self.sheets.get(&e).and_then(|s| s.best_melee().cloned()) else {
                    continue;
                };
                let Def::Attack(x) = &best.def else { continue };
                let x = AttackDef {
                    count: 1,
                    ..x.clone()
                };
                self.spend(&e, "reaction");
                let (en, an) = (self.name(&e), self.name(actor));
                self.line(format!("{en} makes an opportunity attack against {an}"));
                let turn = std::mem::take(&mut self.turn);
                self.attack_once(&e, actor, &x, &best.name);
                self.turn = turn;
                if !self.standing(actor) {
                    return;
                }
            }
        }
        self.turn.steps_left -= (to - from).abs();
        self.space.set(actor, to);
        let n = self.name(actor);
        let band = match self.space.band(actor, &self.alive_ids()) {
            crate::setup::Band::Front => "front",
            crate::setup::Band::Near => "near",
            crate::setup::Band::Far => "far",
            crate::setup::Band::Distant => "distant",
        };
        self.line(format!("{n} moves ({band})"));
        if leaves {
            self.space.withdraw(actor);
            let round = self.state.round;
            self.tallies
                .entry(actor.clone())
                .or_default()
                .withdrew_round = Some(round);
            self.timeline
                .push(format!("R{round} {n} withdraws from the fight"));
        }
    }

    /// The ended state, if any.
    #[must_use]
    pub fn ended(&self) -> bool {
        self.outcome.is_some() || self.state.status == Status::Ended
    }
}

/// How far `sim_auto` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Until {
    Turn,
    Round,
    End,
}

/// An id from a model, cut and stripped to safe characters before it is quoted back.
#[must_use]
pub fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
        .take(24)
        .collect()
}

/// Free text from a model (why, memo): length-capped, control characters and markup dropped.
#[must_use]
pub fn clean_text(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | '{' | '}' | '`'))
        .take(max)
        .collect()
}

/// Replay a transcript on a fresh copy: every oracle decision must come at the step it was made, or the replay
/// stops with the first divergence (never a silent fallback to Basic).
///
/// # Errors
/// "transcript does not match at step N" or a setup the engine refuses.
pub fn replay(setup: &SimSetup, transcript: &[Entry]) -> Result<Sim, String> {
    let mut sim = Sim::new(setup)?;
    let mut at = 0;
    loop {
        match sim.advance() {
            Pending::Ended(_) => break,
            Pending::Decide { .. } => {
                let Some(e) = transcript.get(at).filter(|e| e.step == sim.step) else {
                    return Err(format!("transcript does not match at step {}", sim.step));
                };
                at += 1;
                sim.answer(e.d.clone(), e.by).map_err(|r| {
                    format!("transcript does not match at step {}: {}", e.step, r.0)
                })?;
            }
        }
    }
    if at != transcript.len() {
        return Err(format!(
            "transcript does not match: {} decisions left over",
            transcript.len() - at
        ));
    }
    Ok(sim)
}
