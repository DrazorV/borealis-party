//! Spells in the simulation (0.3.3): SRD casters cast through the tracker's `CastSpell`, with every roll on the
//! run's seeded die. SRD fixtures only.

mod common;

use common::{creature, pcs};
use party_combat::party_dice::SeededDie;
use party_combat::spell::{CastSpec, MechOrigin};
use party_combat::{apply, Actor, CombatantId, Command};
use party_sim::runner::Oracle;
use party_sim::sheet::{Coverage, Def};
use party_sim::spells::{SpellDefn, SpellDefs};
use party_sim::{replay, report, By, Decision, Pending, Sim, SimOpts, SimSetup};
use serde_json::Value;

fn setup(name: &str, ruleset: &str, seed: u32, agent: &str) -> SimSetup {
    let mut s = SimSetup {
        edition: if ruleset == "dnd2014" {
            party_combat::Edition::Srd51
        } else {
            party_combat::Edition::Srd521
        },
        creatures: vec![creature("ec-caster", name, ruleset, name)],
        pcs: pcs(),
        start_effects: vec![],
        rules: vec![],
        opts: SimOpts::default(),
        seed,
        spells: SpellDefs::default(),
    };
    s.opts.control.agent = party_sim::setup::AgentControl::Side(agent.into());
    s
}

fn caster() -> CombatantId {
    CombatantId::new("c5")
}

/// Plays the foes like Basic, except that it casts `spell` (at `slot` when given) the first time it can, on the
/// first `n` targets of the option.
struct Cast {
    spell: &'static str,
    slot: Option<&'static str>,
    n: usize,
    done: bool,
}

impl Oracle for Cast {
    fn decide(&mut self, sim: &Sim, actor: &CombatantId, _: &Value) -> Option<(Decision, By)> {
        if !self.done {
            let want = |o: &&party_sim::menu::Opt| {
                o.name.to_lowercase().starts_with(self.spell)
                    && self.slot.is_none_or(|s| o.o.ends_with(s))
            };
            if let Some(o) = sim.options(actor).iter().find(want) {
                self.done = true;
                let t = o.targets.iter().take(self.n).map(|t| t.0.clone()).collect();
                return Some((
                    Decision {
                        o: o.o.clone(),
                        t,
                        why: Some("scripted".into()),
                        ..Decision::default()
                    },
                    By::Agent,
                ));
            }
        }
        Some((party_sim::basic::decide(sim, actor), By::Agent))
    }
}

fn lines(sim: &Sim) -> Vec<String> {
    sim.log.iter().map(|l| l.dm.to_lowercase()).collect()
}

#[test]
fn the_srd_mage_has_its_spellbook_as_options_per_slot_level() {
    let mut sim = Sim::new(&setup("Mage", "dnd2014", 3, "foes")).unwrap();
    let sheet = &sim.sheets[&caster()];
    let cov = |n: &str| {
        sheet
            .actions
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(n))
            .map(|a| a.coverage)
    };
    assert_eq!(cov("fireball"), Some(Coverage::Automated));
    assert_eq!(
        cov("shield"),
        Some(Coverage::Automated),
        "a reaction the runner casts"
    );
    assert_eq!(cov("counterspell"), Some(Coverage::NotSimulated));
    assert_eq!(cov("light"), Some(Coverage::NotSimulated), "utility");
    // Reach the Mage's turn.
    loop {
        match sim.advance() {
            Pending::Decide { actor, .. } if actor == caster() => break,
            Pending::Decide { actor, .. } => {
                let d = party_sim::basic::decide(&sim, &actor);
                sim.answer(d, By::Agent).unwrap();
            }
            Pending::Ended(_) => panic!("ended first"),
        }
    }
    let opts = sim.options(&caster());
    let fb: Vec<&str> = opts
        .iter()
        .filter(|o| o.name.starts_with("fireball"))
        .map(|o| o.o.as_str())
        .collect();
    assert_eq!(fb.len(), 3, "slots 3, 4 and 5: {fb:?}");
    assert!(fb[1].ends_with("@4") && fb[2].ends_with("@5"), "{fb:?}");
    let up = opts.iter().find(|o| o.o == fb[1]).unwrap();
    assert!(up.summary.contains("9d6"), "{}", up.summary);
    assert!(
        opts.iter()
            .all(|o| !o.name.starts_with("shield") && !o.name.starts_with("counterspell")),
        "reactions never come from a card"
    );
    let card = sim.card(&caster());
    assert!(card.to_string().contains("level 3 at slot 4"));
}

#[test]
fn the_mage_casts_fireball_through_the_engine_and_the_run_replays() {
    let s = setup("Mage", "dnd2014", 5, "foes");
    let mut sim = Sim::new(&s).unwrap();
    sim.run_with(&mut Cast {
        spell: "fireball",
        slot: Some("@4"),
        n: 4,
        done: false,
    });
    let l = lines(&sim);
    assert!(
        l.iter().any(|x| x.contains("casts fireball at level 4")),
        "{l:#?}"
    );
    assert!(
        l.iter().any(|x| x.contains("against fireball")),
        "save rows: {l:#?}"
    );
    let r = report(&sim);
    let mage = r.creatures.iter().find(|c| c.name == "Mage").unwrap();
    assert!(mage.damage_dealt > 0);
    assert!(mage.abilities_used.get("fireball").is_some_and(|n| *n >= 1));
    assert!(sim.tallies[&caster()]
        .slots_spent
        .get(&4)
        .is_some_and(|n| *n >= 1));
    assert_eq!(r.fingerprints["spell:ID_PHB_SPELL_FIREBALL"], "srd:1");
    // Same seed and transcript, the same report.
    let again = replay(&s, &r.transcript).unwrap();
    assert_eq!(
        serde_json::to_string(&report(&again)).unwrap(),
        serde_json::to_string(&r).unwrap()
    );
}

#[test]
fn a_fireball_in_the_sim_matches_the_same_command_in_the_tracker() {
    // The sim's cast is the tracker's command: replaying the copy's state with the same die gives the same HP.
    let s = setup("Mage", "dnd2014", 9, "foes");
    let mut sim = Sim::new(&s).unwrap();
    loop {
        match sim.advance() {
            Pending::Decide { actor, .. } if actor == caster() => break,
            Pending::Decide { actor, .. } => {
                let d = party_sim::basic::decide(&sim, &actor);
                sim.answer(d, By::Agent).unwrap();
            }
            Pending::Ended(_) => panic!("ended first"),
        }
    }
    let before = sim.state.clone();
    let die = sim.die;
    let o = sim
        .options(&caster())
        .into_iter()
        .find(|o| o.name == "fireball")
        .unwrap();
    let targets: Vec<CombatantId> = o.targets.iter().take(2).cloned().collect();
    // Both PCs in the same band, as the area rule wants.
    sim.answer(
        Decision {
            o: o.o.clone(),
            t: targets.iter().map(|t| t.0.clone()).collect(),
            ..Decision::default()
        },
        By::Agent,
    )
    .unwrap();
    let mut die2: SeededDie = die;
    let cmd = Command::CastSpell {
        caster: caster(),
        ability: Some(
            before
                .combatant(&caster())
                .unwrap()
                .abilities
                .iter()
                .find(|a| a.name == "fireball")
                .unwrap()
                .key
                .clone(),
        ),
        spec: None,
        slot: Some(3),
        targets: targets.clone(),
        rows: targets
            .iter()
            .map(|t| party_combat::command::RowInput {
                target: t.clone(),
                ray: 0,
                roll: party_combat::command::RowRoll::App,
                use_lr: false,
            })
            .collect(),
        amounts: vec![party_combat::command::AmountRoll::App],
        crit_amounts: vec![],
        choices: Default::default(),
        precast: false,
        force: false,
        id: None,
    };
    let tracker = apply(&before, &Actor::Dm, &cmd, &mut die2).unwrap().state;
    for t in &targets {
        assert_eq!(
            tracker.combatant(t).unwrap().hp.current,
            sim.state.combatant(t).unwrap().hp.current,
            "{t}"
        );
    }
}

#[test]
fn shield_turns_hits_into_misses_with_the_reaction_and_a_slot() {
    let mut seen = false;
    for seed in 1..40 {
        let s = setup("Mage", "dnd2014", seed, "none");
        let mut sim = Sim::new(&s).unwrap();
        let _ = sim.advance();
        if lines(&sim)
            .iter()
            .any(|l| l.contains("shield turns the hit into a miss"))
        {
            let used = &sim.tallies[&caster()];
            assert!(used.abilities_used.contains_key("shield"));
            assert!(used.slots_spent.values().sum::<u32>() >= 1);
            seen = true;
            break;
        }
    }
    assert!(seen, "some run has the Mage cast Shield");
}

#[test]
fn the_cult_fanatic_holds_a_person_with_repeat_saves_and_concentration() {
    let mut held = false;
    for seed in 1..30 {
        let s = setup("Cult Fanatic", "dnd2014", seed, "foes");
        let mut sim = Sim::new(&s).unwrap();
        sim.run_with(&mut Cast {
            spell: "hold person",
            slot: None,
            n: 1,
            done: false,
        });
        let l = lines(&sim);
        assert!(l.iter().any(|x| x.contains("casts hold person")), "{l:#?}");
        if !l.iter().any(|x| x.contains("fails against hold person")) {
            continue;
        }
        held = true;
        let r = report(&sim);
        assert!(
            r.pcs
                .iter()
                .any(|p| p.conditions.iter().any(|c| c == "paralyzed")),
            "{:?}",
            r.pcs
        );
        // The repeat save at the end of the held PC's turns, or the end of the fanatic's concentration.
        assert!(
            l.iter()
                .any(|x| x.to_lowercase().contains("wis") && x.contains("save"))
                || r.creatures[0].down_round.is_some(),
            "{l:#?}"
        );
        assert_eq!(
            serde_json::to_string(&report(&replay(&s, &r.transcript).unwrap())).unwrap(),
            serde_json::to_string(&r).unwrap()
        );
        break;
    }
    assert!(held, "a PC fails a save against Hold Person in some run");
}

#[test]
fn the_cleric_heals_a_dying_ally_with_the_srd_definition() {
    // known_heal is gone: Healing Word and Cure Wounds come from the curated SRD rows.
    let sim = Sim::new(&common::ogre_setup(1, "none")).unwrap();
    let cleric = CombatantId::new("c2");
    let a = sim.sheets[&cleric]
        .actions
        .iter()
        .find(|a| a.name == "Healing Word")
        .unwrap();
    assert_eq!(a.coverage, Coverage::Automated);
    assert_eq!(a.kind, party_combat::AbilityKind::BonusAction);
    let Def::Spell(s) = &a.def else {
        panic!("a spell")
    };
    assert!(matches!(s.spec.origin, MechOrigin::Srd { .. }));
    assert_eq!(s.spec.dc, Some(13));
    let mut healed = false;
    for seed in 1..30 {
        let mut sim = Sim::new(&common::ogre_setup(seed, "none")).unwrap();
        let _ = sim.advance();
        if lines(&sim)
            .iter()
            .any(|l| l.contains("casts healing word") || l.contains("casts cure wounds"))
        {
            healed = true;
            break;
        }
    }
    assert!(healed, "Basic heals a dying ally in some run");
}

#[test]
fn the_accounts_definition_and_its_revision_are_used_and_fingerprinted() {
    let mut s = setup("Mage", "dnd2014", 4, "none");
    let mut mech = party_spells::data::curated("ID_PHB_SPELL_FIREBALL")
        .unwrap()
        .clone();
    mech.rev = 7;
    mech.name = "Fireball".into();
    s.spells.defs.insert(
        "ID_PHB_SPELL_FIREBALL".into(),
        SpellDefn {
            mech,
            origin: MechOrigin::Def {
                id: "ID_PHB_SPELL_FIREBALL".into(),
                rev: 7,
            },
        },
    );
    let a = Sim::new(&s).unwrap();
    assert_eq!(
        a.fingerprints["spell:ID_PHB_SPELL_FIREBALL"],
        "def:ID_PHB_SPELL_FIREBALL:7"
    );
    let fb = a
        .state
        .combatant(&caster())
        .unwrap()
        .abilities
        .iter()
        .find(|x| x.name == "fireball")
        .unwrap();
    let spec: &CastSpec = fb.mech.as_deref().unwrap();
    assert_eq!(spec.dc, Some(14), "the caster's DC");
    assert!(matches!(spec.origin, MechOrigin::Def { rev: 7, .. }));
    let b = Sim::new(&setup("Mage", "dnd2014", 4, "none")).unwrap();
    assert_ne!(
        a.fingerprints, b.fingerprints,
        "the setup's fingerprints change with the definition"
    );
}

#[test]
fn spell_runs_are_deterministic() {
    for (n, r) in [
        ("Mage", "dnd2014"),
        ("Cult Fanatic", "dnd2014"),
        ("Mage", "dnd2024"),
    ] {
        let run = |seed| {
            let mut sim = Sim::new(&setup(n, r, seed, "none")).unwrap();
            let _ = sim.advance();
            serde_json::to_string(&report(&sim)).unwrap()
        };
        assert_eq!(run(12), run(12), "{n}");
        let t = std::thread::spawn(move || {
            let mut sim = Sim::new(&setup(n, r, 12, "none")).unwrap();
            let _ = sim.advance();
            serde_json::to_string(&report(&sim)).unwrap()
        })
        .join()
        .unwrap();
        assert_eq!(run(12), t);
    }
}

#[test]
fn an_adjudicated_spell_becomes_an_unsaved_draft() {
    // Command (5.1) has no rules row yet: an adjudication of it is shown as a builder draft, never saved.
    let s = setup("Cult Fanatic", "dnd2014", 2, "foes");
    let mut sim = Sim::new(&s).unwrap();
    struct Adjudicate(bool);
    impl Oracle for Adjudicate {
        fn decide(
            &mut self,
            sim: &Sim,
            actor: &CombatantId,
            card: &Value,
        ) -> Option<(Decision, By)> {
            if !self.0 {
                if let Some(o) = sim.options(actor).iter().find(|o| o.name == "command") {
                    let dc = card["adjudicate"]
                        .as_array()
                        .and_then(|a| a.iter().find(|x| x["o"] == o.o.as_str()))
                        .and_then(|x| x["numbers"].as_array().cloned())
                        .and_then(|n| n.iter().position(|x| x["kind"] == "dc"));
                    if let Some(dc) = dc {
                        self.0 = true;
                        let adj: party_sim::adjudicate::Adj = serde_json::from_value(serde_json::json!({
                            "as": "condition", "condition": "prone", "ability": "wis", "dc_ref": dc, "turns": 1,
                            "targets": [o.targets[0].0.clone()]
                        }))
                        .unwrap();
                        return Some((
                            Decision {
                                o: o.o.clone(),
                                t: vec![o.targets[0].0.clone()],
                                adj: Some(adj),
                                ..Decision::default()
                            },
                            By::Agent,
                        ));
                    }
                }
            }
            Some((party_sim::basic::decide(sim, actor), By::Agent))
        }
    }
    sim.run_with(&mut Adjudicate(false));
    let r = report(&sim);
    assert!(!r.adjudications.is_empty(), "{:?}", r.decisions);
    let d = r
        .drafts
        .iter()
        .find(|d| d.spell == "command")
        .expect("a draft");
    assert!(d.status.starts_with("unreviewed"));
    assert_eq!(d.mech.rev, 0);
    assert!(
        sim.tallies[&caster()].slots_spent.contains_key(&1),
        "the cast spent its slot"
    );
}
