//! Property tests: random command sequences from many seeds, checked after every command for the invariants the
//! server and the projection rely on. No extra dependency: the generator is the crate's own seeded die.

mod common;
use common::*;
use party_combat::party_dice::{D20Input, DiceInput, DieRoller, RollMode, SeededDie};
use party_combat::*;

const HIDDEN: [&str; 2] = ["hidden-dragon", "hidden-goblin"];
const ALL: [&str; 9] = [
    "hidden-dragon",
    "hidden-goblin",
    "g",
    "o",
    "m",
    "k",
    "h",
    "w",
    "f",
];

fn roster(seed: u32) -> Combat {
    let mut combat = Combat::new(
        if seed.is_multiple_of(2) {
            Edition::Srd521
        } else {
            Edition::Srd51
        },
        seed,
    );
    add(&mut combat, adult_red_dragon("hidden-dragon"));
    add(&mut combat, goblin("hidden-goblin"));
    for c in [
        goblin("g"),
        owlbear("o"),
        mage("m"),
        knight("k"),
        warhorse("h"),
        wizard_pc("w"),
        fighter_pc("f"),
    ] {
        let mut c = c;
        c.visible = true;
        add(&mut combat, c);
    }
    let mut engine_w = c(&combat, "w").clone();
    engine_w.hp_authority = HpAuthority::Engine; // so random damage exercises the death-save machine
    combat
        .state
        .combatants
        .iter_mut()
        .filter(|x| x.id == id("w"))
        .for_each(|x| *x = engine_w.clone());
    dm(
        &mut combat,
        Command::Link {
            id: id("h"),
            to: Some(id("k")),
        },
    );
    let pool = LegendaryPool {
        max: 3,
        remaining: 3,
        members: vec![id("hidden-dragon"), id("o")],
        guessed: false,
    };
    dm(
        &mut combat,
        Command::SetLegendaryPool {
            pool: "p".into(),
            value: pool,
        },
    );
    dm(
        &mut combat,
        Command::RollInitiative {
            ids: vec![],
            only_missing: false,
        },
    );
    for (k, v) in [("w", 13), ("f", 7)] {
        init(&mut combat, k, v);
    }
    combat
}

struct Gen(SeededDie);

impl Gen {
    fn n(&mut self, sides: u32) -> u32 {
        self.0.roll(sides).unwrap()
    }
    fn who(&mut self) -> CombatantId {
        id(ALL[self.n(ALL.len() as u32) as usize - 1])
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.n(xs.len() as u32) as usize - 1]
    }
}

fn random_command(g: &mut Gen, state: &CombatState) -> Command {
    let effect_id = state.effects.first().map_or(EffectId(1), |e| e.id);
    match g.n(34) {
        1..=4 => Command::NextTurn,
        5 => Command::PreviousTurn,
        6 | 7 => Command::Damage {
            target: g.who(),
            parts: vec![DamagePart {
                amount: if g.n(2) == 1 {
                    DiceInput::typed(g.n(30) as i32)
                } else {
                    DiceInput::app("2d8+2")
                },
                damage_type: Some(g.pick(DamageType::ALL)),
            }],
            magical: g.n(2) == 1,
            apply_qualified: false,
            critical_hit: g.n(4) == 1,
        },
        8 => Command::Heal {
            target: g.who(),
            amount: DiceInput::typed(g.n(20) as i32),
        },
        9 | 29 | 30 => {
            // Aim at someone dying, when there is one, so the machine gets exercised.
            let dying = state
                .combatants
                .iter()
                .find(|c| matches!(c.life, Life::Dying { .. }))
                .map(|c| c.id.clone());
            let who = dying.unwrap_or_else(|| g.who());
            if g.n(8) == 1 {
                Command::Stabilize { id: who }
            } else {
                let n = g.n(20);
                Command::DeathSave {
                    id: who,
                    roll: D20Input::typed_natural(n as i32, n),
                }
            }
        }
        10 => {
            let mut spec = EffectSpec::new("Spell", g.who());
            spec.source = Some(g.who());
            spec.concentration = g.n(2) == 1;
            spec.duration = Duration::SourceTurns {
                remaining: g.n(3),
                at: g.pick(&[Boundary::Start, Boundary::End]),
            };
            spec.modifiers = vec![Modifier::Ac(2)];
            if g.n(2) == 1 {
                spec.follow_up = Some(Box::new(EffectSpec::new("After", spec.target.clone())));
            }
            Command::AddEffect {
                effect: Box::new(spec),
            }
        }
        11 => {
            let mut spec = EffectSpec::new("", g.who());
            spec.conditions = vec![g.pick(Condition::ALL)];
            Command::AddCondition {
                effect: Box::new(spec),
            }
        }
        12 => Command::RemoveCondition {
            target: g.who(),
            condition: g.pick(Condition::ALL),
        },
        13 => Command::SetConcentration {
            id: g.who(),
            name: g.pick(&["Bless", "Fly"]).into(),
        },
        14 => Command::ResolveConcentration {
            id: g.who(),
            roll: D20Input::app(2, RollMode::Normal),
        },
        15 => Command::UseAbility {
            id: g.who(),
            ability: g
                .pick(&["tail", "wing", "fire-breath", "multiattack", "cast"])
                .into(),
            slot: None,
        },
        16 => Command::Reveal { id: g.who() },
        17 => Command::Hide { id: g.who() },
        18 => Command::RemoveEffect {
            effect: effect_id,
            skip_follow_up: g.n(2) == 1,
        },
        19 => Command::SetHp {
            id: g.who(),
            current: g.n(60) as i32 - 1,
            max: None,
            temp: None,
        },
        20 => Command::RemoveCombatant { id: g.who() },
        21 => Command::SetExhaustion {
            id: g.who(),
            level: g.n(7) as u8 - 1,
        },
        22 => Command::Start,
        23 => Command::SetInitiative {
            id: g.who(),
            roll: D20Input::typed(g.n(25) as i32),
        },
        25 => {
            let n = state.next_tiebreak;
            Command::Summon {
                owner: g.who(),
                combatant: Box::new(Combatant::creature(format!("pet-{n}"), "Wolf", 11, 13, 2)),
                initiative: if g.n(3) == 1 {
                    Some(D20Input::typed(g.n(20) as i32))
                } else {
                    None
                },
                visible: if g.n(2) == 1 { Some(g.n(2) == 1) } else { None },
            }
        }
        26 => {
            let pet = state
                .combatants
                .iter()
                .rev()
                .find(|c| c.owner_id.is_some() && !c.removed)
                .map(|c| c.id.clone());
            Command::Dismiss {
                id: pet.unwrap_or_else(|| g.who()),
            }
        }
        27 => Command::Attack {
            attacker: g.who(),
            target: g.who(),
            label: String::new(),
            roll: if g.n(2) == 1 {
                D20Input::typed(g.n(25) as i32)
            } else {
                D20Input::app(4, RollMode::Normal)
            },
        },
        28 => Command::Roll {
            id: g.who(),
            label: "check".into(),
            roll: RollInput::Dice(DiceInput::app("1d8")),
        },
        // 0.3.2: casts, rows, applying, cancelling, standard actions.
        29..=31 => {
            let name = g.pick(&[
                "fireball",
                "hold-person",
                "bless",
                "mage-armor",
                "cure-wounds",
            ]);
            let mut targets = vec![g.who()];
            let second = g.who();
            if !targets.contains(&second) && name != "mage-armor" && name != "cure-wounds" {
                targets.push(second);
            }
            let rows = if g.n(2) == 1 {
                targets
                    .iter()
                    .map(|t| RowInput {
                        target: t.clone(),
                        ray: 0,
                        roll: if g.n(2) == 1 {
                            RowRoll::App
                        } else {
                            RowRoll::Typed {
                                total: g.n(25) as i32,
                                natural: None,
                            }
                        },
                        use_lr: false,
                    })
                    .collect()
            } else {
                vec![]
            };
            let amounts = if g.n(2) == 1 {
                vec![AmountRoll::App]
            } else {
                vec![]
            };
            Command::CastSpell {
                caster: id(g.pick(&["m", "hidden-goblin", "w"])),
                ability: None,
                spec: Some(Box::new(spec_of(name, state.edition, 13, Some(5)))),
                slot: Some(g.pick(&[1, 2, 3, 4])),
                targets,
                rows,
                amounts,
                crit_amounts: vec![],
                choices: spell::CastChoices::default(),
                precast: g.n(4) == 1,
                force: g.n(2) == 1,
                id: None,
            }
        }
        32 => match state.casts.first() {
            Some(cast) => {
                let row = &cast.rows[g.n(cast.rows.len().max(1) as u32) as usize - 1];
                Command::ResolveCastRow {
                    cast: cast.id.clone(),
                    target: row.target.clone(),
                    ray: row.ray,
                    roll: if g.n(2) == 1 {
                        RowRoll::App
                    } else {
                        RowRoll::Typed {
                            total: g.n(25) as i32,
                            natural: Some(g.n(20)),
                        }
                    },
                    use_lr: g.n(5) == 1,
                }
            }
            None => Command::NextTurn,
        },
        33 => match state.casts.first() {
            Some(cast) if g.n(3) == 1 => Command::CancelCast {
                cast: cast.id.clone(),
                refund: g.n(2) == 1,
            },
            Some(cast) => Command::ApplyCast {
                cast: cast.id.clone(),
                amounts: vec![],
                crit_amounts: vec![],
                force: g.n(2) == 1,
            },
            None => Command::StandardAction {
                id: g.who(),
                action: g.pick(&[
                    StandardActionKind::Dodge,
                    StandardActionKind::Dash,
                    StandardActionKind::Disengage,
                ]),
                target: None,
                bonus: g.n(2) == 1,
            },
        },
        _ => Command::UseCharge { effect: effect_id },
    }
}

fn check_invariants(s: &CombatState) {
    for c in &s.combatants {
        assert!(
            c.hp.current >= 0 && c.hp.current <= c.hp.max && c.hp.temp >= 0,
            "{}: {:?}",
            c.id,
            c.hp
        );
        if c.hp_authority == HpAuthority::Engine && c.hp.max > 0 {
            if c.life.is_alive() {
                assert!(c.hp.current > 0, "{} alive at 0 HP", c.id);
            }
            if matches!(c.life, Life::Dying { .. } | Life::Stable) {
                assert_eq!(c.hp.current, 0, "{} dying above 0", c.id);
            }
        }
        if let Life::Dying {
            successes,
            failures,
        } = c.life
        {
            assert!(successes < 3 && failures < 3);
        }
        assert!(c.exhaustion <= 6);
    }
    for e in &s.effects {
        let target = s.combatant(&e.spec.target).expect("target exists");
        assert!(!target.removed, "effect {} on a removed combatant", e.id.0);
        if e.spec.concentration {
            let src = s
                .combatant(e.spec.source.as_ref().expect("source"))
                .expect("source exists");
            assert_eq!(
                src.concentration.as_ref().map(|k| k.name.as_str()),
                Some(e.spec.name.as_str()),
                "a held effect without its concentration"
            );
        }
        if e.origin == EffectOrigin::Dying {
            assert!(!target.life.is_alive(), "Dying effect on a living creature");
        }
        assert!(e.id.0 < s.next_effect);
    }
    for c in &s.combatants {
        if c.concentration.is_some() {
            assert!(c.life.is_alive(), "{} concentrates at 0 HP", c.id);
        }
    }
    for c in &s.combatants {
        if let Some(o) = &c.owner_id {
            assert!(s.combatant(o).is_some(), "{} has an unknown owner", c.id);
        }
        if let Controller::Player { pc } = &c.controller {
            assert!(
                s.combatant(pc).is_some_and(|p| p.is_character()),
                "{} has a bad controller",
                c.id
            );
            assert!(c.visible, "a player's creature {} is hidden", c.id);
        }
    }
    for p in s.legendary_pools.values() {
        assert!(p.remaining <= p.max);
    }
}

fn leaks(events: &Events, before: &CombatState, state: &CombatState) -> bool {
    let text = serde_json::to_string(&events.players).unwrap();
    let seen = |s: &CombatState, h: &str| s.combatant(&id(h)).is_some_and(|c| c.visible);
    HIDDEN
        .iter()
        .any(|h| text.contains(h) && !seen(before, h) && !seen(state, h))
}

#[test]
fn random_sequences_keep_the_invariants_undo_and_determinism() {
    // What the runs reached, so a generator that never gets anywhere fails the test.
    let mut seen = std::collections::BTreeMap::<&str, usize>::new();
    for seed in 0..150u32 {
        let mut combat = roster(seed);
        let mut g = Gen(SeededDie::new(seed.wrapping_mul(2_654_435_761)));
        for step in 0..80 {
            let cmd = random_command(&mut g, &combat.state);
            let prev = combat.state.clone();
            let die_before = combat.die;
            match combat.execute(&Actor::Dm, &cmd) {
                Err(_) => {
                    assert_eq!(combat.state, prev, "a refused command changed the state");
                    continue;
                }
                Ok(events) => {
                    check_invariants(&combat.state);
                    *seen.entry("ok").or_default() += 1;
                    for e in &events.dm {
                        let tag = match &e.kind {
                            DmEventKind::Dying => "dying",
                            DmEventKind::Died => "died",
                            DmEventKind::Stabilized => "stable",
                            DmEventKind::Revived => "revived",
                            DmEventKind::EffectEnded { reason, .. }
                                if reason == "duration expired" =>
                            {
                                "expired"
                            }
                            DmEventKind::EffectEnded { reason, .. }
                                if reason.starts_with("concentration") =>
                            {
                                "cascade"
                            }
                            DmEventKind::ConcentrationEnded { .. } => "conc-ended",
                            DmEventKind::RoundStarted => "round",
                            DmEventKind::LegendaryReset { .. } => "legendary-reset",
                            DmEventKind::Unlinked { .. } => "unlinked",
                            DmEventKind::Summoned { .. } => "summoned",
                            DmEventKind::OwnerDefeated { .. } => "owner-defeated",
                            DmEventKind::Attack { .. } => "attack",
                            DmEventKind::CastApplied { .. } => "cast-applied",
                            DmEventKind::CastRow { .. } => "cast-row",
                            DmEventKind::EffectAdded { .. }
                                if cmd.label() != "add effect"
                                    && cmd.label() != "add condition" =>
                            {
                                "follow-up"
                            }
                            _ => continue,
                        };
                        *seen.entry(tag).or_default() += 1;
                    }
                    assert!(
                        !leaks(&events, &prev, &combat.state),
                        "seed {seed} step {step}: {cmd:?} leaked {:?}",
                        events.players
                    );
                    let after = combat.state.clone();
                    // Undo gives back the exact previous state...
                    combat.undo(&Actor::Dm, true).unwrap();
                    assert_eq!(
                        combat.state, prev,
                        "seed {seed} step {step}: undo of {cmd:?}"
                    );
                    // ...and replaying with the same die gives the same state.
                    combat.die = die_before;
                    combat.execute(&Actor::Dm, &cmd).unwrap();
                    assert_eq!(
                        combat.state, after,
                        "seed {seed} step {step}: replay of {cmd:?}"
                    );
                }
            }
            if combat.state.status == Status::Ended {
                break;
            }
        }
        let back = import_json(&export_json(&combat), 1).unwrap();
        assert_eq!(back.state, combat.state, "seed {seed}: export round trip");
    }
    eprintln!("reached: {seen:?}");
    for tag in [
        "ok",
        "dying",
        "died",
        "stable",
        "revived",
        "expired",
        "cascade",
        "conc-ended",
        "round",
        "legendary-reset",
        "unlinked",
        "follow-up",
        "cast-applied",
        "cast-row",
        "summoned",
        "owner-defeated",
        "attack",
    ] {
        assert!(
            seen.get(tag).copied().unwrap_or(0) > 0,
            "the random runs never reached {tag}: {seen:?}"
        );
    }
}

/// Commands that touch only hidden creatures never produce player events.
#[test]
fn hidden_only_commands_are_silent_for_players() {
    for seed in 0..100u32 {
        let mut combat = roster(seed);
        dm(&mut combat, Command::Start);
        let mut g = Gen(SeededDie::new(seed + 7));
        for _ in 0..40 {
            let target = id(g.pick(&HIDDEN));
            let cmd = match g.n(6) {
                1 => hit(&target.0, g.n(40) as i32, Some(g.pick(DamageType::ALL))),
                2 => Command::Heal {
                    target: target.clone(),
                    amount: DiceInput::typed(5),
                },
                3 => Command::AdjustLegendaryResistance {
                    id: target.clone(),
                    delta: -1,
                },
                4 => {
                    let mut spec = EffectSpec::new("", target.clone());
                    spec.conditions = vec![g.pick(Condition::ALL)];
                    Command::AddCondition {
                        effect: Box::new(spec),
                    }
                }
                5 => Command::SetNotes {
                    id: target.clone(),
                    notes: "x".into(),
                },
                _ => Command::SetConcentration {
                    id: target.clone(),
                    name: "Hold".into(),
                },
            };
            if let Ok(ev) = combat.execute(&Actor::Dm, &cmd) {
                assert!(
                    ev.players.is_empty(),
                    "seed {seed}: {cmd:?} -> {:?}",
                    ev.players
                );
            }
        }
    }
}
