//! The cases of `ui/src/test/dice.test.ts`, plus typed results (D43) and seeded parity with the UI.

use super::*;

fn fmt(text: &str) -> String {
    parse_notation(text).unwrap().to_string()
}

fn seq(values: &[u32]) -> SequenceDie {
    SequenceDie::new(values.to_vec())
}

#[test]
fn parses_single_dice_sums_and_flat_modifiers() {
    assert_eq!(fmt("1d8+3"), "1d8+3");
    assert_eq!(fmt("2d6"), "2d6");
    assert_eq!(fmt("1d4+1d6+2"), "1d4+1d6+2");
    assert_eq!(fmt("d20"), "1d20");
    assert_eq!(fmt(" 1D10 - 1 "), "1d10-1");
    assert_eq!(fmt("-1+1d4"), "-1+1d4");
    assert_eq!(fmt("5"), "5");
}

#[test]
fn keeps_terms_in_order_with_signs() {
    let expr = parse_notation("2d6-1d4+3").unwrap();
    assert_eq!(
        expr.terms,
        vec![
            Term::Dice {
                count: 2,
                sides: 6,
                sign: Sign::Plus
            },
            Term::Dice {
                count: 1,
                sides: 4,
                sign: Sign::Minus
            },
            Term::Flat { value: 3 },
        ]
    );
}

#[test]
fn refuses_bad_notation() {
    let many = format!("{}1", "1+".repeat(40));
    let big = "9".repeat(12);
    for text in [
        "",
        "   ",
        "abc",
        "1d",
        "0d6",
        "1d0",
        "1d6++2",
        "1d6+",
        "+",
        "1d6 2",
        "1d6*2",
        "101d6",
        "1d1001",
        "1d6+1.5",
        many.as_str(),
        big.as_str(),
    ] {
        assert!(parse_notation(text).is_err(), "{text:?} should be refused");
        assert!(try_parse_notation(text).is_none());
    }
    assert_eq!(parse_notation("1d"), Err(DiceError::NoSides));
    assert_eq!(parse_notation("1d6+"), Err(DiceError::TrailingOperator));
    assert_eq!(parse_notation("1d6 2"), Err(DiceError::MissingOperator));
    assert_eq!(parse_notation("1d6++2"), Err(DiceError::TwoOperators));
    assert_eq!(
        parse_notation("1d6*2"),
        Err(DiceError::Unreadable("*2".into()))
    );
    assert_eq!(DiceError::Sides.to_string(), "A die has 1 to 1000 sides.");
}

#[test]
fn damage_dice_drops_damage_types() {
    let dd = |t: &str| damage_dice(t).map(|e| e.to_string());
    assert_eq!(dd("1d8+3 slashing").as_deref(), Some("1d8+3"));
    assert_eq!(
        dd("1d8+3 slashing + 1d6 fire").as_deref(),
        Some("1d8+3+1d6")
    );
    assert_eq!(dd("2d10 Fire").as_deref(), Some("2d10"));
    assert_eq!(dd("1d4-1 piercing").as_deref(), Some("1d4-1"));
    assert_eq!(dd("4 bludgeoning").as_deref(), Some("4"));
    assert_eq!(
        dd("1d8+2 slashing, 2d6 on a hit while raging").as_deref(),
        Some("1d8+2")
    );
    assert_eq!(dd("DC 13 DEX"), None);
    assert_eq!(dd("see notes"), None);
    assert_eq!(dd(""), None);
}

#[test]
fn critical_doubles_dice_only() {
    let c = |t: &str| critical(&parse_notation(t).unwrap()).to_string();
    assert_eq!(c("1d8+3"), "2d8+3");
    assert_eq!(c("2d6+1d4-1"), "4d6+2d4-1");
    assert_eq!(c("4"), "4");
}

#[test]
fn roll_expr_sums_and_writes_breakdown() {
    let r = roll_expr(
        &parse_notation("2d6+1d4+2").unwrap(),
        &mut seq(&[3, 4, 2]),
        &RollOptions::default(),
    )
    .unwrap();
    assert_eq!(r.total, 11);
    assert_eq!(r.breakdown, "2d6 [3, 4] +1d4 [2] +2 = 11");
    assert_eq!(
        r.terms.iter().map(|t| t.rolls.clone()).collect::<Vec<_>>(),
        vec![vec![3, 4], vec![2], vec![]]
    );
    assert_eq!(r.source, RollSource::Rolled);

    let r = roll_expr(
        &parse_notation("1d4-1").unwrap(),
        &mut seq(&[1]),
        &RollOptions::default(),
    )
    .unwrap();
    assert_eq!(r.breakdown, "1d4 [1] -1 = 0");
    let r = roll_expr(
        &parse_notation("1d6-1d4").unwrap(),
        &mut seq(&[5, 2]),
        &RollOptions::default(),
    )
    .unwrap();
    assert_eq!(r.total, 3);
}

#[test]
fn roll_expr_floor_and_label() {
    let opts = RollOptions {
        floor: Some(3),
        label: None,
    };
    let r = roll_expr(&parse_notation("2d6+3").unwrap(), &mut seq(&[1, 5]), &opts).unwrap();
    assert_eq!(r.total, 11);
    assert_eq!(r.breakdown, "2d6 [3, 5] +3 = 11, dice below 3 count as 3");

    let opts = RollOptions {
        floor: None,
        label: Some("critical".into()),
    };
    let r = roll_expr(
        &critical(&parse_notation("1d8+3").unwrap()),
        &mut seq(&[5, 3]),
        &opts,
    )
    .unwrap();
    assert_eq!(r.breakdown, "2d8 [5, 3] +3 = 11, critical");
}

#[test]
fn roll_expr_rejects_a_bad_die_and_an_exhausted_script() {
    let expr = parse_notation("1d6").unwrap();
    assert_eq!(
        roll_expr(&expr, &mut seq(&[7]), &RollOptions::default()),
        Err(DiceError::BadDie { sides: 6, value: 7 })
    );
    assert_eq!(
        roll_expr(&expr, &mut seq(&[0]), &RollOptions::default()),
        Err(DiceError::BadDie { sides: 6, value: 0 })
    );
    assert_eq!(
        roll_expr(&expr, &mut seq(&[]), &RollOptions::default()),
        Err(DiceError::OutOfRolls)
    );
    // a closure is a die too
    let r = roll_expr(&expr, &mut |_s: u32| 4, &RollOptions::default()).unwrap();
    assert_eq!(r.total, 4);
}

#[test]
fn d20_modes_and_breakdown() {
    let r = roll_d20(5, RollMode::Normal, &mut seq(&[14]), None).unwrap();
    assert_eq!((r.total, r.natural), (19, Some(14)));
    assert_eq!(r.breakdown, "d20 [14] +5 = 19");
    assert_eq!(
        roll_d20(5, RollMode::Advantage, &mut seq(&[14, 7]), None)
            .unwrap()
            .breakdown,
        "d20 [14, 7] +5 = 19, advantage"
    );
    assert_eq!(
        roll_d20(5, RollMode::Disadvantage, &mut seq(&[14, 7]), None)
            .unwrap()
            .breakdown,
        "d20 [14, 7] +5 = 12, disadvantage"
    );
    assert_eq!(
        roll_d20(-1, RollMode::Disadvantage, &mut seq(&[3, 18]), None)
            .unwrap()
            .total,
        2
    );
    assert_eq!(
        roll_d20(0, RollMode::Normal, &mut seq(&[9]), None)
            .unwrap()
            .breakdown,
        "d20 [9] +0 = 9"
    );
    assert_eq!(
        roll_d20(-2, RollMode::Normal, &mut seq(&[9]), None)
            .unwrap()
            .breakdown,
        "d20 [9] -2 = 7"
    );
}

#[test]
fn d20_naturals_and_crit_range() {
    let twenty = roll_d20(2, RollMode::Normal, &mut seq(&[20]), None).unwrap();
    assert!(twenty.is_nat20 && twenty.is_crit);
    assert_eq!(twenty.breakdown, "d20 [20] +2 = 22, natural 20");
    assert!(
        roll_d20(2, RollMode::Normal, &mut seq(&[1]), None)
            .unwrap()
            .is_nat1
    );
    assert!(
        roll_d20(2, RollMode::Normal, &mut seq(&[19]), Some(19))
            .unwrap()
            .is_crit
    );
    assert!(
        !roll_d20(2, RollMode::Normal, &mut seq(&[19]), None)
            .unwrap()
            .is_crit
    );
    assert!(
        !roll_d20(2, RollMode::Disadvantage, &mut seq(&[20, 4]), None)
            .unwrap()
            .is_crit
    );
}

#[test]
fn advantage_and_disadvantage_cancel() {
    assert_eq!(RollMode::combine(true, true), RollMode::Normal);
    assert_eq!(RollMode::combine(true, false), RollMode::Advantage);
    assert_eq!(RollMode::combine(false, true), RollMode::Disadvantage);
}

#[test]
fn typed_d20_never_touches_the_die() {
    let mut die = seq(&[]);
    let r = resolve_d20(&D20Input::typed(17), &mut die).unwrap();
    assert_eq!(r.source, RollSource::Typed);
    assert_eq!((r.total, r.natural, r.modifier), (17, None, None));
    assert!(!r.is_nat20 && !r.is_nat1);
    assert_eq!(r.breakdown, "typed 17");

    let r = resolve_d20(&D20Input::typed_natural(24, 20), &mut die).unwrap();
    assert!(r.is_nat20 && r.is_crit);
    assert_eq!(r.breakdown, "typed 24 (natural 20)");
    assert_eq!(
        resolve_d20(&D20Input::typed_natural(5, 21), &mut die),
        Err(DiceError::TypedNatural)
    );
    assert_eq!(
        resolve_d20(&D20Input::typed(2_000_000), &mut die),
        Err(DiceError::TypedTotal)
    );
}

#[test]
fn rolled_d20_input_uses_the_die() {
    let r = resolve_d20(&D20Input::roll(3, RollMode::Advantage), &mut seq(&[4, 11])).unwrap();
    assert_eq!(r.total, 14);
    assert_eq!(r.source, RollSource::Rolled);
}

#[test]
fn typed_and_rolled_dice_input() {
    let r = resolve_dice(&DiceInput::typed(12), &mut seq(&[])).unwrap();
    assert_eq!((r.total, r.source), (12, RollSource::Typed));
    let r = resolve_dice(&DiceInput::roll("1d8+3"), &mut seq(&[5])).unwrap();
    assert_eq!(r.total, 8);
    let crit = DiceInput::Roll {
        notation: "1d8+3".into(),
        critical: true,
        floor: None,
    };
    let r = resolve_dice(&crit, &mut seq(&[5, 3])).unwrap();
    assert_eq!(r.breakdown, "2d8 [5, 3] +3 = 11, critical");
    assert!(resolve_dice(&DiceInput::roll("nope"), &mut seq(&[])).is_err());
}

#[test]
fn inputs_serialize_with_a_kind_tag() {
    let json = serde_json::to_string(&D20Input::typed_natural(15, 12)).unwrap();
    assert_eq!(json, r#"{"kind":"typed","total":15,"natural":12}"#);
    let back: D20Input =
        serde_json::from_str(r#"{"kind":"roll","modifier":2,"mode":"advantage"}"#).unwrap();
    assert_eq!(
        back,
        D20Input::Roll {
            modifier: 2,
            mode: RollMode::Advantage,
            crit_range: None
        }
    );
    let back: DiceInput = serde_json::from_str(r#"{"kind":"typed","total":9}"#).unwrap();
    assert_eq!(back, DiceInput::typed(9));
}

/// The UI's `seededDie(42)` d20s and `seededDie(7)` mixed dice (computed with Node from `ui/src/lib/dice.ts`).
#[test]
fn seeded_die_matches_the_ui() {
    let mut d = SeededDie::new(42);
    let rolls: Vec<u32> = (0..10).map(|_| d.roll(20).unwrap()).collect();
    assert_eq!(rolls, vec![13, 9, 18, 14, 4, 11, 6, 13, 18, 10]);
    let mut e = SeededDie::new(7);
    let rolls: Vec<u32> = [6, 8, 100, 1000, 20, 4]
        .iter()
        .map(|&s| e.roll(s).unwrap())
        .collect();
    assert_eq!(rolls, vec![1, 1, 98, 700, 11, 2]);
}

#[test]
fn seeded_die_is_reproducible_and_serializable() {
    for seed in [0u32, 1, 99, u32::MAX] {
        let mut a = SeededDie::new(seed);
        let _ = a.roll(20);
        let json = serde_json::to_string(&a).unwrap();
        let mut b: SeededDie = serde_json::from_str(&json).unwrap();
        for sides in [2, 4, 6, 8, 10, 12, 20, 100] {
            let (x, y) = (a.roll(sides).unwrap(), b.roll(sides).unwrap());
            assert_eq!(x, y);
            assert!((1..=sides).contains(&x));
        }
    }
}

#[test]
fn seeded_d6_is_close_to_uniform() {
    let mut die = SeededDie::new(12345);
    let mut counts = [0u32; 6];
    let n = 60_000;
    for _ in 0..n {
        counts[die.roll(6).unwrap() as usize - 1] += 1;
    }
    for c in counts {
        assert!((f64::from(c) - f64::from(n) / 6.0).abs() < f64::from(n) / 6.0 * 0.05);
    }
}

#[test]
fn advantage_and_disadvantage_averages() {
    let mut die = SeededDie::new(2024);
    let n = 40_000;
    let mean = |mode, die: &mut SeededDie| {
        (0..n)
            .map(|_| f64::from(roll_d20(0, mode, die, None).unwrap().total))
            .sum::<f64>()
            / f64::from(n)
    };
    assert!((mean(RollMode::Advantage, &mut die) - 13.825).abs() < 0.15);
    assert!((mean(RollMode::Disadvantage, &mut die) - 7.175).abs() < 0.15);
}

/// Property: any parsed expression formats back to text that parses to the same expression, and a seeded roll's
/// total stays within the expression's bounds.
#[test]
fn property_format_round_trip_and_bounds() {
    let mut gen = SeededDie::new(5);
    for _ in 0..2000 {
        let n = gen.roll(4).unwrap();
        let mut text = String::new();
        let (mut lo, mut hi) = (0i64, 0i64);
        for i in 0..n {
            let neg = gen.roll(4).unwrap() == 1;
            if i > 0 || neg {
                text.push(if neg { '-' } else { '+' });
            }
            let s = if neg { -1 } else { 1 };
            if gen.roll(3).unwrap() == 1 {
                let v = gen.roll(20).unwrap();
                text.push_str(&v.to_string());
                lo += s * i64::from(v);
                hi += s * i64::from(v);
            } else {
                let (c, sides) = (
                    gen.roll(5).unwrap(),
                    [4, 6, 8, 10, 12, 20][gen.roll(6).unwrap() as usize - 1],
                );
                text.push_str(&format!("{c}d{sides}"));
                let (a, b) = (i64::from(c), i64::from(c * sides));
                if neg {
                    lo -= b;
                    hi -= a;
                } else {
                    lo += a;
                    hi += b;
                }
            }
        }
        let expr = parse_notation(&text).unwrap();
        assert_eq!(parse_notation(&expr.to_string()).unwrap(), expr, "{text}");
        let total = i64::from(
            roll_expr(&expr, &mut gen, &RollOptions::default())
                .unwrap()
                .total,
        );
        assert!(
            (lo..=hi).contains(&total),
            "{text}: {total} not in {lo}..={hi}"
        );
    }
}
