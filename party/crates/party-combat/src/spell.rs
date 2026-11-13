//! Spell mechanics (0.3.2): a closed, versioned description of what a spell does in combat, with no scripting.
//!
//! - [`SpellMech`] is the content: one spell in one edition (`v: 1`, `deny_unknown_fields`). Curated SRD rows, the
//!   DM's own definitions and statblock features all use it.
//! - [`CastSpec`] is a [`SpellMech`] compiled on the server for one caster: its DC, attack bonus, spellcasting
//!   modifier, cantrip tier and where the definition came from. It is embedded in the cast command, so replay,
//!   undo, export and the simulation never depend on later edits of the definition.
//! - [`Formula`] is a small grammar over dice notation with four variables: `slot`, `slot_above`, `mod` and
//!   `cantrip_tier` (`"8d6+(slot_above)d6"`, `"1+slot_above"`, `"2d8+(2*slot_above)d8"`).
//! - [`Primitive`]s are what a spell does on landing, on a resisted save, or always.
//!
//! The engine never reads rules text: a spell without a definition is a ruling (the slot is spent and the DM
//! decides). Summons never spawn creatures in 0.3.2: [`Primitive::Summon`] is an info payload for the DM's
//! "Add creature" dialog (user decision 2026-11-13).

use crate::model::{AbilityName, Boundary, Condition, DamageType, Edition};
use party_dice::{DiceExpr, Sign, Term};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// The schema version of [`SpellMech`] and [`CastSpec`].
pub const MECH_VERSION: u32 = 1;

fn v1() -> u32 {
    MECH_VERSION
}

fn one() -> Formula {
    Formula::constant(1)
}

// ---------------------------------------------------------------------------------------------------------------
// Formula
// ---------------------------------------------------------------------------------------------------------------

/// The variables a [`Formula`] may use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormulaVars {
    /// The slot level the spell is cast with (0 for a cantrip or a slotless use).
    pub slot: u8,
    /// Levels above the spell's own level (0 at its base level).
    pub slot_above: u8,
    /// The caster's spellcasting ability modifier.
    #[serde(rename = "mod")]
    pub modifier: i32,
    /// The cantrip tier: 1 (levels 1–4), 2 (5–10), 3 (11–16) or 4 (17+).
    pub cantrip_tier: u8,
}

/// Why a formula is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormulaError {
    /// Not in the grammar.
    #[error("Bad formula {0:?}: {1}")]
    Syntax(String, String),
    /// Dice where a number is needed (a target count, a bonus).
    #[error("Formula {0:?} has dice where a number is needed")]
    DiceNotAllowed(String),
    /// Too many dice, or a value out of range.
    #[error("Formula {0:?} is out of range")]
    Range(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ast {
    Num(i64),
    Var(Var),
    Add(Box<Ast>, Box<Ast>),
    Sub(Box<Ast>, Box<Ast>),
    Mul(Box<Ast>, Box<Ast>),
    Neg(Box<Ast>),
    Dice(Box<Ast>, Box<Ast>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Var {
    Slot,
    SlotAbove,
    Mod,
    CantripTier,
}

/// A value while evaluating: a flat number plus dice counts by sides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Poly {
    flat: i64,
    dice: BTreeMap<u32, i64>,
}

impl Poly {
    fn num(n: i64) -> Self {
        Self {
            flat: n,
            dice: BTreeMap::new(),
        }
    }

    fn has_dice(&self) -> bool {
        self.dice.values().any(|c| *c != 0)
    }

    fn add(mut self, o: &Poly, sign: i64) -> Self {
        self.flat += sign * o.flat;
        for (s, c) in &o.dice {
            *self.dice.entry(*s).or_default() += sign * c;
        }
        self
    }

    fn scale(mut self, k: i64) -> Self {
        self.flat *= k;
        for c in self.dice.values_mut() {
            *c *= k;
        }
        self
    }
}

struct Parser<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> FormulaError {
        FormulaError::Syntax(self.src.to_string(), msg.to_string())
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.b.get(self.i).copied()
    }

    fn expr(&mut self) -> Result<Ast, FormulaError> {
        let mut left = self.term()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.i += 1;
                    left = Ast::Add(Box::new(left), Box::new(self.term()?));
                }
                Some(b'-') => {
                    self.i += 1;
                    left = Ast::Sub(Box::new(left), Box::new(self.term()?));
                }
                _ => return Ok(left),
            }
        }
    }

    fn term(&mut self) -> Result<Ast, FormulaError> {
        let mut left = self.unary()?;
        while self.peek() == Some(b'*') {
            self.i += 1;
            left = Ast::Mul(Box::new(left), Box::new(self.unary()?));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Ast, FormulaError> {
        if self.peek() == Some(b'-') {
            self.i += 1;
            return Ok(Ast::Neg(Box::new(self.unary()?)));
        }
        self.dice()
    }

    fn dice(&mut self) -> Result<Ast, FormulaError> {
        let count = if self.peek() == Some(b'd') {
            Ast::Num(1)
        } else {
            self.atom()?
        };
        if self.peek() == Some(b'd') {
            self.i += 1;
            let sides = self.atom()?;
            return Ok(Ast::Dice(Box::new(count), Box::new(sides)));
        }
        Ok(count)
    }

    fn atom(&mut self) -> Result<Ast, FormulaError> {
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let e = self.expr()?;
                if self.peek() != Some(b')') {
                    return Err(self.err("missing )"));
                }
                self.i += 1;
                Ok(e)
            }
            Some(c) if c.is_ascii_digit() => {
                let start = self.i;
                while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                    self.i += 1;
                }
                let n: i64 = self.src[start..self.i]
                    .parse()
                    .map_err(|_| self.err("number too large"))?;
                if n > 10_000 {
                    return Err(FormulaError::Range(self.src.to_string()));
                }
                Ok(Ast::Num(n))
            }
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                let start = self.i;
                while self.i < self.b.len()
                    && (self.b[self.i].is_ascii_alphanumeric() || self.b[self.i] == b'_')
                {
                    self.i += 1;
                }
                match &self.src[start..self.i] {
                    "slot" => Ok(Ast::Var(Var::Slot)),
                    "slot_above" => Ok(Ast::Var(Var::SlotAbove)),
                    "mod" => Ok(Ast::Var(Var::Mod)),
                    "cantrip_tier" => Ok(Ast::Var(Var::CantripTier)),
                    other => Err(self.err(&format!("unknown variable {other}"))),
                }
            }
            _ => Err(self.err("expected a number, a variable or (")),
        }
    }
}

fn eval(ast: &Ast, v: &FormulaVars, src: &str) -> Result<Poly, FormulaError> {
    let range = || FormulaError::Range(src.to_string());
    Ok(match ast {
        Ast::Num(n) => Poly::num(*n),
        Ast::Var(var) => Poly::num(match var {
            Var::Slot => i64::from(v.slot),
            Var::SlotAbove => i64::from(v.slot_above),
            Var::Mod => i64::from(v.modifier),
            Var::CantripTier => i64::from(v.cantrip_tier),
        }),
        Ast::Add(a, b) => eval(a, v, src)?.add(&eval(b, v, src)?, 1),
        Ast::Sub(a, b) => eval(a, v, src)?.add(&eval(b, v, src)?, -1),
        Ast::Neg(a) => eval(a, v, src)?.scale(-1),
        Ast::Mul(a, b) => {
            let (a, b) = (eval(a, v, src)?, eval(b, v, src)?);
            match (a.has_dice(), b.has_dice()) {
                (false, _) => b.scale(a.flat),
                (_, false) => a.scale(b.flat),
                (true, true) => return Err(FormulaError::DiceNotAllowed(src.to_string())),
            }
        }
        Ast::Dice(count, sides) => {
            let (count, sides) = (eval(count, v, src)?, eval(sides, v, src)?);
            if count.has_dice() || sides.has_dice() {
                return Err(FormulaError::DiceNotAllowed(src.to_string()));
            }
            let s = u32::try_from(sides.flat).map_err(|_| range())?;
            if !(1..=1000).contains(&s) {
                return Err(range());
            }
            let mut p = Poly::num(0);
            p.dice.insert(s, count.flat);
            p
        }
    })
}

/// A number or dice expression over `slot`, `slot_above`, `mod` and `cantrip_tier`. Serialized as its text; a
/// formula that doesn't parse is refused when it deserializes.
#[derive(Clone, PartialEq, Eq)]
pub struct Formula {
    text: String,
    ast: Ast,
}

impl Formula {
    /// Parses `text`.
    pub fn parse(text: &str) -> Result<Self, FormulaError> {
        let mut p = Parser {
            src: text,
            b: text.as_bytes(),
            i: 0,
        };
        if p.peek().is_none() {
            return Err(p.err("empty"));
        }
        let ast = p.expr()?;
        if p.peek().is_some() {
            return Err(p.err("unexpected text at the end"));
        }
        Ok(Self {
            text: text.trim().to_string(),
            ast,
        })
    }

    /// A constant.
    #[must_use]
    pub fn constant(n: i32) -> Self {
        Self {
            text: n.to_string(),
            ast: Ast::Num(i64::from(n)),
        }
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The formula has dice at these values.
    pub fn has_dice(&self, vars: &FormulaVars) -> Result<bool, FormulaError> {
        Ok(eval(&self.ast, vars, &self.text)?.has_dice())
    }

    /// A number (a target count, a bonus). Dice are refused.
    pub fn int(&self, vars: &FormulaVars) -> Result<i32, FormulaError> {
        let p = eval(&self.ast, vars, &self.text)?;
        if p.has_dice() {
            return Err(FormulaError::DiceNotAllowed(self.text.clone()));
        }
        i32::try_from(p.flat).map_err(|_| FormulaError::Range(self.text.clone()))
    }

    /// Dice, with terms of the same die merged and the largest die first (`8d6+(slot_above)d6` at slot 4 is
    /// `9d6`).
    pub fn dice(&self, vars: &FormulaVars) -> Result<DiceExpr, FormulaError> {
        let p = eval(&self.ast, vars, &self.text)?;
        let range = || FormulaError::Range(self.text.clone());
        let mut terms = Vec::new();
        for (sides, count) in p.dice.iter().rev() {
            if *count == 0 {
                continue;
            }
            let n = u32::try_from(count.unsigned_abs()).map_err(|_| range())?;
            if n > 100 {
                return Err(range());
            }
            terms.push(Term::Dice {
                count: n,
                sides: *sides,
                sign: if *count < 0 { Sign::Minus } else { Sign::Plus },
            });
        }
        if p.flat != 0 || terms.is_empty() {
            terms.push(Term::Flat {
                value: i32::try_from(p.flat).map_err(|_| range())?,
            });
        }
        Ok(DiceExpr { terms })
    }

    /// The dice notation at these values (`9d6`, `2d8+3`).
    pub fn notation(&self, vars: &FormulaVars) -> Result<String, FormulaError> {
        Ok(self.dice(vars)?.to_string())
    }
}

impl fmt::Debug for Formula {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Formula({:?})", self.text)
    }
}

impl fmt::Display for Formula {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Serialize for Formula {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.text)
    }
}

impl<'de> Deserialize<'de> for Formula {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Num(i32),
        }
        match Raw::deserialize(d)? {
            Raw::Text(t) => Formula::parse(&t).map_err(serde::de::Error::custom),
            Raw::Num(n) => Ok(Formula::constant(n)),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The definition
// ---------------------------------------------------------------------------------------------------------------

/// How much of the spell the engine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Fully automated.
    Auto,
    /// Automated, with notes the DM applies by hand.
    Partial,
    /// Nothing to automate in combat (a utility spell): the cast spends and logs.
    Utility,
    /// The DM decides, with the definition's buttons.
    Ruling,
    /// No definition yet: the cast spends the slot and asks the DM to rule.
    Uncurated,
}

/// The casting time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastTime {
    /// An action.
    Action,
    /// A bonus action.
    Bonus,
    /// A reaction.
    Reaction,
    /// One minute or longer: cast before the fight (precast), or spends the action each turn.
    Minute,
}

/// Which creatures a spell may target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetFilter {
    /// Any creature.
    #[default]
    Any,
    /// A willing creature (the DM confirms; the engine only checks it isn't an enemy unless forced).
    Willing,
    /// The caster's side.
    Ally,
    /// The other side.
    Enemy,
    /// Creatures of these types (`"humanoid"`, `"beast"`); checked against [`crate::CombatStats::creature_type`]
    /// when it is known.
    Type {
        /// Lowercase type names.
        types: Vec<String>,
    },
}

/// An area's shape. Areas are never geometric: the DM taps who is inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AreaShape {
    /// A sphere (radius).
    Sphere,
    /// A cube (side).
    Cube,
    /// A cone (length).
    Cone,
    /// A line (length).
    Line,
    /// A cylinder (radius).
    Cylinder,
    /// An emanation around the caster (2024).
    Emanation,
}

/// Whom a spell affects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Targeting {
    /// The caster.
    #[serde(rename = "self")]
    SelfOnly,
    /// Up to `max` creatures that pass `filter`.
    Creatures {
        /// The most targets (`"3+slot_above"`).
        max: Formula,
        /// Who qualifies.
        #[serde(default)]
        filter: TargetFilter,
        /// The caster may be one of them.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        include_self: bool,
    },
    /// Everyone the DM taps as inside the area.
    Area {
        /// The shape.
        shape: AreaShape,
        /// Its size in feet.
        size_ft: u32,
    },
}

/// Melee or ranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttackKind {
    /// A melee spell (or weapon) attack.
    Melee,
    /// A ranged one.
    Ranged,
}

/// What a successful save does to the damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveSuccess {
    /// Half damage (rounded down), and no `on_land` effects.
    Half,
    /// Nothing from `on_land`.
    None,
}

/// How a spell reaches each target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Delivery {
    /// It just lands.
    Auto,
    /// An attack roll per ray against each target's effective AC.
    Attack {
        /// Melee or ranged.
        attack: AttackKind,
        /// Rays or beams (`"cantrip_tier"` for a scaling cantrip, `"3+slot_above"`). 1 when absent.
        #[serde(default = "one")]
        rays: Formula,
    },
    /// A saving throw per target against the caster's DC.
    Save {
        /// The save.
        ability: AbilityName,
        /// What a success does.
        success: SaveSuccess,
    },
}

/// A save repeated at the end or start of the target's turns; a success ends the effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpellRepeatSave {
    /// The save.
    pub ability: AbilityName,
    /// At the start or end of the target's turn.
    pub at: Boundary,
}

/// What a d20 roll bonus or mode applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollOn {
    /// The creature's attack rolls.
    Attacks,
    /// Its saving throws (all, or one ability's).
    Saves,
    /// Its ability checks.
    Checks,
}

/// What an advantage or disadvantage applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeOn {
    /// Attack rolls the creature makes.
    AttacksBy,
    /// Attack rolls against the creature.
    AttacksAgainst,
    /// Its saving throws.
    Saves,
    /// Its ability checks.
    Checks,
}

/// Advantage or disadvantage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvDis {
    /// Advantage.
    Advantage,
    /// Disadvantage.
    Disadvantage,
}

/// A die added to (or taken from) a roll: Bless's `+1d4`, Bane's `-1d4`. Copy, so [`crate::Modifier`] stays Copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiceTerm {
    /// Dice (1..=100).
    pub count: u32,
    /// Faces.
    pub sides: u32,
    /// Added or subtracted.
    #[serde(default = "plus")]
    pub sign: Sign,
}

fn plus() -> Sign {
    Sign::Plus
}

impl DiceTerm {
    /// As a dice term.
    #[must_use]
    pub fn term(self) -> Term {
        Term::Dice {
            count: self.count,
            sides: self.sides,
            sign: self.sign,
        }
    }
}

impl fmt::Display for DiceTerm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = if self.sign == Sign::Minus { "-" } else { "+" };
        write!(f, "{s}{}d{}", self.count, self.sides)
    }
}

/// Immunity, resistance or vulnerability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefenseKind {
    /// Resistance.
    Resist,
    /// Immunity.
    Immune,
    /// Vulnerability.
    Vuln,
}

/// A movement mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedMode {
    /// Walk.
    Walk,
    /// Fly.
    Fly,
    /// Swim.
    Swim,
    /// Climb.
    Climb,
    /// Burrow.
    Burrow,
}

/// A speed change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpeedChange {
    /// +N feet to every speed.
    Add {
        /// Feet (negative slows).
        ft: i32,
    },
    /// Every speed times N.
    Mult {
        /// The factor.
        n: u32,
    },
    /// Every speed halved.
    Halve,
    /// Gains (or sets) a mode's speed: Fly 60.
    SetMode {
        /// The mode.
        mode: SpeedMode,
        /// Feet.
        ft: u32,
    },
    /// Speed 0.
    Zero,
}

/// A save and damage a zone deals to whoever the DM taps (Spirit Guardians, the 2024 Conjure Animals pack).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneSpec {
    /// The save, if any (none: the damage just lands).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save: Option<ZoneSave>,
    /// The damage dice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage: Option<Formula>,
    /// The damage type, or the caster's choice from `choice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage_type: Option<DamageType>,
    /// Types the caster picks from at cast time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choice: Vec<DamageType>,
    /// Conditions on a failed save.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

/// A zone's save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneSave {
    /// The save.
    pub ability: AbilityName,
    /// What a success does.
    pub success: SaveSuccess,
}

/// A count of creatures of one CR for a summon's options (`"1/4"`, 8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrCount {
    /// The challenge rating as printed (`"2"`, `"1/2"`).
    pub cr: String,
    /// How many.
    pub count: u32,
}

/// A count multiplier at a higher slot (Conjure Animals 2014: ×2 at 5th, ×3 at 7th).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotMultiplier {
    /// From this slot level up.
    pub slot: u8,
    /// Times the base count.
    pub times: u32,
}

/// Where summoned creatures act.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummonInitiative {
    /// Right after the caster.
    #[default]
    AfterOwner,
    /// As one group on their own initiative.
    OwnGroup,
}

/// What a summon or conjure spell offers. In 0.3.2 nothing spawns: the cast shows this as a toast, with a button
/// that opens Add creature pre-filtered and "Controlled by <caster>" preset, optionally bound to the caster's
/// concentration ([`crate::EffectSpec::dismiss_on_end`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummonInfo {
    /// Creature types to pre-filter by (`["beast"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub creature_types: Vec<String>,
    /// The highest CR offered (`"2"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cr: Option<String>,
    /// The options by CR (2014 Conjure Animals).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_cr: Vec<CrCount>,
    /// Count multipliers at higher slots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_slot: Vec<SlotMultiplier>,
    /// Where they act.
    #[serde(default)]
    pub initiative: SummonInitiative,
}

/// A follow-up effect added when the spell's effect ends (Haste's lethargy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowUpSpec {
    /// Its name.
    pub name: String,
    /// How long it lasts.
    #[serde(default)]
    pub duration: SpellDuration,
    /// What it does (no damage, heal or follow-up inside).
    #[serde(default)]
    pub apply: Vec<Primitive>,
}

/// A button of a ruling row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulingButton {
    /// The label (`"Countered"`).
    pub label: String,
    /// What it does.
    #[serde(default)]
    pub action: RulingAction,
}

/// What a ruling button does. Nothing is inferred: the DM taps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RulingAction {
    /// Only logs the ruling.
    #[default]
    Log,
    /// Cancels the target's open cast ([`crate::Command::CancelCast`]), refunding its slot or not.
    CancelTargetCast {
        /// Give the slot back (2024 Counterspell: the slot isn't expended).
        refund: bool,
    },
    /// A helper: roll the caster's spellcasting ability check (2014 Counterspell: against 10 + the countered
    /// spell's level, which the DM reads off the other cast).
    RollCheck {
        /// The DC when the definition knows it; absent: the DM types it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dc: Option<Formula>,
    },
    /// A helper: the target makes this save against the caster's DC (2024 Counterspell).
    AskSave {
        /// The save.
        ability: AbilityName,
    },
}

/// One thing a spell does. Numbers are formulas; text is our own wording, never rules prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Primitive {
    /// Damage: one roll for every target, halved for savers when the save says so.
    Damage {
        /// The dice.
        dice: Formula,
        /// The type, or `None` with `choice`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        damage_type: Option<DamageType>,
        /// Types the caster picks from.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        choice: Vec<DamageType>,
    },
    /// Healing.
    Heal {
        /// The dice.
        dice: Formula,
        /// Add the spellcasting modifier.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        add_mod: bool,
    },
    /// Temporary hit points.
    TempHp {
        /// The dice or number.
        dice: Formula,
        /// Add the spellcasting modifier.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        add_mod: bool,
    },
    /// A condition while the effect lasts.
    Condition {
        /// The condition.
        condition: Condition,
        /// A save repeated at the target's turns ends it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repeat_save: Option<SpellRepeatSave>,
        /// It ends when the target takes damage.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        ends_on_damage: bool,
    },
    /// The base AC becomes `base` (+ DEX): Mage Armor 13 + DEX. The best base counts; it never lowers AC.
    AcBase {
        /// The base.
        base: i32,
        /// Add the DEX modifier.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        add_dex: bool,
    },
    /// +N AC (Shield +5, Shield of Faith +2).
    AcBonus {
        /// The bonus.
        n: i32,
    },
    /// AC can't be lower than N (Barkskin 2014: 16).
    AcMin {
        /// The floor.
        n: i32,
    },
    /// A die added to rolls (Bless +1d4 to attacks and saves).
    RollDice {
        /// Which rolls.
        on: RollOn,
        /// Only this ability's saves or checks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ability: Option<AbilityName>,
        /// The die.
        term: DiceTerm,
    },
    /// Advantage or disadvantage.
    RollMode {
        /// Which rolls.
        on: ModeOn,
        /// Only this ability's saves or checks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ability: Option<AbilityName>,
        /// Advantage or disadvantage.
        mode: AdvDis,
    },
    /// Damage resistance, immunity or vulnerability.
    Defense {
        /// Which.
        defense: DefenseKind,
        /// The types.
        types: Vec<DamageType>,
    },
    /// +N maximum HP (and current HP).
    MaxHp {
        /// The amount (`"5*slot_above"`).
        n: Formula,
    },
    /// A speed change.
    Speed {
        /// The change.
        change: SpeedChange,
    },
    /// A zone: a save and damage the DM applies to whoever they tap, from the effect's [Apply to…] button.
    Zone {
        /// The zone.
        zone: ZoneSpec,
    },
    /// A summon's options (info only in 0.3.2).
    Summon {
        /// The options.
        info: SummonInfo,
    },
    /// An effect added when this one ends.
    FollowUp {
        /// The follow-up.
        effect: Box<FollowUpSpec>,
    },
    /// Ends spell effects on the target up to this level (Dispel Magic's automatic part; higher ones are a
    /// ruling).
    EndEffects {
        /// The highest spell level ended (`"slot"`).
        max_level: Formula,
    },
    /// A question for the DM with buttons.
    Ruling {
        /// The question.
        question: String,
        /// The buttons.
        buttons: Vec<RulingButton>,
    },
    /// A reminder in our own words.
    Note {
        /// The text.
        text: String,
    },
}

/// How long a spell's effects last.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpellDuration {
    /// No lasting effect.
    #[default]
    Instant,
    /// N of the caster's turns, counted at `at` (Shield: 1 at the start).
    SourceTurns {
        /// Turns.
        n: u32,
        /// Counted at the start or end of the caster's turn.
        at: Boundary,
    },
    /// N of the target's turns, counted at `at`.
    TargetTurns {
        /// Turns.
        n: u32,
        /// Counted at the start or end of the target's turn.
        at: Boundary,
    },
    /// N minutes: 10 rounds each, counted at the start of the caster's turns.
    Minutes {
        /// Minutes.
        n: u32,
    },
    /// Hours or longer: lasts the fight.
    Long,
    /// Until dispelled or ended by the DM.
    UntilDispelled,
}

/// One spell in one edition: the curated content. Closed and versioned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpellMech {
    /// Schema version, 1.
    #[serde(default = "v1")]
    pub v: u32,
    /// The spell element id (`ID_PHB_SPELL_FIREBALL`).
    pub key: String,
    /// The edition.
    pub edition: Edition,
    /// The name (SRD names only in shipped data).
    pub name: String,
    /// The definition's revision.
    #[serde(default)]
    pub rev: u32,
    /// 0 for a cantrip.
    pub level: u8,
    /// Needs concentration.
    #[serde(default)]
    pub concentration: bool,
    /// The casting time.
    pub cast: CastTime,
    /// Whom it affects.
    pub targeting: Targeting,
    /// How it reaches them.
    pub delivery: Delivery,
    /// On a failed save, a hit, or always for `auto`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_land: Vec<Primitive>,
    /// On a successful save or a miss (besides half damage).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_resist: Vec<Primitive>,
    /// On every target, whatever the save (and on the caster for a self spell).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub always: Vec<Primitive>,
    /// How long the effects last.
    #[serde(default)]
    pub duration: SpellDuration,
    /// How much is automated.
    pub tier: Tier,
    /// Reminders in our own words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Where a cast's definition came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MechOrigin {
    /// A curated SRD row.
    Srd {
        /// The element id.
        key: String,
        /// Its revision.
        rev: u32,
    },
    /// The DM's own definition.
    Def {
        /// The definition id.
        id: String,
        /// Its revision.
        rev: u32,
    },
    /// A stat block's attack or save (`PerformStatblock`).
    Statblock,
    /// No definition: a ruling.
    Ruling,
}

/// Choices made at cast time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CastChoices {
    /// The damage type, for a spell with a choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage_type: Option<DamageType>,
}

/// A [`SpellMech`] compiled for one caster on the server. Embedded in the cast command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CastSpec {
    /// Schema version, 1.
    #[serde(default = "v1")]
    pub v: u32,
    /// Where the definition came from.
    pub origin: MechOrigin,
    /// The definition.
    pub mech: SpellMech,
    /// The caster's save DC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dc: Option<u32>,
    /// The caster's attack bonus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack: Option<i32>,
    /// The spellcasting modifier (`mod`).
    #[serde(default, rename = "mod")]
    pub modifier: i32,
    /// The cantrip tier, 1..=4 (a one-time DM pick for 2024 blocks without a caster level).
    #[serde(default = "tier1")]
    pub cantrip_tier: u8,
    /// The spell is already part of the stat block's numbers ("included in AC"): its AC primitives don't change
    /// AC, but the effect is listed so it can be ended.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub baked_in: bool,
}

fn tier1() -> u8 {
    1
}

impl CastSpec {
    /// The formula variables for a cast at `slot` (`None`: a cantrip or a slotless use, at the spell's level).
    #[must_use]
    pub fn vars(&self, slot: Option<u8>) -> FormulaVars {
        let level = self.mech.level;
        let slot = slot.unwrap_or(level);
        FormulaVars {
            slot,
            slot_above: slot.saturating_sub(level),
            modifier: self.modifier,
            cantrip_tier: self.cantrip_tier.clamp(1, 4),
        }
    }

    /// A spec for a spell without a definition: the cast spends and the DM rules.
    #[must_use]
    pub fn ruling(key: &str, name: &str, edition: Edition, level: u8, concentration: bool) -> Self {
        Self {
            v: MECH_VERSION,
            origin: MechOrigin::Ruling,
            mech: SpellMech {
                v: MECH_VERSION,
                key: key.to_string(),
                edition,
                name: name.to_string(),
                rev: 0,
                level,
                concentration,
                cast: CastTime::Action,
                targeting: Targeting::Creatures {
                    max: Formula::constant(99),
                    filter: TargetFilter::Any,
                    include_self: true,
                },
                delivery: Delivery::Auto,
                on_land: vec![],
                on_resist: vec![],
                always: vec![],
                duration: SpellDuration::Instant,
                tier: Tier::Uncurated,
                notes: vec![],
            },
            dc: None,
            attack: None,
            modifier: 0,
            cantrip_tier: 1,
            baked_in: false,
        }
    }
}

/// The cantrip tier for a character or caster level: 1 (1–4), 2 (5–10), 3 (11–16), 4 (17+).
#[must_use]
pub fn cantrip_tier(level: u8) -> u8 {
    match level {
        0..=4 => 1,
        5..=10 => 2,
        11..=16 => 3,
        _ => 4,
    }
}

/// A spell entry of a stat block or spellbook, linked to an installed spell element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpellLink {
    /// The linked spell element id; `None` while unlinked (the [Link…] button).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The spell's level (0 for a cantrip).
    pub level: u8,
    /// How it is cast.
    #[serde(default)]
    pub frequency: SpellFrequency,
    /// Always cast at this slot level ("level N version").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_slot: Option<u8>,
    /// "Included in AC": see [`CastSpec::baked_in`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub baked_in: bool,
}

/// How a stat block casts a spell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpellFrequency {
    /// With a spell slot (cantrips: free).
    #[default]
    Slot,
    /// At will.
    AtWill,
    /// N/day: the ability's `uses` resource.
    PerDay,
}

/// The spell an effect comes from, for [End], [Dismiss spell], Dispel and the zone button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpellTag {
    /// The spell element id.
    pub key: String,
    /// The spell's level.
    pub level: u8,
    /// The slot it was cast with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u8>,
    /// The cast it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cast: Option<crate::model::CastId>,
    /// The caster's save DC (for a zone's save).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dc: Option<u32>,
    /// A zone the DM applies with [`crate::Command::ApplyZone`], with its formulas at the cast's slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<Box<ZoneSpec>>,
    /// The formula variables of the cast (to roll the zone's dice at the cast's slot).
    #[serde(default)]
    pub vars: FormulaVars,
    /// AC primitives are already in the stat block's AC.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub baked_in: bool,
}
