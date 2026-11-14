//! Spells and stat block actions (0.3.2): cast, resolve, apply, cancel, zones, standard actions and binding.
//!
//! A cast spends first (slot, economy, uses, concentration), then opens a [`PendingCast`] with one row per target
//! (one per ray). Rows take a typed roll (D21: the table's dice first), an app roll or the DM's outcome. When
//! every row is resolved and the amounts are known, the cast applies: one amount roll for everyone, halved for
//! savers, then the effects. Everything happens inside one command, so a cast with all its rolls is one undo step.

use super::{account_of, Cx};
use crate::command::{AmountRoll, Command, DamagePart, RowInput, RowRoll, StandardActionKind};
use crate::derive::{self, AttackContext};
use crate::error::{invalid, CombatError};
use crate::event::{DmEventKind as K, PlayerEvent};
use crate::model::{
    AbilityKind, Boundary, CastId, CastRow, CombatantId, Controller, Duration, EffectId,
    EffectOrigin, EffectSpec, HpAuthority, Modifier, PendingCast, RepeatSave, RowOutcome,
    SpellTurn,
};
use crate::spell::{
    AdvDis, AttackKind, CastChoices, CastSpec, CastTime, Delivery, FormulaVars, MechOrigin, ModeOn,
    Primitive, SaveSuccess, SpeedChange, SpellDuration, SpellFrequency, SpellMech, SpellTag,
    TargetFilter, Targeting, Tier,
};
use crate::visibility::{is_visible, ViewerKind};
use party_dice::{
    critical, resolve_d20, resolve_dice, roll_d20, roll_expr, D20Input, D20Result, DiceExpr,
    DiceInput, DiceResult, RollOptions, Term,
};

/// Which part of a definition a primitive is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    OnLand,
    OnResist,
    Always,
}

/// What opening a cast needs.
struct Open<'a> {
    caster: CombatantId,
    ability: Option<String>,
    spec: CastSpec,
    slot: Option<u8>,
    vars: FormulaVars,
    targets: &'a [CombatantId],
    rows: &'a [RowInput],
    amounts: &'a [AmountRoll],
    crit: &'a [AmountRoll],
    choices: CastChoices,
    force: bool,
    id: Option<CastId>,
    zone_of: Option<EffectId>,
    statblock: bool,
}

fn sections(m: &SpellMech) -> impl Iterator<Item = (Section, &Primitive)> {
    m.on_land
        .iter()
        .map(|p| (Section::OnLand, p))
        .chain(m.on_resist.iter().map(|p| (Section::OnResist, p)))
        .chain(m.always.iter().map(|p| (Section::Always, p)))
}

fn is_amount(p: &Primitive) -> bool {
    matches!(
        p,
        Primitive::Damage { .. } | Primitive::Heal { .. } | Primitive::TempHp { .. }
    )
}

/// The amount primitives, in order: `on_land`, then `on_resist`, then `always`.
fn amount_prims(m: &SpellMech) -> Vec<(Section, &Primitive)> {
    sections(m).filter(|(_, p)| is_amount(p)).collect()
}

/// A primitive of `section` applies to a row with this outcome. Half damage on a save is handled separately.
fn applies(section: Section, o: RowOutcome) -> bool {
    match section {
        Section::OnLand => o.lands(),
        Section::OnResist => matches!(o, RowOutcome::Saved | RowOutcome::Miss),
        Section::Always => true,
    }
}

fn half_on_save(m: &SpellMech) -> bool {
    matches!(
        m.delivery,
        Delivery::Save {
            success: SaveSuccess::Half,
            ..
        }
    )
}

/// The amount the primitive deals or heals, before rolling.
fn amount_expr(p: &Primitive, vars: &FormulaVars) -> Result<DiceExpr, CombatError> {
    let (dice, add_mod) = match p {
        Primitive::Damage { dice, .. } => (dice, false),
        Primitive::Heal { dice, add_mod } | Primitive::TempHp { dice, add_mod } => (dice, *add_mod),
        _ => return Err(invalid("Not an amount.")),
    };
    let mut e = dice.dice(vars).map_err(|e| invalid(e.to_string()))?;
    if add_mod && vars.modifier != 0 {
        e.terms.push(Term::Flat {
            value: vars.modifier,
        });
    }
    Ok(e)
}

fn duration_of(d: SpellDuration) -> Duration {
    match d {
        SpellDuration::Instant | SpellDuration::Long | SpellDuration::UntilDispelled => {
            Duration::Manual
        }
        SpellDuration::SourceTurns { n, at } => Duration::SourceTurns { remaining: n, at },
        SpellDuration::TargetTurns { n, at } => Duration::TargetTurns { remaining: n, at },
        SpellDuration::Minutes { n } => Duration::SourceTurns {
            remaining: n.saturating_mul(10),
            at: Boundary::Start,
        },
    }
}

/// The side a combatant fights on: the party (PCs and the creatures they control) or the DM's.
fn party_side(c: &crate::model::Combatant) -> bool {
    c.is_character() || matches!(c.controller, Controller::Player { .. })
}

impl Cx<'_> {
    pub(super) fn spell_command(&mut self, cmd: &Command) -> Result<(), CombatError> {
        // A private cast stays private to its end (0.3.3).
        if let Command::ResolveCastRow { cast, .. }
        | Command::ApplyCast { cast, .. }
        | Command::CancelCast { cast, .. } = cmd
        {
            if self.s.casts.iter().any(|c| &c.id == cast && c.private) {
                self.quiet = true;
            }
        }
        match cmd {
            Command::CastSpell {
                caster,
                ability,
                spec,
                slot,
                targets,
                rows,
                amounts,
                crit_amounts,
                choices,
                precast,
                force,
                id,
            } => self.cast_spell(
                caster,
                ability.as_deref(),
                spec.as_deref(),
                *slot,
                Open {
                    caster: caster.clone(),
                    ability: ability.clone(),
                    spec: CastSpec::ruling("", "", self.s.edition, 0, false),
                    slot: *slot,
                    vars: FormulaVars::default(),
                    targets,
                    rows,
                    amounts,
                    crit: crit_amounts,
                    choices: choices.clone(),
                    force: *force,
                    id: id.clone(),
                    zone_of: None,
                    statblock: false,
                },
                *precast,
            ),
            Command::ResolveCastRow {
                cast,
                target,
                ray,
                roll,
                use_lr,
            } => {
                self.resolve_row(cast, target, *ray, roll, *use_lr)?;
                self.apply_if_ready(cast)
            }
            Command::ApplyCast {
                cast,
                amounts,
                crit_amounts,
                force,
            } => self.apply_cast(cast, amounts, crit_amounts, *force),
            Command::CancelCast { cast, refund } => self.cancel_cast(cast, *refund),
            Command::ApplyZone {
                effect,
                targets,
                rows,
                amounts,
                id,
            } => self.apply_zone(*effect, targets, rows, amounts, id.clone()),
            Command::PerformStatblock {
                id,
                ability,
                spec,
                targets,
                rows,
                amounts,
                crit_amounts,
                force,
                cast_id,
                as_reaction,
            } => {
                let i = self.idx(id)?;
                let ab = self.s.combatants[i]
                    .ability(ability)
                    .cloned()
                    .ok_or_else(|| CombatError::UnknownAbility(ability.clone()))?;
                let spec = spec
                    .as_deref()
                    .cloned()
                    .or_else(|| ab.mech.as_deref().cloned())
                    .ok_or_else(|| {
                        invalid(format!(
                            "{} has no attack or save definition: use it as a plain ability.",
                            ab.name
                        ))
                    })?;
                if *as_reaction {
                    // Spend the reaction: use the ability as a reaction for this one use.
                    let k = self.s.combatants[i]
                        .abilities
                        .iter()
                        .position(|a| &a.key == ability)
                        .expect("found above");
                    let kind = self.s.combatants[i].abilities[k].kind;
                    self.s.combatants[i].abilities[k].kind = AbilityKind::Reaction;
                    let used = self.use_ability(id, ability, None);
                    self.s.combatants[i].abilities[k].kind = kind;
                    used?;
                } else {
                    self.use_ability(id, ability, None)?;
                }
                if spec.mech.concentration {
                    self.start_concentration(id, &spec.mech.name);
                }
                let vars = spec.vars(None);
                self.open_cast(Open {
                    caster: id.clone(),
                    ability: Some(ability.clone()),
                    spec,
                    slot: None,
                    vars,
                    targets,
                    rows,
                    amounts,
                    crit: crit_amounts,
                    choices: CastChoices::default(),
                    force: *force,
                    id: cast_id.clone(),
                    zone_of: None,
                    statblock: true,
                })
            }
            Command::StandardAction {
                id,
                action,
                target,
                bonus,
            } => self.standard_action(id, *action, target.as_ref(), *bonus),
            Command::BindToConcentration { id } => self.bind_to_concentration(id),
            _ => Err(invalid("Not a spell command.")),
        }
    }

    // ----- cast -----

    fn cast_spell(
        &mut self,
        caster: &CombatantId,
        ability: Option<&str>,
        spec: Option<&CastSpec>,
        slot: Option<u8>,
        mut open: Open<'_>,
        precast: bool,
    ) -> Result<(), CombatError> {
        let i = self.idx(caster)?;
        let ab = match ability {
            Some(k) => Some(
                self.s.combatants[i]
                    .ability(k)
                    .cloned()
                    .ok_or_else(|| CombatError::UnknownAbility(k.into()))?,
            ),
            None => None,
        };
        let spec = spec
            .cloned()
            .or_else(|| ab.as_ref().and_then(|a| a.mech.as_deref().cloned()))
            .ok_or_else(|| {
                invalid("No definition for this spell: send a ruling spec (CastSpec::ruling).")
            })?;
        if spec.v != crate::spell::MECH_VERSION {
            return Err(invalid(format!(
                "Spell definition version {} isn't supported.",
                spec.v
            )));
        }
        let link = ab.as_ref().and_then(|a| a.spell.as_ref());
        let slot = slot.or_else(|| link.and_then(|l| l.fixed_slot));
        let slotless = link.is_some_and(|l| l.frequency != SpellFrequency::Slot)
            || ab.as_ref().is_some_and(|a| a.uses.is_some());
        let c = &self.s.combatants[i];
        // 0.3.3: a PC's slots are on its player's sheet (the player spends them there): only the level is checked.
        let sheet = c.hp_authority == HpAuthority::Player;
        if sheet {
            let level = spec.mech.level;
            match slot {
                Some(l) if l < level || level == 0 => {
                    return Err(CombatError::Unavailable(crate::Unavailable::SlotTooLow(
                        level,
                    )))
                }
                None if level > 0 && !slotless => {
                    return Err(CombatError::Unavailable(crate::Unavailable::NeedsSlot(
                        level,
                    )))
                }
                _ => {}
            }
        } else {
            derive::slot_status(c, &spec, slot, slotless).map_err(CombatError::Unavailable)?;
        }
        let shown_targets: Vec<CombatantId> = if matches!(spec.mech.targeting, Targeting::SelfOnly)
        {
            vec![caster.clone()]
        } else {
            open.targets.to_vec()
        };
        if crate::visibility::cast_private(&self.s, caster, &spec, &shown_targets) {
            self.quiet = true;
        }
        if !precast && !open.force {
            derive::rules_status(&self.s, c, &spec, slot).map_err(CombatError::Unavailable)?;
        }
        if let Some(a) = &ab {
            if let Some(rule) = derive::rule_blocking(&self.s, c, &a.key) {
                return Err(CombatError::Unavailable(crate::Unavailable::Rule(rule)));
            }
            if let Some(key) = &a.uses {
                match c.resources.get(key) {
                    Some(r) if r.pool.remaining > 0 => {}
                    Some(r) => {
                        return Err(CombatError::Unavailable(crate::Unavailable::NoUses(
                            r.label.clone(),
                        )))
                    }
                    None => {
                        return Err(CombatError::Unavailable(crate::Unavailable::NoUses(
                            key.clone(),
                        )))
                    }
                }
            }
        }

        // Spend: economy, slot, uses, and this turn's spell record.
        let (round, current) = (self.s.round, self.s.current.clone());
        let cast_time = spec.mech.cast;
        let level = spec.mech.level;
        let c = &mut self.s.combatants[i];
        if !precast {
            match cast_time {
                CastTime::Action | CastTime::Minute => c.economy.action = true,
                CastTime::Bonus => c.economy.bonus_action = true,
                CastTime::Reaction => c.economy.reaction = true,
            }
            if let Some(turn) = current {
                let fresh = c
                    .spell_turn
                    .as_ref()
                    .is_none_or(|t| t.round != round || t.turn != turn);
                if fresh {
                    c.spell_turn = Some(SpellTurn {
                        round,
                        turn,
                        slot_spent: false,
                        bonus_spell: false,
                        leveled_action_spell: false,
                    });
                }
                let t = c.spell_turn.as_mut().expect("set");
                t.slot_spent |= slot.is_some();
                t.bonus_spell |= cast_time == CastTime::Bonus;
                t.leveled_action_spell |= cast_time == CastTime::Action && level > 0;
            }
        }
        if let Some(l) = slot.filter(|_| !sheet) {
            if let Some(p) = c.spell_slots.get_mut(&l) {
                p.remaining -= 1;
            }
        }
        if let Some(a) = &ab {
            if let Some(r) = a.uses.as_ref().and_then(|k| c.resources.get_mut(k)) {
                r.pool.remaining -= 1;
            }
            if let Some(x) = c
                .abilities
                .iter_mut()
                .find(|x| x.key == a.key)
                .and_then(|x| x.recharge.as_mut())
            {
                x.ready = false;
            }
        }
        if spec.mech.concentration {
            self.start_concentration(caster, &spec.mech.name);
        }
        open.vars = spec.vars(slot);
        open.slot = slot;
        open.spec = spec;
        self.open_cast(open)
    }

    fn check_targets(&self, o: &Open<'_>, targets: &[CombatantId]) -> Result<(), CombatError> {
        let m = &o.spec.mech;
        let caster = self.s.combatant(&o.caster).expect("checked");
        for t in targets {
            self.idx(t)?;
        }
        let mut unique: Vec<&CombatantId> = Vec::new();
        for t in targets {
            if !unique.contains(&t) {
                unique.push(t);
            }
        }
        match &m.delivery {
            Delivery::Attack { rays, .. } => {
                let rays = rays.int(&o.vars).map_err(|e| invalid(e.to_string()))?;
                if targets.len() > usize::try_from(rays.max(0)).unwrap_or(0) {
                    return Err(invalid(format!("{} has {rays} rays.", m.name)));
                }
            }
            _ if unique.len() != targets.len() => {
                return Err(invalid("A target is listed twice."));
            }
            _ => {}
        }
        if let Targeting::Creatures {
            max,
            filter,
            include_self,
        } = &m.targeting
        {
            let max = max.int(&o.vars).map_err(|e| invalid(e.to_string()))?;
            if !matches!(m.delivery, Delivery::Attack { .. })
                && unique.len() > usize::try_from(max.max(0)).unwrap_or(0)
            {
                return Err(invalid(format!(
                    "{} takes at most {max} targets at this level.",
                    m.name
                )));
            }
            if o.force {
                return Ok(());
            }
            for t in &unique {
                let tc = self.s.combatant(t).expect("checked");
                let n = self.name(t);
                if *t == &o.caster && !include_self {
                    return Err(invalid(format!("{} can't target its caster.", m.name)));
                }
                let same = party_side(tc) == party_side(caster);
                match filter {
                    TargetFilter::Any => {}
                    TargetFilter::Willing | TargetFilter::Ally if !same => {
                        return Err(invalid(format!(
                            "{n} isn't on the caster's side (the DM can apply it anyway)."
                        )))
                    }
                    TargetFilter::Enemy if same => {
                        return Err(invalid(format!(
                            "{n} is on the caster's side (the DM can apply it anyway)."
                        )))
                    }
                    TargetFilter::Type { types } => {
                        if let Some(ty) = tc.stats.as_ref().and_then(|s| s.creature_type.as_ref()) {
                            let ty = ty.to_lowercase();
                            if !types.iter().any(|x| ty.contains(&x.to_lowercase())) {
                                return Err(invalid(format!(
                                    "{n} isn't a {} (the DM can apply it anyway).",
                                    types.join(" or ")
                                )));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if !o.force && sections(m).any(|(_, p)| matches!(p, Primitive::AcBase { .. })) {
            for t in &unique {
                if let Some(armor) = derive::wears_armor(self.s.combatant(t).expect("checked")) {
                    return Err(invalid(format!(
                        "{} wears {armor} armor: {} has no effect (the DM can apply it anyway).",
                        self.name(t),
                        m.name
                    )));
                }
            }
        }
        Ok(())
    }

    fn open_cast(&mut self, o: Open<'_>) -> Result<(), CombatError> {
        let m = &o.spec.mech;
        let targets: Vec<CombatantId> = match m.targeting {
            Targeting::SelfOnly => {
                if o.targets.iter().any(|t| t != &o.caster) && !o.force {
                    return Err(invalid(format!("{} only affects its caster.", m.name)));
                }
                vec![o.caster.clone()]
            }
            _ => o.targets.to_vec(),
        };
        self.check_targets(&o, &targets)?;
        if !o.amounts.is_empty() && o.amounts.len() != amount_prims(m).len() {
            return Err(invalid(format!(
                "{} needs {} amounts (damage, healing, temp HP in order).",
                m.name,
                amount_prims(m).len()
            )));
        }
        let id = match &o.id {
            Some(id) => {
                if self.s.casts.iter().any(|c| &c.id == id) {
                    return Err(invalid(format!("Cast {id} already exists.")));
                }
                id.clone()
            }
            None => {
                let id = CastId(format!("c{}", self.s.next_cast.max(1)));
                self.s.next_cast = self.s.next_cast.max(1) + 1;
                id
            }
        };
        let auto = matches!(m.delivery, Delivery::Auto);
        let mut rows = Vec::new();
        for t in &targets {
            let ray = u32::try_from(rows.iter().filter(|r: &&CastRow| &r.target == t).count())
                .unwrap_or(0);
            rows.push(CastRow {
                target: t.clone(),
                ray,
                outcome: auto.then_some(RowOutcome::Lands),
                roll: None,
                used_lr: false,
            });
        }
        let name = m.name.clone();
        let notes = m.notes.clone();
        let concentration = m.concentration;
        let summons: Vec<_> = sections(m)
            .filter_map(|(_, p)| match p {
                Primitive::Summon { info } => Some(info.clone()),
                _ => None,
            })
            .collect();
        let rulings: Vec<_> = sections(m)
            .filter_map(|(_, p)| match p {
                Primitive::Ruling { question, buttons } => {
                    Some((question.clone(), buttons.clone()))
                }
                _ => None,
            })
            .collect();
        let uncurated = matches!(m.tier, Tier::Uncurated) || o.spec.origin == MechOrigin::Ruling;
        self.s.casts.push(PendingCast {
            id: id.clone(),
            caster: o.caster.clone(),
            ability: o.ability.clone(),
            spec: Box::new(o.spec.clone()),
            slot: o.slot,
            vars: o.vars,
            choices: o.choices.clone(),
            rows,
            round: self.s.round,
            zone_of: o.zone_of,
            statblock: o.statblock,
            private: self.quiet,
        });
        let cn = self.name(&o.caster);
        let slot_text = o
            .slot
            .map_or_else(String::new, |l| format!(" at level {l}"));
        let verb = if o.zone_of.is_some() {
            "applies the zone of"
        } else if o.statblock {
            "uses"
        } else {
            "casts"
        };
        let started = self.dm.len();
        self.ev(
            Some(&o.caster),
            K::CastStarted {
                cast: id.clone(),
                spell: name.clone(),
                slot: o.slot,
                pending: true,
                notes,
            },
            format!("{cn} {verb} {name}{slot_text}"),
        );
        for info in summons {
            self.ev(
                Some(&o.caster),
                K::SummonOptions {
                    caster: o.caster.clone(),
                    spell: name.clone(),
                    slot: o.slot,
                    info,
                    concentration,
                },
                format!("{name}: add the creatures with Add creature, controlled by {cn}"),
            );
        }
        for (question, buttons) in rulings {
            self.ev(
                Some(&o.caster),
                K::RulingNeeded {
                    cast: id.clone(),
                    question: question.clone(),
                    buttons,
                },
                format!("{name}: {question}"),
            );
        }
        if uncurated {
            self.ev(
                Some(&o.caster),
                K::RulingNeeded {
                    cast: id.clone(),
                    question: format!("{name} has no rules yet: apply its effect by hand."),
                    buttons: vec![],
                },
                format!("{name} has no rules yet: the DM rules"),
            );
        }
        for r in o.rows {
            self.resolve_row(&id, &r.target, r.ray, &r.roll, r.use_lr)?;
        }
        let k = self.cast_idx(&id)?;
        let ready = self.s.casts[k].rows.iter().all(|r| r.outcome.is_some());
        if ready && (!self.amounts_needed(k) || !o.amounts.is_empty()) {
            if let K::CastStarted { pending, .. } = &mut self.dm[started].kind {
                *pending = false;
            }
            return self.apply_cast(&id, o.amounts, o.crit, false);
        }
        self.request_saves(k);
        Ok(())
    }

    fn cast_idx(&self, id: &CastId) -> Result<usize, CombatError> {
        self.s
            .casts
            .iter()
            .position(|c| &c.id == id)
            .ok_or_else(|| invalid(format!("No pending cast {id}.")))
    }

    /// Some resolved row needs an amount with dice.
    fn amounts_needed(&self, k: usize) -> bool {
        let cast = &self.s.casts[k];
        let m = &cast.spec.mech;
        let half = half_on_save(m);
        amount_prims(m).iter().any(|(section, p)| {
            let dice = amount_expr(p, &cast.vars)
                .is_ok_and(|e| e.terms.iter().any(|t| matches!(t, Term::Dice { .. })));
            dice && cast.rows.iter().any(|r| {
                r.outcome.is_some_and(|o| {
                    applies(*section, o)
                        || (half
                            && *section == Section::OnLand
                            && matches!(p, Primitive::Damage { .. })
                            && o == RowOutcome::Saved)
                })
            })
        })
    }

    /// Sends a save request to the player of each unresolved save row's PC (anonymized when the caster is hidden).
    fn request_saves(&mut self, k: usize) {
        let cast = self.s.casts[k].clone();
        let Delivery::Save { ability, .. } = cast.spec.mech.delivery else {
            return;
        };
        let seen = is_visible(&self.s, &cast.caster, ViewerKind::Player);
        for r in cast.rows.iter().filter(|r| r.outcome.is_none()) {
            let Some(owner) = self
                .s
                .combatant(&r.target)
                .and_then(|c| account_of(&self.s, c))
            else {
                continue;
            };
            self.prompts.push(PlayerEvent::SaveRequest {
                id: r.target.clone(),
                owner,
                cast: cast.id.clone(),
                ray: r.ray,
                ability,
                dc: cast.spec.dc,
                spell: seen.then(|| cast.spec.mech.name.clone()),
                from: seen.then(|| cast.caster.clone()),
            });
        }
    }

    /// Rolls a d20 test in the app with `bonus`, `mode` and extra dice (Bless).
    fn roll_test(
        &mut self,
        bonus: i32,
        mode: party_dice::RollMode,
        dice: &[crate::spell::DiceTerm],
    ) -> Result<D20Result, CombatError> {
        let mut r = roll_d20(bonus, mode, self.die, None)?;
        for t in dice {
            let e = DiceExpr {
                terms: vec![t.term()],
            };
            let x = roll_expr(&e, self.die, &RollOptions::default())?;
            r.total += x.total;
            let list: Vec<String> = x
                .terms
                .iter()
                .flat_map(|t| t.rolls.iter().map(u32::to_string))
                .collect();
            r.breakdown = format!("{} {t} [{}] = {}", r.breakdown, list.join(", "), r.total);
        }
        Ok(r)
    }

    fn resolve_row(
        &mut self,
        cast: &CastId,
        target: &CombatantId,
        ray: u32,
        roll: &RowRoll,
        use_lr: bool,
    ) -> Result<(), CombatError> {
        let k = self.cast_idx(cast)?;
        let r = self.s.casts[k]
            .rows
            .iter()
            .position(|r| &r.target == target && r.ray == ray)
            .ok_or_else(|| {
                invalid(format!(
                    "{} isn't a target of that cast.",
                    self.name(target)
                ))
            })?;
        let c = self.s.casts[k].clone();
        let t = self.idx(target)?;
        let (outcome, rolled) = match (&c.spec.mech.delivery, roll) {
            (_, RowRoll::Outcome { outcome }) => (*outcome, None),
            (Delivery::Auto, _) => (RowOutcome::Lands, None),
            (Delivery::Save { ability, .. }, roll) => {
                let dc = c
                    .spec
                    .dc
                    .ok_or_else(|| invalid("No save DC: set the outcome."))?;
                match roll {
                    RowRoll::Typed { total, natural } => {
                        let input = D20Input::Typed {
                            total: *total,
                            natural: *natural,
                        };
                        let r = resolve_d20(&input, self.die)?;
                        let ok = r.total >= dc as i32;
                        (
                            if ok {
                                RowOutcome::Saved
                            } else {
                                RowOutcome::Failed
                            },
                            Some(r),
                        )
                    }
                    _ => {
                        let sm = derive::save_mod(&self.s, target, *ability, !c.statblock);
                        if sm.auto_fail {
                            (RowOutcome::Failed, None)
                        } else {
                            let r = self.roll_test(sm.bonus, sm.mode, &sm.dice)?;
                            let ok = r.total >= dc as i32;
                            (
                                if ok {
                                    RowOutcome::Saved
                                } else {
                                    RowOutcome::Failed
                                },
                                Some(r),
                            )
                        }
                    }
                }
            }
            (Delivery::Attack { attack, .. }, roll) => {
                let ac = derive::effective_ac(&self.s, &self.s.combatants[t]).total;
                let cx = AttackContext {
                    ranged: *attack == AttackKind::Ranged,
                    adjacent: *attack == AttackKind::Melee,
                    ..AttackContext::default()
                };
                let am = derive::attack_mode(&self.s, &c.caster, target, cx);
                let r = match roll {
                    RowRoll::Typed { total, natural } => resolve_d20(
                        &D20Input::Typed {
                            total: *total,
                            natural: *natural,
                        },
                        self.die,
                    )?,
                    _ => {
                        let bonus = c
                            .spec
                            .attack
                            .ok_or_else(|| invalid("No attack bonus: type the roll."))?;
                        self.roll_test(bonus, am.mode, &am.dice)?
                    }
                };
                let hit = if r.is_nat20 {
                    true
                } else if r.is_nat1 {
                    false
                } else {
                    let ac = ac.ok_or_else(|| invalid("AC unknown: set hit or miss."))?;
                    r.total >= ac
                };
                let outcome = match (hit, r.is_nat20 || (am.auto_crit && hit)) {
                    (true, true) => RowOutcome::Crit,
                    (true, false) => RowOutcome::Hit,
                    _ => RowOutcome::Miss,
                };
                (outcome, Some(r))
            }
        };
        let mut outcome = outcome;
        let mut used_lr = false;
        if use_lr {
            if outcome != RowOutcome::Failed {
                return Err(invalid(
                    "Legendary Resistance turns a failed save into a success.",
                ));
            }
            let c = &mut self.s.combatants[t];
            match c.legendary_resistance.as_mut() {
                Some(p) if p.remaining > 0 => p.remaining -= 1,
                _ => return Err(invalid("No Legendary Resistance left.")),
            }
            outcome = RowOutcome::Saved;
            used_lr = true;
        }
        let row = &mut self.s.casts[k].rows[r];
        row.outcome = Some(outcome);
        row.roll = rolled.clone();
        row.used_lr = used_lr;
        let n = self.name(target);
        let word = match outcome {
            RowOutcome::Failed => "fails",
            RowOutcome::Saved if used_lr => "uses Legendary Resistance and succeeds",
            RowOutcome::Saved => "succeeds",
            RowOutcome::Hit => "is hit",
            RowOutcome::Crit => "takes a critical hit",
            RowOutcome::Miss => "is missed",
            RowOutcome::Lands => "is affected",
        };
        let detail = rolled
            .as_ref()
            .map_or_else(String::new, |r| format!(" ({})", r.breakdown));
        if let (Some(r), Delivery::Save { ability, .. }) = (&rolled, &c.spec.mech.delivery) {
            self.report_d20(target, &format!("{ability} save"), r);
        }
        self.ev(
            Some(target),
            K::CastRow {
                cast: cast.clone(),
                target: target.clone(),
                outcome,
                roll: rolled,
            },
            format!("{n} {word} against {}{detail}", c.spec.mech.name),
        );
        Ok(())
    }

    /// After a row is resolved: applies the cast when every row is in and no dice amount is needed.
    fn apply_if_ready(&mut self, cast: &CastId) -> Result<(), CombatError> {
        let k = self.cast_idx(cast)?;
        if self.s.casts[k].rows.iter().all(|r| r.outcome.is_some()) && !self.amounts_needed(k) {
            return self.apply_cast(cast, &[], &[], false);
        }
        Ok(())
    }

    fn roll_amount(
        &mut self,
        input: Option<&AmountRoll>,
        expr: &DiceExpr,
        crit: bool,
    ) -> Result<DiceResult, CombatError> {
        Ok(match input {
            Some(AmountRoll::Typed { total }) => {
                let r = resolve_dice(&DiceInput::typed(*total), self.die)?;
                if r.total < 0 {
                    return Err(invalid("An amount can't be negative."));
                }
                r
            }
            _ => {
                let e = if crit { critical(expr) } else { expr.clone() };
                let opts = RollOptions {
                    floor: None,
                    label: crit.then(|| "critical".to_string()),
                };
                roll_expr(&e, self.die, &opts)?
            }
        })
    }

    #[allow(clippy::too_many_lines)]
    fn apply_cast(
        &mut self,
        cast: &CastId,
        amounts: &[AmountRoll],
        crit: &[AmountRoll],
        force: bool,
    ) -> Result<(), CombatError> {
        let k = self.cast_idx(cast)?;
        let c = self.s.casts[k].clone();
        let m = &c.spec.mech;
        let waiting = c.rows.iter().filter(|r| r.outcome.is_none()).count();
        if waiting > 0 && !force {
            return Err(invalid(format!("Waiting for {waiting} rolls.")));
        }
        let prims = amount_prims(m);
        if !amounts.is_empty() && amounts.len() != prims.len() {
            return Err(invalid(format!(
                "{} needs {} amounts.",
                m.name,
                prims.len()
            )));
        }
        let any_crit = c.rows.iter().any(|r| r.outcome == Some(RowOutcome::Crit));
        let half = half_on_save(m);
        // One roll per amount primitive, for everyone.
        let mut totals: Vec<(i32, i32)> = Vec::new(); // (normal, critical)
        let mut results = Vec::new();
        for (j, (section, p)) in prims.iter().enumerate() {
            let needed = c.rows.iter().any(|r| {
                r.outcome.is_some_and(|o| {
                    applies(*section, o)
                        || (half && *section == Section::OnLand && o == RowOutcome::Saved)
                })
            });
            if !needed {
                totals.push((0, 0));
                continue;
            }
            let expr = amount_expr(p, &c.vars)?;
            let normal = self.roll_amount(amounts.get(j), &expr, false)?;
            let critical_total = if any_crit && matches!(p, Primitive::Damage { .. }) {
                let r = self.roll_amount(crit.get(j), &expr, true)?;
                let t = r.total;
                results.push(r);
                t
            } else {
                normal.total
            };
            totals.push((normal.total, critical_total));
            results.push(normal);
        }

        let mut held = 0usize;
        let amount_index = |p: &Primitive| prims.iter().position(|(_, q)| std::ptr::eq(*q, p));
        for row in &c.rows {
            let Some(o) = row.outcome else { continue };
            if self.s.combatant(&row.target).is_none_or(|t| t.removed) {
                continue;
            }
            // Damage, one damage command per target (one concentration check).
            let mut parts = Vec::new();
            for (section, p) in sections(m) {
                let Primitive::Damage {
                    damage_type,
                    choice,
                    ..
                } = p
                else {
                    continue;
                };
                let (normal, critical_total) = amount_index(p).map_or((0, 0), |j| totals[j]);
                let amount = if applies(section, o) {
                    if o == RowOutcome::Crit {
                        critical_total
                    } else {
                        normal
                    }
                } else if half && section == Section::OnLand && o == RowOutcome::Saved {
                    normal / 2
                } else {
                    continue;
                };
                let ty = damage_type
                    .or(c.choices.damage_type)
                    .or_else(|| choice.first().copied());
                parts.push(DamagePart::typed(amount, ty));
            }
            if !parts.is_empty() {
                self.damage(&row.target, &parts, true, false, o == RowOutcome::Crit)?;
            }
            // Healing and temp HP.
            for (section, p) in sections(m) {
                if !applies(section, o) {
                    continue;
                }
                let (amount, _) = amount_index(p).map_or((0, 0), |j| totals[j]);
                match p {
                    Primitive::Heal { .. } => {
                        if self
                            .s
                            .combatant(&row.target)
                            .is_some_and(|t| !t.life.is_dead())
                        {
                            self.heal(&row.target, &DiceInput::typed(amount))?;
                        }
                    }
                    Primitive::TempHp { .. } => self.grant_temp(&row.target, amount),
                    _ => {}
                }
            }
            if self
                .s
                .combatant(&row.target)
                .is_none_or(|t| t.is_defeated())
            {
                continue;
            }
            if let Some(spec) = self.effect_for(&c, &row.target, o)? {
                let held_here = spec.concentration;
                if self.add_effect(spec, EffectOrigin::Command).is_ok() && held_here {
                    held += 1;
                }
            }
            for (section, p) in sections(m) {
                if let (true, Primitive::EndEffects { max_level }) = (applies(section, o), p) {
                    let max = max_level.int(&c.vars).map_err(|e| invalid(e.to_string()))?;
                    self.end_spell_effects(&row.target, max, cast);
                }
            }
        }
        // A zone lives on the caster, for its [Apply to…] button.
        if c.zone_of.is_none() {
            let zones: Vec<_> = sections(m)
                .filter_map(|(_, p)| match p {
                    Primitive::Zone { zone } => Some(zone.clone()),
                    _ => None,
                })
                .collect();
            for zone in zones {
                if self.s.combatant(&c.caster).is_none_or(|x| x.is_defeated()) {
                    break;
                }
                let mut spec = EffectSpec::new(m.name.clone(), c.caster.clone());
                spec.source = Some(c.caster.clone());
                spec.concentration = m.concentration;
                spec.duration = duration_of(m.duration);
                spec.detail = "Zone: apply to the creatures in it.".into();
                spec.spell = Some(self.tag_of(&c, Some(Box::new(zone))));
                if self.add_effect(spec, EffectOrigin::Command).is_ok() && m.concentration {
                    held += 1;
                }
            }
        }
        // A concentration spell that affected nobody ends (everyone saved), unless it summons.
        let summons = sections(m).any(|(_, p)| matches!(p, Primitive::Summon { .. }));
        let rules_by_hand = matches!(m.tier, Tier::Ruling | Tier::Uncurated | Tier::Utility)
            || c.spec.origin == MechOrigin::Ruling;
        if m.concentration && c.zone_of.is_none() && held == 0 && !summons && !rules_by_hand {
            let on_it = self
                .s
                .combatant(&c.caster)
                .and_then(|x| x.concentration.as_ref())
                .is_some_and(|x| x.name == m.name);
            if on_it {
                self.end_concentration(&c.caster, "no creature affected");
            }
        }
        if let Ok(k) = self.cast_idx(cast) {
            self.s.casts.remove(k);
        }
        let text = format!(
            "{} applied{}",
            m.name,
            if results.is_empty() {
                String::new()
            } else {
                format!(
                    ": {}",
                    results
                        .iter()
                        .map(|r| r.breakdown.clone())
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            }
        );
        self.ev(
            Some(&c.caster),
            K::CastApplied {
                cast: cast.clone(),
                amounts: results,
            },
            text,
        );
        Ok(())
    }

    fn grant_temp(&mut self, id: &CombatantId, amount: i32) {
        let Ok(i) = self.idx(id) else { return };
        let n = self.name(id);
        if self.s.combatants[i].hp_authority == HpAuthority::Player {
            self.ev(
                Some(id),
                K::TempHp,
                format!("{n}: the player adds {amount} temporary HP on the sheet"),
            );
            return;
        }
        let hp = &mut self.s.combatants[i].hp;
        hp.temp = hp.temp.max(amount);
        let t = hp.temp;
        self.ev(Some(id), K::TempHp, format!("{n} has {t} temporary HP"));
    }

    fn tag_of(&self, c: &PendingCast, zone: Option<Box<crate::spell::ZoneSpec>>) -> SpellTag {
        SpellTag {
            key: c.spec.mech.key.clone(),
            level: c.spec.mech.level,
            slot: c.slot,
            cast: Some(c.id.clone()),
            dc: c.spec.dc,
            zone,
            vars: c.vars,
            baked_in: c.spec.baked_in,
        }
    }

    /// The lasting effect a cast puts on one target, if it has any.
    fn effect_for(
        &mut self,
        c: &PendingCast,
        target: &CombatantId,
        o: RowOutcome,
    ) -> Result<Option<EffectSpec>, CombatError> {
        let m = &c.spec.mech;
        let mut spec = EffectSpec::new(m.name.clone(), target.clone());
        spec.source = Some(c.caster.clone());
        spec.concentration = m.concentration;
        spec.duration = duration_of(m.duration);
        spec.spell = Some(self.tag_of(c, None));
        let mut any = false;
        let mut notes = Vec::new();
        let engine_hp = self
            .s
            .combatant(target)
            .is_some_and(|t| t.hp_authority == HpAuthority::Engine);
        for (section, p) in sections(m) {
            if !applies(section, o) {
                continue;
            }
            match p {
                Primitive::Condition {
                    condition,
                    repeat_save,
                    ends_on_damage,
                    ends_on_attack,
                } => {
                    any = true;
                    spec.conditions.push(*condition);
                    spec.ends_on_damage |= *ends_on_damage;
                    spec.ends_on_attack |= *ends_on_attack;
                    if let (Some(rs), Some(dc), None) = (repeat_save, c.spec.dc, spec.repeat_save) {
                        spec.repeat_save = Some(RepeatSave {
                            ability: rs.ability,
                            dc,
                            at: rs.at,
                        });
                    }
                }
                Primitive::AcBase { base, add_dex } => {
                    any = true;
                    if c.spec.baked_in {
                        notes.push("included in its AC".to_string());
                    } else {
                        spec.modifiers.push(Modifier::AcBase {
                            base: *base,
                            add_dex: *add_dex,
                        });
                    }
                }
                Primitive::AcBonus { n } => {
                    any = true;
                    if !c.spec.baked_in {
                        spec.modifiers.push(Modifier::Ac(*n));
                    }
                }
                Primitive::AcMin { n } => {
                    any = true;
                    if !c.spec.baked_in {
                        spec.modifiers.push(Modifier::AcMin(*n));
                    }
                }
                Primitive::RollDice { on, ability, term } => {
                    any = true;
                    spec.modifiers.push(Modifier::RollDice {
                        on: *on,
                        ability: *ability,
                        term: *term,
                    });
                }
                Primitive::RollMode { on, ability, mode } => {
                    any = true;
                    spec.modifiers.push(Modifier::RollMode {
                        on: *on,
                        ability: *ability,
                        mode: *mode,
                    });
                }
                Primitive::Defense { defense, types } => {
                    any = true;
                    for t in types {
                        spec.modifiers.push(Modifier::Defense {
                            defense: *defense,
                            damage_type: *t,
                        });
                    }
                }
                Primitive::MaxHp { n } => {
                    any = true;
                    let n = n.int(&c.vars).map_err(|e| invalid(e.to_string()))?;
                    if engine_hp && n != 0 {
                        if let Some(t) = self.s.combatant_mut(target) {
                            t.hp.max += n;
                            t.hp.current += n;
                        }
                        spec.modifiers.push(Modifier::MaxHp(n));
                    } else if n != 0 {
                        notes.push(format!("the player adds {n} max HP on the sheet"));
                    }
                }
                Primitive::Speed { change } => {
                    any = true;
                    spec.modifiers.push(match *change {
                        SpeedChange::Add { ft } => Modifier::SpeedAdd(ft),
                        SpeedChange::Mult { n } => Modifier::SpeedMultiply(n),
                        SpeedChange::Halve => Modifier::SpeedHalve,
                        SpeedChange::Zero => Modifier::SpeedZero,
                        SpeedChange::SetMode { mode, ft } => Modifier::SpeedSetMode { mode, ft },
                    });
                }
                Primitive::FollowUp { effect } => {
                    let mut f = EffectSpec::new(effect.name.clone(), target.clone());
                    f.source = Some(c.caster.clone());
                    f.duration = duration_of(effect.duration);
                    for q in &effect.apply {
                        match q {
                            Primitive::Condition { condition, .. } => f.conditions.push(*condition),
                            Primitive::Speed {
                                change: SpeedChange::Zero,
                            } => f.modifiers.push(Modifier::SpeedZero),
                            Primitive::Note { text } => f.detail = text.clone(),
                            _ => {}
                        }
                    }
                    // A follow-up on a source-turn duration needs the source to count it.
                    if matches!(f.duration, Duration::SourceTurns { .. }) && f.source.is_none() {
                        f.duration = Duration::Manual;
                    }
                    spec.follow_up = Some(Box::new(f));
                }
                Primitive::Note { text } => notes.push(text.clone()),
                _ => {}
            }
        }
        if !any {
            return Ok(None);
        }
        spec.detail = notes.join("; ");
        Ok(Some(spec))
    }

    /// Ends spell effects on `target` of level `max` or lower (not those of `except`).
    fn end_spell_effects(&mut self, target: &CombatantId, max: i32, except: &CastId) {
        let hits: Vec<EffectId> = self
            .s
            .effects
            .iter()
            .filter(|e| {
                &e.spec.target == target
                    && e.spec.spell.as_ref().is_some_and(|t| {
                        i32::from(t.level) <= max && t.cast.as_ref() != Some(except)
                    })
            })
            .map(|e| e.id)
            .collect();
        for e in hits {
            self.end_effect(e, "dispelled", true);
        }
    }

    fn cancel_cast(&mut self, cast: &CastId, refund: bool) -> Result<(), CombatError> {
        let k = self.cast_idx(cast)?;
        let c = self.s.casts.remove(k);
        let mut refunded = false;
        if refund {
            if let Ok(i) = self.idx(&c.caster) {
                let x = &mut self.s.combatants[i];
                if let Some(p) = c.slot.and_then(|l| x.spell_slots.get_mut(&l)) {
                    p.remaining = (p.remaining + 1).min(p.max);
                    refunded = true;
                }
                if let Some(a) = c.ability.as_ref().and_then(|k| x.ability(k)).cloned() {
                    if let Some(r) = a.uses.as_ref().and_then(|u| x.resources.get_mut(u)) {
                        r.pool.remaining = (r.pool.remaining + 1).min(r.pool.max);
                        refunded = true;
                    }
                }
                if c.slot.is_some() {
                    if let Some(t) = x.spell_turn.as_mut() {
                        t.slot_spent = false;
                    }
                }
            }
        }
        let m = &c.spec.mech;
        let held = self.s.effects.iter().any(|e| {
            e.spec.concentration
                && e.spec.source.as_ref() == Some(&c.caster)
                && e.spec.name == m.name
        });
        let on_it = self
            .s
            .combatant(&c.caster)
            .and_then(|x| x.concentration.as_ref())
            .is_some_and(|x| x.name == m.name);
        if m.concentration && on_it && !held {
            self.end_concentration(&c.caster, "cast cancelled");
        }
        let n = self.name(&c.caster);
        self.ev(
            Some(&c.caster),
            K::CastCancelled {
                cast: cast.clone(),
                refunded,
            },
            format!(
                "{n}'s {} is cancelled{}",
                m.name,
                if refunded { " (slot refunded)" } else { "" }
            ),
        );
        Ok(())
    }

    fn apply_zone(
        &mut self,
        effect: EffectId,
        targets: &[CombatantId],
        rows: &[RowInput],
        amounts: &[AmountRoll],
        id: Option<CastId>,
    ) -> Result<(), CombatError> {
        let e = self
            .s
            .effect(effect)
            .cloned()
            .ok_or(CombatError::UnknownEffect(effect))?;
        let tag = e
            .spec
            .spell
            .clone()
            .ok_or_else(|| invalid("That effect has no zone."))?;
        let zone = tag
            .zone
            .clone()
            .ok_or_else(|| invalid("That effect has no zone."))?;
        let caster = e.spec.source.clone().unwrap_or(e.spec.target.clone());
        self.idx(&caster)?;
        let mut on_land = Vec::new();
        if let Some(dice) = &zone.damage {
            on_land.push(Primitive::Damage {
                dice: dice.clone(),
                damage_type: zone.damage_type,
                choice: zone.choice.clone(),
            });
        }
        for cond in &zone.conditions {
            on_land.push(Primitive::Condition {
                condition: *cond,
                repeat_save: None,
                ends_on_damage: false,
                ends_on_attack: false,
            });
        }
        let spec = CastSpec {
            v: crate::spell::MECH_VERSION,
            origin: MechOrigin::Ruling,
            mech: SpellMech {
                v: crate::spell::MECH_VERSION,
                key: tag.key.clone(),
                edition: self.s.edition,
                name: e.spec.name.clone(),
                rev: 0,
                level: tag.level,
                concentration: false,
                cast: CastTime::Action,
                targeting: Targeting::Area {
                    shape: crate::spell::AreaShape::Sphere,
                    size_ft: 0,
                },
                delivery: zone.save.map_or(Delivery::Auto, |s| Delivery::Save {
                    ability: s.ability,
                    success: s.success,
                }),
                on_land,
                on_resist: vec![],
                always: vec![],
                duration: SpellDuration::Instant,
                tier: Tier::Auto,
                notes: vec![],
            },
            dc: tag.dc,
            attack: None,
            modifier: tag.vars.modifier,
            cantrip_tier: tag.vars.cantrip_tier.max(1),
            baked_in: false,
        };
        self.open_cast(Open {
            caster,
            ability: None,
            spec,
            slot: tag.slot,
            vars: tag.vars,
            targets,
            rows,
            amounts,
            crit: &[],
            choices: CastChoices::default(),
            force: true,
            id,
            zone_of: Some(effect),
            statblock: false,
        })
    }

    // ----- standard actions and binding -----

    fn standard_action(
        &mut self,
        id: &CombatantId,
        action: StandardActionKind,
        target: Option<&CombatantId>,
        bonus: bool,
    ) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        let c = &self.s.combatants[i];
        if c.is_defeated() {
            return Err(CombatError::Unavailable(crate::Unavailable::Defeated));
        }
        if !c.life.is_alive() {
            return Err(CombatError::Unavailable(crate::Unavailable::Down));
        }
        if derive::incapacitated(&self.s, id) {
            return Err(CombatError::Unavailable(crate::Unavailable::Incapacitated));
        }
        if bonus && c.economy.bonus_action {
            return Err(CombatError::Unavailable(
                crate::Unavailable::BonusActionSpent,
            ));
        }
        if !bonus && c.economy.action {
            return Err(CombatError::Unavailable(crate::Unavailable::ActionSpent));
        }
        let help_target = match (action, target) {
            (StandardActionKind::Help, Some(t)) => {
                self.idx(t)?;
                if t == id {
                    return Err(invalid("Help is for someone else."));
                }
                Some(t.clone())
            }
            (StandardActionKind::Help, None) => return Err(invalid("Help needs an ally.")),
            _ => None,
        };
        let c = &mut self.s.combatants[i];
        if bonus {
            c.economy.bonus_action = true;
        } else {
            c.economy.action = true;
        }
        let (name, on, duration) = match action {
            StandardActionKind::Dodge => (
                derive::DODGE,
                id.clone(),
                Duration::SourceTurns {
                    remaining: 1,
                    at: Boundary::Start,
                },
            ),
            StandardActionKind::Dash => (
                "Dash",
                id.clone(),
                Duration::SourceTurns {
                    remaining: 1,
                    at: Boundary::End,
                },
            ),
            StandardActionKind::Disengage => (
                "Disengage",
                id.clone(),
                Duration::SourceTurns {
                    remaining: 1,
                    at: Boundary::End,
                },
            ),
            StandardActionKind::Help => (
                "Helped",
                help_target.clone().expect("checked"),
                Duration::SourceTurns {
                    remaining: 1,
                    at: Boundary::Start,
                },
            ),
        };
        let mut spec = EffectSpec::new(name, on);
        spec.source = Some(id.clone());
        spec.duration = duration;
        match action {
            StandardActionKind::Dodge => {
                spec.detail =
                    "Attacks against it have disadvantage; advantage on DEX saves.".into();
                spec.modifiers = vec![
                    Modifier::RollMode {
                        on: ModeOn::AttacksAgainst,
                        ability: None,
                        mode: AdvDis::Disadvantage,
                    },
                    Modifier::RollMode {
                        on: ModeOn::Saves,
                        ability: Some(crate::model::AbilityName::Dex),
                        mode: AdvDis::Advantage,
                    },
                ];
            }
            StandardActionKind::Dash => {
                spec.detail = "Extra movement equal to its speed this turn.".into();
                spec.modifiers = vec![Modifier::SpeedMultiply(2)];
            }
            StandardActionKind::Disengage => {
                spec.detail = "No opportunity attacks against it this turn.".into();
            }
            StandardActionKind::Help => {
                spec.detail = "Advantage on its next attack or ability check.".into();
                spec.charges = Some(1);
            }
        }
        self.add_effect(spec, EffectOrigin::Command)?;
        let n = self.name(id);
        let what = match action {
            StandardActionKind::Dodge => "takes the Dodge action".to_string(),
            StandardActionKind::Dash => "dashes".to_string(),
            StandardActionKind::Disengage => "disengages".to_string(),
            StandardActionKind::Help => format!(
                "helps {}",
                self.name(help_target.as_ref().expect("checked"))
            ),
        };
        self.ev(
            Some(id),
            K::StandardAction { action },
            format!("{n} {what}"),
        );
        Ok(())
    }

    fn bind_to_concentration(&mut self, id: &CombatantId) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        let owner = self.s.combatants[i]
            .owner_id
            .clone()
            .ok_or_else(|| invalid("Only a controlled creature is bound to its owner."))?;
        let k = self
            .s
            .combatant(&owner)
            .and_then(|o| o.concentration.clone())
            .ok_or_else(|| invalid(format!("{} isn't concentrating.", self.name(&owner))))?;
        if self.s.effects.iter().any(|e| {
            &e.spec.target == id && e.spec.dismiss_on_end && e.spec.source.as_ref() == Some(&owner)
        }) {
            return Err(invalid("Already bound."));
        }
        let mut spec = EffectSpec::new(k.name.clone(), id.clone());
        spec.source = Some(owner.clone());
        spec.concentration = true;
        spec.dismiss_on_end = true;
        spec.detail = format!("Bound to {}: dismissed when it ends.", k.name);
        self.add_effect(spec, EffectOrigin::Command)?;
        Ok(())
    }
}
