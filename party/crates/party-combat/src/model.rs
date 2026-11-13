//! The serializable combat state: [`CombatState`] and everything in it.
//!
//! Every type here is plain data. The rules that change it live in [`crate::engine`]; the read-only derived
//! values (turn order, effective AC and speed, the conditions on a combatant) live in [`crate::derive`] and
//! [`crate::visibility`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// A combatant's id. The server makes them random UUIDs, so ids never tell players how many creatures exist.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CombatantId(pub String);

impl CombatantId {
    /// An id from text.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl fmt::Display for CombatantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for CombatantId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// An active effect's id, unique within one combat. It counts up, so a player projection must not show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectId(pub u32);

/// The rules edition a combat runs under.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Edition {
    /// 2014 rules (SRD 5.1).
    #[serde(rename = "2014")]
    Srd51,
    /// 2024 rules (SRD 5.2.1).
    #[default]
    #[serde(rename = "2024")]
    Srd521,
}

macro_rules! lowercase_enum {
    ($(#[$m:meta])* $name:ident { $( $(#[$vm:meta])* $variant:ident => $text:literal ),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name { $( $(#[$vm])* #[serde(rename = $text)] $variant ),+ }

        impl $name {
            /// Every value in declaration order.
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];

            /// The serialized name.
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $( $name::$variant => $text ),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.as_str()) }
        }
    };
}

lowercase_enum! {
    /// A damage type. Serialized exactly as `borealis_content::monster_types::DamageType`.
    DamageType {
        /// Acid.
        Acid => "acid",
        /// Bludgeoning.
        Bludgeoning => "bludgeoning",
        /// Cold.
        Cold => "cold",
        /// Fire.
        Fire => "fire",
        /// Force.
        Force => "force",
        /// Lightning.
        Lightning => "lightning",
        /// Necrotic.
        Necrotic => "necrotic",
        /// Piercing.
        Piercing => "piercing",
        /// Poison.
        Poison => "poison",
        /// Psychic.
        Psychic => "psychic",
        /// Radiant.
        Radiant => "radiant",
        /// Slashing.
        Slashing => "slashing",
        /// Thunder.
        Thunder => "thunder",
    }
}

lowercase_enum! {
    /// An SRD condition. Exhaustion has levels and is [`Combatant::exhaustion`] instead.
    Condition {
        /// Blinded.
        Blinded => "blinded",
        /// Charmed.
        Charmed => "charmed",
        /// Deafened.
        Deafened => "deafened",
        /// Frightened.
        Frightened => "frightened",
        /// Grappled.
        Grappled => "grappled",
        /// Incapacitated.
        Incapacitated => "incapacitated",
        /// Invisible.
        Invisible => "invisible",
        /// Paralyzed.
        Paralyzed => "paralyzed",
        /// Petrified.
        Petrified => "petrified",
        /// Poisoned.
        Poisoned => "poisoned",
        /// Prone.
        Prone => "prone",
        /// Restrained.
        Restrained => "restrained",
        /// Stunned.
        Stunned => "stunned",
        /// Unconscious.
        Unconscious => "unconscious",
    }
}

impl Condition {
    /// Incapacitated, or a condition that includes it (both editions). These end concentration (C10) and stop
    /// actions and reactions.
    #[must_use]
    pub fn incapacitates(self) -> bool {
        matches!(
            self,
            Condition::Incapacitated
                | Condition::Paralyzed
                | Condition::Petrified
                | Condition::Stunned
                | Condition::Unconscious
        )
    }

    /// The condition makes the creature's speed 0 (or it can't move).
    #[must_use]
    pub fn stops_movement(self) -> bool {
        matches!(
            self,
            Condition::Grappled
                | Condition::Restrained
                | Condition::Paralyzed
                | Condition::Petrified
                | Condition::Stunned
                | Condition::Unconscious
        )
    }
}

lowercase_enum! {
    /// An ability, for saving throws.
    AbilityName {
        /// Strength.
        Str => "str",
        /// Dexterity.
        Dex => "dex",
        /// Constitution.
        Con => "con",
        /// Intelligence.
        Int => "int",
        /// Wisdom.
        Wis => "wis",
        /// Charisma.
        Cha => "cha",
    }
}

/// One clause of a damage immunity, resistance or vulnerability list. Same shape as
/// `borealis_content::monster_types::DamageClause`, so a stat block's clauses deserialize into it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DamageClause {
    /// The damage types the clause names.
    #[serde(default)]
    pub types: Vec<DamageType>,
    /// The clause verbatim when it is more than a plain list of types ("from nonmagical attacks"). Shown to the DM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// When a clause applies, read from its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseScope {
    /// A plain list: always applies.
    Always,
    /// "From nonmagical attacks": applies unless the damage is magical.
    Nonmagical,
    /// Any other qualifier: applies only when the DM says so (`apply_qualified` on the damage command).
    Qualified,
}

impl DamageClause {
    /// A plain list.
    #[must_use]
    pub fn plain(types: &[DamageType]) -> Self {
        Self {
            types: types.to_vec(),
            text: None,
        }
    }

    /// A clause with qualifier text.
    #[must_use]
    pub fn qualified(types: &[DamageType], text: &str) -> Self {
        Self {
            types: types.to_vec(),
            text: Some(text.to_string()),
        }
    }

    /// When the clause applies.
    #[must_use]
    pub fn scope(&self) -> ClauseScope {
        match &self.text {
            None => ClauseScope::Always,
            Some(t) if t.to_lowercase().contains("nonmagical") => ClauseScope::Nonmagical,
            Some(_) => ClauseScope::Qualified,
        }
    }
}

/// Damage immunities, resistances and vulnerabilities, and condition immunities.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defenses {
    /// Damage immunities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub immunities: Vec<DamageClause>,
    /// Damage resistances.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resistances: Vec<DamageClause>,
    /// Damage vulnerabilities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vulnerabilities: Vec<DamageClause>,
    /// Condition immunities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub condition_immunities: Vec<Condition>,
}

/// What a combatant is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CombatantKind {
    /// A monster or NPC run by the DM.
    Creature,
    /// A player character, owned by an account.
    Character {
        /// The owning account (the server's account id).
        owner: String,
    },
}

/// Who writes a combatant's HP.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HpAuthority {
    /// The engine changes HP directly (creatures).
    #[default]
    Engine,
    /// The player's own character file holds HP (D7). Damage becomes a prompt to the player, and the server
    /// reports the result back with [`crate::Command::SyncPlayerHp`].
    Player,
}

/// What happens at 0 HP.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroHpRule {
    /// The creature dies (most monsters).
    #[default]
    Dies,
    /// The creature falls unconscious and makes death saves (PCs, and NPCs the DM chooses).
    DeathSaves,
}

/// Alive, dying, stable or dead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Life {
    /// Above 0 HP.
    #[default]
    Alive,
    /// At 0 HP, making death saves.
    Dying {
        /// Successes so far (0..=2).
        successes: u8,
        /// Failures so far (0..=2).
        failures: u8,
    },
    /// At 0 HP, stable.
    Stable,
    /// Dead (a creature with [`ZeroHpRule::Dies`] at 0 HP, three failed death saves, or massive damage).
    Dead,
}

impl Life {
    /// Up and acting.
    #[must_use]
    pub fn is_alive(self) -> bool {
        self == Life::Alive
    }

    /// Dead: skipped in the turn order (C4).
    #[must_use]
    pub fn is_dead(self) -> bool {
        self == Life::Dead
    }
}

/// Hit points.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hp {
    /// Current HP (0..=max).
    pub current: i32,
    /// Maximum HP.
    pub max: i32,
    /// Temporary HP.
    #[serde(default)]
    pub temp: i32,
}

/// The health band players may see (D6): the same rule as the party hub's card band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthBand {
    /// Above half.
    Unhurt,
    /// Half or less.
    Bloodied,
    /// 0 HP.
    Down,
}

impl Hp {
    /// The band, or `None` when max HP is unknown (0).
    #[must_use]
    pub fn band(&self) -> Option<HealthBand> {
        if self.max <= 0 {
            None
        } else if self.current <= 0 {
            Some(HealthBand::Down)
        } else if self.current.saturating_mul(2) <= self.max {
            Some(HealthBand::Bloodied)
        } else {
            Some(HealthBand::Unhurt)
        }
    }
}

/// Speeds in feet. A missing mode means the creature doesn't have it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Speeds {
    /// Walk.
    #[serde(default)]
    pub walk: u32,
    /// Fly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fly: Option<u32>,
    /// Swim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swim: Option<u32>,
    /// Climb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub climb: Option<u32>,
    /// Burrow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burrow: Option<u32>,
    /// Hover.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hover: bool,
}

/// The turn's action economy (C5): `true` means spent. Reset at the start of the combatant's turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Economy {
    /// Action spent.
    #[serde(default)]
    pub action: bool,
    /// Bonus action spent.
    #[serde(default)]
    pub bonus_action: bool,
    /// Reaction spent.
    #[serde(default)]
    pub reaction: bool,
}

/// A counter with a maximum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pool {
    /// The maximum.
    pub max: u32,
    /// What's left.
    pub remaining: u32,
}

impl Pool {
    /// A full pool.
    #[must_use]
    pub fn full(max: u32) -> Self {
        Self {
            max,
            remaining: max,
        }
    }
}

/// When a resource refills.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reset {
    /// At the start of the owner's turn.
    TurnStart,
    /// At the start of each round.
    RoundStart,
    /// Never during combat (per day, per rest): restored by the DM.
    #[default]
    Never,
}

/// A per-day or per-turn resource (C18): "3/day", a monster's at-will list marker, a ward's uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    /// Shown to the DM.
    pub label: String,
    /// Uses.
    pub pool: Pool,
    /// When it refills.
    #[serde(default)]
    pub reset: Reset,
}

/// Which part of a stat block an ability comes from, and so what it spends (C14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbilityKind {
    /// Spends the action.
    Action,
    /// Spends the bonus action.
    BonusAction,
    /// Spends the reaction.
    Reaction,
    /// Spends legendary actions from the combatant's pool.
    Legendary,
    /// A lair action or other free use: spends nothing from the economy.
    Free,
    /// A trait: nothing to use.
    Passive,
}

/// "Recharge 5–6": ready, or waiting for a d6 of `min` or more (C15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recharge {
    /// The lowest d6 that recharges it (5 for "Recharge 5–6", 6 for "Recharge 6").
    pub min: u8,
    /// Ready to use.
    pub ready: bool,
}

/// One usable entry of a stat block (C14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ability {
    /// Unique per combatant; encounter rules and requests refer to it.
    pub key: String,
    /// Shown to the DM.
    pub name: String,
    /// What it spends.
    pub kind: AbilityKind,
    /// The rules text, from the stat block snapshot.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Legendary action cost (for [`AbilityKind::Legendary`]; 1 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legendary_cost: Option<u32>,
    /// A recharge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recharge: Option<Recharge>,
    /// The [`Combatant::resources`] key it spends one use of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uses: Option<String>,
    /// A group tag: the group's per-round limit applies to it (C21).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_limit: Option<String>,
    /// Usable only while an effect of this name is on the combatant, or it concentrates on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_effect: Option<String>,
}

/// Concentration on one spell or effect (C11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concentration {
    /// What it concentrates on.
    pub name: String,
}

/// A concentration check waiting for its save (C11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcentrationCheck {
    /// The Constitution save DC.
    pub dc: u32,
    /// The damage that caused it.
    pub damage: i32,
}

/// One combatant: a creature or a PC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Combatant {
    /// Unique in the combat.
    pub id: CombatantId,
    /// Creature or character.
    #[serde(flatten)]
    pub kind: CombatantKind,
    /// The DM's label ("Goblin 3"). DM only.
    pub label: String,
    /// The name players see once revealed. For PCs, the character name.
    pub public_name: String,
    /// Players can see it. PCs are always visible.
    pub visible: bool,
    /// It was visible at some point (a hide after reveal warns the DM).
    #[serde(default)]
    pub ever_revealed: bool,
    /// Removed mid-fight (C7). Kept as a tombstone so the turn order stays anchored; never shown.
    #[serde(default)]
    pub removed: bool,

    /// Initiative total; `None` = not rolled. A PC with no initiative is not in this fight (C1).
    #[serde(default)]
    pub initiative: Option<i32>,
    /// The initiative modifier (2024: the stat block's Initiative; 2014: DEX). Also the first tie-break.
    #[serde(default)]
    pub initiative_bonus: i32,
    /// Rolls initiative with advantage.
    #[serde(default)]
    pub initiative_advantage: bool,
    /// The last tie-break: lower acts first. The DM can swap tied combatants.
    #[serde(default)]
    pub tiebreak: u32,
    /// Not in this fight (C1): skipped and not shown in the order.
    #[serde(default)]
    pub absent: bool,
    /// Acts right after this combatant, at its initiative (C3: a mount after its rider). Cleared when that
    /// combatant goes down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_to: Option<CombatantId>,
    /// Surprised at the start of combat (2014: no turn in round 1; 2024: initiative with disadvantage).
    #[serde(default)]
    pub surprised: bool,

    /// Hit points (a mirror of the character file for PCs).
    pub hp: Hp,
    /// Who writes HP.
    #[serde(default)]
    pub hp_authority: HpAuthority,
    /// What happens at 0 HP.
    #[serde(default)]
    pub zero_hp: ZeroHpRule,
    /// Alive, dying, stable or dead.
    #[serde(default)]
    pub life: Life,
    /// Base armor class.
    pub ac: i32,
    /// Base speeds.
    #[serde(default)]
    pub speed: Speeds,
    /// Immunities, resistances and vulnerabilities.
    #[serde(default)]
    pub defenses: Defenses,
    /// Exhaustion level, 0..=6.
    #[serde(default)]
    pub exhaustion: u8,

    /// Current concentration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concentration: Option<Concentration>,
    /// Concentration saves still to make.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_concentration: Vec<ConcentrationCheck>,

    /// This turn's action economy.
    #[serde(default)]
    pub economy: Economy,
    /// Usable stat block entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub abilities: Vec<Ability>,
    /// Legendary Resistance uses (C17).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legendary_resistance: Option<Pool>,
    /// Spell slots by level (C18).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spell_slots: BTreeMap<u8, Pool>,
    /// Other resources by key (C18).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub resources: BTreeMap<String, Resource>,
    /// A group tag (C21: "cultists").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The DM's notes. DM only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

impl Combatant {
    /// A creature with its HP, AC and initiative modifier; hidden until revealed.
    #[must_use]
    pub fn creature(
        id: impl Into<String>,
        label: impl Into<String>,
        hp: i32,
        ac: i32,
        initiative_bonus: i32,
    ) -> Self {
        let label = label.into();
        Self {
            id: CombatantId::new(id),
            kind: CombatantKind::Creature,
            public_name: label.clone(),
            label,
            visible: false,
            ever_revealed: false,
            removed: false,
            initiative: None,
            initiative_bonus,
            initiative_advantage: false,
            tiebreak: 0,
            absent: false,
            linked_to: None,
            surprised: false,
            hp: Hp {
                current: hp,
                max: hp,
                temp: 0,
            },
            hp_authority: HpAuthority::Engine,
            zero_hp: ZeroHpRule::Dies,
            life: Life::Alive,
            ac,
            speed: Speeds {
                walk: 30,
                ..Speeds::default()
            },
            defenses: Defenses::default(),
            exhaustion: 0,
            concentration: None,
            pending_concentration: vec![],
            economy: Economy::default(),
            abilities: vec![],
            legendary_resistance: None,
            spell_slots: BTreeMap::new(),
            resources: BTreeMap::new(),
            group: None,
            notes: String::new(),
        }
    }

    /// A player character owned by `owner`: visible, death saves at 0 HP, HP written by the player (D7).
    #[must_use]
    pub fn character(
        id: impl Into<String>,
        owner: impl Into<String>,
        name: impl Into<String>,
        hp: Hp,
        ac: i32,
        initiative_bonus: i32,
    ) -> Self {
        let mut c = Self::creature(id, name, 0, ac, initiative_bonus);
        c.kind = CombatantKind::Character {
            owner: owner.into(),
        };
        c.visible = true;
        c.ever_revealed = true;
        c.hp = hp;
        c.hp_authority = HpAuthority::Player;
        c.zero_hp = ZeroHpRule::DeathSaves;
        c.life = if hp.current > 0 {
            Life::Alive
        } else {
            Life::Stable
        };
        c
    }

    /// The owning account, for a PC.
    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        match &self.kind {
            CombatantKind::Character { owner } => Some(owner),
            CombatantKind::Creature => None,
        }
    }

    /// A PC.
    #[must_use]
    pub fn is_character(&self) -> bool {
        matches!(self.kind, CombatantKind::Character { .. })
    }

    /// Skipped by the turn order: dead or removed (C4).
    #[must_use]
    pub fn is_defeated(&self) -> bool {
        self.removed || self.life.is_dead()
    }

    /// The ability with `key`.
    #[must_use]
    pub fn ability(&self, key: &str) -> Option<&Ability> {
        self.abilities.iter().find(|a| a.key == key)
    }
}

/// A turn boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    /// The start of a turn.
    Start,
    /// The end of a turn.
    End,
}

/// How long an effect lasts (C12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Duration {
    /// Until the DM removes it (or a charge, save, concentration or defeat ends it).
    #[default]
    Manual,
    /// Counted in the SOURCE's turns: one is used at each `at` boundary of the source's turn. "1 minute" is 10.
    SourceTurns {
        /// Turns left.
        remaining: u32,
        /// Counted at the start or end of the source's turn.
        at: Boundary,
    },
    /// Counted in the TARGET's turns ("until the end of its next turn" is 1 at the end).
    TargetTurns {
        /// Turns left.
        remaining: u32,
        /// Counted at the start or end of the target's turn.
        at: Boundary,
    },
}

/// Cover (C13, C22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cover {
    /// +2 AC and DEX saves.
    Half,
    /// +5 AC and DEX saves.
    ThreeQuarters,
}

impl Cover {
    /// The AC bonus.
    #[must_use]
    pub fn ac_bonus(self) -> i32 {
        match self {
            Cover::Half => 2,
            Cover::ThreeQuarters => 5,
        }
    }
}

/// A simple modifier an effect carries (C13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Modifier {
    /// +N (or −N) AC.
    Ac(i32),
    /// +N feet to every speed the creature has.
    SpeedAdd(i32),
    /// Every speed times N (2 for "double").
    SpeedMultiply(u32),
    /// Every speed halved (rounded down).
    SpeedHalve,
    /// Speed 0.
    SpeedZero,
    /// Cover: the best cover counts, covers don't stack.
    Cover(Cover),
}

/// A save repeated at a turn boundary of the target; a success ends the effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeatSave {
    /// The saving throw.
    pub ability: AbilityName,
    /// The DC.
    pub dc: u32,
    /// Made at the start or end of the target's turn.
    pub at: Boundary,
}

/// What [`crate::Command::AddEffect`] takes: an effect before it gets its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectSpec {
    /// Shown to the DM ("Bless", "Shield of Faith").
    pub name: String,
    /// Who made it: its turns count the duration, and its concentration holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CombatantId>,
    /// Who it is on.
    pub target: CombatantId,
    /// The rules text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// How long it lasts.
    #[serde(default)]
    pub duration: Duration,
    /// Uses left (a ward with N uses); it ends when the last is spent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charges: Option<u32>,
    /// Held by the source's concentration: ends when it ends (the cascade).
    #[serde(default)]
    pub concentration: bool,
    /// Conditions it gives the target while it lasts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
    /// AC and speed modifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<Modifier>,
    /// A repeated save that ends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_save: Option<RepeatSave>,
    /// It ends when any of these goes down (C22: a mount's cover for its rider).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ends_on_defeat_of: Vec<CombatantId>,
    /// Added when this effect ends on its own (expiry, charges, save, concentration, defeat), but not when the DM
    /// removes it with `skip_follow_up`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up: Option<Box<EffectSpec>>,
}

impl EffectSpec {
    /// An effect with a name and a target; everything else empty.
    #[must_use]
    pub fn new(name: impl Into<String>, target: impl Into<CombatantId>) -> Self {
        Self {
            name: name.into(),
            source: None,
            target: target.into(),
            detail: String::new(),
            duration: Duration::Manual,
            charges: None,
            concentration: false,
            conditions: vec![],
            modifiers: vec![],
            repeat_save: None,
            ends_on_defeat_of: vec![],
            follow_up: None,
        }
    }
}

impl From<String> for CombatantId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Why an effect was put on: by a command, or by the 0 HP rule (the Unconscious of a dying creature).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectOrigin {
    /// Added by a command.
    #[default]
    Command,
    /// Added by the 0 HP rule; removed when the creature regains HP.
    Dying,
}

/// An active effect (C12) or condition (C10) on a combatant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    /// Unique in the combat.
    pub id: EffectId,
    /// What it is.
    #[serde(flatten)]
    pub spec: EffectSpec,
    /// Why it is there.
    #[serde(default)]
    pub origin: EffectOrigin,
}

/// A legendary action pool, which several combatants can share (C16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegendaryPool {
    /// Actions per round.
    pub max: u32,
    /// Actions left.
    pub remaining: u32,
    /// The members, in reset order: the pool refills at the start of the turn of the first member that isn't
    /// defeated.
    pub members: Vec<CombatantId>,
}

/// Which abilities an encounter rule switches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum RuleTarget {
    /// One combatant.
    Combatant(CombatantId),
    /// Every combatant with this group tag.
    Group(String),
}

/// One on/off flag of an encounter rule (D42).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleFlag {
    /// Whose ability.
    pub target: RuleTarget,
    /// The ability key.
    pub ability: String,
    /// `true`: usable only while the rule is active. `false`: usable only once it has ended.
    pub while_active: bool,
}

/// An encounter-wide rule tied to a creature's life, shown as a banner to the DM (C20, D42). Not a rules engine:
/// a text plus ability flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncounterRule {
    /// Unique in the combat.
    pub id: String,
    /// The banner title.
    pub name: String,
    /// The banner text while active.
    #[serde(default)]
    pub text_active: String,
    /// The banner text once ended.
    #[serde(default)]
    pub text_ended: String,
    /// The rule ends for good when this combatant goes down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<CombatantId>,
    /// Active now.
    pub active: bool,
    /// Ability switches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<RuleFlag>,
}

/// A group's per-round limit (C21: "only one cultist may use the breath each round").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupLimit {
    /// Uses per round.
    pub per_round: u32,
    /// Used this round.
    pub used: u32,
}

/// Combat settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Show players an "Enemy turn" slot while hidden creatures act (D5 opt-in; it tells them hidden creatures
    /// exist).
    #[serde(default)]
    pub show_enemy_turn: bool,
    /// Roll creatures' recharges at the start of their turns with the server's die (D21). When off, the turn
    /// start reminds the DM and the DM types the d6 (D43).
    #[serde(default = "yes")]
    pub auto_recharge: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_enemy_turn: false,
            auto_recharge: true,
        }
    }
}

/// Setup, running or ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Before the first turn.
    #[default]
    Setup,
    /// Running.
    Active,
    /// Ended.
    Ended,
}

/// The last turn players saw: the turn pointer stays here while hidden creatures act (D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisibleMark {
    /// The combatant.
    pub id: CombatantId,
    /// The round of that turn.
    pub round: u32,
}

/// The whole combat state. Plain data: serializable, comparable, and the unit of undo (C8) and export (D45).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatState {
    /// The rules edition.
    pub edition: Edition,
    /// Settings.
    #[serde(default)]
    pub settings: Settings,
    /// Setup, active or ended.
    #[serde(default)]
    pub status: Status,
    /// The round (0 before the start).
    #[serde(default)]
    pub round: u32,
    /// Whose turn it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<CombatantId>,
    /// The last turn of a visible combatant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_visible: Option<VisibleMark>,
    /// Every combatant, in the order added (removed ones stay as tombstones).
    #[serde(default)]
    pub combatants: Vec<Combatant>,
    /// Active effects and conditions.
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// Legendary action pools by id.
    #[serde(default)]
    pub legendary_pools: BTreeMap<String, LegendaryPool>,
    /// Encounter rules.
    #[serde(default)]
    pub rules: Vec<EncounterRule>,
    /// Group per-round limits by tag.
    #[serde(default)]
    pub group_limits: BTreeMap<String, GroupLimit>,
    /// The next effect id.
    #[serde(default)]
    pub next_effect: u32,
    /// The next tie-break number.
    #[serde(default)]
    pub next_tiebreak: u32,
}

impl CombatState {
    /// An empty combat under `edition`.
    #[must_use]
    pub fn new(edition: Edition) -> Self {
        Self {
            edition,
            settings: Settings::default(),
            status: Status::Setup,
            round: 0,
            current: None,
            last_visible: None,
            combatants: vec![],
            effects: vec![],
            legendary_pools: BTreeMap::new(),
            rules: vec![],
            group_limits: BTreeMap::new(),
            next_effect: 1,
            next_tiebreak: 0,
        }
    }

    /// The combatant with `id`, removed ones included.
    #[must_use]
    pub fn combatant(&self, id: &CombatantId) -> Option<&Combatant> {
        self.combatants.iter().find(|c| &c.id == id)
    }

    pub(crate) fn combatant_mut(&mut self, id: &CombatantId) -> Option<&mut Combatant> {
        self.combatants.iter_mut().find(|c| &c.id == id)
    }

    /// The effect with `id`.
    #[must_use]
    pub fn effect(&self, id: EffectId) -> Option<&Effect> {
        self.effects.iter().find(|e| e.id == id)
    }
}
