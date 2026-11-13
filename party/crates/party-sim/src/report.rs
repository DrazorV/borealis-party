//! Reports: what happened, described (D16: no XP, CR, difficulty or rating words, no odds of winning). A batch gives
//! plain counts out of N.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runner::{AdjRec, By, Coverage, DecisionRec, Entry, Outcome, Sim};
use crate::setup::{Side, SimSetup};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeathSaves {
    pub successes: u32,
    pub failures: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcReport {
    pub id: String,
    pub name: String,
    pub start_hp: i32,
    pub lowest_hp: i32,
    pub final_hp: i32,
    pub max_hp: i32,
    pub rounds_at_zero: u32,
    pub death_saves: DeathSaves,
    pub died: bool,
    pub damage_taken_by: BTreeMap<String, i32>,
    pub damage_dealt: i32,
    pub slots_spent: BTreeMap<String, u32>,
    pub resources_spent: BTreeMap<String, u32>,
    pub conditions: Vec<String>,
    pub concentration_broken: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatureReport {
    pub id: String,
    pub name: String,
    pub damage_dealt: i32,
    pub damage_taken: i32,
    pub final_hp: i32,
    pub max_hp: i32,
    pub down_round: Option<u32>,
    pub withdrew_round: Option<u32>,
    pub abilities_used: BTreeMap<String, u32>,
    pub recharges: u32,
    pub legendary_resistance_spent: u32,
    /// The round each ability was first used.
    pub first_used: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Steering {
    pub agent: u32,
    pub assistant: u32,
    pub basic: u32,
    pub fallback: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Standing {
    pub party: Vec<String>,
    pub creatures: Vec<String>,
    pub withdrew: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionOut {
    pub step: u32,
    pub round: u32,
    pub actor: String,
    pub by: By,
    pub option: String,
    pub targets: Vec<String>,
    /// AI reasoning, not rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why_ai_reasoning_not_rules: Option<String>,
}

/// The report of one run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub outcome: Outcome,
    pub standing: Standing,
    pub rounds: u32,
    pub timeline: Vec<String>,
    pub pcs: Vec<PcReport>,
    pub creatures: Vec<CreatureReport>,
    pub adjudications: Vec<AdjRec>,
    pub steering: Steering,
    pub coverage: Coverage,
    pub assumptions: Vec<String>,
    pub decisions: Vec<DecisionOut>,
    pub seed: u32,
    pub fingerprints: BTreeMap<String, String>,
    pub transcript: Vec<Entry>,
}

/// The fixed assumptions of every run.
#[must_use]
pub fn assumptions(sim: &Sim) -> Vec<String> {
    let mut out: Vec<String> = crate::space::assumptions()
        .into_iter()
        .map(str::to_string)
        .collect();
    out.push("Each side plays to win with only what it can see; Basic plays every combatant no agent controls.".into());
    out.push("Creatures use their average HP. Initiative is rolled by the engine.".into());
    out.push("Area effects are aimed at enemies only.".into());
    if sim.opts.fog {
        out.push(
            "Fog: a side sees enemies' health as unhurt / bloodied / down, not their HP.".into(),
        );
    }
    if !sim.opts.adjudicate {
        out.push("Text-only options (most spells, class features, unparsed actions) were skipped (small profile).".into());
    }
    out
}

fn steering(decisions: &[DecisionRec]) -> Steering {
    let mut s = Steering::default();
    for d in decisions {
        match d.by {
            By::Agent => s.agent += 1,
            By::Assistant => s.assistant += 1,
            By::Basic => s.basic += 1,
            By::Fallback => s.fallback += 1,
        }
    }
    s
}

/// The report of a finished (or stopped) run.
#[must_use]
pub fn report(sim: &Sim) -> Report {
    let mut pcs = Vec::new();
    let mut creatures = Vec::new();
    let mut standing = Standing {
        party: vec![],
        creatures: vec![],
        withdrew: vec![],
    };
    for c in &sim.state.combatants {
        let t = sim.tallies.get(&c.id).cloned().unwrap_or_default();
        let name = sim.name(&c.id);
        if !sim.space.present(&c.id) {
            standing.withdrew.push(name.clone());
        } else if sim.standing(&c.id) {
            match sim.side(&c.id) {
                Side::Party => standing.party.push(name.clone()),
                Side::Foes => standing.creatures.push(name.clone()),
            }
        }
        match sim.side(&c.id) {
            Side::Party => pcs.push(PcReport {
                id: c.id.0.clone(),
                name,
                start_hp: t.start_hp,
                lowest_hp: t.lowest_hp.max(0),
                final_hp: c.hp.current,
                max_hp: c.hp.max,
                rounds_at_zero: u32::try_from(t.rounds_at_zero.len()).unwrap_or(0),
                death_saves: DeathSaves {
                    successes: t.death_saves.0,
                    failures: t.death_saves.1,
                },
                died: c.life.is_dead(),
                damage_taken_by: t.taken_by.clone(),
                damage_dealt: t.damage_dealt,
                slots_spent: t
                    .slots_spent
                    .iter()
                    .map(|(k, v)| (format!("level {k}"), *v))
                    .collect(),
                resources_spent: t.resources_spent.clone(),
                conditions: t.conditions.iter().cloned().collect(),
                concentration_broken: t.concentration_broken,
            }),
            Side::Foes => creatures.push(CreatureReport {
                id: c.id.0.clone(),
                name,
                damage_dealt: t.damage_dealt,
                damage_taken: t.damage_taken,
                final_hp: c.hp.current,
                max_hp: c.hp.max,
                down_round: t.died_round.or(t.down_round),
                withdrew_round: t.withdrew_round,
                abilities_used: t.abilities_used.clone(),
                recharges: t.recharges,
                legendary_resistance_spent: t.legendary_resistance_spent,
                first_used: t.first_used.clone(),
            }),
        }
    }
    Report {
        outcome: sim.outcome.unwrap_or(Outcome::RoundCap),
        standing,
        rounds: sim.state.round,
        timeline: sim.timeline.clone(),
        pcs,
        creatures,
        adjudications: sim.adjudications.clone(),
        steering: steering(&sim.decisions),
        coverage: sim.coverage.clone(),
        assumptions: assumptions(sim),
        decisions: sim
            .decisions
            .iter()
            .filter(|d| d.by != By::Basic || d.option != "end")
            .map(|d| DecisionOut {
                step: d.step,
                round: d.round,
                actor: sim.name(&d.actor),
                by: d.by,
                option: d.option.clone(),
                targets: d.targets.clone(),
                why_ai_reasoning_not_rules: d.why.clone().filter(|w| !w.starts_with("basic")),
            })
            .collect(),
        seed: sim.seed,
        fingerprints: sim.fingerprints.clone(),
        transcript: sim.transcript.clone(),
    }
}

/// splitmix32: the seed of run `i` of a batch.
#[must_use]
pub fn splitmix32(x: u32) -> u32 {
    let mut z = x.wrapping_add(0x9e37_79b9);
    z = (z ^ (z >> 16)).wrapping_mul(0x85eb_ca6b);
    z = (z ^ (z >> 13)).wrapping_mul(0xc2b2_ae35);
    z ^ (z >> 16)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Counts {
    pub party_standing: u32,
    pub creatures_standing: u32,
    pub both_withdrew: u32,
    pub round_cap: u32,
    /// Out of this many runs.
    pub of: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spread {
    pub min: u32,
    pub median: u32,
    pub max: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcBatch {
    pub name: String,
    pub went_down: u32,
    pub died: u32,
    pub median_hp_left: i32,
    pub median_slots_spent: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatureBatch {
    pub name: String,
    pub defeated: u32,
    pub withdrew: u32,
    pub median_rounds_survived: u32,
}

/// The report of a batch: counts out of N only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchReport {
    pub runs: u32,
    /// The time budget ran out before all runs finished.
    pub truncated: bool,
    pub seed: u32,
    pub outcomes: Counts,
    pub rounds: Spread,
    pub pcs: Vec<PcBatch>,
    pub creatures: Vec<CreatureBatch>,
    pub notable: Vec<String>,
    /// Run indexes worth replaying (shortest, longest, first with a PC death).
    pub notable_runs: Vec<u32>,
    pub steering: &'static str,
    pub coverage: Coverage,
    pub assumptions: Vec<String>,
}

fn median<T: Ord + Copy + Default>(mut v: Vec<T>) -> T {
    if v.is_empty() {
        return T::default();
    }
    v.sort_unstable();
    v[v.len() / 2]
}

/// Run `runs` seeded runs with Basic only (`keep_going` is the caller's time budget, checked between runs).
///
/// # Errors
/// A setup the engine refuses.
pub fn batch(
    setup: &SimSetup,
    runs: u32,
    seed: u32,
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<BatchReport, String> {
    let mut reports: Vec<Report> = Vec::new();
    let mut truncated = false;
    let mut coverage = Coverage::default();
    let mut assumptions_out = Vec::new();
    for i in 0..runs {
        if i > 0 && !keep_going() {
            truncated = true;
            break;
        }
        let mut s = setup.clone();
        s.seed = splitmix32(seed ^ i);
        s.opts.control.agent = crate::setup::AgentControl::Side("none".into());
        let mut sim = Sim::new(&s)?;
        let _ = sim.advance();
        if i == 0 {
            coverage = sim.coverage.clone();
            assumptions_out = assumptions(&sim);
        }
        reports.push(report(&sim));
    }
    let n = u32::try_from(reports.len()).unwrap_or(0);
    let mut counts = Counts {
        of: n,
        ..Counts::default()
    };
    for r in &reports {
        match r.outcome {
            Outcome::PartyStanding => counts.party_standing += 1,
            Outcome::CreaturesStanding => counts.creatures_standing += 1,
            Outcome::BothWithdrew => counts.both_withdrew += 1,
            Outcome::RoundCap => counts.round_cap += 1,
        }
    }
    let rounds: Vec<u32> = reports.iter().map(|r| r.rounds).collect();
    let spread = Spread {
        min: rounds.iter().copied().min().unwrap_or(0),
        median: median(rounds.clone()),
        max: rounds.iter().copied().max().unwrap_or(0),
    };
    let mut pcs = Vec::new();
    if let Some(first) = reports.first() {
        for (k, p) in first.pcs.iter().enumerate() {
            let all: Vec<&PcReport> = reports.iter().filter_map(|r| r.pcs.get(k)).collect();
            pcs.push(PcBatch {
                name: p.name.clone(),
                went_down: u32::try_from(
                    all.iter()
                        .filter(|x| x.rounds_at_zero > 0 || x.lowest_hp == 0)
                        .count(),
                )
                .unwrap_or(0),
                died: u32::try_from(all.iter().filter(|x| x.died).count()).unwrap_or(0),
                median_hp_left: median(all.iter().map(|x| x.final_hp.max(0)).collect()),
                median_slots_spent: median(
                    all.iter()
                        .map(|x| x.slots_spent.values().sum::<u32>())
                        .collect(),
                ),
            });
        }
    }
    let mut creatures = Vec::new();
    if let Some(first) = reports.first() {
        for (k, c) in first.creatures.iter().enumerate() {
            let all: Vec<(&CreatureReport, u32)> = reports
                .iter()
                .filter_map(|r| r.creatures.get(k).map(|c| (c, r.rounds)))
                .collect();
            creatures.push(CreatureBatch {
                name: c.name.clone(),
                defeated: u32::try_from(
                    all.iter()
                        .filter(|(x, _)| x.down_round.is_some() && x.withdrew_round.is_none())
                        .count(),
                )
                .unwrap_or(0),
                withdrew: u32::try_from(
                    all.iter()
                        .filter(|(x, _)| x.withdrew_round.is_some())
                        .count(),
                )
                .unwrap_or(0),
                median_rounds_survived: median(
                    all.iter()
                        .map(|(x, r)| x.down_round.or(x.withdrew_round).unwrap_or(*r))
                        .collect(),
                ),
            });
        }
    }
    // Notable, as templates.
    let mut notable = Vec::new();
    if let Some((name, ability, k)) = round_one_ability(&reports) {
        notable.push(format!("{name} used {ability} in round 1 in {k}/{n} runs"));
    }
    if let Some(p) = pcs
        .iter()
        .filter(|p| p.went_down > 0)
        .max_by_key(|p| p.went_down)
    {
        notable.push(format!("{} went down in {}/{n} runs", p.name, p.went_down));
    }
    if let Some(c) = creatures
        .iter()
        .filter(|c| c.withdrew > 0)
        .max_by_key(|c| c.withdrew)
    {
        notable.push(format!("{} withdrew in {}/{n} runs", c.name, c.withdrew));
    } else if let Some(c) = creatures.iter().max_by_key(|c| c.median_rounds_survived) {
        notable.push(format!(
            "{} lasted a median of {} rounds",
            c.name, c.median_rounds_survived
        ));
    }
    notable.truncate(3);
    let mut notable_runs = Vec::new();
    if let Some((i, _)) = reports.iter().enumerate().min_by_key(|(_, r)| r.rounds) {
        notable_runs.push(u32::try_from(i).unwrap_or(0));
    }
    if let Some((i, _)) = reports.iter().enumerate().max_by_key(|(_, r)| r.rounds) {
        let i = u32::try_from(i).unwrap_or(0);
        if !notable_runs.contains(&i) {
            notable_runs.push(i);
        }
    }
    if let Some((i, _)) = reports
        .iter()
        .enumerate()
        .find(|(_, r)| r.pcs.iter().any(|p| p.died))
    {
        let i = u32::try_from(i).unwrap_or(0);
        if !notable_runs.contains(&i) {
            notable_runs.push(i);
        }
    }
    Ok(BatchReport {
        runs: n,
        truncated,
        seed,
        outcomes: counts,
        rounds: spread,
        pcs,
        creatures,
        notable,
        notable_runs,
        steering: "basic",
        coverage,
        assumptions: assumptions_out,
    })
}

/// The creature ability first used in round 1 in the most runs (at least 2 runs).
fn round_one_ability(reports: &[Report]) -> Option<(String, String, u32)> {
    let mut counts: BTreeMap<(String, String), u32> = BTreeMap::new();
    for r in reports {
        for c in &r.creatures {
            for (ability, round) in &c.first_used {
                if *round == 1 {
                    *counts.entry((c.name.clone(), ability.clone())).or_default() += 1;
                }
            }
        }
    }
    let mut best: Option<((String, String), u32)> = None;
    for (k, v) in counts {
        if v >= 2 && best.as_ref().is_none_or(|(_, b)| v > *b) {
            best = Some((k, v));
        }
    }
    best.map(|((a, b), k)| (a, b, k))
}

/// The JSON of a report (stable key order: struct order).
#[must_use]
pub fn to_value<T: Serialize>(r: &T) -> Value {
    serde_json::to_value(r).unwrap_or(Value::Null)
}
