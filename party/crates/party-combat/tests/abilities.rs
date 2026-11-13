//! Action economy (C5), Use (C14), recharge (C15), per-creature and DM-linked legendary pools (C16), Legendary Resistance (C17), slots
//! and resources (C18), encounter rules (C20) and group limits (C21).

mod common;
use common::*;
use party_combat::party_dice::DiceInput;
use party_combat::*;

fn use_ab(id_: &str, ability: &str) -> Command {
    Command::UseAbility {
        id: id(id_),
        ability: ability.into(),
        slot: None,
    }
}

fn unavailable(combat: &mut Combat, cmd: Command) -> Unavailable {
    match dm_err(combat, cmd) {
        CombatError::Unavailable(u) => u,
        e => panic!("{e:?}"),
    }
}

#[test]
fn use_spends_the_action_economy_and_the_turn_start_resets_it() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, mage("m"));
    add(&mut combat, goblin("g"));
    init(&mut combat, "m", 15);
    init(&mut combat, "g", 10);
    dm(&mut combat, Command::Start);
    dm(
        &mut combat,
        Command::UseAbility {
            id: id("m"),
            ability: "cast".into(),
            slot: Some(3),
        },
    );
    assert!(c(&combat, "m").economy.action);
    assert_eq!(c(&combat, "m").spell_slots[&3].remaining, 2);
    assert_eq!(
        unavailable(&mut combat, use_ab("m", "cast")),
        Unavailable::ActionSpent
    );
    assert_eq!(
        unavailable(&mut combat, use_ab("m", "cast")).to_string(),
        "Action spent"
    );
    dm(&mut combat, use_ab("m", "shield"));
    assert_eq!(
        unavailable(&mut combat, use_ab("m", "shield")),
        Unavailable::ReactionSpent
    );
    dm(
        &mut combat,
        Command::SetEconomy {
            id: id("m"),
            action: Some(false),
            bonus_action: None,
            reaction: None,
        },
    );
    assert!(!c(&combat, "m").economy.action);
    dm(&mut combat, Command::NextTurn);
    dm(&mut combat, Command::NextTurn);
    assert_eq!(c(&combat, "m").economy, Economy::default());
}

#[test]
fn slots_run_out_and_adjust() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, mage("m"));
    dm(
        &mut combat,
        Command::AdjustSlot {
            id: id("m"),
            level: 5,
            delta: -1,
        },
    );
    let cast5 = Command::UseAbility {
        id: id("m"),
        ability: "cast".into(),
        slot: Some(5),
    };
    assert_eq!(unavailable(&mut combat, cast5), Unavailable::NoSlot(5));
    dm(
        &mut combat,
        Command::AdjustSlot {
            id: id("m"),
            level: 5,
            delta: 9,
        },
    );
    assert_eq!(
        c(&combat, "m").spell_slots[&5],
        Pool {
            max: 1,
            remaining: 1
        }
    );
    dm(
        &mut combat,
        Command::SetSlots {
            id: id("m"),
            level: 6,
            pool: Pool::full(1),
        },
    );
    assert_eq!(c(&combat, "m").spell_slots[&6].remaining, 1);
}

#[test]
fn opt_in_auto_recharge_is_spent_by_use_and_rolled_at_turn_start() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    dm(
        &mut combat,
        Command::SetSettings {
            settings: Settings {
                show_enemy_turn: false,
                auto_recharge: true,
            },
        },
    );
    add(&mut combat, adult_red_dragon("d"));
    add(&mut combat, goblin("g"));
    init(&mut combat, "d", 15);
    init(&mut combat, "g", 10);
    dm(&mut combat, Command::Start);
    dm(&mut combat, use_ab("d", "fire-breath"));
    dm(
        &mut combat,
        Command::SetEconomy {
            id: id("d"),
            action: Some(false),
            bonus_action: None,
            reaction: None,
        },
    );
    assert_eq!(
        unavailable(&mut combat, use_ab("d", "fire-breath")),
        Unavailable::NeedsRecharge
    );
    dm(&mut combat, Command::NextTurn);
    // Auto recharge with the server die: a 4 fails "Recharge 5-6"
    let ev = scripted(&mut combat, Command::NextTurn, &[4]);
    assert!(ev
        .dm
        .iter()
        .any(|e| matches!(&e.kind, DmEventKind::Recharge { ready: false, .. })));
    dm(&mut combat, Command::NextTurn);
    let ev = scripted(&mut combat, Command::NextTurn, &[5]);
    assert!(ev
        .dm
        .iter()
        .any(|e| matches!(&e.kind, DmEventKind::Recharge { ready: true, .. })));
    assert!(
        c(&combat, "d")
            .ability("fire-breath")
            .unwrap()
            .recharge
            .unwrap()
            .ready
    );
}

#[test]
fn with_auto_recharge_off_the_dm_types_the_d6() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    dm(
        &mut combat,
        Command::SetSettings {
            settings: Settings {
                show_enemy_turn: false,
                auto_recharge: false,
            },
        },
    );
    let mut d = adult_red_dragon("d");
    d.abilities[1].recharge = Some(Recharge {
        min: 5,
        ready: false,
    });
    add(&mut combat, d);
    init(&mut combat, "d", 15);
    let ev = scripted(&mut combat, Command::Start, &[]);
    assert!(ev.dm.iter().any(
        |e| matches!(&e.kind, DmEventKind::RechargeDue { ability } if ability == "fire-breath")
    ));
    let roll = |n| Command::RollRecharge {
        id: id("d"),
        ability: "fire-breath".into(),
        roll: DiceInput::typed(n),
    };
    assert!(matches!(
        dm_err(&mut combat, roll(7)),
        CombatError::Invalid(_)
    ));
    dm(&mut combat, roll(6));
    assert!(
        c(&combat, "d")
            .ability("fire-breath")
            .unwrap()
            .recharge
            .unwrap()
            .ready
    );
    dm(
        &mut combat,
        Command::SetRecharge {
            id: id("d"),
            ability: "fire-breath".into(),
            ready: false,
        },
    );
    assert!(
        !c(&combat, "d")
            .ability("fire-breath")
            .unwrap()
            .recharge
            .unwrap()
            .ready
    );
}

/// C16: a DM link (one multi-headed creature, opt-in): two creatures spend one pool, which refills at the start
/// of the first living member's turn. Unlinked, each legendary creature has its own pool (tests/legendary.rs).
#[test]
fn a_dm_linked_legendary_pool_across_two_creatures() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, adult_red_dragon("d1"));
    add(&mut combat, adult_red_dragon("d2"));
    add(&mut combat, wizard_pc("w"));
    init(&mut combat, "d1", 18);
    init(&mut combat, "w", 12);
    init(&mut combat, "d2", 6);
    let pool = LegendaryPool {
        max: 3,
        remaining: 3,
        members: vec![id("d2"), id("d1")],
        guessed: false,
    };
    dm(
        &mut combat,
        Command::SetLegendaryPool {
            pool: "pair".into(),
            value: pool,
        },
    );
    dm(&mut combat, Command::Start); // d1's turn
    assert_eq!(
        unavailable(&mut combat, use_ab("d1", "tail")),
        Unavailable::OwnTurn
    );
    dm(&mut combat, Command::NextTurn); // w
    dm(&mut combat, use_ab("d1", "wing")); // 2
    dm(&mut combat, use_ab("d2", "tail")); // 1
    assert_eq!(combat.state.legendary_pools["pair"].remaining, 0);
    assert_eq!(
        unavailable(&mut combat, use_ab("d1", "tail")),
        Unavailable::NotEnoughLegendary
    );
    let ev = dm(&mut combat, Command::NextTurn); // d2's turn: the reset owner
    assert!(ev
        .dm
        .iter()
        .any(|e| matches!(&e.kind, DmEventKind::LegendaryReset { pool } if pool == "pair")));
    assert_eq!(combat.state.legendary_pools["pair"].remaining, 3);
    dm(&mut combat, use_ab("d1", "wing"));
    assert_eq!(combat.state.legendary_pools["pair"].remaining, 1);
    // d2 falls: d1 becomes the reset owner.
    dm(&mut combat, hit("d2", 300, None));
    dm(&mut combat, Command::NextTurn); // round 2, d1
    assert_eq!(combat.state.legendary_pools["pair"].remaining, 3);
    dm(
        &mut combat,
        Command::AdjustLegendary {
            pool: "pair".into(),
            delta: -5,
        },
    );
    assert_eq!(combat.state.legendary_pools["pair"].remaining, 0);
    // Not in a pool: no legendary actions.
    let mut g = goblin("g");
    g.abilities = vec![ability("x", "X", AbilityKind::Legendary)];
    add(&mut combat, g);
    assert_eq!(
        unavailable(&mut combat, use_ab("g", "x")),
        Unavailable::NoLegendaryPool
    );
}

#[test]
fn legendary_resistance_is_used_and_restored() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, adult_red_dragon("d"));
    dm(
        &mut combat,
        Command::AdjustLegendaryResistance {
            id: id("d"),
            delta: -1,
        },
    );
    assert_eq!(
        c(&combat, "d").legendary_resistance,
        Some(Pool {
            max: 3,
            remaining: 2
        })
    );
    dm(
        &mut combat,
        Command::AdjustLegendaryResistance {
            id: id("d"),
            delta: -9,
        },
    );
    assert_eq!(c(&combat, "d").legendary_resistance.unwrap().remaining, 0);
    dm(
        &mut combat,
        Command::AdjustLegendaryResistance {
            id: id("d"),
            delta: 1,
        },
    );
    assert_eq!(c(&combat, "d").legendary_resistance.unwrap().remaining, 1);
    add(&mut combat, goblin("g"));
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::AdjustLegendaryResistance {
                id: id("g"),
                delta: -1
            }
        ),
        CombatError::Invalid(_)
    ));
}

#[test]
fn per_day_and_per_turn_resources() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    let mut m = mage("m");
    m.resources.insert(
        "misty".into(),
        Resource {
            label: "Misty Step 1/day".into(),
            pool: Pool::full(1),
            reset: Reset::Never,
        },
    );
    m.resources.insert(
        "sneak".into(),
        Resource {
            label: "Once per turn".into(),
            pool: Pool::full(1),
            reset: Reset::TurnStart,
        },
    );
    let mut misty = ability("misty-step", "Misty Step", AbilityKind::BonusAction);
    misty.uses = Some("misty".into());
    let mut strike = ability("strike", "Strike", AbilityKind::Free);
    strike.uses = Some("sneak".into());
    m.abilities.extend([misty, strike]);
    add(&mut combat, m);
    init(&mut combat, "m", 10);
    dm(&mut combat, Command::Start);
    dm(&mut combat, use_ab("m", "misty-step"));
    dm(&mut combat, use_ab("m", "strike"));
    dm(&mut combat, Command::NextTurn); // round 2, its turn again
    assert_eq!(c(&combat, "m").resources["sneak"].pool.remaining, 1);
    assert_eq!(
        unavailable(&mut combat, use_ab("m", "misty-step")),
        Unavailable::NoUses("Misty Step 1/day".into())
    );
    dm(
        &mut combat,
        Command::AdjustResource {
            id: id("m"),
            key: "misty".into(),
            delta: 1,
        },
    );
    dm(&mut combat, use_ab("m", "misty-step"));
}

#[test]
fn incapacitated_down_and_passive_abilities() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    let mut o = owlbear("o");
    o.abilities.push(ability(
        "keen",
        "Keen Sight and Smell",
        AbilityKind::Passive,
    ));
    o.abilities
        .push(ability("lair", "Lair action", AbilityKind::Free));
    add(&mut combat, o);
    assert_eq!(
        unavailable(&mut combat, use_ab("o", "keen")),
        Unavailable::Passive
    );
    let mut spec = effect("", "o");
    spec.conditions = vec![Condition::Stunned];
    dm(
        &mut combat,
        Command::AddCondition {
            effect: Box::new(spec),
        },
    );
    assert_eq!(
        unavailable(&mut combat, use_ab("o", "multiattack")),
        Unavailable::Incapacitated
    );
    dm(&mut combat, use_ab("o", "lair"));
    dm(&mut combat, hit("o", 100, None));
    assert_eq!(
        unavailable(&mut combat, use_ab("o", "multiattack")),
        Unavailable::Defeated
    );
    assert!(matches!(
        dm_err(&mut combat, use_ab("o", "nope")),
        CombatError::UnknownAbility(_)
    ));
}

#[test]
fn requires_an_active_effect() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    let mut m = mage("m");
    let mut mark = ability("mark-move", "Move Hunter's Mark", AbilityKind::BonusAction);
    mark.requires_effect = Some("Hunter's Mark".into());
    m.abilities.push(mark);
    add(&mut combat, m);
    assert_eq!(
        unavailable(&mut combat, use_ab("m", "mark-move")).to_string(),
        "Requires Hunter's Mark active"
    );
    dm(
        &mut combat,
        Command::SetConcentration {
            id: id("m"),
            name: "Hunter's Mark".into(),
        },
    );
    dm(&mut combat, use_ab("m", "mark-move"));
}

/// C20 and C21: while the leader lives, the group gets a breath, once per round for the whole group; the rule ends
/// for good when the leader drops.
#[test]
fn encounter_rule_and_group_limit() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    let mut priest = mage("priest");
    priest.label = "Priest".into();
    add(&mut combat, priest);
    for k in ["g1", "g2"] {
        let mut g = goblin(k);
        g.group = Some("goblins".into());
        let mut breath = ability("breath", "Shared Breath", AbilityKind::BonusAction);
        breath.group_limit = Some("goblins".into());
        g.abilities = vec![breath];
        add(&mut combat, g);
    }
    for (k, v) in [("priest", 18), ("g1", 12), ("g2", 8)] {
        init(&mut combat, k, v);
    }
    let rule = EncounterRule {
        id: "blessing".into(),
        name: "Blessing".into(),
        text_active: "Goblins: one shared breath per round".into(),
        text_ended: "Goblins: no breath".into(),
        linked: Some(id("priest")),
        active: true,
        flags: vec![RuleFlag {
            target: RuleTarget::Group("goblins".into()),
            ability: "breath".into(),
            while_active: true,
        }],
    };
    dm(&mut combat, Command::SetRule { rule });
    dm(
        &mut combat,
        Command::SetGroupLimit {
            group: "goblins".into(),
            per_round: 1,
        },
    );
    dm(&mut combat, Command::Start);
    dm(&mut combat, use_ab("g1", "breath"));
    assert_eq!(
        unavailable(&mut combat, use_ab("g2", "breath")),
        Unavailable::GroupLimit("goblins".into())
    );
    dm(&mut combat, Command::NextTurn);
    dm(&mut combat, Command::NextTurn);
    dm(&mut combat, Command::NextTurn); // round 2: the limit refreshes
    assert_eq!(combat.state.group_limits["goblins"].used, 0);
    dm(&mut combat, use_ab("g2", "breath"));

    let ev = dm(&mut combat, hit("priest", 100, None));
    assert!(ev
        .dm
        .iter()
        .any(|e| matches!(&e.kind, DmEventKind::Rule { active: false, .. })));
    assert!(!combat.state.rules[0].active);
    dm(&mut combat, Command::NextTurn);
    dm(&mut combat, Command::NextTurn); // round 3
    assert_eq!(
        unavailable(&mut combat, use_ab("g1", "breath")),
        Unavailable::Rule("Blessing".into())
    );
    // The DM can switch it back on by hand.
    dm(
        &mut combat,
        Command::SetRuleActive {
            id: "blessing".into(),
            active: true,
        },
    );
    assert!(derive::rule_blocking(&combat.state, c(&combat, "g1"), "breath").is_none());
    dm(
        &mut combat,
        Command::RemoveRule {
            id: "blessing".into(),
        },
    );
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::RemoveRule {
                id: "blessing".into()
            }
        ),
        CombatError::UnknownRule(_)
    ));
}

#[test]
fn a_flag_can_switch_an_ability_on_once_a_rule_ends() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, knight("k"));
    let mut o = owlbear("o");
    o.abilities
        .push(ability("frenzy", "Frenzy", AbilityKind::Action));
    add(&mut combat, o);
    let rule = EncounterRule {
        id: "calm".into(),
        name: "Calmed".into(),
        text_active: String::new(),
        text_ended: String::new(),
        linked: Some(id("k")),
        active: true,
        flags: vec![RuleFlag {
            target: RuleTarget::Combatant(id("o")),
            ability: "frenzy".into(),
            while_active: false,
        }],
    };
    dm(&mut combat, Command::SetRule { rule });
    assert_eq!(
        unavailable(&mut combat, use_ab("o", "frenzy")),
        Unavailable::Rule("Calmed".into())
    );
    dm(&mut combat, hit("k", 100, None));
    dm(&mut combat, use_ab("o", "frenzy"));
}
