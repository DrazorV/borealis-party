//! Opt-in: every spell of a local 5etools checkout, drafted by `party-spells` and cast in a simulation by an SRD
//! Mage in place of its Fireball. Set `BOREALIS_TEST_5ETOOLS_DIR` to the folder that holds `data/`. Prints counts
//! only; never names a spell. Skipped when the variable is not set.

mod common;

use party_combat::spell::MechOrigin;
use party_combat::Edition;
use party_sim::sheet::{Coverage, Def};
use party_sim::spells::{SpellDefn, SpellDefs};
use party_sim::{report, Sim, SimOpts, SimSetup};
use serde_json::Value;

fn spells_of(dir: &str) -> Vec<Value> {
    let folder = std::path::Path::new(dir).join("data").join("spells");
    let mut out = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap_or_else(|e| panic!("{}: {e}", folder.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("spells-") && n.ends_with(".json"))
        })
        .collect();
    files.sort();
    for f in files {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        out.extend(v["spell"].as_array().cloned().unwrap_or_default());
    }
    out
}

#[test]
fn every_drafted_spell_casts_in_a_simulation() {
    let Ok(dir) = std::env::var("BOREALIS_TEST_5ETOOLS_DIR") else {
        eprintln!("skipped: set BOREALIS_TEST_5ETOOLS_DIR to a local 5etools source checkout");
        return;
    };
    let spells = spells_of(&dir);
    assert!(!spells.is_empty(), "no spells in {dir}");
    let (mut automated, mut adjudicable, mut not_simulated, mut cast, mut runs) = (0, 0, 0, 0, 0);
    for (i, raw) in spells.iter().enumerate() {
        let new = raw["source"].as_str() == Some("XPHB");
        let (edition, ruleset, key) = if new {
            (Edition::Srd521, "dnd2024", "ID_2024_SPELL_FIREBALL")
        } else {
            (Edition::Srd51, "dnd2014", "ID_PHB_SPELL_FIREBALL")
        };
        let draft = party_spells::draft::from_5etools(&format!("CORPUS_{i}"), edition, raw);
        let mut defs = SpellDefs::default();
        defs.defs.insert(
            key.into(),
            SpellDefn {
                mech: draft.mech,
                origin: MechOrigin::Def {
                    id: key.into(),
                    rev: 0,
                },
                drafted: true,
            },
        );
        let setup = SimSetup {
            edition,
            creatures: vec![common::creature("ec-mage", "Mage", ruleset, "Mage")],
            pcs: common::pcs(),
            start_effects: vec![],
            rules: vec![],
            opts: SimOpts {
                max_rounds: 3,
                ..SimOpts::default()
            },
            seed: u32::try_from(i).unwrap_or(0),
            spells: defs,
        };
        let mut setup = setup;
        setup.opts.control.agent = party_sim::setup::AgentControl::Side("none".into());
        let mut sim = Sim::new(&setup).expect("a sim");
        let id = party_combat::CombatantId::new("c5");
        let action = sim.sheets[&id]
            .actions
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("fireball") && matches!(a.def, Def::Spell(_)))
            .cloned()
            .expect("the drafted spell on the sheet");
        match action.coverage {
            Coverage::Automated => automated += 1,
            Coverage::Adjudicable => adjudicable += 1,
            Coverage::NotSimulated => not_simulated += 1,
        }
        assert!(sim.fingerprints[&format!("spell:{key}")].starts_with("draft:"));
        let _ = sim.advance();
        let r = report(&sim);
        runs += 1;
        if sim.tallies.get(&id).is_some_and(|t| {
            t.abilities_used
                .keys()
                .any(|k| k.eq_ignore_ascii_case("fireball"))
        }) {
            cast += 1;
        }
        // Determinism on a sample.
        if i % 97 == 0 {
            let mut again = Sim::new(&setup).unwrap();
            let _ = again.advance();
            assert_eq!(
                serde_json::to_string(&report(&again)).unwrap(),
                serde_json::to_string(&r).unwrap()
            );
        }
    }
    eprintln!(
        "corpus: {runs} spells drafted and run: {automated} automated, {adjudicable} adjudicable, {not_simulated} not simulated; cast by Basic in {cast} runs"
    );
    assert_eq!(runs, spells.len());
}
