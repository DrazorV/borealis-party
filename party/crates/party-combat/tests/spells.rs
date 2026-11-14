//! 0.3.2: casting, resolving and applying spells and stat block actions; AC, saves and the edition rules.

mod common;
use common::*;
use party_combat::party_dice::{RollMode, SequenceDie};
use party_combat::spell::*;
use party_combat::*;
use serde_json::json;

const E14: Edition = Edition::Srd51;
const E24: Edition = Edition::Srd521;

fn stats(mods: [i32; 6], saves: [i32; 6], ty: &str) -> CombatStats {
    CombatStats {
        mods,
        saves,
        magic_resistance: false,
        creature_type: Some(ty.into()),
        armor: None,
    }
}

/// The SRD Mage: AC 12 (15 with Mage Armor in 2024), DEX +2, INT +3, INT and WIS saves proficient.
fn the_mage(key: &str, edition: Edition) -> Combatant {
    let mut c = mage(key);
    c.ac = Some(if edition == E24 { 15 } else { 12 });
    c.stats = Some(stats([-1, 2, 0, 3, 1, 0], [-1, 2, 0, 6, 4, 0], "humanoid"));
    c
}

/// Mage m (20), goblin a (15), owlbear o (12), wizard PC w (10, Alice), fighter PC f (5, Bob); started, m's turn.
fn fight(edition: Edition) -> Combat {
    let mut cb = Combat::new(edition, 11);
    add(&mut cb, the_mage("m", edition));
    let mut g = goblin("a");
    g.stats = Some(stats(
        [-1, 2, 0, 0, -1, -1],
        [-1, 2, 0, 0, -1, -1],
        "humanoid",
    ));
    add(&mut cb, g);
    let mut o = owlbear("o");
    o.stats = Some(stats(
        [5, 1, 3, -4, 1, -2],
        [5, 1, 3, -4, 1, -2],
        "monstrosity",
    ));
    o.defenses.resistances = vec![DamageClause::plain(&[DamageType::Fire])];
    add(&mut cb, o);
    let mut w = wizard_pc("w");
    w.spell_slots = [(1, Pool::full(4)), (2, Pool::full(3)), (3, Pool::full(2))]
        .into_iter()
        .collect();
    add(&mut cb, w);
    add(&mut cb, fighter_pc("f"));
    for (k, n) in [("m", 20), ("a", 15), ("o", 12), ("w", 10), ("f", 5)] {
        init(&mut cb, k, n);
    }
    dm(&mut cb, Command::Start);
    cb
}

fn cast(caster: &str, spec: CastSpec, slot: Option<u8>, targets: &[&str]) -> CastBuilder {
    CastBuilder {
        caster: caster.into(),
        spec,
        slot,
        targets: targets.iter().map(|t| id(t)).collect(),
        rows: vec![],
        amounts: vec![],
        force: false,
        precast: false,
    }
}

struct CastBuilder {
    caster: String,
    spec: CastSpec,
    slot: Option<u8>,
    targets: Vec<CombatantId>,
    rows: Vec<RowInput>,
    amounts: Vec<AmountRoll>,
    force: bool,
    precast: bool,
}

impl CastBuilder {
    fn row(mut self, t: &str, roll: RowRoll) -> Self {
        self.rows.push(RowInput {
            target: id(t),
            ray: 0,
            roll,
            use_lr: false,
        });
        self
    }
    fn typed(self, t: &str, total: i32) -> Self {
        self.row(
            t,
            RowRoll::Typed {
                total,
                natural: None,
            },
        )
    }
    fn amount(mut self, a: AmountRoll) -> Self {
        self.amounts.push(a);
        self
    }
    fn force(mut self) -> Self {
        self.force = true;
        self
    }
    fn precast(mut self) -> Self {
        self.precast = true;
        self
    }
    fn cmd(self) -> Command {
        Command::CastSpell {
            caster: id(&self.caster),
            ability: None,
            spec: Some(Box::new(self.spec)),
            slot: self.slot,
            targets: self.targets,
            rows: self.rows,
            amounts: self.amounts,
            crit_amounts: vec![],
            choices: CastChoices::default(),
            precast: self.precast,
            force: self.force,
            id: None,
        }
    }
}

fn typed(total: i32) -> AmountRoll {
    AmountRoll::Typed { total }
}

fn ac(cb: &Combat, k: &str) -> derive::AcBreakdown {
    derive::effective_ac(&cb.state, c(cb, k))
}

fn adhoc(v: serde_json::Value) -> CastSpec {
    let m: SpellMech = serde_json::from_value(v).expect("definition");
    CastSpec {
        v: 1,
        origin: MechOrigin::Def {
            id: "test".into(),
            rev: 1,
        },
        mech: m,
        dc: Some(14),
        attack: Some(6),
        modifier: 3,
        cantrip_tier: 1,
        baked_in: false,
    }
}

fn pending(cb: &Combat) -> &PendingCast {
    cb.state.casts.last().expect("a pending cast")
}

fn alice() -> Actor {
    Actor::Player {
        account: ALICE.into(),
    }
}

fn bob() -> Actor {
    Actor::Player {
        account: BOB.into(),
    }
}

// ----- AC -----

#[test]
fn mage_armor_shield_and_the_breakdown() {
    let mut cb = fight(E14);
    assert_eq!(ac(&cb, "m").total, Some(12));
    dm(
        &mut cb,
        cast(
            "m",
            spec_of("mage-armor", E14, 14, Some(6)),
            Some(1),
            &["m"],
        )
        .cmd(),
    );
    let a = ac(&cb, "m");
    assert_eq!(a.total, Some(15));
    assert_eq!(a.text, "15 = Mage Armor 13 + DEX 2");
    assert_eq!(c(&cb, "m").spell_slots[&1].remaining, 3);
    assert!(c(&cb, "m").economy.action);
    // Shield as a reaction: 20, until the start of the mage's next turn.
    dm(
        &mut cb,
        cast("m", spec_of("shield", E14, 14, Some(6)), Some(1), &[]).cmd(),
    );
    assert_eq!(ac(&cb, "m").total, Some(20));
    assert!(c(&cb, "m").economy.reaction);
    for _ in 0..5 {
        dm(&mut cb, Command::NextTurn);
    }
    assert_eq!(current(&cb), "m");
    assert_eq!(
        ac(&cb, "m").total,
        Some(15),
        "Shield ended, Mage Armor stays"
    );
}

#[test]
fn a_baked_in_mage_armor_is_listed_but_doesnt_change_ac() {
    let mut cb = fight(E24);
    let mut spec = spec_of("mage-armor", E24, 14, Some(6));
    spec.baked_in = true;
    dm(&mut cb, cast("m", spec, Some(1), &["m"]).precast().cmd());
    assert_eq!(ac(&cb, "m").total, Some(15));
    let e = cb
        .state
        .effects
        .iter()
        .find(|e| e.spec.name == "Mage Armor")
        .expect("listed");
    assert!(e.spec.detail.contains("included in its AC"));
    let eid = e.id;
    dm(
        &mut cb,
        Command::RemoveEffect {
            effect: eid,
            skip_follow_up: false,
        },
    );
    assert_eq!(ac(&cb, "m").total, Some(15));
    // Precast: the slot is spent, the economy isn't.
    assert_eq!(c(&cb, "m").spell_slots[&1].remaining, 3);
    assert!(!c(&cb, "m").economy.action);
}

#[test]
fn ac_minimum_and_armor() {
    let bark = |n: i32| {
        adhoc(json!({
            "key": "bark", "edition": "2014", "name": "Bark", "level": 2, "concentration": true,
            "cast": "action", "targeting": {"kind": "creatures", "max": "1", "filter": {"kind": "willing"}},
            "delivery": {"kind": "auto"}, "always": [{"kind": "ac_min", "n": n}],
            "duration": {"kind": "minutes", "n": 60}, "tier": "auto"
        }))
    };
    let mut cb = fight(E14);
    dm(&mut cb, cast("m", bark(16), Some(2), &["o"]).cmd());
    let a = ac(&cb, "o");
    assert_eq!((a.total, a.min), (Some(16), Some(16)));
    let mut cover = effect("Low wall", "o");
    cover.modifiers = vec![Modifier::Cover(Cover::Half)];
    dm(
        &mut cb,
        Command::AddEffect {
            effect: Box::new(cover),
        },
    );
    assert_eq!(ac(&cb, "o").total, Some(18), "the minimum, then cover");

    // Mage Armor on a creature in armor is refused unless the DM applies it anyway.
    let mut cb = fight(E14);
    let mut k = knight("k");
    k.stats = Some(stats([3, 0, 2, 0, 0, 2], [3, 0, 4, 0, 2, 2], "humanoid"));
    k.stats.as_mut().unwrap().armor = Some("plate".into());
    add(&mut cb, k);
    let err = dm_err(
        &mut cb,
        cast("m", spec_of("mage-armor", E14, 14, None), Some(1), &["k"]).cmd(),
    );
    assert!(err.to_string().contains("wears plate"), "{err}");
    dm(
        &mut cb,
        cast("m", spec_of("mage-armor", E14, 14, None), Some(1), &["k"])
            .force()
            .cmd(),
    );
    assert_eq!(ac(&cb, "k").total, Some(18), "13 + DEX 0 never lowers AC");
}

// ----- damage spells -----

#[test]
fn fireball_one_roll_halved_with_resistance_and_a_pc_row() {
    let mut cb = fight(E24);
    dm(
        &mut cb,
        Command::SetConcentration {
            id: id("o"),
            name: "Something".into(),
        },
    );
    let ev = dm(
        &mut cb,
        cast(
            "m",
            spec_of("fireball", E24, 14, None),
            Some(3),
            &["a", "o", "w"],
        )
        .typed("a", 10)
        .typed("o", 15)
        .amount(typed(28))
        .cmd(),
    );
    // The PC's row waits for Alice; the mage is hidden, so the request names neither it nor the spell.
    let req = ev
        .players
        .iter()
        .find_map(|p| match p {
            PlayerEvent::SaveRequest {
                owner,
                ability,
                dc,
                spell,
                from,
                ..
            } => Some((owner.clone(), *ability, *dc, spell.clone(), from.clone())),
            _ => None,
        })
        .expect("a save request");
    assert_eq!(
        req,
        (ALICE.to_string(), AbilityName::Dex, Some(14), None, None)
    );
    let rows = player_casts(&cb.state, ALICE);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("spell").is_none() && rows[0].get("from").is_none());
    assert!(player_casts(&cb.state, BOB).is_empty());
    assert_eq!(c(&cb, "m").spell_slots[&3].remaining, 2, "spent at cast");
    let cast_id = pending(&cb).id.clone();

    // Only Alice answers her row, and only with a roll.
    let row = |roll: RowRoll| Command::ResolveCastRow {
        cast: cast_id.clone(),
        target: id("w"),
        ray: 0,
        roll,
        use_lr: false,
    };
    assert!(cb.execute(&bob(), &row(RowRoll::App)).is_err());
    assert!(cb
        .execute(
            &alice(),
            &row(RowRoll::Outcome {
                outcome: RowOutcome::Saved
            })
        )
        .is_err());
    cb.execute(
        &alice(),
        &row(RowRoll::Typed {
            total: 9,
            natural: None,
        }),
    )
    .unwrap();
    assert_eq!(cb.state.casts.len(), 1, "waits for the damage");
    let ev = dm(
        &mut cb,
        Command::ApplyCast {
            cast: cast_id,
            amounts: vec![typed(28)],
            crit_amounts: vec![],
            force: false,
        },
    );
    assert!(cb.state.casts.is_empty());
    assert!(c(&cb, "a").life.is_dead());
    assert_eq!(
        c(&cb, "o").hp.current,
        59 - 7,
        "14 for the save, resisted to 7"
    );
    assert_eq!(c(&cb, "o").pending_concentration.len(), 1);
    assert!(ev
        .players
        .iter()
        .any(|p| matches!(p, PlayerEvent::DamagePrompt { amount: 28, .. })));
}

#[test]
fn fireball_upcast_with_every_roll_is_one_undo_step() {
    let mut cb = fight(E14);
    let cmd = cast(
        "m",
        spec_of("fireball", E14, 14, None),
        Some(4),
        &["a", "o"],
    )
    .typed("a", 3)
    .row(
        "o",
        RowRoll::Outcome {
            outcome: RowOutcome::Saved,
        },
    )
    .amount(AmountRoll::App)
    .cmd();
    let before = cb.state.clone();
    let ev = scripted(&mut cb, cmd, &[3; 9]);
    let applied = ev
        .dm
        .iter()
        .find_map(|e| match &e.kind {
            DmEventKind::CastApplied { amounts, .. } => Some(amounts.clone()),
            _ => None,
        })
        .expect("applied at once");
    assert_eq!(applied[0].notation, "9d6");
    assert_eq!(applied[0].total, 27);
    assert_eq!(c(&cb, "o").hp.current, 59 - 6, "13 halved, resisted");
    assert!(cb.state.casts.is_empty());
    assert_eq!(cb.undo_labels()[0], "cast spell");
    cb.undo(&Actor::Dm, false).unwrap();
    assert_eq!(cb.state, before);
}

#[test]
fn the_same_seed_gives_the_same_state() {
    let run = || {
        let mut cb = fight(E24);
        dm(
            &mut cb,
            cast(
                "m",
                spec_of("fireball", E24, 14, None),
                Some(3),
                &["a", "o"],
            )
            .row("a", RowRoll::App)
            .row("o", RowRoll::App)
            .amount(AmountRoll::App)
            .cmd(),
        );
        cb.state
    };
    assert_eq!(run(), run());
}

// ----- conditions and concentration -----

#[test]
fn hold_person_repeat_save_and_the_cascade() {
    let mut cb = fight(E14);
    let hp = || spec_of("hold-person", E14, 14, None);
    // Owlbears aren't humanoids; level 2 holds one target.
    assert!(dm_err(&mut cb, cast("m", hp(), Some(2), &["o"]).cmd())
        .to_string()
        .contains("isn't a humanoid"));
    assert!(dm_err(&mut cb, cast("m", hp(), Some(2), &["a", "m"]).cmd())
        .to_string()
        .contains("at most 1"));
    dm(
        &mut cb,
        cast("m", hp(), Some(2), &["a"]).typed("a", 5).cmd(),
    );
    assert!(cb.state.casts.is_empty(), "no damage: applied at once");
    assert!(derive::has_condition(
        &cb.state,
        &id("a"),
        Condition::Paralyzed
    ));
    let held = cb
        .state
        .effects
        .iter()
        .find(|e| e.spec.name == "Hold Person")
        .unwrap();
    assert_eq!(
        held.spec.repeat_save,
        Some(RepeatSave {
            ability: AbilityName::Wis,
            dc: 14,
            at: Boundary::End
        })
    );
    assert_eq!(
        c(&cb, "m").concentration.as_ref().unwrap().name,
        "Hold Person"
    );
    // The mage is hit and fails concentration: the paralysis ends.
    dm(&mut cb, hit("m", 12, None));
    dm(
        &mut cb,
        Command::ResolveConcentration {
            id: id("m"),
            roll: party_dice::D20Input::typed(4),
        },
    );
    assert!(!derive::has_condition(
        &cb.state,
        &id("a"),
        Condition::Paralyzed
    ));
    assert!(c(&cb, "m").concentration.is_none());
}

#[test]
fn everyone_saves_so_concentration_ends_and_legendary_resistance() {
    let mut cb = fight(E24);
    let mut a = c(&cb, "a").clone();
    a.id = id("boss");
    a.label = "Boss".into();
    a.legendary_resistance = Some(Pool::full(1));
    add(&mut cb, a);
    let cmd = Command::CastSpell {
        caster: id("m"),
        ability: None,
        spec: Some(Box::new(spec_of("hold-person", E24, 14, None))),
        slot: Some(2),
        targets: vec![id("boss")],
        rows: vec![RowInput {
            target: id("boss"),
            ray: 0,
            roll: RowRoll::Typed {
                total: 2,
                natural: None,
            },
            use_lr: true,
        }],
        amounts: vec![],
        crit_amounts: vec![],
        choices: CastChoices::default(),
        precast: false,
        force: false,
        id: Some(CastId("x-1".into())),
    };
    dm(&mut cb, cmd);
    assert_eq!(c(&cb, "boss").legendary_resistance.unwrap().remaining, 0);
    assert!(!derive::has_condition(
        &cb.state,
        &id("boss"),
        Condition::Paralyzed
    ));
    assert!(c(&cb, "m").concentration.is_none(), "nobody held");
}

#[test]
fn bless_dice_count_in_app_saves() {
    let mut cb = fight(E14);
    dm(
        &mut cb,
        cast("m", spec_of("bless", E14, 14, None), Some(1), &["m", "a"]).cmd(),
    );
    let sm = derive::save_mod(&cb.state, &id("a"), AbilityName::Wis, true);
    assert_eq!(sm.dice.len(), 1);
    assert_eq!(sm.bonus, -1);
    // The wizard holds the goblin: the app save is d20 10 - 1 + d4 4 = 13 < 14.
    dm(&mut cb, Command::NextTurn);
    let cmd = cast("w", spec_of("hold-person", E14, 14, None), Some(2), &["a"])
        .row("a", RowRoll::App)
        .force()
        .cmd();
    let ev = scripted(&mut cb, cmd, &[10, 4]);
    let roll = ev
        .dm
        .iter()
        .find_map(|e| match &e.kind {
            DmEventKind::CastRow { roll, .. } => roll.clone(),
            _ => None,
        })
        .unwrap();
    assert_eq!(roll.total, 13);
    assert!(roll.breakdown.contains("1d4 [4]"), "{}", roll.breakdown);
    assert!(derive::has_condition(
        &cb.state,
        &id("a"),
        Condition::Paralyzed
    ));
}

#[test]
fn bound_creatures_leave_with_concentration_and_undo_restores_them() {
    let mut cb = fight(E14);
    let conjure = adhoc(json!({
        "key": "conjure", "edition": "2014", "name": "Conjure Beasts", "level": 3, "concentration": true,
        "cast": "action", "targeting": {"kind": "self"}, "delivery": {"kind": "auto"},
        "always": [{"kind": "summon", "info": {"creature_types": ["beast"],
            "by_cr": [{"cr": "2", "count": 1}, {"cr": "1/4", "count": 8}],
            "per_slot": [{"slot": 5, "times": 2}], "initiative": "own_group"}}],
        "duration": {"kind": "minutes", "n": 60}, "tier": "partial"
    }));
    let ev = dm(&mut cb, cast("m", conjure, Some(3), &[]).cmd());
    let info = ev
        .dm
        .iter()
        .find_map(|e| match &e.kind {
            DmEventKind::SummonOptions {
                info,
                concentration,
                ..
            } => Some((info.clone(), *concentration)),
            _ => None,
        })
        .expect("summon options");
    assert!(info.1 && info.0.by_cr.len() == 2);
    assert_eq!(cb.state.combatants.len(), 5, "nothing spawns");
    assert_eq!(
        c(&cb, "m").concentration.as_ref().unwrap().name,
        "Conjure Beasts"
    );
    for k in ["wolf1", "wolf2"] {
        dm(
            &mut cb,
            Command::Summon {
                owner: id("m"),
                combatant: Box::new(Combatant::creature(k, "Wolf", 11, 13, 2)),
                initiative: None,
                visible: None,
            },
        );
        dm(&mut cb, Command::BindToConcentration { id: id(k) });
    }
    // A wolf going down doesn't end the spell.
    dm(&mut cb, hit("wolf2", 20, None));
    assert!(c(&cb, "m").concentration.is_some());
    let before = cb.state.clone();
    dm(&mut cb, Command::EndConcentration { id: id("m") });
    assert!(c(&cb, "wolf1").removed, "dismissed with the spell");
    cb.undo(&Actor::Dm, false).unwrap();
    assert_eq!(cb.state, before);
    assert!(!c(&cb, "wolf1").removed);
}

// ----- healing -----

#[test]
fn cure_wounds_per_edition() {
    for (edition, rolls, expect) in [(E14, vec![5, 5], 13), (E24, vec![1, 1, 1, 1], 7)] {
        let mut cb = fight(edition);
        dm(&mut cb, hit("o", 30, None));
        let cmd = cast(
            "m",
            spec_of("cure-wounds", edition, 14, None),
            Some(2),
            &["o"],
        )
        .amount(AmountRoll::App)
        .cmd();
        scripted(&mut cb, cmd, &rolls);
        assert_eq!(c(&cb, "o").hp.current, 29 + expect, "{edition:?}");
    }
}

// ----- refusals and the edition rules -----

#[test]
fn illegal_slots_are_refused() {
    let mut cb = fight(E14);
    let fb = || spec_of("fireball", E14, 14, None);
    assert!(matches!(
        dm_err(&mut cb, cast("m", fb(), Some(2), &["a"]).cmd()),
        CombatError::Unavailable(Unavailable::SlotTooLow(3))
    ));
    assert!(matches!(
        dm_err(&mut cb, cast("m", fb(), None, &["a"]).cmd()),
        CombatError::Unavailable(Unavailable::NeedsSlot(3))
    ));
    dm(
        &mut cb,
        Command::SetSlots {
            id: id("m"),
            level: 3,
            pool: Pool {
                max: 3,
                remaining: 0,
            },
        },
    );
    assert!(matches!(
        dm_err(&mut cb, cast("m", fb(), Some(3), &["a"]).force().cmd()),
        CombatError::Unavailable(Unavailable::NoSlot(3))
    ));
    // A refused cast spends nothing.
    assert!(!c(&cb, "m").economy.action);
}

#[test]
fn edition_rules_for_spells_in_one_turn() {
    let step = || {
        adhoc(json!({
            "key": "step", "edition": "2014", "name": "Step", "level": 2, "cast": "bonus",
            "targeting": {"kind": "self"}, "delivery": {"kind": "auto"}, "tier": "utility"
        }))
    };
    let bolt = || {
        adhoc(json!({
            "key": "bolt", "edition": "2014", "name": "Bolt", "level": 0, "cast": "action",
            "targeting": {"kind": "creatures", "max": "1"},
            "delivery": {"kind": "attack", "attack": "ranged"},
            "on_land": [{"kind": "damage", "dice": "(cantrip_tier)d10", "damage_type": "fire"}],
            "tier": "auto"
        }))
    };
    // 2014: after a bonus action spell, only an action cantrip.
    let mut cb = fight(E14);
    dm(&mut cb, cast("m", step(), Some(2), &[]).cmd());
    assert!(matches!(
        dm_err(
            &mut cb,
            cast("m", spec_of("fireball", E14, 14, None), Some(3), &["a"]).cmd()
        ),
        CombatError::Unavailable(Unavailable::BonusSpellRule)
    ));
    dm(&mut cb, cast("m", bolt(), None, &["a"]).typed("a", 1).cmd());
    // 2024: one slot per turn, reactions included; the DM can override.
    let mut cb = fight(E24);
    dm(
        &mut cb,
        cast("m", spec_of("shield", E24, 14, None), Some(1), &[]).cmd(),
    );
    assert!(matches!(
        dm_err(
            &mut cb,
            cast("m", spec_of("fireball", E24, 14, None), Some(3), &["a"]).cmd()
        ),
        CombatError::Unavailable(Unavailable::OneSlotPerTurn)
    ));
    dm(
        &mut cb,
        cast("m", spec_of("fireball", E24, 14, None), Some(3), &["a"])
            .typed("a", 1)
            .amount(typed(20))
            .force()
            .cmd(),
    );
    // A new turn clears it.
    dm(&mut cb, Command::NextTurn);
    let mut g = cb.state.clone();
    g.combatants
        .iter_mut()
        .find(|x| x.id == id("m"))
        .unwrap()
        .economy = Economy::default();
    let s = &g;
    assert!(derive::rules_status(
        s,
        s.combatant(&id("m")).unwrap(),
        &spec_of("fireball", E24, 14, None),
        Some(3)
    )
    .is_ok());
}

// ----- rulings and cancelling -----

#[test]
fn counterspell_is_a_ruling_and_cancel_refunds() {
    let mut cb = fight(E24);
    dm(&mut cb, Command::NextTurn); // goblin
    dm(&mut cb, Command::NextTurn); // owlbear
    dm(&mut cb, Command::NextTurn); // wizard
                                    // The wizard's fireball waits for the creatures' saves.
    dm(
        &mut cb,
        cast(
            "w",
            spec_of("fireball", E24, 15, None),
            Some(3),
            &["a", "o"],
        )
        .cmd(),
    );
    let theirs = pending(&cb).id.clone();
    // 0.3.3: a PC's slots are on the player's sheet: the combat never spends them.
    assert_eq!(c(&cb, "w").spell_slots[&3].remaining, 2);
    let ev = dm(
        &mut cb,
        cast("m", spec_of("counterspell", E24, 14, None), Some(3), &["w"]).cmd(),
    );
    let buttons = ev
        .dm
        .iter()
        .find_map(|e| match &e.kind {
            DmEventKind::RulingNeeded { buttons, .. } => Some(buttons.clone()),
            _ => None,
        })
        .expect("a ruling");
    assert_eq!(buttons.len(), 3);
    assert!(c(&cb, "m").economy.reaction);
    dm(
        &mut cb,
        Command::CancelCast {
            cast: theirs,
            refund: true,
        },
    );
    assert!(cb.state.casts.is_empty());
    assert_eq!(
        c(&cb, "w").spell_slots[&3].remaining,
        2,
        "a PC's slot is never spent in the combat"
    );
}

// ----- zones, stat blocks and standard actions -----

#[test]
fn a_zone_applies_to_the_creatures_the_dm_taps() {
    let mut cb = fight(E14);
    let guardians = adhoc(json!({
        "key": "guard", "edition": "2014", "name": "Guardians", "level": 3, "concentration": true,
        "cast": "action", "targeting": {"kind": "self"}, "delivery": {"kind": "auto"},
        "always": [{"kind": "zone", "zone": {"save": {"ability": "wis", "success": "half"},
            "damage": "3d8+(slot_above)d8", "damage_type": "radiant"}}],
        "duration": {"kind": "minutes", "n": 10}, "tier": "auto"
    }));
    dm(&mut cb, cast("m", guardians, Some(4), &[]).cmd());
    let zone = cb
        .state
        .effects
        .iter()
        .find(|e| e.spec.name == "Guardians")
        .expect("the zone lives on the caster");
    let tag = zone.spec.spell.clone().unwrap();
    assert_eq!(tag.vars.slot, 4);
    let eid = zone.id;
    let slots = c(&cb, "m").spell_slots.clone();
    let cmd = Command::ApplyZone {
        effect: eid,
        targets: vec![id("o")],
        rows: vec![RowInput {
            target: id("o"),
            ray: 0,
            roll: RowRoll::App,
            use_lr: false,
        }],
        amounts: vec![AmountRoll::App],
        id: None,
    };
    // WIS save d20 3 + 1 = 4 fails; 4d8 all 2 = 8.
    let ev = scripted(&mut cb, cmd, &[3, 2, 2, 2, 2]);
    assert!(ev
        .dm
        .iter()
        .any(|e| matches!(&e.kind, DmEventKind::CastApplied { amounts, .. } if amounts[0].notation == "4d8")));
    assert_eq!(c(&cb, "o").hp.current, 59 - 8);
    assert_eq!(c(&cb, "m").spell_slots, slots, "a zone spends nothing");
}

#[test]
fn stat_block_attacks_spend_the_action() {
    let mut cb = fight(E14);
    dm(&mut cb, Command::NextTurn);
    dm(&mut cb, Command::NextTurn); // owlbear's turn
    let beak = adhoc(json!({
        "key": "beak", "edition": "2014", "name": "Beak", "level": 0, "cast": "action",
        "targeting": {"kind": "creatures", "max": "1"},
        "delivery": {"kind": "attack", "attack": "melee"},
        "on_land": [{"kind": "damage", "dice": "1d10+5", "damage_type": "piercing"}],
        "tier": "auto"
    }));
    let mut cb2 = cb.clone();
    let mut ab = Ability::new("beak", "Beak", AbilityKind::Action);
    ab.mech = Some(Box::new(CastSpec {
        origin: MechOrigin::Statblock,
        attack: Some(7),
        ..beak
    }));
    let mut o = c(&cb2, "o").clone();
    o.abilities.push(ab);
    cb2.state.combatants.retain(|x| x.id != id("o"));
    cb2.state.combatants.push(o);
    cb = cb2;
    let perform_as = |rows: Vec<RowInput>, as_reaction: bool| Command::PerformStatblock {
        id: id("o"),
        ability: "beak".into(),
        spec: None,
        targets: vec![id("f")],
        rows,
        amounts: vec![typed(10)],
        crit_amounts: vec![],
        force: false,
        cast_id: None,
        as_reaction,
    };
    let perform = |rows: Vec<RowInput>| perform_as(rows, false);
    let ev = dm(
        &mut cb,
        perform(vec![RowInput {
            target: id("f"),
            ray: 0,
            roll: RowRoll::Typed {
                total: 22,
                natural: Some(15),
            },
            use_lr: false,
        }]),
    );
    assert!(ev
        .players
        .iter()
        .any(|p| matches!(p, PlayerEvent::DamagePrompt { amount: 10, .. })));
    assert!(c(&cb, "o").economy.action);
    assert!(matches!(
        dm_err(&mut cb, perform(vec![])),
        CombatError::Unavailable(Unavailable::ActionSpent)
    ));
    // An opportunity attack spends the reaction instead, once; the ability stays an action.
    let miss = || {
        vec![RowInput {
            target: id("f"),
            ray: 0,
            roll: RowRoll::Typed {
                total: 3,
                natural: Some(1),
            },
            use_lr: false,
        }]
    };
    dm(&mut cb, perform_as(miss(), true));
    assert!(c(&cb, "o").economy.reaction);
    assert_eq!(
        c(&cb, "o").ability("beak").unwrap().kind,
        AbilityKind::Action
    );
    assert_eq!(c(&cb, "f").hp.current, 44, "a natural 1 misses");
    assert!(matches!(
        dm_err(&mut cb, perform_as(miss(), true)),
        CombatError::Unavailable(Unavailable::ReactionSpent)
    ));
}

#[test]
fn invisibility_carries_the_attack_reminder() {
    let mut cb = fight(E14);
    let invis = adhoc(json!({
        "key": "invis", "edition": "2014", "name": "Unseen", "level": 2, "concentration": true,
        "cast": "action", "targeting": {"kind": "creatures", "max": "1", "include_self": true},
        "delivery": {"kind": "auto"},
        "always": [{"kind": "condition", "condition": "invisible", "ends_on_attack": true}],
        "duration": {"kind": "minutes", "n": 60}, "tier": "auto"
    }));
    dm(&mut cb, cast("m", invis, Some(2), &["m"]).cmd());
    let e = cb
        .state
        .effects
        .iter()
        .find(|e| e.spec.name == "Unseen")
        .unwrap();
    assert!(e.spec.ends_on_attack && !e.spec.ends_on_damage);
    assert!(derive::has_condition(
        &cb.state,
        &id("m"),
        Condition::Invisible
    ));
}

#[test]
fn dodge_and_help() {
    let mut cb = fight(E14);
    dm(
        &mut cb,
        Command::StandardAction {
            id: id("m"),
            action: StandardActionKind::Dodge,
            target: None,
            bonus: false,
        },
    );
    assert!(c(&cb, "m").economy.action);
    let am = derive::attack_mode(
        &cb.state,
        &id("a"),
        &id("m"),
        derive::AttackContext::default(),
    );
    assert_eq!(am.mode, RollMode::Disadvantage);
    let sm = derive::save_mod(&cb.state, &id("m"), AbilityName::Dex, false);
    assert_eq!(sm.mode, RollMode::Advantage);
    assert!(matches!(
        dm_err(
            &mut cb,
            Command::StandardAction {
                id: id("m"),
                action: StandardActionKind::Dash,
                target: None,
                bonus: false,
            }
        ),
        CombatError::Unavailable(Unavailable::ActionSpent)
    ));
    dm(
        &mut cb,
        Command::StandardAction {
            id: id("m"),
            action: StandardActionKind::Help,
            target: Some(id("a")),
            bonus: true,
        },
    );
    assert!(cb.state.effects.iter().any(|e| e.spec.name == "Helped"));
}

#[test]
fn paralyzed_creatures_fail_dex_saves_without_a_roll() {
    let mut cb = fight(E14);
    let mut p = effect("Held", "a");
    p.conditions = vec![Condition::Paralyzed];
    dm(
        &mut cb,
        Command::AddCondition {
            effect: Box::new(p),
        },
    );
    let mut die = SequenceDie::new(vec![]);
    let cmd = cast("m", spec_of("fireball", E14, 14, None), Some(3), &["a"])
        .row("a", RowRoll::App)
        .cmd();
    cb.execute_with(&Actor::Dm, &cmd, &mut die).unwrap();
    assert_eq!(pending(&cb).rows[0].outcome, Some(RowOutcome::Failed));
}

// ----- what players see -----

#[test]
fn every_projected_field_is_classified() {
    use party_combat::visibility::*;
    let mut cb = fight(E14);
    dm(
        &mut cb,
        cast("m", spec_of("hold-person", E14, 14, None), Some(2), &["a"]).cmd(),
    );
    dm(
        &mut cb,
        cast("m", spec_of("mage-armor", E14, 14, None), Some(1), &["m"])
            .force()
            .cmd(),
    );
    let mut s = cb.state.clone();
    // Fill every optional field so every key shows.
    let full = {
        let mut c = s.combatant(&id("m")).unwrap().clone();
        c.removed = false;
        c.linked_to = Some(id("a"));
        c.owner_id = Some(id("a"));
        c.legendary_resistance = Some(Pool::full(1));
        c.resources.insert(
            "x".into(),
            Resource {
                label: "X".into(),
                pool: Pool::full(1),
                reset: Reset::Never,
            },
        );
        c.group = Some("g".into());
        c.notes = "n".into();
        c.concentration = Some(Concentration { name: "k".into() });
        c.pending_concentration = vec![ConcentrationCheck { dc: 10, damage: 1 }];
        c.casting = vec![Casting {
            ability: AbilityName::Int,
            dc: Some(14),
            attack: Some(6),
            caster_level: Some(9),
            cantrip_tier: Some(2),
        }];
        let mut ab = Ability::new("fb", "Fireball", AbilityKind::Action);
        ab.text = "t".into();
        ab.legendary_cost = Some(1);
        ab.recharge = Some(Recharge {
            min: 5,
            ready: true,
        });
        ab.uses = Some("x".into());
        ab.group_limit = Some("g".into());
        ab.requires_effect = Some("e".into());
        ab.spell = Some(SpellLink {
            key: Some("k".into()),
            level: 3,
            frequency: SpellFrequency::Slot,
            fixed_slot: Some(3),
            baked_in: true,
        });
        ab.mech = Some(Box::new(spec_of("fireball", E14, 14, None)));
        c.abilities = vec![ab];
        c
    };
    let v = serde_json::to_value(&full).unwrap();
    for k in v.as_object().unwrap().keys() {
        assert!(
            COMBATANT_PLAYER.contains(&k.as_str()) || COMBATANT_DM_ONLY.contains(&k.as_str()),
            "combatant field {k} is not classified"
        );
    }
    for k in v["abilities"][0].as_object().unwrap().keys() {
        assert!(
            ABILITY_PLAYER.contains(&k.as_str()) || ABILITY_DM_ONLY.contains(&k.as_str()),
            "ability field {k} is not classified"
        );
    }
    let mut e = s.effects[0].clone();
    e.spec.ends_on_defeat_of = vec![id("a")];
    e.spec.follow_up = Some(Box::new(effect("f", "a")));
    e.spec.dismiss_on_end = true;
    e.spec.ends_on_damage = true;
    e.spec.charges = Some(1);
    e.spec.detail = "d".into();
    e.spec.source = Some(id("m"));
    let ev = serde_json::to_value(&e).unwrap();
    for k in ev.as_object().unwrap().keys() {
        assert!(
            EFFECT_PLAYER.contains(&k.as_str()) || EFFECT_DM_ONLY.contains(&k.as_str()),
            "effect field {k} is not classified"
        );
    }
    s.last_visible = Some(VisibleMark {
        id: id("m"),
        round: 1,
    });
    s.rules = vec![];
    let sv = serde_json::to_value(&s).unwrap();
    for k in sv.as_object().unwrap().keys() {
        assert!(
            STATE_PROJECTED.contains(&k.as_str()) || STATE_DM_ONLY.contains(&k.as_str()),
            "state field {k} is not classified"
        );
    }
    // The projection keeps only player fields: no casting data, no spell links, no live legendary counts.
    s.combatants.retain(|x| x.id != id("m"));
    let mut shown = full.clone();
    shown.visible = true;
    s.combatants.push(shown);
    let view = player_view_of(&s, &id("m")).unwrap();
    for k in view.as_object().unwrap().keys() {
        assert!(
            COMBATANT_PLAYER.contains(&k.as_str()) || k == "effects" || k == "conditions",
            "{k} reached players"
        );
    }
    for k in [
        "stats",
        "casting",
        "spell_turn",
        "legendary_resistance",
        "label",
        "notes",
    ] {
        assert!(view.get(k).is_none(), "{k} reached players");
    }
    assert!(view["abilities"][0].get("mech").is_none());
    assert!(view["abilities"][0].get("spell").is_none());
    for fx in view["effects"].as_array().unwrap() {
        assert!(fx.get("spell").is_none());
    }

    // 0.3.3, the DM's side: every combatant key is classified for a player's combatant too (the DM sees it, or it
    // is the player's number), and the DM view of a PC and of a player's creature holds only the allowlist.
    for k in v.as_object().unwrap().keys() {
        let dm = COMBATANT_DM_OF_PLAYER.contains(&k.as_str());
        let private = COMBATANT_PLAYER_PRIVATE.contains(&k.as_str());
        assert!(
            dm != private,
            "combatant field {k} is not classified for the DM (or is in both lists)"
        );
    }
    let mut pc = full.clone();
    pc.id = id("pc-full");
    pc.kind = CombatantKind::Character {
        owner: ALICE.into(),
    };
    pc.hp = Hp {
        current: 17,
        max: 31,
        temp: 4,
    };
    pc.life = Life::Dying {
        successes: 1,
        failures: 2,
    };
    let mut pet = full.clone();
    pet.id = id("pet-full");
    pet.controller = Controller::Player { pc: id("pc-full") };
    s.combatants.push(pc);
    s.combatants.push(pet);
    let dm_view = dm_view_of(&s);
    for who in ["pc-full", "pet-full"] {
        let entry = dm_view["combatants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == who)
            .unwrap();
        for k in entry.as_object().unwrap().keys() {
            assert!(
                COMBATANT_DM_OF_PLAYER.contains(&k.as_str()) || k == "down",
                "{k} of a player's combatant reached the DM"
            );
        }
        for k in COMBATANT_PLAYER_PRIVATE {
            assert!(entry.get(*k).is_none(), "{k} reached the DM");
        }
        assert_eq!(entry["concentration"], true, "yes/no only");
    }
    // The DM's own creature is untouched.
    let mage = dm_view["combatants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "a")
        .unwrap();
    assert!(mage.get("hp").is_some());
}

#[test]
fn a_revealed_caster_is_named_in_the_save_request() {
    let mut cb = fight(E24);
    dm(&mut cb, Command::Reveal { id: id("m") });
    let ev = dm(
        &mut cb,
        cast(
            "m",
            spec_of("fireball", E24, 14, None),
            Some(3),
            &["w", "f"],
        )
        .cmd(),
    );
    let reqs: Vec<_> = ev
        .players
        .iter()
        .filter_map(|p| match p {
            PlayerEvent::SaveRequest {
                owner, spell, from, ..
            } => Some((owner.clone(), spell.clone(), from.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(reqs.len(), 2);
    assert!(reqs
        .iter()
        .all(|(_, s, f)| s.as_deref() == Some("Fireball") && f == &Some(id("m"))));
    let rows = player_casts(&cb.state, BOB);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["spell"], "Fireball");
    assert_eq!(rows[0]["target"], "f");
}

// ----- players cast (0.3.3: the DM's screen and the player's screen) -----

#[test]
fn a_player_casts_fireball_and_the_dm_resolves_the_creatures_saves() {
    use party_combat::visibility::dm_may_see;
    let mut cb = fight(E24);
    dm(&mut cb, Command::Reveal { id: id("a") });
    dm(&mut cb, Command::Reveal { id: id("o") });
    let ev = cb
        .execute(
            &alice(),
            &cast(
                "w",
                spec_of("fireball", E24, 15, None),
                Some(3),
                &["a", "o"],
            )
            .cmd(),
        )
        .unwrap();
    // The slot is spent on the sheet, never in the combat (the DM never sees the player's slots).
    assert_eq!(c(&cb, "w").spell_slots[&3].remaining, 2);
    // The DM hears of it: the cast and its open rows.
    assert!(ev.dm.iter().any(|e| !e.private
        && dm_may_see(&cb.state, e)
        && matches!(&e.kind, DmEventKind::CastStarted { spell, .. } if spell == "Fireball")));
    let theirs = pending(&cb).id.clone();
    assert!(!pending(&cb).private);
    // The creatures' saves are the DM's to roll, not the player's.
    let row = |t: &str| Command::ResolveCastRow {
        cast: theirs.clone(),
        target: id(t),
        ray: 0,
        roll: RowRoll::Typed {
            total: 5,
            natural: Some(5),
        },
        use_lr: false,
    };
    assert_eq!(
        cb.execute(&alice(), &row("a")),
        Err(CombatError::NotAllowed)
    );
    dm(&mut cb, row("a"));
    dm(&mut cb, row("o"));
    // The player rolls their own damage.
    cb.execute(
        &alice(),
        &Command::ApplyCast {
            cast: theirs,
            amounts: vec![AmountRoll::Typed { total: 20 }],
            crit_amounts: vec![],
            force: false,
        },
    )
    .unwrap();
    assert!(c(&cb, "a").hp.current < c(&cb, "a").hp.max);
}

#[test]
fn a_player_cannot_cast_for_others_or_force_or_precast() {
    let mut cb = fight(E24);
    dm(&mut cb, Command::Reveal { id: id("a") });
    dm(&mut cb, Command::Reveal { id: id("o") });
    for cmd in [
        cast("f", spec_of("fireball", E24, 15, None), Some(3), &["a"]).cmd(),
        cast("m", spec_of("fireball", E24, 15, None), Some(3), &["a"]).cmd(),
        cast("w", spec_of("fireball", E24, 15, None), Some(3), &["a"])
            .force()
            .cmd(),
    ] {
        assert_eq!(cb.execute(&alice(), &cmd), Err(CombatError::NotAllowed));
    }
    // A hidden creature is no target for a player.
    dm(&mut cb, Command::Hide { id: id("o") });
    assert_eq!(
        cb.execute(
            &alice(),
            &cast("w", spec_of("fireball", E24, 15, None), Some(3), &["o"]).cmd()
        ),
        Err(CombatError::NotAllowed)
    );
}

#[test]
fn cure_wounds_on_an_ally_never_reaches_the_dm() {
    use party_combat::visibility::{dm_may_see, dm_view_of};
    let mut cb = fight(E24);
    let ev = cb
        .execute(
            &alice(),
            &cast("w", spec_of("cure-wounds", E24, 15, None), Some(1), &["f"])
                .amount(AmountRoll::Typed { total: 9 })
                .cmd(),
        )
        .unwrap();
    assert!(!ev.dm.is_empty());
    assert!(ev.dm.iter().all(|e| e.private && !dm_may_see(&cb.state, e)));
    assert!(ev
        .players
        .iter()
        .any(|p| matches!(p, PlayerEvent::HealPrompt { owner, amount: 9, .. } if owner == BOB)));
    assert!(
        dm_view_of(&cb.state).get("casts").is_none(),
        "no open cast for the DM"
    );
}

#[test]
fn shield_on_yourself_is_shown_to_the_dm() {
    use party_combat::visibility::{dm_may_see, dm_view_of};
    let mut cb = fight(E24);
    let ev = cb
        .execute(
            &alice(),
            &cast("w", spec_of("shield", E24, 15, None), Some(1), &[]).cmd(),
        )
        .unwrap();
    assert!(ev.dm.iter().any(|e| dm_may_see(&cb.state, e)
        && matches!(&e.kind, DmEventKind::CastStarted { spell, .. } if spell == "Shield")));
    // The chip is on the PC, and the DM sees the AC it resolves attacks against.
    let view = dm_view_of(&cb.state);
    assert!(view["effects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["name"] == "Shield" && e["target"] == "w"));
    assert!(derive::effective_ac(&cb.state, c(&cb, "w")).total >= Some(17));
}

#[test]
fn a_pcs_hp_and_rolls_stay_private_and_the_dms_damage_is_a_prompt() {
    use party_combat::visibility::{dm_may_see, dm_view_of};
    let mut cb = fight(E24);
    let ev = cb
        .execute(
            &Actor::System,
            &Command::SyncPlayerHp {
                id: id("w"),
                current: 9,
                max: 22,
                temp: 3,
                damage: None,
            },
        )
        .unwrap();
    assert!(ev.dm.iter().all(|e| !dm_may_see(&cb.state, e)));
    let ev = dm(&mut cb, hit("w", 13, Some(DamageType::Bludgeoning)));
    assert!(
        ev.dm.iter().all(|e| dm_may_see(&cb.state, e)),
        "the DM knows what it sent"
    );
    assert!(ev.players.iter().any(
        |p| matches!(p, PlayerEvent::DamagePrompt { owner, amount: 13, .. } if owner == ALICE)
    ));
    let ev = cb
        .execute(
            &alice(),
            &Command::Roll {
                id: id("w"),
                label: "Perception".into(),
                roll: RollInput::D20(party_combat::party_dice::D20Input::typed(14)),
            },
        )
        .unwrap();
    assert!(ev.dm.iter().all(|e| !dm_may_see(&cb.state, e)));
    let view = dm_view_of(&cb.state);
    let w = view["combatants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "w")
        .unwrap()
        .clone();
    for k in [
        "hp",
        "life",
        "spell_slots",
        "resources",
        "pending_concentration",
    ] {
        assert!(w.get(k).is_none(), "{k} reached the DM");
    }
    assert_eq!(w["down"], false);
    assert!(w.get("ac").is_some());
    // Legacy log rows (no flag) are filtered by kind.
    let mut legacy = ev.dm[0].clone();
    legacy.private = false;
    assert!(!dm_may_see(&cb.state, &legacy));
}
