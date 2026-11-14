//! The rules: [`apply`] turns a state and a command into a new state and its events. Pure and deterministic: the
//! only randomness is the injected die.

use crate::command::{Actor, Command, DamagePart, DamageTaken, RollInput};
use crate::damage::{concentration_dc, type_damage, DamageResult, PartResult};
use crate::derive::{
    ability_status, anchor_order, effective_initiative, incapacitated, pool_of, pool_reset_owner,
    takes_turn,
};
use crate::error::{invalid, CombatError};
use crate::event::{DmEvent, DmEventKind as K, Events, PlayerEvent, PromptPart, RollOutcome};
use crate::model::{
    AbilityKind, Boundary, CombatState, CombatantId, Concentration, ConcentrationCheck, Condition,
    Controller, Duration, Edition, Effect, EffectId, EffectOrigin, EffectSpec, HpAuthority,
    InitiativeMode, Life, Reset, Status, VisibleMark, ZeroHpRule,
};
use crate::visibility::PlayerSummary;
use party_dice::{
    resolve_d20, resolve_dice, roll_d20, D20Input, D20Result, DiceInput, DiceResult, DieRoller,
    RollMode,
};
use serde::{Deserialize, Serialize};

mod cast;

/// The new state and what happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// The state after the command.
    pub state: CombatState,
    /// DM events and player events.
    pub events: Events,
}

/// Applies `cmd` from `actor` to `state`. On `Err` nothing changed. The die is used only for automatic rolls; a
/// typed result (D43) never touches it.
pub fn apply(
    state: &CombatState,
    actor: &Actor,
    cmd: &Command,
    die: &mut dyn DieRoller,
) -> Result<Outcome, CombatError> {
    authorize(state, actor, cmd)?;
    let before = PlayerSummary::of(state);
    let mut cx = Cx {
        s: state.clone(),
        die,
        dm: Vec::new(),
        prompts: Vec::new(),
    };
    cx.run(cmd)?;
    let mut players = before.diff(&PlayerSummary::of(&cx.s));
    players.extend(cx.prompts);
    Ok(Outcome {
        state: cx.s,
        events: Events { dm: cx.dm, players },
    })
}

fn authorize(state: &CombatState, actor: &Actor, cmd: &Command) -> Result<(), CombatError> {
    if state.status == Status::Ended {
        return Err(CombatError::Ended);
    }
    match actor {
        Actor::Dm | Actor::System => Ok(()),
        Actor::Player { account } if player_may(state, account, cmd) => Ok(()),
        Actor::Player { .. } => Err(CombatError::NotAllowed),
    }
}

/// The account that plays a combatant: a PC's owner, or the owner of the PC that controls a creature.
#[must_use]
pub fn account_of(state: &CombatState, c: &crate::model::Combatant) -> Option<String> {
    if let Some(owner) = c.owner() {
        return Some(owner.to_string());
    }
    match &c.controller {
        Controller::Player { pc } => state
            .combatant(pc)
            .and_then(|p| p.owner())
            .map(str::to_string),
        Controller::Dm => None,
    }
}

/// Whether a player may issue `cmd` (the server still checks party membership). A player acts only for their own
/// PC and the creatures that PC controls, never for another player's or the DM's.
/// - For the PC: initiative, death saves, concentration, effect saves, rolls, attacks, summons. Its HP lives on
///   the player's sheet (D7), so no HP commands.
/// - For its creatures: all of that, plus HP, temp HP, conditions, effects, abilities, resources and dismiss.
/// - An attack's target must be one the player can see.
#[must_use]
pub fn player_may(state: &CombatState, account: &str, cmd: &Command) -> bool {
    let live = |id: &CombatantId| state.combatant(id).filter(|c| !c.removed);
    let mine = |id: &CombatantId| {
        live(id).is_some_and(|c| account_of(state, c).as_deref() == Some(account))
    };
    let my_creature = |id: &CombatantId| mine(id) && live(id).is_some_and(|c| !c.is_character());
    let effect_target = |e: &EffectId| state.effect(*e).map(|e| e.spec.target.clone());
    match cmd {
        Command::SetInitiative { id, .. }
        | Command::DeathSave { id, .. }
        | Command::SetConcentration { id, .. }
        | Command::EndConcentration { id }
        | Command::ResolveConcentration { id, .. }
        | Command::Roll { id, .. }
        | Command::Summon { owner: id, .. } => mine(id),
        Command::ResolveEffectSave { effect, .. } => {
            effect_target(effect).is_some_and(|t| mine(&t))
        }
        Command::Attack {
            attacker, target, ..
        } => {
            mine(attacker)
                && crate::visibility::is_visible(
                    state,
                    target,
                    crate::visibility::ViewerKind::Player,
                )
        }
        Command::Dismiss { id }
        | Command::SetInitiativeMode { id, .. }
        | Command::SetHp { id, .. }
        | Command::GrantTempHp { id, .. }
        | Command::RollHp { id, .. }
        | Command::SetExhaustion { id, .. }
        | Command::UseAbility { id, .. }
        | Command::SetEconomy { id, .. }
        | Command::RollRecharge { id, .. }
        | Command::SetRecharge { id, .. }
        | Command::AdjustSlot { id, .. }
        | Command::AdjustResource { id, .. }
        | Command::Damage { target: id, .. }
        | Command::Heal { target: id, .. }
        | Command::RemoveCondition { target: id, .. } => my_creature(id),
        Command::AddCondition { effect } | Command::AddEffect { effect } => {
            my_creature(&effect.target) && effect.source.as_ref().is_none_or(mine)
        }
        Command::RemoveEffect { effect, .. } | Command::UseCharge { effect } => {
            effect_target(effect).is_some_and(|t| my_creature(&t))
        }
        _ => false,
    }
}

struct Cx<'a> {
    s: CombatState,
    die: &'a mut dyn DieRoller,
    dm: Vec<DmEvent>,
    prompts: Vec<PlayerEvent>,
}

fn clamp_delta(value: u32, delta: i32, max: u32) -> u32 {
    (i64::from(value) + i64::from(delta)).clamp(0, i64::from(max)) as u32
}

impl Cx<'_> {
    // ----- lookups and logging -----

    fn idx(&self, id: &CombatantId) -> Result<usize, CombatError> {
        self.s
            .combatants
            .iter()
            .position(|c| &c.id == id && !c.removed)
            .ok_or_else(|| CombatError::UnknownCombatant(id.clone()))
    }

    fn name(&self, id: &CombatantId) -> String {
        self.s
            .combatant(id)
            .map_or_else(|| id.0.clone(), |c| c.label.clone())
    }

    fn ev(&mut self, subject: Option<&CombatantId>, kind: K, text: impl Into<String>) {
        self.dm.push(DmEvent {
            round: self.s.round,
            subject: subject.cloned(),
            kind,
            text: text.into(),
        });
    }

    fn effect_idx(&self, id: EffectId) -> Result<usize, CombatError> {
        self.s
            .effects
            .iter()
            .position(|e| e.id == id)
            .ok_or(CombatError::UnknownEffect(id))
    }

    fn need_active(&self) -> Result<(), CombatError> {
        if self.s.status == Status::Active {
            Ok(())
        } else {
            Err(CombatError::NotActive)
        }
    }

    // ----- dispatch -----

    #[allow(clippy::too_many_lines)]
    fn run(&mut self, cmd: &Command) -> Result<(), CombatError> {
        match cmd {
            Command::AddCombatant {
                combatant,
                roll_initiative,
            } => self.add_combatant((**combatant).clone(), *roll_initiative),
            Command::RemoveCombatant { id } => self.remove(id),
            Command::Rename { id, label } => {
                let i = self.idx(id)?;
                let old = std::mem::replace(&mut self.s.combatants[i].label, label.clone());
                self.ev(Some(id), K::Edited, format!("{old} renamed to {label}"));
                Ok(())
            }
            Command::SetPublicName { id, name } => {
                let i = self.idx(id)?;
                self.s.combatants[i].public_name = name.clone();
                let n = self.name(id);
                self.ev(
                    Some(id),
                    K::Edited,
                    format!("{n}: players see it as {name}"),
                );
                Ok(())
            }
            Command::Reveal { id } => {
                let i = self.idx(id)?;
                let c = &mut self.s.combatants[i];
                c.visible = true;
                c.ever_revealed = true;
                self.mark_visible_if_current(id);
                let n = self.name(id);
                self.ev(Some(id), K::Revealed, format!("{n} revealed"));
                Ok(())
            }
            Command::Hide { id } => {
                let i = self.idx(id)?;
                if self.s.combatants[i].is_character()
                    || matches!(self.s.combatants[i].controller, Controller::Player { .. })
                {
                    return Err(invalid(
                        "A player character and the creatures it controls are always visible.",
                    ));
                }
                let was_seen = self.s.combatants[i].ever_revealed;
                self.s.combatants[i].visible = false;
                let n = self.name(id);
                let warn = if was_seen {
                    " (players have already seen it)"
                } else {
                    ""
                };
                self.ev(
                    Some(id),
                    K::Hidden { was_seen },
                    format!("{n} hidden{warn}"),
                );
                Ok(())
            }
            Command::SetAbsent { id, absent } => {
                let i = self.idx(id)?;
                self.s.combatants[i].absent = *absent;
                let n = self.name(id);
                let what = if *absent {
                    "not in this fight"
                } else {
                    "back in the fight"
                };
                self.ev(Some(id), K::OrderChanged, format!("{n} {what}"));
                Ok(())
            }
            Command::SetNotes { id, notes } => {
                let i = self.idx(id)?;
                self.s.combatants[i].notes = notes.clone();
                Ok(())
            }
            Command::SetSettings { settings } => {
                self.s.settings = *settings;
                self.ev(None, K::Edited, "Settings changed");
                Ok(())
            }
            Command::RollInitiative { ids, only_missing } => {
                self.roll_initiative(ids, *only_missing)
            }
            Command::SetInitiative { id, roll } => {
                let i = self.idx(id)?;
                let r = resolve_d20(roll, self.die)?;
                self.set_initiative(i, r);
                Ok(())
            }
            Command::ClearInitiative { id } => {
                let i = self.idx(id)?;
                self.s.combatants[i].initiative = None;
                let n = self.name(id);
                self.ev(
                    Some(id),
                    K::OrderChanged,
                    format!("{n}: initiative cleared"),
                );
                Ok(())
            }
            Command::SwapTie { a, b } => self.swap_tie(a, b),
            Command::Link { id, to } => self.link(id, to.as_ref()),
            Command::Start => self.start(),
            Command::NextTurn => {
                self.need_active()?;
                self.next_turn()
            }
            Command::PreviousTurn => {
                self.need_active()?;
                self.previous_turn()
            }
            Command::End => {
                self.s.status = Status::Ended;
                self.ev(None, K::Ended, "Combat ended");
                Ok(())
            }
            Command::SetEconomy {
                id,
                action,
                bonus_action,
                reaction,
            } => {
                let i = self.idx(id)?;
                let e = &mut self.s.combatants[i].economy;
                if let Some(v) = action {
                    e.action = *v;
                }
                if let Some(v) = bonus_action {
                    e.bonus_action = *v;
                }
                if let Some(v) = reaction {
                    e.reaction = *v;
                }
                let n = self.name(id);
                self.ev(Some(id), K::Economy, format!("{n}: action economy changed"));
                Ok(())
            }
            Command::Damage {
                target,
                parts,
                magical,
                apply_qualified,
                critical_hit,
            } => self.damage(target, parts, *magical, *apply_qualified, *critical_hit),
            Command::Heal { target, amount } => self.heal(target, amount),
            Command::SetHp {
                id,
                current,
                max,
                temp,
            } => self.set_hp(id, *current, *max, *temp),
            Command::GrantTempHp { id, amount } => {
                let i = self.idx(id)?;
                if self.s.combatants[i].hp_authority == HpAuthority::Player {
                    return Err(invalid(
                        "A player character's temporary HP lives on the player's sheet.",
                    ));
                }
                if *amount < 0 {
                    return Err(invalid("Temporary HP can't be negative."));
                }
                let hp = &mut self.s.combatants[i].hp;
                hp.temp = hp.temp.max(*amount);
                let (n, t) = (self.name(id), self.s.combatants[i].hp.temp);
                self.ev(Some(id), K::TempHp, format!("{n} has {t} temporary HP"));
                Ok(())
            }
            Command::SyncPlayerHp {
                id,
                current,
                max,
                temp,
                damage,
            } => self.sync_player_hp(id, *current, *max, *temp, *damage),
            Command::DeathSave { id, roll } => self.death_save(id, roll),
            Command::Stabilize { id } => {
                let i = self.idx(id)?;
                if !matches!(self.s.combatants[i].life, Life::Dying { .. }) {
                    return Err(CombatError::NotDying);
                }
                self.s.combatants[i].life = Life::Stable;
                let n = self.name(id);
                self.ev(Some(id), K::Stabilized, format!("{n} is stable"));
                Ok(())
            }
            Command::SetDeathSaves {
                id,
                successes,
                failures,
            } => {
                let i = self.idx(id)?;
                if !matches!(self.s.combatants[i].life, Life::Dying { .. }) {
                    return Err(CombatError::NotDying);
                }
                if *successes > 2 || *failures > 2 {
                    return Err(invalid("Death saves are 0 to 2 (three ends them)."));
                }
                self.s.combatants[i].life = Life::Dying {
                    successes: *successes,
                    failures: *failures,
                };
                let n = self.name(id);
                self.ev(
                    Some(id),
                    K::HpSet,
                    format!("{n}: death saves set to {successes} successes, {failures} failures"),
                );
                Ok(())
            }
            Command::AddCondition { effect } => self.add_condition((**effect).clone()),
            Command::RemoveCondition { target, condition } => {
                self.remove_condition(target, *condition)
            }
            Command::SetExhaustion { id, level } => self.set_exhaustion(id, *level),
            Command::SetConcentration { id, name } => {
                let i = self.idx(id)?;
                if !self.s.combatants[i].life.is_alive() {
                    return Err(invalid("A creature at 0 HP can't concentrate."));
                }
                self.start_concentration(id, name);
                Ok(())
            }
            Command::EndConcentration { id } => {
                let i = self.idx(id)?;
                if self.s.combatants[i].concentration.is_none() {
                    return Err(invalid("Not concentrating."));
                }
                self.end_concentration(id, "ended");
                Ok(())
            }
            Command::ResolveConcentration { id, roll } => self.resolve_concentration(id, roll),
            Command::AddEffect { effect } => self
                .add_effect((**effect).clone(), EffectOrigin::Command)
                .map(|_| ()),
            Command::RemoveEffect {
                effect,
                skip_follow_up,
            } => {
                self.effect_idx(*effect)?;
                self.end_effect(*effect, "removed", !skip_follow_up);
                Ok(())
            }
            Command::UseCharge { effect } => self.use_charge(*effect),
            Command::ResolveEffectSave { effect, roll } => self.resolve_effect_save(*effect, roll),
            Command::UseAbility { id, ability, slot } => self.use_ability(id, ability, *slot),
            Command::RollRecharge { id, ability, roll } => self.roll_recharge(id, ability, roll),
            Command::SetRecharge { id, ability, ready } => {
                let i = self.idx(id)?;
                let a = self.s.combatants[i]
                    .abilities
                    .iter_mut()
                    .find(|a| &a.key == ability)
                    .ok_or_else(|| CombatError::UnknownAbility(ability.clone()))?;
                let r = a
                    .recharge
                    .as_mut()
                    .ok_or_else(|| invalid("That ability has no recharge."))?;
                r.ready = *ready;
                let n = self.name(id);
                let state = if *ready { "ready" } else { "spent" };
                self.ev(Some(id), K::RechargeSet, format!("{n}: {ability} {state}"));
                Ok(())
            }
            Command::SetLegendaryPool { pool, value } => {
                for m in &value.members {
                    self.idx(m)?;
                }
                let mut v = value.clone();
                v.remaining = v.remaining.min(v.max);
                for (k, p) in &mut self.s.legendary_pools {
                    if k != pool {
                        p.members.retain(|m| !v.members.contains(m));
                    }
                }
                self.s.legendary_pools.retain(|_, p| !p.members.is_empty());
                self.s.legendary_pools.insert(pool.clone(), v);
                self.ev(None, K::Resources, format!("Legendary pool {pool} set"));
                Ok(())
            }
            Command::AdjustLegendary { pool, delta } => {
                let p = self
                    .s
                    .legendary_pools
                    .get_mut(pool)
                    .ok_or_else(|| CombatError::UnknownPool(pool.clone()))?;
                p.remaining = clamp_delta(p.remaining, *delta, p.max);
                let (r, m) = (p.remaining, p.max);
                self.ev(
                    None,
                    K::Resources,
                    format!("Legendary actions ({pool}): {r}/{m}"),
                );
                Ok(())
            }
            Command::AdjustLegendaryResistance { id, delta } => {
                let i = self.idx(id)?;
                let p = self.s.combatants[i]
                    .legendary_resistance
                    .as_mut()
                    .ok_or_else(|| invalid("No Legendary Resistance."))?;
                p.remaining = clamp_delta(p.remaining, *delta, p.max);
                let (r, m, n) = (p.remaining, p.max, self.name(id));
                self.ev(
                    Some(id),
                    K::Resources,
                    format!("{n}: Legendary Resistance {r}/{m}"),
                );
                Ok(())
            }
            Command::AdjustSlot { id, level, delta } => {
                let i = self.idx(id)?;
                let p = self.s.combatants[i]
                    .spell_slots
                    .get_mut(level)
                    .ok_or_else(|| invalid(format!("No level {level} slots.")))?;
                p.remaining = clamp_delta(p.remaining, *delta, p.max);
                let (r, m, n) = (p.remaining, p.max, self.name(id));
                self.ev(
                    Some(id),
                    K::Resources,
                    format!("{n}: level {level} slots {r}/{m}"),
                );
                Ok(())
            }
            Command::SetSlots { id, level, pool } => {
                let i = self.idx(id)?;
                if !(1..=9).contains(level) {
                    return Err(invalid("Spell slot levels are 1 to 9."));
                }
                let mut p = *pool;
                p.remaining = p.remaining.min(p.max);
                if p.max == 0 {
                    self.s.combatants[i].spell_slots.remove(level);
                } else {
                    self.s.combatants[i].spell_slots.insert(*level, p);
                }
                let n = self.name(id);
                self.ev(
                    Some(id),
                    K::Resources,
                    format!("{n}: level {level} slots set"),
                );
                Ok(())
            }
            Command::AdjustResource { id, key, delta } => {
                let i = self.idx(id)?;
                let r = self.s.combatants[i]
                    .resources
                    .get_mut(key)
                    .ok_or_else(|| CombatError::UnknownResource(key.clone()))?;
                r.pool.remaining = clamp_delta(r.pool.remaining, *delta, r.pool.max);
                let (label, rem, max, n) =
                    (r.label.clone(), r.pool.remaining, r.pool.max, self.name(id));
                self.ev(Some(id), K::Resources, format!("{n}: {label} {rem}/{max}"));
                Ok(())
            }
            Command::SetRule { rule } => {
                if let Some(l) = &rule.linked {
                    self.idx(l)?;
                }
                match self.s.rules.iter_mut().find(|r| r.id == rule.id) {
                    Some(r) => *r = rule.clone(),
                    None => self.s.rules.push(rule.clone()),
                }
                self.ev(
                    None,
                    K::Rule {
                        rule: rule.id.clone(),
                        active: rule.active,
                    },
                    format!("Rule {} set", rule.name),
                );
                Ok(())
            }
            Command::SetRuleActive { id, active } => {
                let r = self
                    .s
                    .rules
                    .iter_mut()
                    .find(|r| &r.id == id)
                    .ok_or_else(|| CombatError::UnknownRule(id.clone()))?;
                r.active = *active;
                let text = format!("{} {}", r.name, if *active { "active" } else { "ended" });
                self.ev(
                    None,
                    K::Rule {
                        rule: id.clone(),
                        active: *active,
                    },
                    text,
                );
                Ok(())
            }
            Command::RemoveRule { id } => {
                let before = self.s.rules.len();
                self.s.rules.retain(|r| &r.id != id);
                if self.s.rules.len() == before {
                    return Err(CombatError::UnknownRule(id.clone()));
                }
                self.ev(
                    None,
                    K::Rule {
                        rule: id.clone(),
                        active: false,
                    },
                    format!("Rule {id} removed"),
                );
                Ok(())
            }
            Command::SetGroupLimit { group, per_round } => {
                if *per_round == 0 {
                    self.s.group_limits.remove(group);
                } else {
                    let used = self.s.group_limits.get(group).map_or(0, |g| g.used);
                    self.s.group_limits.insert(
                        group.clone(),
                        crate::model::GroupLimit {
                            per_round: *per_round,
                            used,
                        },
                    );
                }
                self.ev(
                    None,
                    K::Resources,
                    format!("Group {group}: {per_round} per round"),
                );
                Ok(())
            }
            Command::Roll { id, label, roll } => self.roll(id, label, roll),
            Command::Attack {
                attacker,
                target,
                label,
                roll,
            } => self.attack(attacker, target, label, roll),
            Command::RollHp { id, roll } => self.roll_hp(id, roll),
            Command::Summon {
                owner,
                combatant,
                initiative,
                visible,
            } => self.summon(owner, (**combatant).clone(), initiative.as_ref(), *visible),
            Command::Dismiss { id } => {
                let i = self.idx(id)?;
                if self.s.combatants[i].owner_id.is_none() {
                    return Err(invalid(
                        "Only a summon, pet or companion is dismissed; remove other combatants.",
                    ));
                }
                let n = self.name(id);
                self.ev(Some(id), K::Dismissed, format!("{n} is dismissed"));
                self.remove(id)
            }
            Command::SetInitiativeMode { id, mode, roll } => {
                let i = self.idx(id)?;
                if self.s.combatants[i].owner_id.is_none() {
                    return Err(invalid("Only a controlled creature follows an owner."));
                }
                if let Some(r) = roll {
                    let r = resolve_d20(r, self.die)?;
                    self.set_initiative(i, r);
                }
                if *mode == InitiativeMode::Own && self.s.combatants[i].initiative.is_none() {
                    return Err(invalid(
                        "Its own initiative needs a roll (in the app or typed).",
                    ));
                }
                self.s.combatants[i].initiative_mode = *mode;
                let n = self.name(id);
                let text = match mode {
                    InitiativeMode::Own => format!("{n} acts on its own initiative"),
                    InitiativeMode::AfterOwner => format!("{n} acts right after its owner"),
                };
                self.ev(Some(id), K::OrderChanged, text);
                Ok(())
            }
            Command::CastSpell { .. }
            | Command::ResolveCastRow { .. }
            | Command::ApplyCast { .. }
            | Command::CancelCast { .. }
            | Command::ApplyZone { .. }
            | Command::PerformStatblock { .. }
            | Command::StandardAction { .. }
            | Command::BindToConcentration { .. } => self.spell_command(cmd),
        }
    }

    // ----- rolls (D21) -----

    /// Sends a roll's result to the player who plays `id`, if any.
    #[allow(clippy::too_many_arguments)]
    fn report(
        &mut self,
        id: &CombatantId,
        label: &str,
        total: i32,
        natural: Option<u32>,
        source: party_dice::RollSource,
        breakdown: &str,
        hit: Option<bool>,
        critical: bool,
    ) {
        let Some(owner) = self.s.combatant(id).and_then(|c| account_of(&self.s, c)) else {
            return;
        };
        self.prompts.push(PlayerEvent::RollResult {
            id: id.clone(),
            owner,
            label: label.to_string(),
            total,
            natural,
            source,
            breakdown: breakdown.to_string(),
            hit,
            critical,
        });
    }

    fn report_d20(&mut self, id: &CombatantId, label: &str, r: &D20Result) {
        self.report(
            id,
            label,
            r.total,
            r.natural,
            r.source,
            &r.breakdown,
            None,
            false,
        );
    }

    fn report_dice(&mut self, id: &CombatantId, label: &str, r: &DiceResult) {
        self.report(
            id,
            label,
            r.total,
            None,
            r.source,
            &r.breakdown,
            None,
            false,
        );
    }

    fn roll(&mut self, id: &CombatantId, label: &str, roll: &RollInput) -> Result<(), CombatError> {
        self.idx(id)?;
        let n = self.name(id);
        let result = match roll {
            RollInput::D20(input) => {
                let r = resolve_d20(input, self.die)?;
                self.report_d20(id, label, &r);
                RollOutcome::D20(r)
            }
            RollInput::Dice(input) => {
                let r = resolve_dice(input, self.die)?;
                self.report_dice(id, label, &r);
                RollOutcome::Dice(r)
            }
        };
        let breakdown = match &result {
            RollOutcome::D20(r) => r.breakdown.clone(),
            RollOutcome::Dice(r) => r.breakdown.clone(),
        };
        self.ev(
            Some(id),
            K::Rolled {
                label: label.to_string(),
                result,
            },
            format!("{n}: {label} {breakdown}"),
        );
        Ok(())
    }

    fn attack(
        &mut self,
        attacker: &CombatantId,
        target: &CombatantId,
        label: &str,
        roll: &D20Input,
    ) -> Result<(), CombatError> {
        self.idx(attacker)?;
        let t = self.idx(target)?;
        let r = resolve_d20(roll, self.die)?;
        let ac = crate::derive::effective_ac(&self.s, &self.s.combatants[t]).total;
        let hit = if r.is_nat20 {
            Some(true)
        } else if r.is_nat1 {
            Some(false)
        } else {
            ac.map(|ac| r.total >= ac)
        };
        let critical = r.is_crit && hit != Some(false);
        let (na, nt) = (self.name(attacker), self.name(target));
        let what = if label.is_empty() { "attacks" } else { label };
        let verdict = match hit {
            Some(true) if critical => "critical hit".to_string(),
            Some(true) => "hit".to_string(),
            Some(false) => "miss".to_string(),
            None => "AC unknown: the DM decides".to_string(),
        };
        let ac_text = ac.map_or_else(|| "AC ?".to_string(), |a| format!("AC {a}"));
        let text = format!("{na} {what} {nt}: {} vs {ac_text}, {verdict}", r.breakdown);
        let label = if label.is_empty() { "Attack" } else { label };
        self.report(
            attacker,
            label,
            r.total,
            r.natural,
            r.source,
            &r.breakdown,
            hit,
            critical,
        );
        self.ev(
            Some(attacker),
            K::Attack {
                target: target.clone(),
                roll: r,
                target_ac: ac,
                hit,
                critical,
            },
            text,
        );
        Ok(())
    }

    fn roll_hp(&mut self, id: &CombatantId, roll: &DiceInput) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        if self.s.combatants[i].hp_authority == HpAuthority::Player {
            return Err(invalid(
                "A player character's HP lives on the player's sheet (D7).",
            ));
        }
        let r = resolve_dice(roll, self.die)?;
        let hp = r.total.max(1);
        let c = &mut self.s.combatants[i];
        c.hp.max = hp;
        c.hp.current = hp;
        let n = self.name(id);
        self.report_dice(id, "Hit points", &r);
        let text = format!("{n} HP rolled: {} ({hp})", r.breakdown);
        self.ev(Some(id), K::HpRolled { roll: r }, text);
        self.revive(i);
        Ok(())
    }

    // ----- controlled creatures -----

    fn summon(
        &mut self,
        owner: &CombatantId,
        mut c: crate::model::Combatant,
        initiative: Option<&D20Input>,
        visible: Option<bool>,
    ) -> Result<(), CombatError> {
        let o = self.idx(owner)?;
        if c.is_character() {
            return Err(invalid("A player character can't be summoned."));
        }
        let owner_c = &self.s.combatants[o];
        c.controller = if owner_c.is_character() {
            Controller::Player {
                pc: owner_c.id.clone(),
            }
        } else {
            owner_c.controller.clone()
        };
        c.visible = match c.controller {
            Controller::Player { .. } => true,
            Controller::Dm => visible.unwrap_or(owner_c.visible),
        };
        c.owner_id = Some(owner.clone());
        c.hp_authority = HpAuthority::Engine;
        c.linked_to = None;
        c.initiative = None;
        c.initiative_mode = if initiative.is_some() {
            InitiativeMode::Own
        } else {
            InitiativeMode::AfterOwner
        };
        let id = c.id.clone();
        let label = c.label.clone();
        self.add_combatant(c, false)?;
        if let Some(r) = initiative {
            let r = resolve_d20(r, self.die)?;
            let i = self.idx(&id)?;
            self.set_initiative(i, r);
        }
        let on = self.name(owner);
        self.ev(
            Some(&id),
            K::Summoned {
                owner: owner.clone(),
            },
            format!("{on} summons {label}"),
        );
        Ok(())
    }

    /// Following `linked_to` and `owner_id` from `from` reaches `id`.
    fn reaches(&self, from: &CombatantId, id: &CombatantId) -> bool {
        let mut stack = vec![from.clone()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(cur) = stack.pop() {
            if &cur == id {
                return true;
            }
            if !seen.insert(cur.clone()) {
                continue;
            }
            if let Some(c) = self.s.combatant(&cur) {
                stack.extend(c.linked_to.iter().cloned());
                stack.extend(c.owner_id.iter().cloned());
            }
        }
        false
    }

    // ----- roster -----

    fn add_combatant(
        &mut self,
        mut c: crate::model::Combatant,
        roll: bool,
    ) -> Result<(), CombatError> {
        if c.id.0.trim().is_empty() {
            return Err(invalid("A combatant needs an id."));
        }
        if self.s.combatant(&c.id).is_some() {
            return Err(CombatError::DuplicateCombatant(c.id));
        }
        if c.hp.max < 0 || c.hp.temp < 0 {
            return Err(invalid("HP can't be negative."));
        }
        if c.exhaustion > 6 {
            return Err(invalid("Exhaustion is 0 to 6."));
        }
        if let Some(l) = &c.linked_to {
            self.idx(l)?;
        }
        if let Some(o) = &c.owner_id {
            self.idx(o)?;
        }
        if let Controller::Player { pc } = &c.controller {
            let p = self.idx(pc)?;
            if !self.s.combatants[p].is_character() {
                return Err(invalid("A player controller must be a player character."));
            }
        }
        c.hp.current = c.hp.current.clamp(0, c.hp.max);
        if c.is_character() || matches!(c.controller, Controller::Player { .. }) {
            c.visible = true;
        }
        c.ever_revealed |= c.visible;
        c.removed = false;
        if c.hp.current == 0 && c.hp.max > 0 && c.life.is_alive() {
            c.life = match c.zero_hp {
                ZeroHpRule::Dies => Life::Dead,
                ZeroHpRule::DeathSaves => Life::Stable,
            };
        }
        c.tiebreak = self.s.next_tiebreak;
        self.s.next_tiebreak += 1;
        let id = c.id.clone();
        let text = format!("{} joins the fight", c.label);
        self.s.combatants.push(c);
        self.ev(Some(&id), K::Added, text);
        if roll {
            let i = self.idx(&id)?;
            self.roll_one(i)?;
        }
        Ok(())
    }

    fn remove(&mut self, id: &CombatantId) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        self.went_down(id, "removed");
        let targeted: Vec<EffectId> = self
            .s
            .effects
            .iter()
            .filter(|e| &e.spec.target == id)
            .map(|e| e.id)
            .collect();
        for e in targeted {
            self.end_effect(e, "target removed", false);
        }
        self.s.combatants[i].removed = true;
        let n = self.name(id);
        self.ev(Some(id), K::Removed, format!("{n} removed"));
        Ok(())
    }

    // ----- initiative -----

    fn roll_one(&mut self, i: usize) -> Result<(), CombatError> {
        let c = &self.s.combatants[i];
        let surprised_2024 = c.surprised && self.s.edition == Edition::Srd521;
        let mode = RollMode::combine(c.initiative_advantage, surprised_2024);
        let r = roll_d20(c.initiative_bonus, mode, self.die, None)?;
        self.set_initiative(i, r);
        Ok(())
    }

    fn set_initiative(&mut self, i: usize, r: D20Result) {
        let c = &mut self.s.combatants[i];
        c.initiative = Some(r.total);
        c.absent = false;
        let id = c.id.clone();
        let text = format!("{} initiative {} ({})", c.label, r.total, r.breakdown);
        self.report_d20(&id, "Initiative", &r);
        self.ev(Some(&id), K::Initiative { roll: r }, text);
    }

    fn roll_initiative(
        &mut self,
        ids: &[CombatantId],
        only_missing: bool,
    ) -> Result<(), CombatError> {
        let targets: Vec<usize> = if ids.is_empty() {
            (0..self.s.combatants.len())
                .filter(|&i| {
                    let c = &self.s.combatants[i];
                    !c.removed && !c.absent && !c.is_character()
                })
                .collect()
        } else {
            ids.iter()
                .map(|id| self.idx(id))
                .collect::<Result<_, _>>()?
        };
        for i in targets {
            if only_missing && self.s.combatants[i].initiative.is_some() {
                continue;
            }
            self.roll_one(i)?;
        }
        Ok(())
    }

    fn swap_tie(&mut self, a: &CombatantId, b: &CombatantId) -> Result<(), CombatError> {
        let (i, j) = (self.idx(a)?, self.idx(b)?);
        let (ca, cb) = (&self.s.combatants[i], &self.s.combatants[j]);
        let (la, lb) = (
            crate::derive::leader(&self.s, ca).map(|l| &l.id),
            crate::derive::leader(&self.s, cb).map(|l| &l.id),
        );
        // Two roots tied on initiative and modifier, or two followers of the same leader (their sub-order).
        let tied = match (la, lb) {
            (None, None) => {
                effective_initiative(&self.s, ca).is_some()
                    && effective_initiative(&self.s, ca) == effective_initiative(&self.s, cb)
                    && ca.initiative_bonus == cb.initiative_bonus
            }
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
        if !tied || i == j {
            return Err(invalid(
                "Only two combatants tied on initiative, or two that follow the same leader, can swap.",
            ));
        }
        let t = self.s.combatants[i].tiebreak;
        self.s.combatants[i].tiebreak = self.s.combatants[j].tiebreak;
        self.s.combatants[j].tiebreak = t;
        let (na, nb) = (self.name(a), self.name(b));
        self.ev(
            Some(a),
            K::OrderChanged,
            format!("{na} and {nb} swap places"),
        );
        Ok(())
    }

    fn link(&mut self, id: &CombatantId, to: Option<&CombatantId>) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        if let Some(to) = to {
            self.idx(to)?;
            // No cycles through links or owners: walking from `to` must never reach `id`.
            if self.reaches(to, id) {
                return Err(invalid("That link would make a loop."));
            }
        }
        self.s.combatants[i].linked_to = to.cloned();
        let n = self.name(id);
        let text = match to {
            Some(t) => format!("{n} acts right after {}", self.name(t)),
            None => format!("{n} acts on its own initiative"),
        };
        self.ev(Some(id), K::OrderChanged, text);
        Ok(())
    }

    // ----- turn flow (C4) -----

    fn start(&mut self) -> Result<(), CombatError> {
        if self.s.status != Status::Setup {
            return Err(CombatError::AlreadyStarted);
        }
        let order = anchor_order(&self.s);
        let first = order
            .iter()
            .find(|id| self.s.combatant(id).is_some_and(takes_turn))
            .cloned()
            .ok_or(CombatError::NoInitiative)?;
        self.s.status = Status::Active;
        self.s.round = 1;
        self.ev(None, K::Started, "Combat starts");
        self.round_start();
        self.turn_start(&first)?;
        Ok(())
    }

    fn next_turn(&mut self) -> Result<(), CombatError> {
        let order = anchor_order(&self.s);
        let n = order.len();
        let from = self.s.current.clone();
        let start = from
            .as_ref()
            .and_then(|f| order.iter().position(|id| id == f));
        let mut next = None;
        for step in 1..=n {
            let at = start.map_or(step - 1, |i| i + step);
            let id = &order[at % n];
            if self.s.combatant(id).is_some_and(takes_turn) {
                next = Some((id.clone(), start.is_some() && at >= n));
                break;
            }
        }
        let (next, wrapped) = next.ok_or(CombatError::NoInitiative)?;
        if let Some(from) = &from {
            self.turn_end(from)?;
        }
        if wrapped {
            self.s.round += 1;
            self.round_start();
        }
        self.turn_start(&next)
    }

    fn previous_turn(&mut self) -> Result<(), CombatError> {
        let order = anchor_order(&self.s);
        let n = order.len();
        let start = self
            .s
            .current
            .as_ref()
            .and_then(|f| order.iter().position(|id| id == f));
        let Some(start) = start else {
            return Err(CombatError::NoInitiative);
        };
        for step in 1..=n {
            let wrapped = step > start;
            let id = &order[(start + n - step) % n];
            if self.s.combatant(id).is_some_and(takes_turn) {
                if wrapped {
                    if self.s.round <= 1 {
                        return Err(invalid("This is the first turn."));
                    }
                    self.s.round -= 1;
                }
                let id = id.clone();
                self.s.current = Some(id.clone());
                self.mark_visible_if_current(&id);
                let name = self.name(&id);
                self.ev(Some(&id), K::TurnStarted, format!("Back to {name}'s turn"));
                return Ok(());
            }
        }
        Err(CombatError::NoInitiative)
    }

    fn mark_visible_if_current(&mut self, id: &CombatantId) {
        if self.s.current.as_ref() == Some(id)
            && crate::visibility::is_visible(&self.s, id, crate::visibility::ViewerKind::Player)
        {
            self.s.last_visible = Some(VisibleMark {
                id: id.clone(),
                round: self.s.round,
            });
        }
    }

    fn round_start(&mut self) {
        for g in self.s.group_limits.values_mut() {
            g.used = 0;
        }
        for c in &mut self.s.combatants {
            for r in c
                .resources
                .values_mut()
                .filter(|r| r.reset == Reset::RoundStart)
            {
                r.pool.remaining = r.pool.max;
            }
        }
        let round = self.s.round;
        self.ev(None, K::RoundStarted, format!("Round {round}"));
    }

    fn turn_start(&mut self, id: &CombatantId) -> Result<(), CombatError> {
        self.s.current = Some(id.clone());
        self.mark_visible_if_current(id);
        let i = self.idx(id)?;
        let name = self.name(id);
        self.ev(Some(id), K::TurnStarted, format!("{name}'s turn"));

        let surprised =
            self.s.edition == Edition::Srd51 && self.s.round == 1 && self.s.combatants[i].surprised;
        let c = &mut self.s.combatants[i];
        c.economy = crate::model::Economy {
            action: surprised,
            bonus_action: surprised,
            reaction: surprised,
        };
        for r in c
            .resources
            .values_mut()
            .filter(|r| r.reset == Reset::TurnStart)
        {
            r.pool.remaining = r.pool.max;
        }
        if surprised {
            self.ev(
                Some(id),
                K::Surprised,
                format!("{name} is surprised: no actions this turn"),
            );
        }

        let pools: Vec<String> = self
            .s
            .legendary_pools
            .iter()
            .filter(|(_, p)| pool_reset_owner(&self.s, p) == Some(id))
            .map(|(k, _)| k.clone())
            .collect();
        for k in pools {
            let p = self.s.legendary_pools.get_mut(&k).expect("listed");
            p.remaining = p.max;
            let max = p.max;
            self.ev(
                Some(id),
                K::LegendaryReset { pool: k.clone() },
                format!("Legendary actions ({k}) reset to {max}"),
            );
        }

        let creature = !self.s.combatants[i].is_character();
        let due: Vec<(String, u8)> = self.s.combatants[i]
            .abilities
            .iter()
            .filter_map(|a| {
                a.recharge
                    .filter(|r| !r.ready)
                    .map(|r| (a.key.clone(), r.min))
            })
            .collect();
        for (key, min) in due {
            if self.s.settings.auto_recharge && creature {
                let roll = resolve_dice(&DiceInput::app("1d6"), self.die)?;
                self.finish_recharge(i, &key, min, roll);
            } else {
                self.ev(
                    Some(id),
                    K::RechargeDue {
                        ability: key.clone(),
                    },
                    format!("{name}: roll recharge for {key}"),
                );
            }
        }

        self.tick_effects(id, Boundary::Start);

        let c = &self.s.combatants[i];
        if let Some(owner) = account_of(&self.s, c) {
            self.prompts.push(PlayerEvent::YourTurn {
                id: id.clone(),
                owner: owner.clone(),
            });
            if matches!(c.life, Life::Dying { .. }) {
                self.prompts.push(PlayerEvent::DeathSaveDue {
                    id: id.clone(),
                    owner,
                });
            }
        }
        Ok(())
    }

    fn turn_end(&mut self, id: &CombatantId) -> Result<(), CombatError> {
        if self.s.combatant(id).is_none() {
            return Ok(());
        }
        self.tick_effects(id, Boundary::End);
        if let Some(c) = self.s.combatant_mut(id) {
            c.surprised = false;
        }
        Ok(())
    }

    /// Counts down durations at a boundary of `id`'s turn and raises repeated saves.
    fn tick_effects(&mut self, id: &CombatantId, at: Boundary) {
        let mut expired = Vec::new();
        let mut saves = Vec::new();
        for e in &mut self.s.effects {
            let counts = match &mut e.spec.duration {
                Duration::SourceTurns { remaining, at: b }
                    if *b == at && e.spec.source.as_ref() == Some(id) =>
                {
                    Some(remaining)
                }
                Duration::TargetTurns { remaining, at: b } if *b == at && &e.spec.target == id => {
                    Some(remaining)
                }
                _ => None,
            };
            if let Some(remaining) = counts {
                *remaining = remaining.saturating_sub(1);
                if *remaining == 0 {
                    expired.push(e.id);
                }
            }
            if e.spec.repeat_save.is_some_and(|s| s.at == at) && &e.spec.target == id {
                saves.push(e.id);
            }
        }
        for e in expired {
            self.end_effect(e, "duration expired", true);
        }
        for e in saves {
            if let Some(eff) = self.s.effect(e) {
                let s = eff.spec.repeat_save.expect("filtered");
                let text = format!(
                    "{}: {} save DC {} against {}",
                    self.name(id),
                    s.ability,
                    s.dc,
                    eff.spec.name
                );
                self.ev(Some(id), K::SaveDue { effect: e }, text);
            }
        }
    }

    // ----- HP -----

    fn damage(
        &mut self,
        target: &CombatantId,
        parts: &[DamagePart],
        magical: bool,
        apply_qualified: bool,
        critical_hit: bool,
    ) -> Result<(), CombatError> {
        let i = self.idx(target)?;
        if parts.is_empty() {
            return Err(invalid("No damage to apply."));
        }
        let mut results = Vec::new();
        for part in parts {
            let roll = resolve_dice(&part.amount, self.die)?;
            if roll.total < 0 {
                return Err(invalid("Damage can't be negative."));
            }
            let t = type_damage(
                &self.s.combatants[i].defenses,
                part.damage_type,
                roll.total,
                magical,
                apply_qualified,
            );
            results.push(PartResult {
                damage_type: part.damage_type,
                roll,
                amount: t.amount,
                immune: t.immune,
                resisted: t.resisted,
                vulnerable: t.vulnerable,
                clauses: t.clauses,
            });
        }
        let total: i32 = results.iter().map(|p| p.amount).sum();
        let result = DamageResult {
            parts: results,
            total,
        };
        let name = self.name(target);
        let detail: Vec<String> = result
            .parts
            .iter()
            .map(|p| {
                let ty = p.damage_type.map_or_else(String::new, |t| format!(" {t}"));
                let how = if p.immune {
                    " (immune)"
                } else if p.resisted && p.vulnerable {
                    " (resisted, vulnerable)"
                } else if p.resisted {
                    " (resisted)"
                } else if p.vulnerable {
                    " (vulnerable)"
                } else {
                    ""
                };
                format!("{}{ty}{how}", p.amount)
            })
            .collect();
        let c = &self.s.combatants[i];
        if c.hp_authority == HpAuthority::Player {
            let owner = c.owner().unwrap_or_default().to_string();
            let prompt_parts = result
                .parts
                .iter()
                .map(|p| PromptPart {
                    amount: p.amount,
                    damage_type: p.damage_type,
                })
                .collect();
            self.prompts.push(PlayerEvent::DamagePrompt {
                id: target.clone(),
                owner,
                amount: total,
                parts: prompt_parts,
                critical_hit,
            });
            let text = format!("{name}: asked to take {} ({})", total, detail.join(" + "));
            self.ev(
                Some(target),
                K::Damage {
                    result,
                    prompted: true,
                },
                text,
            );
            return Ok(());
        }
        let text = format!("{name} takes {} ({})", total, detail.join(" + "));
        self.ev(
            Some(target),
            K::Damage {
                result,
                prompted: false,
            },
            text,
        );
        self.lose_hp(i, total, critical_hit);
        Ok(())
    }

    /// Takes `amount` (already typed) from temp HP, then HP, and applies the 0 HP rules.
    fn lose_hp(&mut self, i: usize, amount: i32, critical_hit: bool) {
        if amount <= 0 {
            return;
        }
        let id = self.s.combatants[i].id.clone();
        let c = &mut self.s.combatants[i];
        let absorbed = amount.min(c.hp.temp);
        c.hp.temp -= absorbed;
        let rest = amount - absorbed;
        let before = c.hp.current;
        c.hp.current = (before - rest).max(0);
        let overflow = rest - before;
        self.after_hp_loss(i, before, rest, overflow, amount, critical_hit, &id);
    }

    /// The consequences of losing HP: dropping to 0, damage at 0 HP, massive damage and concentration checks.
    #[allow(clippy::too_many_arguments)]
    fn after_hp_loss(
        &mut self,
        i: usize,
        before: i32,
        rest: i32,
        overflow: i32,
        amount: i32,
        critical_hit: bool,
        id: &CombatantId,
    ) {
        let name = self.name(id);
        let c = &self.s.combatants[i];
        let (max, rule, life) = (c.hp.max, c.zero_hp, c.life);
        if before > 0 && c.hp.current == 0 {
            if rule == ZeroHpRule::Dies || overflow >= max {
                let why = if rule == ZeroHpRule::DeathSaves {
                    " (massive damage)"
                } else {
                    ""
                };
                self.s.combatants[i].life = Life::Dead;
                self.ev(Some(id), K::Died, format!("{name} dies{why}"));
            } else {
                self.s.combatants[i].life = Life::Dying {
                    successes: 0,
                    failures: 0,
                };
                self.ev(
                    Some(id),
                    K::Dying,
                    format!("{name} drops to 0 HP and is dying"),
                );
                self.add_dying_effect(id);
            }
            self.went_down(id, "dropped to 0 HP");
            return;
        }
        if before == 0 && rest > 0 {
            if rest >= max && rule == ZeroHpRule::DeathSaves {
                self.s.combatants[i].life = Life::Dead;
                self.ev(Some(id), K::Died, format!("{name} dies (massive damage)"));
                return;
            }
            let add = if critical_hit { 2 } else { 1 };
            let (successes, failures) = match life {
                Life::Dying {
                    successes,
                    failures,
                } => (successes, failures),
                Life::Stable => (0, 0),
                Life::Alive | Life::Dead => return,
            };
            let failures = failures + add;
            if failures >= 3 {
                self.s.combatants[i].life = Life::Dead;
                self.ev(Some(id), K::Died, format!("{name} dies (damage at 0 HP)"));
            } else {
                self.s.combatants[i].life = Life::Dying {
                    successes,
                    failures,
                };
                self.ev(
                    Some(id),
                    K::Dying,
                    format!("{name}: damage at 0 HP, {failures} death-save failures"),
                );
            }
            return;
        }
        if self.s.combatants[i].concentration.is_some() && amount > 0 {
            let dc = concentration_dc(self.s.edition, amount);
            self.s.combatants[i]
                .pending_concentration
                .push(ConcentrationCheck { dc, damage: amount });
            self.ev(
                Some(id),
                K::ConcentrationCheck { dc },
                format!("{name}: concentration save DC {dc}"),
            );
            if let Some(owner) = self.s.combatants[i].owner().map(str::to_string) {
                self.prompts.push(PlayerEvent::ConcentrationCheck {
                    id: id.clone(),
                    owner,
                    dc,
                });
            }
        }
    }

    fn add_dying_effect(&mut self, id: &CombatantId) {
        let mut spec = EffectSpec::new("Dying", id.clone());
        spec.conditions = vec![Condition::Unconscious];
        spec.detail = "At 0 HP: unconscious until it regains HP.".into();
        let _ = self.add_effect(spec, EffectOrigin::Dying);
    }

    /// Back above 0 HP: alive, death saves cleared, the dying Unconscious removed.
    fn revive(&mut self, i: usize) {
        let id = self.s.combatants[i].id.clone();
        if self.s.combatants[i].life.is_alive() {
            return;
        }
        self.s.combatants[i].life = Life::Alive;
        let dying: Vec<EffectId> = self
            .s
            .effects
            .iter()
            .filter(|e| e.spec.target == id && e.origin == EffectOrigin::Dying)
            .map(|e| e.id)
            .collect();
        for e in dying {
            self.end_effect(e, "regained HP", false);
        }
        let name = self.name(&id);
        self.ev(Some(&id), K::Revived, format!("{name} is back up"));
    }

    /// The combatant went down (0 HP, death or removal): concentration ends, mounts unlink, effects tied to it end,
    /// and rules tied to its life end for good.
    fn went_down(&mut self, id: &CombatantId, reason: &str) {
        if self
            .s
            .combatant(id)
            .is_some_and(|c| c.concentration.is_some())
        {
            self.end_concentration(id, reason);
        }
        let pets: Vec<CombatantId> = self
            .s
            .combatants
            .iter()
            .filter(|c| !c.removed && c.owner_id.as_ref() == Some(id))
            .map(|c| c.id.clone())
            .collect();
        for pet in pets {
            let (np, no) = (self.name(&pet), self.name(id));
            self.ev(
                Some(&pet),
                K::OwnerDefeated { owner: id.clone() },
                format!(
                    "{no} is down: keep {np} after it, dismiss it, or give it its own initiative"
                ),
            );
        }
        let followers: Vec<usize> = (0..self.s.combatants.len())
            .filter(|&j| self.s.combatants[j].linked_to.as_ref() == Some(id))
            .collect();
        for j in followers {
            self.s.combatants[j].linked_to = None;
            let f = self.s.combatants[j].id.clone();
            let (nf, nl) = (self.name(&f), self.name(id));
            self.ev(
                Some(&f),
                K::Unlinked { from: id.clone() },
                format!("{nf} now acts on its own initiative ({nl} is down)"),
            );
        }
        let tied: Vec<EffectId> = self
            .s
            .effects
            .iter()
            .filter(|e| e.spec.ends_on_defeat_of.contains(id))
            .map(|e| e.id)
            .collect();
        let name = self.name(id);
        for e in tied {
            self.end_effect(e, &format!("{name} is down"), true);
        }
        let rules: Vec<usize> = (0..self.s.rules.len())
            .filter(|&r| self.s.rules[r].active && self.s.rules[r].linked.as_ref() == Some(id))
            .collect();
        for r in rules {
            self.s.rules[r].active = false;
            let (rid, rname) = (self.s.rules[r].id.clone(), self.s.rules[r].name.clone());
            self.ev(
                Some(id),
                K::Rule {
                    rule: rid,
                    active: false,
                },
                format!("{rname} ends: {name} is down"),
            );
        }
    }

    fn heal(&mut self, target: &CombatantId, amount: &DiceInput) -> Result<(), CombatError> {
        let i = self.idx(target)?;
        let roll = resolve_dice(amount, self.die)?;
        if roll.total < 0 {
            return Err(invalid("Healing can't be negative."));
        }
        let c = &self.s.combatants[i];
        if c.life.is_dead() {
            return Err(CombatError::Dead);
        }
        let name = self.name(target);
        if c.hp_authority == HpAuthority::Player {
            let owner = c.owner().unwrap_or_default().to_string();
            self.prompts.push(PlayerEvent::HealPrompt {
                id: target.clone(),
                owner,
                amount: roll.total,
            });
            self.ev(
                Some(target),
                K::Healed {
                    amount: roll.total,
                    prompted: true,
                },
                format!("{name}: asked to regain {}", roll.total),
            );
            return Ok(());
        }
        let hp = &mut self.s.combatants[i].hp;
        let before = hp.current;
        hp.current = (hp.current + roll.total).min(hp.max);
        let gained = hp.current - before;
        let (cur, max) = (hp.current, hp.max);
        self.ev(
            Some(target),
            K::Healed {
                amount: gained,
                prompted: false,
            },
            format!("{name} regains {gained} HP · {cur}/{max}"),
        );
        if cur > 0 {
            self.revive(i);
        }
        Ok(())
    }

    fn set_hp(
        &mut self,
        id: &CombatantId,
        current: i32,
        max: Option<i32>,
        temp: Option<i32>,
    ) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        if self.s.combatants[i].hp_authority == HpAuthority::Player {
            return Err(invalid(
                "A player character's HP lives on the player's sheet (D7).",
            ));
        }
        if max.is_some_and(|m| m < 0) || temp.is_some_and(|t| t < 0) {
            return Err(invalid("HP can't be negative."));
        }
        let hp = &mut self.s.combatants[i].hp;
        if let Some(m) = max {
            hp.max = m;
        }
        if let Some(t) = temp {
            hp.temp = t;
        }
        let before = hp.current;
        hp.current = current.clamp(0, hp.max);
        let (cur, mx) = (hp.current, hp.max);
        let name = self.name(id);
        self.ev(Some(id), K::HpSet, format!("{name} HP set to {cur}/{mx}"));
        if cur > 0 {
            self.revive(i);
        } else if before > 0 || self.s.combatants[i].life.is_alive() {
            self.drop_to_zero(i);
        }
        Ok(())
    }

    /// Down to 0 HP without damage (HP set or synced): dies or starts dying, without death-save failures.
    fn drop_to_zero(&mut self, i: usize) {
        let id = self.s.combatants[i].id.clone();
        if !self.s.combatants[i].life.is_alive() {
            return;
        }
        let name = self.name(&id);
        match self.s.combatants[i].zero_hp {
            ZeroHpRule::Dies => {
                self.s.combatants[i].life = Life::Dead;
                self.ev(Some(&id), K::Died, format!("{name} dies"));
            }
            ZeroHpRule::DeathSaves => {
                self.s.combatants[i].life = Life::Dying {
                    successes: 0,
                    failures: 0,
                };
                self.ev(
                    Some(&id),
                    K::Dying,
                    format!("{name} drops to 0 HP and is dying"),
                );
                self.add_dying_effect(&id);
            }
        }
        self.went_down(&id, "dropped to 0 HP");
    }

    fn sync_player_hp(
        &mut self,
        id: &CombatantId,
        current: i32,
        max: i32,
        temp: i32,
        damage: Option<DamageTaken>,
    ) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        if self.s.combatants[i].hp_authority != HpAuthority::Player {
            return Err(invalid("Only a player character's HP is synced."));
        }
        if max < 0 || temp < 0 {
            return Err(invalid("HP can't be negative."));
        }
        let old = self.s.combatants[i].hp;
        let hp = &mut self.s.combatants[i].hp;
        hp.max = max;
        hp.temp = temp;
        hp.current = current.clamp(0, max);
        let cur = hp.current;
        let name = self.name(id);
        self.ev(
            Some(id),
            K::HpSet,
            format!("{name} HP {cur}/{max} (from the player's sheet)"),
        );
        match damage {
            Some(d) if d.amount > 0 => {
                let rest = (d.amount - old.temp).max(0);
                let overflow = rest - old.current;
                self.after_hp_loss(i, old.current, rest, overflow, d.amount, d.critical_hit, id);
            }
            _ => {
                if cur > 0 {
                    self.revive(i);
                } else {
                    self.drop_to_zero(i);
                }
            }
        }
        Ok(())
    }

    // ----- death saves (D18) -----

    fn death_save(&mut self, id: &CombatantId, roll: &D20Input) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        let Life::Dying {
            successes,
            failures,
        } = self.s.combatants[i].life
        else {
            return Err(CombatError::NotDying);
        };
        let r = resolve_d20(roll, self.die)?;
        self.report_d20(id, "Death save", &r);
        let natural = r
            .natural
            .or_else(|| u32::try_from(r.total).ok().filter(|t| *t == 1 || *t == 20));
        let name = self.name(id);
        let text = format!("{name} death save: {}", r.breakdown);
        self.ev(Some(id), K::DeathSave { roll: r.clone() }, text);
        if natural == Some(20) {
            let c = &mut self.s.combatants[i];
            c.hp.current = 1;
            if let Some(owner) = c.owner().map(str::to_string) {
                if c.hp_authority == HpAuthority::Player {
                    self.prompts.push(PlayerEvent::HealPrompt {
                        id: id.clone(),
                        owner,
                        amount: 1,
                    });
                }
            }
            self.revive(i);
            return Ok(());
        }
        let (s, f) = if natural == Some(1) {
            (successes, failures + 2)
        } else if r.total >= 10 {
            (successes + 1, failures)
        } else {
            (successes, failures + 1)
        };
        if f >= 3 {
            self.s.combatants[i].life = Life::Dead;
            self.ev(
                Some(id),
                K::Died,
                format!("{name} dies (three failed death saves)"),
            );
        } else if s >= 3 {
            self.s.combatants[i].life = Life::Stable;
            self.ev(Some(id), K::Stabilized, format!("{name} is stable"));
        } else {
            self.s.combatants[i].life = Life::Dying {
                successes: s,
                failures: f,
            };
        }
        Ok(())
    }

    // ----- conditions (C10) -----

    fn add_condition(&mut self, mut spec: EffectSpec) -> Result<(), CombatError> {
        let i = self.idx(&spec.target)?;
        if spec.conditions.is_empty() {
            return Err(invalid("No condition given."));
        }
        let immune = &self.s.combatants[i].defenses.condition_immunities;
        let blocked: Vec<Condition> = spec
            .conditions
            .iter()
            .copied()
            .filter(|c| immune.contains(c))
            .collect();
        spec.conditions.retain(|c| !blocked.contains(c));
        if !blocked.is_empty() {
            let name = self.name(&spec.target);
            let list: Vec<&str> = blocked.iter().map(|c| c.as_str()).collect();
            self.ev(
                Some(&spec.target),
                K::ConditionImmune,
                format!("{name} is immune to {}", list.join(", ")),
            );
        }
        if spec.conditions.is_empty() {
            return Ok(());
        }
        if spec.name.trim().is_empty() {
            spec.name = spec
                .conditions
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ");
        }
        self.add_effect(spec, EffectOrigin::Command).map(|_| ())
    }

    fn remove_condition(
        &mut self,
        target: &CombatantId,
        condition: Condition,
    ) -> Result<(), CombatError> {
        self.idx(target)?;
        let hits: Vec<EffectId> = self
            .s
            .effects
            .iter()
            .filter(|e| &e.spec.target == target && e.spec.conditions.contains(&condition))
            .map(|e| e.id)
            .collect();
        if hits.is_empty() {
            return Err(invalid(format!("Not {condition}.")));
        }
        for e in hits {
            let k = self.effect_idx(e)?;
            let spec = &self.s.effects[k].spec;
            if spec.conditions.len() == 1 && spec.modifiers.is_empty() && spec.charges.is_none() {
                self.end_effect(e, "removed", false);
            } else {
                self.s.effects[k]
                    .spec
                    .conditions
                    .retain(|c| *c != condition);
                let name = self.name(target);
                self.ev(
                    Some(target),
                    K::EffectEnded {
                        effect: e,
                        reason: "condition removed".into(),
                    },
                    format!("{name} is no longer {condition}"),
                );
            }
        }
        Ok(())
    }

    fn set_exhaustion(&mut self, id: &CombatantId, level: u8) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        if level > 6 {
            return Err(invalid("Exhaustion is 0 to 6."));
        }
        self.s.combatants[i].exhaustion = level;
        let name = self.name(id);
        self.ev(
            Some(id),
            K::Exhaustion { level },
            format!("{name}: exhaustion {level}"),
        );
        if level == 6 && !self.s.combatants[i].life.is_dead() {
            self.s.combatants[i].life = Life::Dead;
            self.ev(Some(id), K::Died, format!("{name} dies (exhaustion 6)"));
            self.went_down(id, "died");
        }
        Ok(())
    }

    // ----- concentration (C11) -----

    fn start_concentration(&mut self, id: &CombatantId, name: &str) {
        let current = self.s.combatant(id).and_then(|c| c.concentration.clone());
        match current {
            Some(k) if k.name == name => return,
            Some(_) => self.end_concentration(id, "concentrates on something else"),
            None => {}
        }
        if let Some(c) = self.s.combatant_mut(id) {
            c.concentration = Some(Concentration {
                name: name.to_string(),
            });
        }
        let n = self.name(id);
        self.ev(
            Some(id),
            K::ConcentrationStarted,
            format!("{n} concentrates on {name}"),
        );
    }

    /// Ends concentration and every effect it holds (the cascade); their follow-ups are added.
    fn end_concentration(&mut self, id: &CombatantId, reason: &str) {
        let Some(c) = self.s.combatant_mut(id) else {
            return;
        };
        let Some(k) = c.concentration.take() else {
            return;
        };
        c.pending_concentration.clear();
        let held: Vec<EffectId> = self
            .s
            .effects
            .iter()
            .filter(|e| e.spec.concentration && e.spec.source.as_ref() == Some(id))
            .map(|e| e.id)
            .collect();
        for e in held {
            self.end_effect(e, &format!("concentration ended ({reason})"), true);
        }
        let n = self.name(id);
        self.ev(
            Some(id),
            K::ConcentrationEnded {
                name: k.name.clone(),
            },
            format!("{n} stops concentrating on {} ({reason})", k.name),
        );
    }

    fn resolve_concentration(
        &mut self,
        id: &CombatantId,
        roll: &D20Input,
    ) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        if self.s.combatants[i].pending_concentration.is_empty() {
            return Err(CombatError::NoPendingCheck);
        }
        let r = resolve_d20(roll, self.die)?;
        self.report_d20(id, "Concentration save", &r);
        let check = self.s.combatants[i].pending_concentration.remove(0);
        let kept = r.total >= check.dc as i32;
        let name = self.name(id);
        let text = format!(
            "{name} concentration save {} vs DC {}: {}",
            r.total,
            check.dc,
            if kept { "kept" } else { "lost" }
        );
        self.ev(Some(id), K::ConcentrationSave { roll: r, kept }, text);
        if !kept {
            self.end_concentration(id, "failed the save");
        }
        Ok(())
    }

    // ----- effects (C12, C13) -----

    fn add_effect(
        &mut self,
        spec: EffectSpec,
        origin: EffectOrigin,
    ) -> Result<EffectId, CombatError> {
        let t = self.idx(&spec.target)?;
        if let Some(src) = &spec.source {
            self.idx(src)?;
        }
        if matches!(spec.duration, Duration::SourceTurns { .. }) && spec.source.is_none() {
            return Err(invalid("A duration in the source's turns needs a source."));
        }
        if spec.concentration {
            let src = spec
                .source
                .clone()
                .ok_or_else(|| invalid("A concentration effect needs a source."))?;
            if !self.s.combatant(&src).is_some_and(|c| c.life.is_alive()) {
                return Err(invalid("A creature at 0 HP can't concentrate."));
            }
            self.start_concentration(&src, &spec.name);
        }
        for d in &spec.ends_on_defeat_of {
            self.idx(d)?;
        }
        let mut spec = spec;
        let immune = self.s.combatants[t].defenses.condition_immunities.clone();
        spec.conditions.retain(|c| !immune.contains(c));
        let id = EffectId(self.s.next_effect);
        self.s.next_effect += 1;
        let incap = spec.conditions.iter().any(|c| c.incapacitates());
        let target = spec.target.clone();
        let text = format!("{} on {}", spec.name, self.name(&target));
        self.s.effects.push(Effect { id, spec, origin });
        self.ev(Some(&target), K::EffectAdded { effect: id }, text);
        if incap && self.s.combatants[t].concentration.is_some() && incapacitated(&self.s, &target)
        {
            self.end_concentration(&target, "incapacitated");
        }
        Ok(id)
    }

    /// Removes an effect. With `follow_up`, its follow-up effect is added.
    fn end_effect(&mut self, id: EffectId, reason: &str, follow_up: bool) {
        let Some(k) = self.s.effects.iter().position(|e| e.id == id) else {
            return;
        };
        let e = self.s.effects.remove(k);
        let target = e.spec.target.clone();
        let text = format!("{} on {} ends ({reason})", e.spec.name, self.name(&target));
        self.ev(
            Some(&target),
            K::EffectEnded {
                effect: id,
                reason: reason.to_string(),
            },
            text,
        );
        // The last effect held by a concentration ends that concentration.
        if e.spec.concentration {
            if let Some(src) = &e.spec.source {
                let still = self.s.effects.iter().any(|o| {
                    o.spec.concentration
                        && o.spec.source.as_ref() == Some(src)
                        && o.spec.name == e.spec.name
                });
                let on_it = self
                    .s
                    .combatant(src)
                    .and_then(|c| c.concentration.as_ref())
                    .is_some_and(|c| c.name == e.spec.name);
                if !still && on_it {
                    let src = src.clone();
                    if let Some(c) = self.s.combatant_mut(&src) {
                        c.concentration = None;
                        c.pending_concentration.clear();
                    }
                    let n = self.name(&src);
                    self.ev(
                        Some(&src),
                        K::ConcentrationEnded {
                            name: e.spec.name.clone(),
                        },
                        format!("{n} stops concentrating on {}", e.spec.name),
                    );
                }
            }
        }
        if follow_up {
            if let Some(next) = e.spec.follow_up {
                if self.s.combatant(&next.target).is_some_and(|c| !c.removed) {
                    let _ = self.add_effect(*next, EffectOrigin::Command);
                }
            }
        }
    }

    fn use_charge(&mut self, effect: EffectId) -> Result<(), CombatError> {
        let k = self.effect_idx(effect)?;
        let e = &mut self.s.effects[k];
        let charges = e
            .spec
            .charges
            .as_mut()
            .ok_or_else(|| invalid("That effect has no charges."))?;
        if *charges == 0 {
            return Err(invalid("No charges left."));
        }
        *charges -= 1;
        let remaining = *charges;
        let (name, target) = (e.spec.name.clone(), e.spec.target.clone());
        self.ev(
            Some(&target),
            K::ChargeUsed { effect, remaining },
            format!("{name}: {remaining} charges left"),
        );
        if remaining == 0 {
            self.end_effect(effect, "charges used", true);
        }
        Ok(())
    }

    fn resolve_effect_save(
        &mut self,
        effect: EffectId,
        roll: &D20Input,
    ) -> Result<(), CombatError> {
        let k = self.effect_idx(effect)?;
        let save = self.s.effects[k]
            .spec
            .repeat_save
            .ok_or(CombatError::NoSave)?;
        let r = resolve_d20(roll, self.die)?;
        let success = r.total >= save.dc as i32;
        let target = self.s.effects[k].spec.target.clone();
        self.report_d20(&target, &format!("{} save", save.ability), &r);
        let text = format!(
            "{} {} save {} vs DC {}: {}",
            self.name(&target),
            save.ability,
            r.total,
            save.dc,
            if success { "success" } else { "failure" }
        );
        self.ev(
            Some(&target),
            K::EffectSave {
                effect,
                roll: r,
                success,
            },
            text,
        );
        if success {
            self.end_effect(effect, "saved", true);
        }
        Ok(())
    }

    // ----- abilities (C14–C18, C21) -----

    fn use_ability(
        &mut self,
        id: &CombatantId,
        key: &str,
        slot: Option<u8>,
    ) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        let ability = self.s.combatants[i]
            .ability(key)
            .cloned()
            .ok_or_else(|| CombatError::UnknownAbility(key.into()))?;
        ability_status(&self.s, &self.s.combatants[i], &ability, slot)
            .map_err(CombatError::Unavailable)?;
        let pool = pool_of(&self.s, id).map(|(k, _)| k.clone());
        let c = &mut self.s.combatants[i];
        match ability.kind {
            AbilityKind::Action => c.economy.action = true,
            AbilityKind::BonusAction => c.economy.bonus_action = true,
            AbilityKind::Reaction => c.economy.reaction = true,
            AbilityKind::Legendary | AbilityKind::Free | AbilityKind::Passive => {}
        }
        if let Some(a) = c.abilities.iter_mut().find(|a| a.key == key) {
            if let Some(r) = a.recharge.as_mut() {
                r.ready = false;
            }
        }
        if let Some(res) = ability.uses.as_ref().and_then(|k| c.resources.get_mut(k)) {
            res.pool.remaining -= 1;
        }
        if let Some(level) = slot {
            if let Some(p) = c.spell_slots.get_mut(&level) {
                p.remaining -= 1;
            }
        }
        if let Some(tag) = &ability.group_limit {
            if let Some(g) = self.s.group_limits.get_mut(tag) {
                g.used += 1;
            }
        }
        if ability.kind == AbilityKind::Legendary {
            if let Some(p) = pool.and_then(|k| self.s.legendary_pools.get_mut(&k)) {
                p.remaining -= ability.legendary_cost.unwrap_or(1);
            }
        }
        let name = self.name(id);
        let slot_text = slot.map_or_else(String::new, |l| format!(" (level {l} slot)"));
        self.ev(
            Some(id),
            K::AbilityUsed {
                ability: key.to_string(),
            },
            format!("{name} uses {}{slot_text}", ability.name),
        );
        Ok(())
    }

    fn roll_recharge(
        &mut self,
        id: &CombatantId,
        key: &str,
        roll: &DiceInput,
    ) -> Result<(), CombatError> {
        let i = self.idx(id)?;
        let a = self.s.combatants[i]
            .ability(key)
            .ok_or_else(|| CombatError::UnknownAbility(key.into()))?;
        let min = a
            .recharge
            .ok_or_else(|| invalid("That ability has no recharge."))?
            .min;
        let r = resolve_dice(roll, self.die)?;
        self.report_dice(id, &format!("Recharge {key}"), &r);
        if !(1..=6).contains(&r.total) {
            return Err(invalid("A recharge d6 is 1 to 6."));
        }
        self.finish_recharge(i, key, min, r);
        Ok(())
    }

    fn finish_recharge(&mut self, i: usize, key: &str, min: u8, roll: party_dice::DiceResult) {
        let ready = roll.total >= i32::from(min);
        let id = self.s.combatants[i].id.clone();
        if let Some(r) = self.s.combatants[i]
            .abilities
            .iter_mut()
            .find(|a| a.key == key)
            .and_then(|a| a.recharge.as_mut())
        {
            r.ready = ready;
        }
        let name = self.name(&id);
        let text = format!(
            "{name}: recharge {key} d6 = {} · {}",
            roll.total,
            if ready { "ready" } else { "not ready" }
        );
        self.ev(
            Some(&id),
            K::Recharge {
                ability: key.to_string(),
                roll,
                ready,
            },
            text,
        );
    }
}
