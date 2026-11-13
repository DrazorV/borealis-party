//! Conditions (C10), concentration (C11), active effects with expiry, cascade and follow-ups (C12), and AC and
//! speed modifiers (C13, C22).

mod common;
use common::*;
use party_combat::party_dice::D20Input;
use party_combat::*;

fn effects_on<'a>(combat: &'a Combat, target: &str) -> Vec<&'a str> {
    combat
        .state
        .effects
        .iter()
        .filter(|e| e.spec.target == id(target))
        .map(|e| e.spec.name.as_str())
        .collect()
}

#[test]
fn conditions_are_effects_and_respect_immunity() {
    let mut combat = Combat::new(Edition::Srd51, 1);
    add(&mut combat, skeleton("s"));
    let mut spec = effect("", "s");
    spec.conditions = vec![Condition::Poisoned];
    let ev = dm(
        &mut combat,
        Command::AddCondition {
            effect: Box::new(spec),
        },
    );
    assert!(ev.dm.iter().any(|e| e.kind == DmEventKind::ConditionImmune));
    assert!(combat.state.effects.is_empty());
    let mut spec = effect("", "s");
    spec.conditions = vec![Condition::Prone, Condition::Poisoned];
    dm(
        &mut combat,
        Command::AddCondition {
            effect: Box::new(spec),
        },
    );
    assert_eq!(
        derive::conditions(&combat.state, &id("s")),
        vec![Condition::Prone]
    );
    assert_eq!(effects_on(&combat, "s"), ["prone"]);
    dm(
        &mut combat,
        Command::RemoveCondition {
            target: id("s"),
            condition: Condition::Prone,
        },
    );
    assert!(combat.state.effects.is_empty());
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::RemoveCondition {
                target: id("s"),
                condition: Condition::Prone
            }
        ),
        CombatError::Invalid(_)
    ));
}

#[test]
fn removing_one_condition_keeps_an_effect_that_gives_more() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, owlbear("o"));
    let mut spec = effect("Net", "o");
    spec.conditions = vec![Condition::Restrained, Condition::Prone];
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    dm(
        &mut combat,
        Command::RemoveCondition {
            target: id("o"),
            condition: Condition::Prone,
        },
    );
    assert_eq!(
        derive::conditions(&combat.state, &id("o")),
        vec![Condition::Restrained]
    );
    assert_eq!(effects_on(&combat, "o"), ["Net"]);
}

#[test]
fn incapacitating_conditions_end_concentration() {
    for condition in Condition::ALL.iter().copied() {
        let mut combat = Combat::new(Edition::Srd521, 1);
        add(&mut combat, mage("m"));
        dm(
            &mut combat,
            Command::SetConcentration {
                id: id("m"),
                name: "Fly".into(),
            },
        );
        let mut spec = effect("", "m");
        spec.conditions = vec![condition];
        dm(
            &mut combat,
            Command::AddCondition {
                effect: Box::new(spec),
            },
        );
        let kept = c(&combat, "m").concentration.is_some();
        assert_eq!(kept, !condition.incapacitates(), "{condition}");
    }
    let ends: Vec<&str> = Condition::ALL
        .iter()
        .filter(|c| c.incapacitates())
        .map(|c| c.as_str())
        .collect();
    assert_eq!(
        ends,
        [
            "incapacitated",
            "paralyzed",
            "petrified",
            "stunned",
            "unconscious"
        ]
    );
}

#[test]
fn damage_raises_a_concentration_check_resolved_by_a_typed_save() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, mage("m"));
    dm(
        &mut combat,
        Command::SetConcentration {
            id: id("m"),
            name: "Fly".into(),
        },
    );
    let ev = dm(&mut combat, hit("m", 30, None));
    assert!(ev
        .dm
        .iter()
        .any(|e| e.kind == DmEventKind::ConcentrationCheck { dc: 15 }));
    dm(&mut combat, hit("m", 4, None));
    assert_eq!(c(&combat, "m").pending_concentration.len(), 2);
    dm(
        &mut combat,
        Command::ResolveConcentration {
            id: id("m"),
            roll: D20Input::typed(15),
        },
    );
    assert!(c(&combat, "m").concentration.is_some(), "15 meets DC 15");
    dm(
        &mut combat,
        Command::ResolveConcentration {
            id: id("m"),
            roll: D20Input::typed(9),
        },
    );
    assert!(c(&combat, "m").concentration.is_none(), "9 fails DC 10");
    assert_eq!(
        dm_err(
            &mut combat,
            Command::ResolveConcentration {
                id: id("m"),
                roll: D20Input::typed(20)
            }
        ),
        CombatError::NoPendingCheck
    );
}

#[test]
fn immune_damage_needs_no_concentration_check() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, adult_red_dragon("d"));
    dm(
        &mut combat,
        Command::SetConcentration {
            id: id("d"),
            name: "Hold".into(),
        },
    );
    dm(&mut combat, hit("d", 30, Some(DamageType::Fire)));
    assert!(c(&combat, "d").pending_concentration.is_empty());
}

#[test]
fn zero_hp_ends_concentration_and_its_effects() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, mage("m"));
    add(&mut combat, goblin("g"));
    let mut spec = effect("Fly", "g");
    spec.source = Some(id("m"));
    spec.concentration = true;
    spec.modifiers = vec![Modifier::SpeedAdd(30)];
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    assert_eq!(
        c(&combat, "m")
            .concentration
            .as_ref()
            .map(|k| k.name.as_str()),
        Some("Fly")
    );
    dm(&mut combat, hit("m", 40, None));
    assert!(c(&combat, "m").concentration.is_none());
    assert!(combat.state.effects.is_empty());
}

/// C12: concentrating on something new ends the old effects; the last held effect ending ends concentration.
#[test]
fn concentration_cascade() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, mage("m"));
    add(&mut combat, goblin("g1"));
    add(&mut combat, goblin("g2"));
    for t in ["g1", "g2"] {
        let mut spec = effect("Bane", t);
        spec.source = Some(id("m"));
        spec.concentration = true;
        dm(
            &mut combat,
            Command::AddEffect {
                effect: Box::new(spec),
            },
        );
    }
    assert_eq!(combat.state.effects.len(), 2);
    // One target's effect dispelled: still concentrating on the other.
    let first = combat.state.effects[0].id;
    dm(
        &mut combat,
        Command::RemoveEffect {
            effect: first,
            skip_follow_up: false,
        },
    );
    assert!(c(&combat, "m").concentration.is_some());
    // A new concentration spell ends the old one and its effects.
    let mut spec = effect("Fly", "m");
    spec.source = Some(id("m"));
    spec.concentration = true;
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    assert_eq!(effects_on(&combat, "g2"), Vec::<&str>::new());
    assert_eq!(effects_on(&combat, "m"), ["Fly"]);
    // The last held effect removed ends concentration.
    let fly = combat.state.effects[0].id;
    dm(
        &mut combat,
        Command::RemoveEffect {
            effect: fly,
            skip_follow_up: false,
        },
    );
    assert!(c(&combat, "m").concentration.is_none());
}

/// C12: a duration counts the SOURCE's turns; an expiring effect can leave a follow-up (the 2014 Haste lethargy).
#[test]
fn source_turn_duration_and_follow_up() {
    let mut combat = Combat::new(Edition::Srd51, 1);
    add(&mut combat, mage("m"));
    add(&mut combat, knight("k"));
    init(&mut combat, "m", 15);
    init(&mut combat, "k", 10);
    let mut lethargy = effect("Haste lethargy", "k");
    lethargy.conditions = vec![Condition::Incapacitated];
    lethargy.duration = Duration::TargetTurns {
        remaining: 1,
        at: Boundary::End,
    };
    let mut haste = effect("Haste", "k");
    haste.source = Some(id("m"));
    haste.concentration = true;
    haste.duration = Duration::SourceTurns {
        remaining: 2,
        at: Boundary::Start,
    };
    haste.modifiers = vec![Modifier::Ac(2), Modifier::SpeedMultiply(2)];
    haste.follow_up = Some(Box::new(lethargy));
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(haste),
        },
    );
    assert_eq!(
        derive::effective_ac(&combat.state, c(&combat, "k")).total,
        Some(20)
    );
    assert_eq!(
        derive::effective_speed(&combat.state, c(&combat, "k")).walk,
        60
    );

    dm(&mut combat, Command::Start); // mage's turn start: 2 -> 1
    assert_eq!(effects_on(&combat, "k"), ["Haste"]);
    dm(&mut combat, Command::NextTurn); // knight
    dm(&mut combat, Command::NextTurn); // round 2, mage's turn start: 1 -> 0, Haste ends
    assert_eq!(effects_on(&combat, "k"), ["Haste lethargy"]);
    assert!(c(&combat, "m").concentration.is_none());
    assert!(derive::incapacitated(&combat.state, &id("k")));
    dm(&mut combat, Command::NextTurn); // knight's turn
    assert_eq!(effects_on(&combat, "k"), ["Haste lethargy"]);
    dm(&mut combat, Command::NextTurn); // knight's turn ended
    assert!(effects_on(&combat, "k").is_empty());
}

#[test]
fn removing_with_skip_follow_up_leaves_nothing_behind() {
    let mut combat = Combat::new(Edition::Srd51, 1);
    add(&mut combat, knight("k"));
    let mut spec = effect("Haste", "k");
    spec.follow_up = Some(Box::new(effect("Haste lethargy", "k")));
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    let e = combat.state.effects[0].id;
    dm(
        &mut combat,
        Command::RemoveEffect {
            effect: e,
            skip_follow_up: true,
        },
    );
    assert!(combat.state.effects.is_empty());
}

#[test]
fn charges_end_the_effect() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, knight("k"));
    let mut spec = effect("Ward", "k");
    spec.charges = Some(2);
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    let e = combat.state.effects[0].id;
    dm(&mut combat, Command::UseCharge { effect: e });
    assert_eq!(combat.state.effects[0].spec.charges, Some(1));
    dm(&mut combat, Command::UseCharge { effect: e });
    assert!(combat.state.effects.is_empty());
    assert_eq!(
        dm_err(&mut combat, Command::UseCharge { effect: e }),
        CombatError::UnknownEffect(e)
    );
}

#[test]
fn a_repeated_save_is_raised_at_the_turn_end_and_ends_on_success() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, goblin("g"));
    add(&mut combat, mage("m"));
    init(&mut combat, "g", 15);
    init(&mut combat, "m", 10);
    let mut spec = effect("Hold Person", "g");
    spec.source = Some(id("m"));
    spec.concentration = true;
    spec.conditions = vec![Condition::Paralyzed];
    spec.repeat_save = Some(RepeatSave {
        ability: AbilityName::Wis,
        dc: 13,
        at: Boundary::End,
    });
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    let e = combat.state.effects[0].id;
    dm(&mut combat, Command::Start);
    let ev = dm(&mut combat, Command::NextTurn);
    assert!(ev
        .dm
        .iter()
        .any(|d| d.kind == DmEventKind::SaveDue { effect: e }));
    dm(
        &mut combat,
        Command::ResolveEffectSave {
            effect: e,
            roll: D20Input::typed(12),
        },
    );
    assert_eq!(combat.state.effects.len(), 1);
    scripted(
        &mut combat,
        Command::ResolveEffectSave {
            effect: e,
            roll: D20Input::app(0, Default::default()),
        },
        &[13],
    );
    assert!(combat.state.effects.is_empty());
    assert!(
        c(&combat, "m").concentration.is_none(),
        "the held effect ending ends concentration"
    );
}

/// C13 and C22: cover from a mount ends when the mount drops.
#[test]
fn mounted_cover_ends_when_the_mount_drops() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, knight("k"));
    add(&mut combat, warhorse("h"));
    let mut cover = effect("Mounted", "k");
    cover.source = Some(id("h"));
    cover.modifiers = vec![Modifier::Cover(Cover::Half)];
    cover.ends_on_defeat_of = vec![id("h")];
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(cover),
        },
    );
    let mut shield = effect("Shield of Faith", "k");
    shield.modifiers = vec![Modifier::Ac(2), Modifier::Cover(Cover::ThreeQuarters)];
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(shield),
        },
    );
    let ac = derive::effective_ac(&combat.state, c(&combat, "k"));
    assert_eq!(
        (ac.base, ac.effects, ac.cover, ac.total),
        (Some(18), 2, 5, Some(25)),
        "covers don't stack"
    );
    dm(&mut combat, hit("h", 50, None));
    assert_eq!(effects_on(&combat, "k"), ["Shield of Faith"]);
}

#[test]
fn speed_modifiers_conditions_and_exhaustion() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, adult_red_dragon("d"));
    let speed = |combat: &Combat| derive::effective_speed(&combat.state, c(combat, "d"));
    assert_eq!((speed(&combat).walk, speed(&combat).fly), (40, Some(80)));
    let mut spec = effect("Longstrider", "d");
    spec.modifiers = vec![Modifier::SpeedAdd(10)];
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    let mut slow = effect("Slow", "d");
    slow.modifiers = vec![Modifier::SpeedHalve];
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(slow),
        },
    );
    assert_eq!((speed(&combat).walk, speed(&combat).fly), (25, Some(45)));
    dm(
        &mut combat,
        Command::SetExhaustion {
            id: id("d"),
            level: 2,
        },
    );
    assert_eq!(speed(&combat).walk, 15, "2024: -5 ft per level");
    let mut grab = effect("", "d");
    grab.conditions = vec![Condition::Grappled];
    dm(
        &mut combat,
        Command::AddCondition {
            effect: Box::new(grab),
        },
    );
    assert_eq!(
        (
            speed(&combat).walk,
            speed(&combat).fly,
            speed(&combat).climb
        ),
        (0, Some(0), Some(0))
    );

    let mut old = Combat::new(Edition::Srd51, 1);
    add(&mut old, owlbear("o"));
    dm(
        &mut old,
        Command::SetExhaustion {
            id: id("o"),
            level: 2,
        },
    );
    assert_eq!(
        derive::effective_speed(&old.state, c(&old, "o")).walk,
        20,
        "2014: halved at 2"
    );
    dm(
        &mut old,
        Command::SetExhaustion {
            id: id("o"),
            level: 5,
        },
    );
    assert_eq!(
        derive::effective_speed(&old.state, c(&old, "o")).walk,
        0,
        "2014: 0 at 5"
    );
    dm(
        &mut old,
        Command::SetExhaustion {
            id: id("o"),
            level: 6,
        },
    );
    assert!(c(&old, "o").life.is_dead());
}

#[test]
fn effect_validation() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, goblin("g"));
    let mut spec = effect("Bless", "g");
    spec.duration = Duration::SourceTurns {
        remaining: 10,
        at: Boundary::Start,
    };
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::AddEffect {
                effect: Box::new(spec)
            }
        ),
        CombatError::Invalid(_)
    ));
    let mut spec = effect("Bless", "g");
    spec.concentration = true;
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::AddEffect {
                effect: Box::new(spec)
            }
        ),
        CombatError::Invalid(_)
    ));
    let spec = effect("Bless", "nobody");
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::AddEffect {
                effect: Box::new(spec)
            }
        ),
        CombatError::UnknownCombatant(_)
    ));
}
