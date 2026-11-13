//! Fixtures: SRD creatures and two PCs. Stats are the SRD's, trimmed to what the engine reads.

#![allow(dead_code)]

use party_combat::party_dice::{D20Input, DiceInput, SequenceDie};
use party_combat::*;

pub const ALICE: &str = "acct-alice";
pub const BOB: &str = "acct-bob";

pub fn id(s: &str) -> CombatantId {
    CombatantId::new(s)
}

pub fn goblin(key: &str) -> Combatant {
    let mut c = Combatant::creature(key, format!("Goblin {key}"), 7, 15, 2);
    c.public_name = "Goblin".into();
    c
}

pub fn owlbear(key: &str) -> Combatant {
    let mut c = Combatant::creature(key, "Owlbear", 59, 13, 1);
    c.speed = Speeds {
        walk: 40,
        ..Speeds::default()
    };
    c.abilities = vec![ability("multiattack", "Multiattack", AbilityKind::Action)];
    c
}

pub fn skeleton(key: &str) -> Combatant {
    let mut c = Combatant::creature(key, "Skeleton", 13, 13, 2);
    c.defenses.vulnerabilities = vec![DamageClause::plain(&[DamageType::Bludgeoning])];
    c.defenses.immunities = vec![DamageClause::plain(&[DamageType::Poison])];
    c.defenses.condition_immunities = vec![Condition::Poisoned];
    c
}

pub fn werewolf(key: &str) -> Combatant {
    let mut c = Combatant::creature(key, "Werewolf", 58, 11, 1);
    c.defenses.immunities = vec![DamageClause::qualified(
        &[
            DamageType::Bludgeoning,
            DamageType::Piercing,
            DamageType::Slashing,
        ],
        "bludgeoning, piercing, and slashing from nonmagical attacks that aren't silvered",
    )];
    c
}

pub fn knight(key: &str) -> Combatant {
    Combatant::creature(key, "Knight", 52, 18, 0)
}

pub fn warhorse(key: &str) -> Combatant {
    let mut c = Combatant::creature(key, "Warhorse", 19, 11, 1);
    c.speed = Speeds {
        walk: 60,
        ..Speeds::default()
    };
    c
}

pub fn ability(key: &str, name: &str, kind: AbilityKind) -> Ability {
    Ability {
        key: key.into(),
        name: name.into(),
        kind,
        text: String::new(),
        legendary_cost: None,
        recharge: None,
        uses: None,
        group_limit: None,
        requires_effect: None,
    }
}

pub fn adult_red_dragon(key: &str) -> Combatant {
    let mut c = Combatant::creature(key, "Adult Red Dragon", 256, 19, 0);
    c.defenses.immunities = vec![DamageClause::plain(&[DamageType::Fire])];
    c.speed = Speeds {
        walk: 40,
        fly: Some(80),
        climb: Some(40),
        ..Speeds::default()
    };
    c.legendary_resistance = Some(Pool::full(3));
    let mut breath = ability("fire-breath", "Fire Breath", AbilityKind::Action);
    breath.recharge = Some(Recharge {
        min: 5,
        ready: true,
    });
    let mut tail = ability("tail", "Tail Attack", AbilityKind::Legendary);
    tail.legendary_cost = Some(1);
    let mut wing = ability("wing", "Wing Attack", AbilityKind::Legendary);
    wing.legendary_cost = Some(2);
    c.abilities = vec![
        ability("multiattack", "Multiattack", AbilityKind::Action),
        breath,
        tail,
        wing,
    ];
    c
}

pub fn mage(key: &str) -> Combatant {
    let mut c = Combatant::creature(key, "Mage", 40, 12, 2);
    c.spell_slots = [
        (1, Pool::full(4)),
        (2, Pool::full(3)),
        (3, Pool::full(3)),
        (4, Pool::full(3)),
        (5, Pool::full(1)),
    ]
    .into_iter()
    .collect();
    let mut cast = ability("cast", "Spellcasting", AbilityKind::Action);
    cast.text = "Casts a prepared spell.".into();
    let mut shield = ability("shield", "Shield", AbilityKind::Reaction);
    shield.text = "+5 AC until the start of its next turn.".into();
    c.abilities = vec![cast, shield];
    c
}

pub fn wizard_pc(key: &str) -> Combatant {
    Combatant::character(
        key,
        ALICE,
        "Wizard",
        Hp {
            current: 22,
            max: 22,
            temp: 0,
        },
        12,
        3,
    )
}

pub fn fighter_pc(key: &str) -> Combatant {
    Combatant::character(
        key,
        BOB,
        "Fighter",
        Hp {
            current: 44,
            max: 44,
            temp: 0,
        },
        18,
        1,
    )
}

/// An engine-written PC (for death-save tests without the sync round trip).
pub fn engine_pc(key: &str) -> Combatant {
    let mut c = wizard_pc(key);
    c.hp_authority = HpAuthority::Engine;
    c
}

pub fn add(combat: &mut Combat, c: Combatant) {
    combat
        .execute(
            &Actor::Dm,
            &Command::AddCombatant {
                combatant: Box::new(c),
                roll_initiative: false,
            },
        )
        .expect("add");
}

pub fn init(combat: &mut Combat, who: &str, total: i32) {
    dm(
        combat,
        Command::SetInitiative {
            id: id(who),
            roll: D20Input::typed(total),
        },
    );
}

pub fn dm(combat: &mut Combat, cmd: Command) -> Events {
    combat
        .execute(&Actor::Dm, &cmd)
        .unwrap_or_else(|e| panic!("{cmd:?}: {e}"))
}

pub fn dm_err(combat: &mut Combat, cmd: Command) -> CombatError {
    combat
        .execute(&Actor::Dm, &cmd)
        .expect_err("should be refused")
}

pub fn scripted(combat: &mut Combat, cmd: Command, rolls: &[u32]) -> Events {
    let mut die = SequenceDie::new(rolls.to_vec());
    combat
        .execute_with(&Actor::Dm, &cmd, &mut die)
        .unwrap_or_else(|e| panic!("{cmd:?}: {e}"))
}

pub fn hit(target: &str, amount: i32, t: Option<DamageType>) -> Command {
    Command::Damage {
        target: id(target),
        parts: vec![DamagePart::typed(amount, t)],
        magical: false,
        apply_qualified: false,
        critical_hit: false,
    }
}

pub fn typed(n: i32) -> DiceInput {
    DiceInput::typed(n)
}

pub fn c<'a>(combat: &'a Combat, key: &str) -> &'a Combatant {
    combat.state.combatant(&id(key)).expect("combatant")
}

pub fn current(combat: &Combat) -> String {
    combat
        .state
        .current
        .as_ref()
        .map(|c| c.0.clone())
        .unwrap_or_default()
}

pub fn order(combat: &Combat) -> Vec<String> {
    derive::turn_order(&combat.state)
        .into_iter()
        .map(|c| c.0)
        .collect()
}

/// Goblin a (init 15), owlbear o (12), wizard PC w (10); started.
pub fn small_fight(edition: Edition) -> Combat {
    let mut combat = Combat::new(edition, 7);
    add(&mut combat, goblin("a"));
    add(&mut combat, owlbear("o"));
    add(&mut combat, wizard_pc("w"));
    init(&mut combat, "a", 15);
    init(&mut combat, "o", 12);
    init(&mut combat, "w", 10);
    dm(&mut combat, Command::Start);
    combat
}

pub fn effect(name: &str, target: &str) -> EffectSpec {
    EffectSpec::new(name, id(target))
}

pub fn player_events_of(e: &Events) -> usize {
    e.players.len()
}
