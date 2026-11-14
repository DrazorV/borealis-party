//! Basic: the deterministic policy, a pure function of the simulation. Integer expected values (×1000) over the
//! card's options and legal targets, with a few habits by intelligence:
//! - INT 4 or less attacks the nearest enemy and sticks to it;
//! - INT 5-9 sticks to its target while it stands;
//! - INT 10 or more focuses on casters and concentration, and on what it can drop.
//!
//! Limited uses (recharge, per day, slots) are spent only when worth at least 1.3× the best at-will option. A PC
//! heals a dying ally first. A creature with INT 6 or more that isn't undead or a construct flees at a quarter of
//! its HP once its side has lost half its HP. Ties break by combatant order, then by option id.

use party_combat::{derive, CombatantId, Life};

use crate::menu::{Decision, Opt};
use crate::rules;
use crate::runner::{Sim, Window};
use crate::setup::Side;
use crate::sheet::{parts_avg2, ActionDef, AttackDef, Def, SaveDef, SpellDef};

/// Expected damage of one attack on `target`, ×1000 (in units of 1/2 HP × 1/400 chance, rescaled).
fn attack_ev(sim: &Sim, actor: &CombatantId, target: &CombatantId, x: &AttackDef) -> i64 {
    let Some(t) = sim.state.combatant(target) else {
        return 0;
    };
    let ac = derive::effective_ac(&sim.state, t).total.unwrap_or(12);
    let ranged = !x.melee;
    let cx = sim.context(actor, target, ranged, x);
    let m = rules::attack_mode(&sim.state, actor, target, cx);
    let p = i64::from(rules::chance_400(x.to_hit, ac, m.mode, true));
    let mut dmg = i64::from(parts_avg2(&x.parts));
    if m.auto_crit {
        dmg = dmg * 3 / 2;
    }
    // p/400 × dmg/2 × 1000
    p * dmg * 1000 / 800 * i64::from(x.count.max(1))
}

fn save_ev_one(sim: &Sim, target: &CombatantId, s: &SaveDef) -> i64 {
    let bonus = sim.sheets.get(target).map_or(0, |sh| sh.save(s.ability));
    let mr = sim
        .sheets
        .get(target)
        .is_some_and(|sh| sh.traits.magic_resistance);
    let m = rules::save_mode(&sim.state, target, s.ability, s.spell, mr);
    let succeed = if m.auto_fail {
        0
    } else {
        i64::from(rules::chance_400(bonus, s.dc, m.mode, false))
    };
    let fail = 400 - succeed;
    let full = i64::from(parts_avg2(&s.parts));
    let half = if s.half { full / 2 } else { 0 };
    let cond = if s.rider.is_some() { 6 } else { 0 }; // a condition is worth about 3 HP
    (fail * (full + cond) + succeed * half) * 1000 / 800
}

/// Expected value of a spell on one target, ×1000 (the same units as attacks): its damage and conditions through
/// the save or attack chance, its healing for a dying ally. Pure buffs count 0 (Basic doesn't spend turns on them).
fn spell_ev_one(
    sim: &Sim,
    actor: &CombatantId,
    target: &CombatantId,
    s: &SpellDef,
    slot: Option<u8>,
) -> i64 {
    use party_combat::spell::{Delivery, Primitive, SaveSuccess};
    let spec = &s.spec;
    let m = &spec.mech;
    let vars = spec.vars(slot);
    let Some(t) = sim.state.combatant(target) else {
        return 0;
    };
    let enemy = sim.side(target) != sim.side(actor);
    let mut dmg2: i64 = 0;
    let mut cond2: i64 = 0;
    let mut heal2: i64 = 0;
    for p in &m.on_land {
        match p {
            Primitive::Damage { .. } if enemy => {
                if let Primitive::Damage { dice, .. } = p {
                    dmg2 += i64::from(crate::spells::avg2(dice, &vars));
                }
            }
            Primitive::Condition { condition, .. } if enemy => {
                if !t.defenses.condition_immunities.contains(condition)
                    && !derive::has_condition(&sim.state, target, *condition)
                {
                    cond2 += i64::from(crate::spells::condition_value2(*condition, t.hp.current));
                }
            }
            Primitive::Heal { dice, add_mod } if !enemy => {
                heal2 += i64::from(crate::spells::avg2(dice, &vars))
                    + if *add_mod {
                        i64::from(spec.modifier) * 2
                    } else {
                        0
                    };
            }
            _ => {}
        }
    }
    if heal2 > 0 {
        let urgent = matches!(t.life, Life::Dying { .. })
            || (target == actor && t.hp.current * 4 <= t.hp.max);
        return if urgent { heal2 * 1000 + 100_000 } else { 0 };
    }
    if !enemy || !t.life.is_alive() {
        return 0;
    }
    match &m.delivery {
        Delivery::Save { ability, success } => {
            let sm = derive::save_mod(&sim.state, target, *ability, true);
            let dc = i32::try_from(spec.dc.unwrap_or(10)).unwrap_or(10);
            let succeed = if sm.auto_fail {
                0
            } else {
                i64::from(rules::chance_400(sm.bonus, dc, sm.mode, false))
            };
            let fail = 400 - succeed;
            let half = if *success == SaveSuccess::Half {
                dmg2 / 2
            } else {
                0
            };
            (fail * (dmg2 + cond2) + succeed * half) * 1000 / 800
        }
        Delivery::Attack { attack, .. } => {
            let ac = derive::effective_ac(&sim.state, t).total.unwrap_or(12);
            let cx = party_combat::derive::AttackContext {
                ranged: *attack == party_combat::spell::AttackKind::Ranged,
                adjacent: *attack == party_combat::spell::AttackKind::Melee,
                ..Default::default()
            };
            let am = derive::attack_mode(&sim.state, actor, target, cx);
            let p = i64::from(rules::chance_400(
                spec.attack.unwrap_or(0),
                ac,
                am.mode,
                true,
            ));
            p * (dmg2 + cond2) * 1000 / 800
        }
        Delivery::Auto => (dmg2 + cond2) * 1000 / 2,
    }
}

/// The best targets of a spell option and its score: the area around the best enemy, the top creatures up to its
/// maximum, or every ray on the best target. A new concentration spell while concentrating is skipped, and a
/// higher slot costs a little so Basic upcasts only when it pays.
fn spell_pick(
    sim: &Sim,
    actor: &CombatantId,
    opt: &Opt,
    s: &SpellDef,
) -> Option<(i64, Vec<String>)> {
    use party_combat::spell::Targeting;
    if s.concentration()
        && sim
            .state
            .combatant(actor)
            .is_some_and(|c| c.concentration.is_some())
    {
        return None;
    }
    let mut scored: Vec<(i64, CombatantId)> = opt
        .targets
        .iter()
        .map(|t| {
            let ev = spell_ev_one(sim, actor, t, s, opt.slot);
            let k = if sim.side(t) == sim.side(actor) {
                100
            } else {
                priority(sim, actor, t, ev)
            };
            (ev * k / 100, t.clone())
        })
        .filter(|(ev, _)| *ev > 0)
        .collect();
    // Highest first; ties by roster order (the targets' listed order).
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    let (best, first) = scored.first().cloned()?;
    let max = usize::try_from(opt.max_targets.max(1)).unwrap_or(1);
    let (score, targets) = match &s.spec.mech.targeting {
        Targeting::Area { .. } => {
            let mut out = vec![first.clone()];
            let mut total = best;
            for (ev, t) in &scored[1..] {
                if out.len() < max && sim.space.steps(t, &first) == 0 {
                    out.push(t.clone());
                    total += ev;
                }
            }
            (total, out)
        }
        _ if opt.repeat_targets => (best * i64::try_from(max).unwrap_or(1), vec![first]),
        _ => {
            let top: Vec<(i64, CombatantId)> = scored.into_iter().take(max).collect();
            (
                top.iter().map(|(ev, _)| ev).sum(),
                top.into_iter().map(|(_, t)| t).collect(),
            )
        }
    };
    let above = i64::from(opt.slot.unwrap_or(s.level()).saturating_sub(s.level()));
    let score = score - above * 1500;
    (score > 0).then(|| (score, targets.into_iter().map(|t| t.0).collect()))
}

/// The targets a save effect would catch around `primary`.
fn area_targets(sim: &Sim, opt: &Opt, s: &SaveDef, primary: &CombatantId) -> Vec<CombatantId> {
    let max = usize::try_from(s.area.as_ref().map_or(1, |a| a.max_targets)).unwrap_or(1);
    if s.area.is_none() {
        return vec![primary.clone()];
    }
    let mut out = vec![primary.clone()];
    for t in &opt.targets {
        if t != primary && sim.standing(t) && sim.space.steps(t, primary) == 0 && out.len() < max {
            out.push(t.clone());
        }
    }
    out
}

/// A multiplier ×100 for what this target means to `actor`.
fn priority(sim: &Sim, actor: &CombatantId, target: &CombatantId, ev: i64) -> i64 {
    let int = sim.sheets.get(actor).map_or(10, |s| s.int);
    let Some(t) = sim.state.combatant(target) else {
        return 100;
    };
    let mut k = 100;
    // Dropping it now (expected damage at least its HP).
    if ev / 500 >= i64::from(t.hp.current.max(1)) {
        k += 50;
    }
    if int >= 10 {
        if t.concentration.is_some() {
            k += 30;
        }
        if sim.sheets.get(target).is_some_and(|s| s.caster) {
            k += 25;
        }
    }
    if !t.life.is_alive() {
        // Downed: only the mindless keep hitting.
        k = if int <= 4 { 40 } else { 0 };
    }
    k
}

fn limited(sim: &Sim, actor: &CombatantId, a: &ActionDef) -> bool {
    let Some(c) = sim.state.combatant(actor) else {
        return false;
    };
    let ab = c.ability(&a.key);
    a.slot.is_some()
        || a.resource.is_some()
        || ab.is_some_and(|x| x.recharge.is_some() || x.uses.is_some())
}

struct Pick {
    score: i64,
    o: String,
    t: Vec<String>,
    limited: bool,
}

fn evaluate(sim: &Sim, actor: &CombatantId, opt: &Opt) -> Option<Pick> {
    let a = sim.sheets.get(actor)?.actions.get(opt.action?)?;
    if let (Def::Spell(s), crate::menu::OptKind::Spell) = (&a.def, &opt.kind) {
        let (score, t) = spell_pick(sim, actor, opt, s)?;
        return Some(Pick {
            score,
            o: opt.o.clone(),
            t,
            limited: opt.slot.is_some() || limited(sim, actor, a),
        });
    }
    let int = sim.sheets.get(actor).map_or(10, |s| s.int);
    let sticky = sim
        .sticky
        .get(actor)
        .filter(|t| opt.targets.contains(t) && sim.standing(t))
        .cloned();
    let mut candidates: Vec<CombatantId> = opt
        .targets
        .iter()
        .filter(|t| sim.standing(t))
        .cloned()
        .collect();
    if candidates.is_empty() && int <= 4 {
        candidates = opt.targets.clone();
    }
    if int <= 4 {
        // The nearest, unless it already has one.
        let nearest = sticky.clone().or_else(|| {
            candidates
                .iter()
                .min_by_key(|t| sim.space.steps(actor, t))
                .cloned()
        });
        candidates = nearest.into_iter().collect();
    } else if int <= 9 {
        if let Some(s) = sticky.clone() {
            candidates = vec![s];
        }
    }
    let mut best: Option<(i64, Vec<String>)> = None;
    for t in &candidates {
        let (ev, targets) = match &a.def {
            Def::Attack(x) => (attack_ev(sim, actor, t, x), vec![t.0.clone()]),
            Def::Multiattack { steps, .. } => {
                let mut ev = 0;
                for st in steps {
                    match sim.action_by_key(actor, &st.key).map(|d| &d.def) {
                        Some(Def::Attack(x)) => {
                            ev += attack_ev(sim, actor, t, x) * i64::from(st.count);
                        }
                        Some(Def::Save(s)) => ev += save_ev_one(sim, t, s),
                        _ => {}
                    }
                }
                (ev, vec![t.0.clone()])
            }
            Def::Save(s) => {
                let ts = area_targets(sim, opt, s, t);
                let ev: i64 = ts
                    .iter()
                    .map(|x| {
                        save_ev_one(sim, x, s) * priority(sim, actor, x, save_ev_one(sim, x, s))
                            / 100
                    })
                    .sum();
                (ev, ts.iter().map(|x| x.0.clone()).collect())
            }
            Def::Heal { dice } => {
                // Only for a dying ally (or itself at a quarter of its HP).
                let Some(c) = sim.state.combatant(t) else {
                    continue;
                };
                let urgent = matches!(c.life, Life::Dying { .. })
                    || (t == actor && c.hp.current * 4 <= c.hp.max);
                if !urgent || sim.side(t) != sim.side(actor) {
                    continue;
                }
                let avg = party_combat::party_dice::try_parse_notation(dice)
                    .map_or(0, |e| crate::sheet::avg2(&e));
                (i64::from(avg) * 1000 + 100_000, vec![t.0.clone()])
            }
            Def::Spell(_) | Def::Text => continue,
        };
        let scored = if matches!(a.def, Def::Save(_) | Def::Heal { .. }) {
            ev
        } else {
            ev * priority(sim, actor, t, ev) / 100
        };
        if best.as_ref().is_none_or(|(b, _)| scored > *b) {
            best = Some((scored, targets));
        }
    }
    // Heal options also look at dying allies outside `standing`.
    if matches!(a.def, Def::Heal { .. }) && best.is_none() {
        for t in &opt.targets {
            if sim
                .state
                .combatant(t)
                .is_some_and(|c| matches!(c.life, Life::Dying { .. }))
            {
                best = Some((200_000, vec![t.0.clone()]));
                break;
            }
        }
    }
    let (score, t) = best?;
    (score > 0).then(|| Pick {
        score,
        o: opt.o.clone(),
        t,
        limited: limited(sim, actor, a),
    })
}

/// Fleeing: INT 6 or more, not fearless, at a quarter of its HP, its side down to half its total HP.
fn wants_to_flee(sim: &Sim, actor: &CombatantId) -> bool {
    let Some(c) = sim.state.combatant(actor) else {
        return false;
    };
    let Some(sheet) = sim.sheets.get(actor) else {
        return false;
    };
    if sim.side(actor) != Side::Foes || sheet.int < 6 || sheet.fearless {
        return false;
    }
    if c.hp.current * 4 > c.hp.max {
        return false;
    }
    let (cur, max) = sim
        .state
        .combatants
        .iter()
        .filter(|o| sim.side(&o.id) == Side::Foes && !o.removed)
        .fold((0i64, 0i64), |(a, b), o| {
            (a + i64::from(o.hp.current.max(0)), b + i64::from(o.hp.max))
        });
    cur * 2 <= max
}

/// Basic's decision for `actor` now.
#[must_use]
pub fn decide(sim: &Sim, actor: &CombatantId) -> Decision {
    let options = sim.options(actor);
    let legendary = matches!(sim.window, Window::Legendary { .. });
    let picks: Vec<Pick> = options
        .iter()
        .filter_map(|o| evaluate(sim, actor, o))
        .collect();
    let best_at_will = picks
        .iter()
        .filter(|p| !p.limited)
        .map(|p| p.score)
        .max()
        .unwrap_or(0);
    let mut best: Option<&Pick> = None;
    for p in &picks {
        if p.limited && p.score * 10 < best_at_will * 13 && best_at_will > 0 {
            continue;
        }
        if best.is_none_or(|b| p.score > b.score) {
            best = Some(p);
        }
    }
    if legendary {
        return match best {
            Some(p) => Decision {
                o: p.o.clone(),
                t: p.t.clone(),
                why: Some("basic".into()),
                ..Decision::default()
            },
            None => Decision {
                o: "pass".into(),
                ..Decision::default()
            },
        };
    }
    let has = |o: &str| options.iter().any(|x| x.o == o);
    if wants_to_flee(sim, actor) {
        let engaged = !sim.space.engaged_with(actor, &sim.alive_ids()).is_empty();
        if engaged && has("disengage") {
            return Decision {
                o: "disengage".into(),
                mv: Some("back".into()),
                why: Some("basic: flee".into()),
                ..Decision::default()
            };
        }
        if has("dash") {
            return Decision {
                o: "dash".into(),
                mv: Some("back".into()),
                why: Some("basic: flee".into()),
                ..Decision::default()
            };
        }
        return Decision {
            o: "end".into(),
            mv: Some("back".into()),
            ..Decision::default()
        };
    }
    if let Some(p) = best {
        let mv = approach(sim, actor, &options, p);
        return Decision {
            o: p.o.clone(),
            t: p.t.clone(),
            mv,
            why: Some("basic".into()),
            ..Decision::default()
        };
    }
    // Nothing in reach: close in (dash if the action is free and there is nothing better to do).
    let alive = sim.alive_ids();
    let target = sim
        .sticky
        .get(actor)
        .filter(|t| sim.standing(t))
        .cloned()
        .or_else(|| {
            alive
                .iter()
                .filter(|o| sim.side(o) != sim.side(actor) && sim.standing(o))
                .min_by_key(|o| sim.space.steps(actor, o))
                .cloned()
        });
    let melee_only = sim.sheets.get(actor).is_some_and(|s| {
        !s.actions
            .iter()
            .any(|a| matches!(&a.def, Def::Attack(x) if x.ranged))
    });
    match target {
        Some(t) if sim.space.steps(actor, &t) > 0 => {
            let mv = if melee_only {
                format!("engage:{}", t.0)
            } else {
                "near".into()
            };
            if has("dash") && melee_only {
                Decision {
                    o: "dash".into(),
                    mv: Some(mv),
                    why: Some("basic: close in".into()),
                    ..Decision::default()
                }
            } else {
                Decision {
                    o: "end".into(),
                    mv: Some(mv),
                    ..Decision::default()
                }
            }
        }
        _ if has("dodge")
            && sim
                .state
                .combatant(actor)
                .is_some_and(|c| c.hp.current * 2 <= c.hp.max) =>
        {
            Decision {
                o: "dodge".into(),
                ..Decision::default()
            }
        }
        _ => Decision {
            o: "end".into(),
            ..Decision::default()
        },
    }
}

/// The move before a ranged attack: closer when its melee would do more next turn, or into normal range.
fn approach(sim: &Sim, actor: &CombatantId, options: &[Opt], p: &Pick) -> Option<String> {
    let opt = options.iter().find(|o| o.o == p.o)?;
    let a = sim.sheets.get(actor)?.actions.get(opt.action?)?;
    let x = match &a.def {
        Def::Attack(x) => x,
        Def::Multiattack { steps, .. } => match steps
            .first()
            .and_then(|s| sim.action_by_key(actor, &s.key))
            .map(|d| &d.def)
        {
            Some(Def::Attack(x)) => x,
            _ => return None,
        },
        _ => return None,
    };
    let t = CombatantId::new(p.t.first()?.clone());
    let d = sim.space.steps(actor, &t);
    if d == 0 || (x.melee && d <= sim.turn.steps_left) {
        return None; // melee in reach: the runner moves in by itself
    }
    // A melee-minded creature (its best melee hits harder than this ranged option) closes in.
    let sheet = sim.sheets.get(actor)?;
    let melee_best = sheet
        .actions
        .iter()
        .filter_map(|m| match &m.def {
            Def::Attack(mx) if mx.melee => {
                Some(parts_avg2(&mx.parts) * i32::try_from(mx.count).unwrap_or(1))
            }
            Def::Multiattack { steps, .. } => Some(
                steps
                    .iter()
                    .filter_map(|s| match sim.action_by_key(actor, &s.key).map(|d| &d.def) {
                        Some(Def::Attack(mx)) if mx.melee => {
                            Some(parts_avg2(&mx.parts) * i32::try_from(s.count).unwrap_or(1))
                        }
                        _ => None,
                    })
                    .sum(),
            ),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let ranged_now = parts_avg2(&x.parts) * i32::try_from(x.count).unwrap_or(1);
    let in_reach_after = d - sim.turn.steps_left;
    if melee_best > ranged_now && in_reach_after >= 1 {
        return Some(format!("engage:{}", t.0));
    }
    // Into normal range.
    let normal = i32::try_from(x.range / crate::space::STEP_FT).unwrap_or(0);
    if d > normal && sim.turn.steps_left > 0 {
        return Some("near".into());
    }
    None
}
