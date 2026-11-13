//! Decision cards: what a combatant may do now, with short option ids and the legal targets of each, built from the
//! engine's `ability_status`, the sheet and the bands. An oracle answers with ids only.

use party_combat::{derive, AbilityKind, CombatantId, Life};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::adjudicate::{numbers_of, Adj, Number};
use crate::rules;
use crate::runner::{Sim, Window};
use crate::setup::Side;
use crate::sheet::{parts_avg2, ActionDef, Coverage, Def};
use crate::space::Move;

/// One answer to a card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    /// An option id from the card, or `repeat` / `basic`.
    pub o: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub t: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mv: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adj: Option<Adj>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// What an option does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptKind {
    Attack,
    Multiattack,
    Save,
    Heal,
    Adjudicate,
    Dodge,
    Dash,
    Disengage,
    Yield,
    End,
    Pass,
}

/// One option on a card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Opt {
    pub o: String,
    pub name: String,
    pub kind: OptKind,
    pub cost: &'static str,
    pub summary: String,
    pub targets: Vec<CombatantId>,
    /// How many targets it takes at most (0: none).
    pub max_targets: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_hint: Option<String>,
    /// The sheet action behind it.
    #[serde(skip)]
    pub action: Option<usize>,
}

fn cost_of(kind: AbilityKind) -> &'static str {
    match kind {
        AbilityKind::Action => "action",
        AbilityKind::BonusAction => "bonus action",
        AbilityKind::Reaction => "reaction",
        AbilityKind::Legendary => "legendary",
        AbilityKind::Free | AbilityKind::Passive => "free",
    }
}

fn short_type(t: Option<party_combat::DamageType>) -> String {
    t.map_or_else(String::new, |t| {
        let s = t.as_str();
        format!(" {}", if s.len() > 5 { &s[..5] } else { s })
    })
}

fn dmg_text(parts: &[crate::sheet::DamageRoll]) -> String {
    parts
        .iter()
        .map(|p| format!("{}{}", p.dice, short_type(p.damage_type)))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// A one-line summary of an action (numbers from the sheet, never its text).
#[must_use]
pub fn summary(sim: &Sim, a: &ActionDef) -> String {
    match &a.def {
        Def::Attack(x) => {
            let reach = if x.melee && x.ranged {
                format!("reach {}, range {}/{}", x.reach, x.range, x.long)
            } else if x.ranged {
                format!("range {}/{}", x.range, x.long)
            } else {
                format!("reach {}", x.reach)
            };
            let times = if x.count > 1 {
                format!(" x{}", x.count)
            } else {
                String::new()
            };
            format!("{:+}, {}, {reach}{times}", x.to_hit, dmg_text(&x.parts))
        }
        Def::Save(s) => {
            let area = s.area.as_ref().map_or_else(
                || "one target".to_string(),
                |ar| format!("{}-ft {}, up to {}", ar.size, ar.shape, ar.max_targets),
            );
            let dmg = if s.parts.is_empty() {
                String::new()
            } else {
                format!(
                    ", {}{}",
                    dmg_text(&s.parts),
                    if s.half { " half on save" } else { "" }
                )
            };
            let cond = s
                .rider
                .as_ref()
                .map_or_else(String::new, |r| format!(", {} on fail", r.condition));
            format!(
                "DC {} {}{dmg}{cond}, {area}",
                s.dc,
                s.ability.as_str().to_uppercase()
            )
        }
        Def::Multiattack { steps, .. } => steps
            .iter()
            .map(|st| {
                let name = sim
                    .sheet_of_key(a, &st.key)
                    .map_or_else(|| st.key.clone(), |x| x.name.clone());
                format!("{}x {name}", st.count)
            })
            .collect::<Vec<_>>()
            .join(", "),
        Def::Spell(s) => {
            let lvl = if s.level == 0 {
                "cantrip".to_string()
            } else {
                format!("level {}", s.level)
            };
            let dc = s.dc.map_or_else(String::new, |d| format!(", DC {d}"));
            let atk = s
                .attack
                .map_or_else(String::new, |d| format!(", {d:+} to hit"));
            let c = if s.concentration {
                ", concentration"
            } else {
                ""
            };
            format!("spell, {lvl}{dc}{atk}{c}: adjudicate")
        }
        Def::Heal { dice } => format!("heals {dice}"),
        Def::Text => "text only: adjudicate".into(),
    }
}

impl Sim {
    /// The options of `actor` in the current window.
    #[must_use]
    pub fn options(&self, actor: &CombatantId) -> Vec<Opt> {
        let mut out = Vec::new();
        let Some(c) = self.state.combatant(actor) else {
            return out;
        };
        let Some(sheet) = self.sheets.get(actor) else {
            return out;
        };
        let alive = self.alive_ids();
        let enemies: Vec<CombatantId> = alive
            .iter()
            .filter(|o| self.space.side_of(o) != self.space.side_of(actor))
            .cloned()
            .collect();
        let allies: Vec<CombatantId> = self
            .state
            .combatants
            .iter()
            .filter(|o| !o.removed && !o.life.is_dead() && self.space.present(&o.id))
            .filter(|o| self.space.side_of(&o.id) == self.space.side_of(actor))
            .map(|o| o.id.clone())
            .collect();
        let legendary = matches!(self.window, Window::Legendary { .. });
        for (i, a) in sheet.actions.iter().enumerate() {
            let wanted = if legendary {
                a.kind == AbilityKind::Legendary
            } else {
                matches!(
                    a.kind,
                    AbilityKind::Action | AbilityKind::BonusAction | AbilityKind::Free
                )
            };
            if !wanted || a.coverage == Coverage::NotSimulated {
                continue;
            }
            if a.coverage == Coverage::Adjudicable && !self.opts.adjudicate {
                continue;
            }
            let Some(ability) = c.ability(&a.key) else {
                continue;
            };
            if derive::ability_status(&self.state, c, ability, a.slot).is_err() {
                continue;
            }
            let (kind, targets, max) = match &a.def {
                Def::Attack(x) => {
                    let t = self.attack_targets(actor, x, &enemies);
                    (OptKind::Attack, t, x.count)
                }
                Def::Multiattack { steps, .. } => {
                    let mut t: Vec<CombatantId> = Vec::new();
                    let mut n = 0;
                    for st in steps {
                        if let Some(Def::Attack(x)) = self.sheet_of_key(a, &st.key).map(|d| &d.def)
                        {
                            for id in self.attack_targets(actor, x, &enemies) {
                                if !t.contains(&id) {
                                    t.push(id);
                                }
                            }
                        }
                        n += st.count;
                    }
                    (OptKind::Multiattack, t, n)
                }
                Def::Save(s) => {
                    let t: Vec<CombatantId> = enemies
                        .iter()
                        .filter(|e| {
                            s.range == 0
                                || self.space.feet(actor, e)
                                    <= s.range.max(30) + self.reach_bonus(actor)
                        })
                        .cloned()
                        .collect();
                    (
                        OptKind::Save,
                        t,
                        s.area.as_ref().map_or(1, |ar| ar.max_targets),
                    )
                }
                Def::Heal { .. } => {
                    let t: Vec<CombatantId> = allies.clone();
                    (OptKind::Heal, t, 1)
                }
                Def::Spell(_) | Def::Text => {
                    let mut t = enemies.clone();
                    t.extend(allies.iter().cloned());
                    (OptKind::Adjudicate, t, 6)
                }
            };
            if targets.is_empty() && !matches!(kind, OptKind::Adjudicate) {
                continue;
            }
            let prefix = match (&a.def, a.kind) {
                (_, AbilityKind::Legendary) => "l",
                (_, AbilityKind::BonusAction) => "b",
                (Def::Spell(_), _) => "s",
                (Def::Text, _) if a.resource.is_some() => "r",
                (Def::Text, _) => "x",
                _ => "a",
            };
            let mut name = a.name.clone();
            if let Some(r) = c
                .abilities
                .iter()
                .find(|x| x.key == a.key)
                .and_then(|x| x.recharge)
            {
                name.push_str(&format!(" (recharge {}-6)", r.min));
            }
            out.push(Opt {
                o: format!("{prefix}{}", i + 1),
                name,
                kind,
                cost: cost_of(a.kind),
                summary: summary(self, a),
                mode_hint: self.mode_hint(actor, &targets, a),
                targets,
                max_targets: max,
                action: Some(i),
            });
        }
        if legendary {
            out.push(generic(
                "pass",
                "Pass",
                OptKind::Pass,
                "free",
                "no legendary action now",
            ));
            return out;
        }
        if !c.economy.action && c.life == Life::Alive {
            out.push(generic(
                "dodge",
                "Dodge",
                OptKind::Dodge,
                "action",
                "attacks against it have disadvantage until its next turn",
            ));
            out.push(generic(
                "dash",
                "Dash",
                OptKind::Dash,
                "action",
                "double movement this turn (use with mv)",
            ));
            out.push(generic(
                "disengage",
                "Disengage",
                OptKind::Disengage,
                "action",
                "move away without opportunity attacks (use with mv)",
            ));
            if self.space.side_of(actor) == Side::Foes && self.morale_low(actor) {
                out.push(generic(
                    "yield",
                    "Surrender",
                    OptKind::Yield,
                    "action",
                    "stops fighting (counted as withdrew)",
                ));
            }
        }
        out.push(generic(
            "end",
            "End turn",
            OptKind::End,
            "free",
            "end the turn (mv still applies)",
        ));
        out
    }

    /// Steps a legendary action that moves may take now (half its speed), else 0.
    #[must_use]
    pub fn legendary_steps(&self, actor: &CombatantId) -> i32 {
        if !matches!(self.window, Window::Legendary { .. }) {
            return 0;
        }
        let moves = self.sheets.get(actor).is_some_and(|s| {
            s.actions.iter().any(|a| {
                a.kind == AbilityKind::Legendary
                    && matches!(a.def, Def::Multiattack { moves: true, .. })
            })
        });
        let speed = self
            .state
            .combatant(actor)
            .map_or(0, |c| derive::effective_speed(&self.state, c).walk);
        if moves {
            crate::space::budget(speed / 2)
        } else {
            0
        }
    }

    fn reach_bonus(&self, actor: &CombatantId) -> u32 {
        // Movement this turn brings a ranged effect closer.
        u32::try_from(self.turn.steps_left.max(0)).unwrap_or(0)
            * crate::space::STEP_FT
            * u32::from(self.turn.actor.as_ref() == Some(actor))
    }

    /// Enemies an attack can reach this turn (moving if needed).
    #[must_use]
    pub fn attack_targets(
        &self,
        actor: &CombatantId,
        x: &crate::sheet::AttackDef,
        enemies: &[CombatantId],
    ) -> Vec<CombatantId> {
        let steps_left = if self.turn.actor.as_ref() == Some(actor) {
            self.turn.steps_left
        } else {
            self.legendary_steps(actor)
        };
        enemies
            .iter()
            .filter(|e| {
                let d = self.space.steps(actor, e);
                let melee_ok = x.melee && d <= steps_left;
                let ranged_ok = x.ranged && {
                    let long = x.long.max(x.range);
                    let reach_steps = i32::try_from(long / crate::space::STEP_FT).unwrap_or(0);
                    d <= reach_steps + steps_left
                };
                melee_ok || ranged_ok
            })
            .cloned()
            .collect()
    }

    fn mode_hint(
        &self,
        actor: &CombatantId,
        targets: &[CombatantId],
        a: &ActionDef,
    ) -> Option<String> {
        let x = match &a.def {
            Def::Attack(x) => x,
            Def::Multiattack { steps, .. } => match steps
                .first()
                .and_then(|s| self.sheet_of_key(a, &s.key))
                .map(|d| &d.def)
            {
                Some(Def::Attack(x)) => x,
                _ => return None,
            },
            _ => return None,
        };
        let mut hints = Vec::new();
        for t in targets.iter().take(4) {
            let ranged = !x.melee || self.space.steps(actor, t) > 0;
            let cx = self.context(actor, t, ranged, x);
            let m = rules::attack_mode(&self.state, actor, t, cx);
            if m.mode != party_combat::party_dice::RollMode::Normal {
                let w = match m.mode {
                    party_combat::party_dice::RollMode::Advantage => "adv",
                    _ => "dis",
                };
                hints.push(format!("{t}: {w}"));
            }
        }
        (!hints.is_empty()).then(|| hints.join(", "))
    }

    /// The board, from `viewer`'s side, for its card (fogged: enemies' health as bands only).
    #[must_use]
    pub fn card(&self, actor: &CombatantId) -> Value {
        let alive = self.alive_ids();
        let side = self.space.side_of(actor);
        let fog = self.opts.fog;
        let c = self.state.combatant(actor);
        let sheet = self.sheets.get(actor);
        let options = self.options(actor);
        let seen: Vec<Value> = self
            .state
            .combatants
            .iter()
            .filter(|o| !o.removed && &o.id != actor && self.space.present(&o.id))
            .map(|o| {
                let own = self.space.side_of(&o.id) == side;
                let health = if own || !fog {
                    json!({ "hp": o.hp.current, "max": o.hp.max })
                } else {
                    json!(o.hp.band().map_or("unknown", |b| match b {
                        party_combat::HealthBand::Unhurt => "unhurt",
                        party_combat::HealthBand::Bloodied => "bloodied",
                        party_combat::HealthBand::Down => "down",
                    }))
                };
                json!({
                    "id": o.id,
                    "name": o.label,
                    "side": self.space.side_of(&o.id),
                    "steps_away": self.space.steps(actor, &o.id),
                    "engaged": !self.space.engaged_with(&o.id, &alive).is_empty(),
                    "health": health,
                    "down": !o.life.is_alive(),
                    "conditions": derive::conditions(&self.state, &o.id),
                    "concentrating": o.concentration.is_some(),
                })
            })
            .collect();
        let mut adjudicate = Vec::new();
        for o in &options {
            if o.kind != OptKind::Adjudicate {
                continue;
            }
            let Some(a) = o.action.and_then(|i| sheet.map(|s| &s.actions[i])) else {
                continue;
            };
            let numbers: Vec<Number> = numbers_of(self, actor, a);
            adjudicate.push(json!({
                "o": o.o,
                "numbers": numbers,
                "text": { "untrusted_stat_block_text": a.text },
            }));
        }
        let economy = c.map_or(json!({}), |c| {
            json!({
                "action": !c.economy.action,
                "bonus_action": !c.economy.bonus_action,
                "reaction": !c.economy.reaction,
                "move_steps": if self.turn.actor.as_ref() == Some(actor) { self.turn.steps_left } else { 0 },
            })
        });
        let resources: Vec<Value> = c.map_or(vec![], |c| {
            let mut r: Vec<Value> = c
                .spell_slots
                .iter()
                .map(|(l, p)| json!({ "slot": l, "left": p.remaining }))
                .collect();
            r.extend(
                c.resources
                    .values()
                    .map(|x| json!({ "name": x.label, "left": x.pool.remaining })),
            );
            if let Some(lr) = c.legendary_resistance {
                r.push(json!({ "name": "Legendary Resistance", "left": lr.remaining }));
            }
            r
        });
        json!({
            "step": self.step,
            "round": self.state.round,
            "window": match self.window { Window::Turn => "turn", Window::Legendary { .. } => "legendary" },
            "actor": {
                "id": actor,
                "name": c.map(|c| c.label.clone()),
                "side": side,
                "hp": c.map(|c| c.hp.current),
                "max_hp": c.map(|c| c.hp.max),
                "conditions": derive::conditions(&self.state, actor),
                "steps_to_nearest_enemy": self.space.nearest_enemy(actor, &alive).map(|e| self.space.steps(actor, &e)),
                "engaged_with": self.space.engaged_with(actor, &alive),
                "economy": economy,
                "resources": resources,
                "int": sheet.map(|s| s.int),
                "type": sheet.map(|s| s.creature_type.clone()),
                "memo": self.memos.get(actor).map(|m| json!({ "untrusted_own_note": m })),
            },
            "seen": seen,
            "morale": self.morale_facts(actor),
            "recent": self.recent_lines(8),
            "options": options,
            "adjudicate": if adjudicate.is_empty() { Value::Null } else { Value::Array(adjudicate) },
        })
    }

    /// Plain facts about how a side is doing (no hidden number).
    #[must_use]
    pub fn morale_facts(&self, actor: &CombatantId) -> Vec<String> {
        let side = self.space.side_of(actor);
        let mut out = Vec::new();
        for (s, word) in [(side, "allies"), (other(side), "enemies")] {
            let members: Vec<_> = self
                .state
                .combatants
                .iter()
                .filter(|c| !c.removed && self.space.side_of(&c.id) == s && &c.id != actor)
                .collect();
            if members.is_empty() {
                continue;
            }
            let down = members
                .iter()
                .filter(|c| !c.life.is_alive() || !self.space.present(&c.id))
                .count();
            out.push(format!("{down} of {} {word} down or gone", members.len()));
        }
        if let Some(c) = self.state.combatant(actor) {
            if c.hp.band() == Some(party_combat::HealthBand::Bloodied) {
                out.push("you are bloodied".into());
            }
        }
        out
    }

    /// Its side has lost at least half its members, or it is badly hurt (a surrender may be offered).
    #[must_use]
    pub fn morale_low(&self, actor: &CombatantId) -> bool {
        let Some(c) = self.state.combatant(actor) else {
            return false;
        };
        let side = self.space.side_of(actor);
        let members: Vec<_> = self
            .state
            .combatants
            .iter()
            .filter(|o| !o.removed && self.space.side_of(&o.id) == side)
            .collect();
        let down = members.iter().filter(|o| !o.life.is_alive()).count();
        c.hp.current * 4 <= c.hp.max || down * 2 >= members.len()
    }

    /// Validate a move string.
    #[must_use]
    pub fn parse_move(text: &str) -> Option<Move> {
        Move::parse(text)
    }
}

fn generic(o: &str, name: &str, kind: OptKind, cost: &'static str, summary: &str) -> Opt {
    Opt {
        o: o.into(),
        name: name.into(),
        kind,
        cost,
        summary: summary.into(),
        targets: vec![],
        max_targets: 0,
        mode_hint: None,
        action: None,
    }
}

/// The other side.
#[must_use]
pub fn other(s: Side) -> Side {
    match s {
        Side::Party => Side::Foes,
        Side::Foes => Side::Party,
    }
}

/// The expected damage of an action's parts, twice over (for summaries and Basic).
#[must_use]
pub fn avg2_of(a: &ActionDef) -> i32 {
    match &a.def {
        Def::Attack(x) => parts_avg2(&x.parts) * i32::try_from(x.count).unwrap_or(1),
        Def::Save(s) => parts_avg2(&s.parts),
        _ => 0,
    }
}
