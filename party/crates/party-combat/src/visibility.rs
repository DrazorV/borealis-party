//! What each kind of viewer may see: the visible turn order and the turn pointer (D5). Part D's projection
//! (`view_for`) builds on these; it must not read hidden combatants any other way.

use crate::derive::{conditions, turn_order};
use crate::event::{DmEvent, DmEventKind};
use crate::model::{CombatState, Combatant, CombatantId, Controller, Status};
use crate::spell::{CastSpec, Delivery, Primitive, Tier};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The DM, or a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerKind {
    /// Sees everything.
    Dm,
    /// Sees PCs and revealed creatures only.
    Player,
}

/// Whose turn it is, for a viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnPointer {
    /// Not started.
    NotStarted,
    /// Ended.
    Ended,
    /// This combatant's turn, in this round.
    Combatant {
        /// Whose turn.
        id: CombatantId,
        /// The round of that turn.
        round: u32,
    },
    /// Hidden creatures are acting and the encounter shows an "Enemy turn" slot (D5 opt-in).
    EnemyTurn {
        /// The current round.
        round: u32,
    },
    /// Hidden creatures act before any visible combatant has (players see no pointer yet).
    Waiting {
        /// The round players last saw.
        round: u32,
    },
}

/// The combatant is visible to the viewer. The DM sees everyone not removed. Players see PCs, creatures a player
/// controls (their summons and pets), and revealed creatures, if not removed. A DM creature's pet has its own
/// `visible` flag: it stays hidden while hidden, whatever its owner's state (revealing an owner reveals no pets).
#[must_use]
pub fn is_visible(state: &CombatState, id: &CombatantId, viewer: ViewerKind) -> bool {
    state.combatant(id).is_some_and(|c| {
        !c.removed
            && (viewer == ViewerKind::Dm
                || c.visible
                || c.is_character()
                || matches!(c.controller, Controller::Player { .. }))
    })
}

/// The turn order as the viewer may see it. For players, hidden creatures don't exist.
#[must_use]
pub fn visible_order(state: &CombatState, viewer: ViewerKind) -> Vec<CombatantId> {
    turn_order(state)
        .into_iter()
        .filter(|id| is_visible(state, id, viewer))
        .collect()
}

/// Whose turn it is for the viewer. For players the pointer stays on the last visible combatant while hidden
/// creatures act (D5), and its round is the round of that turn, so players can't count hidden turns.
#[must_use]
pub fn turn_pointer(state: &CombatState, viewer: ViewerKind) -> TurnPointer {
    match state.status {
        Status::Setup => return TurnPointer::NotStarted,
        Status::Ended => return TurnPointer::Ended,
        Status::Active => {}
    }
    let Some(current) = &state.current else {
        return TurnPointer::NotStarted;
    };
    if is_visible(state, current, viewer) {
        return TurnPointer::Combatant {
            id: current.clone(),
            round: state.round,
        };
    }
    if state.settings.show_enemy_turn {
        return TurnPointer::EnemyTurn { round: state.round };
    }
    match &state.last_visible {
        Some(mark) if is_visible(state, &mark.id, viewer) => TurnPointer::Combatant {
            id: mark.id.clone(),
            round: mark.round,
        },
        Some(mark) => TurnPointer::Waiting { round: mark.round },
        None => TurnPointer::Waiting { round: 1 },
    }
}

/// Everything players may see of one combatant, or `None` when it is hidden from them. A revealed creature shows
/// its full stat block (D20 b): exact HP, conditions, effects, concentration, resources, and its abilities' text,
/// legendary actions included. DM-only data is left out: legendary-action and Legendary Resistance counts (never
/// shown to players), the DM label, notes, tie-break, effect ids, follow-ups, and any reference to a hidden
/// combatant (an effect's hidden source and its duration, which counts that source's turns; a hidden owner or
/// link). Part D's projection may narrow it further per viewer (D6: other players' PCs show a health band only).
#[must_use]
pub fn player_view_of(state: &CombatState, id: &CombatantId) -> Option<Value> {
    let c = state.combatant(id)?;
    if !is_visible(state, id, ViewerKind::Player) {
        return None;
    }
    let seen = |other: &CombatantId| is_visible(state, other, ViewerKind::Player);
    let mut v = serde_json::to_value(c).ok()?;
    let o = v.as_object_mut()?;
    // An allowlist (0.3.2): a field added later stays DM only until it is classified here.
    o.retain(|k, _| COMBATANT_PLAYER.contains(&k.as_str()));
    if let Some(Value::Array(abilities)) = o.get_mut("abilities") {
        for a in abilities.iter_mut().filter_map(Value::as_object_mut) {
            a.retain(|k, _| ABILITY_PLAYER.contains(&k.as_str()));
        }
    }
    for (field, target) in [("owner_id", &c.owner_id), ("linked_to", &c.linked_to)] {
        if target.as_ref().is_some_and(|t| !seen(t)) {
            o.remove(field);
        }
    }
    if let Controller::Player { pc } = &c.controller {
        if !seen(pc) {
            o.remove("controller");
        }
    }
    let effects: Vec<Value> = state
        .effects
        .iter()
        .filter(|e| &e.spec.target == id)
        .map(|e| {
            let s = &e.spec;
            let source_seen = s.source.as_ref().is_none_or(seen);
            let mut fx = serde_json::json!({
                "name": s.name,
                "detail": s.detail,
                "concentration": s.concentration,
                "conditions": s.conditions,
                "modifiers": s.modifiers,
                "charges": s.charges,
                "repeat_save": s.repeat_save,
            });
            if source_seen {
                fx["source"] = serde_json::to_value(&s.source).unwrap_or(Value::Null);
                fx["duration"] = serde_json::to_value(s.duration).unwrap_or(Value::Null);
            }
            fx
        })
        .collect();
    o.insert("effects".into(), Value::Array(effects));
    o.insert(
        "conditions".into(),
        serde_json::to_value(conditions(state, id)).ok()?,
    );
    Some(v)
}

/// The open cast rows a player answers: saves their own PCs (and its creatures) make against pending casts, each
/// `{cast, target, ray, ability, dc, spell?, from?, resolved}`. The spell and caster are left out when the
/// caster is hidden ("DEX save DC 15 from an unseen creature"). Nothing else of a cast reaches players.
#[must_use]
pub fn player_casts(state: &CombatState, account: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for cast in &state.casts {
        let Delivery::Save { ability, .. } = cast.spec.mech.delivery else {
            continue;
        };
        let seen = is_visible(state, &cast.caster, ViewerKind::Player);
        for r in &cast.rows {
            let mine = state
                .combatant(&r.target)
                .filter(|c| !c.removed)
                .and_then(|c| crate::engine::account_of(state, c))
                .is_some_and(|a| a == account);
            if !mine {
                continue;
            }
            let mut row = serde_json::json!({
                "cast": cast.id,
                "target": r.target,
                "ray": r.ray,
                "ability": ability,
                "dc": cast.spec.dc,
                "resolved": r.outcome.is_some(),
            });
            if seen {
                row["spell"] = Value::String(cast.spec.mech.name.clone());
                row["from"] = serde_json::to_value(&cast.caster).unwrap_or(Value::Null);
            }
            out.push(row);
        }
    }
    out
}

/// The player's own open casts (0.3.3: a player casts from their PC or its creatures, and resolves what is theirs),
/// each `{cast, caster, spell, slot?, delivery: "auto"|"attack"|"save", rows: [{target, ray, outcome?}],
/// amounts: [{kind: "damage"|"heal"|"temp_hp", dice, damage_type?, choice?, add_mod?}], needs_apply}`. Rows on a
/// combatant hidden from players are left out. `amounts` are in the order `apply_cast` takes them; `needs_apply` is
/// true when every row has an outcome and the amounts are still to roll (the player's `apply_cast`).
#[must_use]
pub fn player_own_casts(state: &CombatState, account: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for cast in &state.casts {
        let mine = state
            .combatant(&cast.caster)
            .and_then(|c| crate::engine::account_of(state, c))
            .is_some_and(|a| a == account);
        if !mine {
            continue;
        }
        let m = &cast.spec.mech;
        let delivery = match m.delivery {
            Delivery::Auto => "auto",
            Delivery::Attack { .. } => "attack",
            Delivery::Save { .. } => "save",
        };
        let rows: Vec<Value> = cast
            .rows
            .iter()
            .filter(|r| is_visible(state, &r.target, ViewerKind::Player))
            .map(|r| serde_json::json!({ "target": r.target, "ray": r.ray, "outcome": r.outcome }))
            .collect();
        let dice = |f: &crate::spell::Formula| f.notation(&cast.vars).unwrap_or_default();
        let amounts: Vec<Value> = m
            .on_land
            .iter()
            .chain(&m.on_resist)
            .chain(&m.always)
            .filter_map(|p| match p {
                Primitive::Damage {
                    dice: d,
                    damage_type,
                    choice,
                } => Some(serde_json::json!({
                    "kind": "damage", "dice": dice(d), "damage_type": damage_type, "choice": choice,
                })),
                Primitive::Heal { dice: d, add_mod } => {
                    Some(serde_json::json!({ "kind": "heal", "dice": dice(d), "add_mod": add_mod }))
                }
                Primitive::TempHp { dice: d, add_mod } => Some(
                    serde_json::json!({ "kind": "temp_hp", "dice": dice(d), "add_mod": add_mod }),
                ),
                _ => None,
            })
            .collect();
        let needs_apply = !amounts.is_empty() && cast.rows.iter().all(|r| r.outcome.is_some());
        out.push(serde_json::json!({
            "cast": cast.id,
            "caster": cast.caster,
            "spell": m.name,
            "slot": cast.slot,
            "delivery": delivery,
            "rows": rows,
            "amounts": amounts,
            "needs_apply": needs_apply,
        }));
    }
    out
}

/// Combatant keys players may see (subject to the hidden-reference rules of [`player_view_of`]).
pub const COMBATANT_PLAYER: &[&str] = &[
    "id",
    "kind",
    "owner",
    "public_name",
    "initiative",
    "initiative_bonus",
    "initiative_advantage",
    "absent",
    "linked_to",
    "surprised",
    "owner_id",
    "controller",
    "initiative_mode",
    "hp",
    "hp_authority",
    "zero_hp",
    "life",
    "ac",
    "speed",
    "defenses",
    "exhaustion",
    "concentration",
    "pending_concentration",
    "economy",
    "abilities",
    "spell_slots",
    "resources",
    "group",
];

/// Combatant keys that stay with the DM: the label and notes, bookkeeping, Legendary Resistance counts (user,
/// 2026-11-13: a pool shared with a hidden creature would show its uses), and the 0.3.2 casting data (save
/// bonuses, DCs, links and compiled mechanics).
pub const COMBATANT_DM_ONLY: &[&str] = &[
    "label",
    "notes",
    "tiebreak",
    "ever_revealed",
    "visible",
    "removed",
    "legendary_resistance",
    "stats",
    "casting",
    "spell_turn",
];

/// Ability keys players may see.
pub const ABILITY_PLAYER: &[&str] = &[
    "key",
    "name",
    "kind",
    "text",
    "legendary_cost",
    "recharge",
    "uses",
    "group_limit",
    "requires_effect",
];

/// Ability keys that stay with the DM: the spell link and compiled mechanics.
pub const ABILITY_DM_ONLY: &[&str] = &["spell", "mech"];

/// Effect keys players see, in [`player_view_of`]'s `effects` (`source` and `duration` only when the source is
/// visible).
pub const EFFECT_PLAYER: &[&str] = &[
    "name",
    "detail",
    "concentration",
    "conditions",
    "modifiers",
    "charges",
    "repeat_save",
    "source",
    "duration",
];

/// Effect keys that stay with the DM.
pub const EFFECT_DM_ONLY: &[&str] = &[
    "id",
    "origin",
    "target",
    "ends_on_defeat_of",
    "follow_up",
    "spell",
    "dismiss_on_end",
    "ends_on_damage",
    "ends_on_attack",
];

/// Combat state keys that reach players only through this module's functions (the turn pointer, the visible
/// order, [`player_view_of`] and [`player_casts`]).
pub const STATE_PROJECTED: &[&str] = &[
    "edition",
    "status",
    "round",
    "current",
    "combatants",
    "effects",
];

/// Combat state keys that never reach players: legendary pools (live counts), rules, limits, counters, pending
/// casts (only each player's own rows, through [`player_casts`]) and settings.
pub const STATE_DM_ONLY: &[&str] = &[
    "settings",
    "last_visible",
    "legendary_pools",
    "legendary_per_creature",
    "rules",
    "group_limits",
    "next_effect",
    "next_tiebreak",
    "casts",
    "next_cast",
];

// ----- The DM's side (0.3.3) -----
//
// The user's rules of 2026-11-14: "DM should never know players HP and resources", "when players cast spell it
// should be shown to dm if it applies an effect", and the Combat tab is the DM's screen while the Play tab is the
// player's. This replaces D6's "the DM and the owner see the full card".

/// The DM sees that a player character (or a creature a player controls) is down: its life isn't `alive` (0 HP on
/// the sheet, so dying, stable or dead). Open question for the user (2026-11-14): whether the DM should see "down"
/// at all. With `false`, the DM gets no `down`, no dying/dead/revived events and no "Dying" (Unconscious) effect.
pub const DM_SEES_PLAYER_DOWN: bool = true;

/// A life without its numbers: `alive`, `dying`, `stable` or `dead`.
#[must_use]
pub fn life_state(life: crate::model::Life) -> &'static str {
    use crate::model::Life;
    match life {
        Life::Alive => "alive",
        Life::Dying { .. } => "dying",
        Life::Stable => "stable",
        Life::Dead => "dead",
    }
}

/// The combatant is the player's side: a PC, or a creature a player controls (a summon, a pet, a companion). Its
/// numbers are its player's: the DM never sees them ([`dm_view_of`]).
#[must_use]
pub fn player_owned(c: &Combatant) -> bool {
    c.is_character() || matches!(c.controller, Controller::Player { .. })
}

/// Combatant keys the DM sees of a player's combatant ([`player_owned`]): who it is, its turn, its AC (the DM
/// resolves attacks against it), speed, exhaustion and action economy. `concentration` becomes `true`/`false`, and a
/// `down` flag is added while [`DM_SEES_PLAYER_DOWN`]. The DM's own label, notes and bookkeeping stay.
pub const COMBATANT_DM_OF_PLAYER: &[&str] = &[
    "id",
    "kind",
    "label",
    "public_name",
    "visible",
    "ever_revealed",
    "removed",
    "initiative",
    "tiebreak",
    "absent",
    "linked_to",
    "surprised",
    "owner_id",
    "controller",
    "initiative_mode",
    "ac",
    "speed",
    "exhaustion",
    "concentration",
    "economy",
    "group",
    "notes",
];

/// Combatant keys of a player's combatant the DM never sees: HP, how it drops at 0, its life (death saves),
/// defenses and save bonuses from the sheet, concentration checks (the DC tells the damage taken), its abilities
/// (uses and recharges), slots, resources and casting data, and its initiative bonus.
pub const COMBATANT_PLAYER_PRIVATE: &[&str] = &[
    "hp",
    "hp_authority",
    "zero_hp",
    "life",
    "initiative_bonus",
    "initiative_advantage",
    "defenses",
    "pending_concentration",
    "abilities",
    "legendary_resistance",
    "spell_slots",
    "resources",
    "stats",
    "casting",
    "spell_turn",
];

/// The combat state as the DM sees it: everything, except the numbers of the player's side. Every combatant of a
/// player ([`player_owned`]) keeps only [`COMBATANT_DM_OF_PLAYER`] (an allowlist: a field added later stays out until
/// it is classified), and player casts with nothing the DM sees ([`cast_private`]) are left out.
#[must_use]
pub fn dm_view_of(state: &CombatState) -> Value {
    let mut v = serde_json::to_value(state).unwrap_or(Value::Null);
    if let Some(Value::Array(list)) = v.get_mut("combatants") {
        for (entry, c) in list.iter_mut().zip(&state.combatants) {
            if !player_owned(c) {
                continue;
            }
            let Some(o) = entry.as_object_mut() else {
                continue;
            };
            o.retain(|k, _| COMBATANT_DM_OF_PLAYER.contains(&k.as_str()));
            o.insert(
                "concentration".into(),
                Value::Bool(c.concentration.is_some()),
            );
            if DM_SEES_PLAYER_DOWN {
                o.insert("down".into(), Value::Bool(!c.life.is_alive()));
            }
        }
    }
    if !DM_SEES_PLAYER_DOWN {
        if let Some(Value::Array(list)) = v.get_mut("effects") {
            let mut keep = state.effects.iter().map(|e| {
                e.origin != crate::model::EffectOrigin::Dying
                    || state
                        .combatant(&e.spec.target)
                        .is_none_or(|c| !player_owned(c))
            });
            list.retain(|_| keep.next().unwrap_or(true));
        }
    }
    if let Some(Value::Array(list)) = v.get_mut("casts") {
        let mut keep = state.casts.iter().map(|c| !c.private);
        list.retain(|_| keep.next().unwrap_or(true));
        if list.is_empty() {
            if let Some(o) = v.as_object_mut() {
                o.remove("casts");
            }
        }
    }
    v
}

/// The combat state with the player's side's numbers reset (0.3.3), for what the DM takes away whole: the combat
/// export (D45), which must still import. Every [`COMBATANT_PLAYER_PRIVATE`] field of a player's combatant goes back
/// to its empty value (HP 0/0, no slots, resources, abilities or casting data), except who writes its HP and its 0 HP
/// rule (no numbers, and the import needs them). A down PC is `stable` (down, no death saves) while
/// [`DM_SEES_PLAYER_DOWN`], `alive` otherwise. Private casts are left out. The PCs' HP comes back from their sheets
/// when the import's party plays.
#[must_use]
pub fn redacted_for_dm(state: &CombatState) -> CombatState {
    let mut s = state.clone();
    for c in s.combatants.iter_mut().filter(|c| player_owned(c)) {
        c.hp = crate::model::Hp::default();
        c.life = if DM_SEES_PLAYER_DOWN && !c.life.is_alive() {
            crate::model::Life::Stable
        } else {
            crate::model::Life::Alive
        };
        c.initiative_bonus = 0;
        c.initiative_advantage = false;
        c.defenses = crate::model::Defenses::default();
        c.pending_concentration.clear();
        c.abilities.clear();
        c.legendary_resistance = None;
        c.spell_slots.clear();
        c.resources.clear();
        c.stats = None;
        c.casting.clear();
        c.spell_turn = None;
    }
    s.casts.retain(|c| !c.private);
    s
}

/// DM event kinds that are a number of the player's side when they are about a player's combatant: HP, temporary
/// HP, rolled HP, death saves, slots and resources, its rolls, concentration checks, recharges, and damage or
/// healing applied to it (a prompt the DM sent stays: the DM knows what it sent). Dying, death and revival too,
/// unless [`DM_SEES_PLAYER_DOWN`].
#[must_use]
pub fn private_kind(kind: &DmEventKind) -> bool {
    use DmEventKind as K;
    match kind {
        K::HpSet
        | K::TempHp
        | K::DeathSave { .. }
        | K::Resources
        | K::HpRolled { .. }
        | K::Rolled { .. }
        | K::ConcentrationCheck { .. }
        | K::ConcentrationSave { .. }
        | K::Recharge { .. }
        | K::RechargeDue { .. }
        | K::RechargeSet => true,
        K::Damage { prompted, .. } | K::Healed { prompted, .. } => !prompted,
        K::Dying | K::Died | K::Revived | K::Stabilized => !DM_SEES_PLAYER_DOWN,
        _ => false,
    }
}

/// Whether the DM may see a logged event: not [`DmEvent::private`], and not a [`private_kind`] about a player's
/// combatant (this also covers events logged before 0.3.3, which carry no flag).
#[must_use]
pub fn dm_may_see(state: &CombatState, ev: &DmEvent) -> bool {
    if ev.private {
        return false;
    }
    let player_side = ev
        .subject
        .as_ref()
        .and_then(|id| state.combatant(id))
        .is_some_and(player_owned);
    !(player_side && private_kind(&ev.kind))
}

/// A player's cast the DM never hears of (0.3.3, the user's rule "when players cast spell it should be shown to dm
/// if it applies an effect"): cast by the player's side, on the player's side only, fully automated or a utility
/// spell, no concentration, and nothing but damage, healing, temporary HP and notes (no condition, effect, zone,
/// summon or ruling). Shield on yourself is an AC effect the DM resolves attacks against, so it is shown; Cure
/// Wounds on an ally isn't.
#[must_use]
pub fn cast_private(
    state: &CombatState,
    caster: &CombatantId,
    spec: &CastSpec,
    targets: &[CombatantId],
) -> bool {
    let m = &spec.mech;
    let player_side = |id: &CombatantId| state.combatant(id).is_some_and(player_owned);
    player_side(caster)
        && !m.concentration
        && matches!(m.tier, Tier::Auto | Tier::Utility)
        && targets.iter().all(player_side)
        && m.on_land
            .iter()
            .chain(&m.on_resist)
            .chain(&m.always)
            .all(|p| {
                matches!(
                    p,
                    Primitive::Damage { .. }
                        | Primitive::Heal { .. }
                        | Primitive::TempHp { .. }
                        | Primitive::Note { .. }
                )
            })
}

/// A coarse summary of everything players may see. Two states with the same summary are the same to players, so
/// a command whose before and after summaries match produces no player events.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlayerSummary {
    status: Status,
    pointer: TurnPointer,
    order: Vec<CombatantId>,
    seen: BTreeMap<CombatantId, Value>,
}

impl PlayerSummary {
    pub(crate) fn of(state: &CombatState) -> Self {
        let seen = state
            .combatants
            .iter()
            .filter_map(|c| player_view_of(state, &c.id).map(|v| (c.id.clone(), v)))
            .collect();
        Self {
            status: state.status,
            pointer: turn_pointer(state, ViewerKind::Player),
            order: visible_order(state, ViewerKind::Player),
            seen,
        }
    }

    /// The player events that turn `self` into `after`.
    pub(crate) fn diff(&self, after: &Self) -> Vec<crate::event::PlayerEvent> {
        use crate::event::PlayerEvent as P;
        let mut out = Vec::new();
        if self.status != after.status {
            match after.status {
                Status::Active => out.push(P::Started),
                Status::Ended => out.push(P::Ended),
                Status::Setup => {}
            }
        }
        for id in self.seen.keys().filter(|id| !after.seen.contains_key(*id)) {
            out.push(P::Disappeared { id: id.clone() });
        }
        for (id, seen) in &after.seen {
            match self.seen.get(id) {
                None => out.push(P::Appeared { id: id.clone() }),
                Some(before) if before != seen => out.push(P::Changed { id: id.clone() }),
                Some(_) => {}
            }
        }
        if self.order != after.order {
            out.push(P::Order);
        }
        if self.pointer != after.pointer {
            out.push(P::Turn {
                pointer: after.pointer.clone(),
            });
        }
        out
    }
}
