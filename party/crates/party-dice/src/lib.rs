//! Dice for Borealis Party combat: a Rust port of `ui/src/lib/dice.ts`.
//!
//! - **Notation:** `1d8+3`, `2d6`, `d20`, `1d4+1d6+2` ([`parse_notation`]), and the dice of a printed damage text
//!   such as `1d8+3 slashing + 1d6 fire` ([`damage_dice`]).
//! - **Rolls:** [`roll_expr`] (with a die floor), [`critical`] (every die rolled twice as many times) and
//!   [`roll_d20`] (advantage, disadvantage and a crit range).
//! - **Dice:** every roll takes a [`DieRoller`], so the server injects [`SeededDie`] (reproducible, serializable)
//!   and tests inject [`SequenceDie`]. [`SeededDie`] is the same mulberry32 generator as the UI's `seededDie`, so
//!   the same seed gives the same rolls in both.
//! - **Typed results (D43):** players and the DM roll physical dice at the table, so every roll API also accepts a
//!   typed result: [`D20Input::Typed`] and [`DiceInput::Typed`], resolved by [`resolve_d20`] and [`resolve_dice`].
//!   An automatic roll is always optional.
//!
//! No I/O, no global state: the same inputs and the same die give the same result.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use std::fmt;

/// At most this many dice in one term.
pub const MAX_DICE: u32 = 100;
/// At most this many sides on a die.
pub const MAX_SIDES: u32 = 1000;
/// At most this many terms in one expression.
pub const MAX_TERMS: usize = 20;
/// The largest flat modifier.
pub const MAX_FLAT: i32 = 10_000;
/// The largest absolute value of a typed total.
pub const MAX_TYPED: i32 = 1_000_000;
const MAX_TEXT: usize = 200;

/// Why a notation could not be read or a roll could not be made. The messages are the UI's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiceError {
    /// Empty text.
    #[error("No dice to roll.")]
    Empty,
    /// Text over 200 characters.
    #[error("The dice text is too long.")]
    TooLong,
    /// Text that is not dice notation.
    #[error("Can't read \"{0}\".")]
    Unreadable(String),
    /// `1+-2`.
    #[error("Two operators in a row.")]
    TwoOperators,
    /// `1d6 2`.
    #[error("Join the dice with + or -.")]
    MissingOperator,
    /// `1d6+`.
    #[error("The dice text ends with an operator.")]
    TrailingOperator,
    /// `1d`.
    #[error("A die needs a number of sides, as in 1d6.")]
    NoSides,
    /// `0d6` or `101d6`.
    #[error("Roll 1 to {MAX_DICE} dice at a time.")]
    DiceCount,
    /// `1d0` or `1d1001`.
    #[error("A die has 1 to {MAX_SIDES} sides.")]
    Sides,
    /// `+10001`.
    #[error("A modifier above {MAX_FLAT}.")]
    Flat,
    /// More than 20 terms.
    #[error("At most {MAX_TERMS} parts.")]
    Terms,
    /// A die returned a value outside 1..=sides.
    #[error("A d{sides} rolled {value}.")]
    BadDie {
        /// The die's sides.
        sides: u32,
        /// What it returned.
        value: u32,
    },
    /// A scripted die ran out.
    #[error("The scripted die ran out of rolls.")]
    OutOfRolls,
    /// A typed total outside ±1,000,000.
    #[error("A typed total must be between -{MAX_TYPED} and {MAX_TYPED}.")]
    TypedTotal,
    /// A typed natural d20 outside 1..=20.
    #[error("A natural d20 is 1 to 20.")]
    TypedNatural,
}

/// Rolls one die. `roll(sides)` returns an integer in `1..=sides`; anything else is reported as
/// [`DiceError::BadDie`] by the roll functions.
pub trait DieRoller {
    /// One die with `sides` faces. `Err` only for scripted dice that ran out.
    fn roll(&mut self, sides: u32) -> Result<u32, DiceError>;
}

impl<F: FnMut(u32) -> u32> DieRoller for F {
    fn roll(&mut self, sides: u32) -> Result<u32, DiceError> {
        Ok(self(sides))
    }
}

/// A reproducible die: mulberry32, as the UI's `seededDie`. Serializable, so a combat can store where its dice are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeededDie {
    state: u32,
}

impl SeededDie {
    /// A die that starts from `seed`.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// The next raw 32-bit value.
    pub fn next_u32(&mut self) -> u32 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }
}

impl DieRoller for SeededDie {
    fn roll(&mut self, sides: u32) -> Result<u32, DiceError> {
        // floor(r * sides) + 1 with r = v / 2^32, exactly as the UI computes it in doubles.
        let v = u64::from(self.next_u32());
        Ok(((v * u64::from(sides)) >> 32) as u32 + 1)
    }
}

/// Plays back fixed rolls (tests). Fails with [`DiceError::OutOfRolls`] when it runs out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SequenceDie {
    values: Vec<u32>,
    next: usize,
}

impl SequenceDie {
    /// A die that returns `values` in order.
    #[must_use]
    pub fn new(values: impl Into<Vec<u32>>) -> Self {
        Self {
            values: values.into(),
            next: 0,
        }
    }

    /// How many values are left.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.values.len() - self.next
    }
}

impl DieRoller for SequenceDie {
    fn roll(&mut self, _sides: u32) -> Result<u32, DiceError> {
        let v = *self.values.get(self.next).ok_or(DiceError::OutOfRolls)?;
        self.next += 1;
        Ok(v)
    }
}

/// `+` or `-` in front of a dice term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sign {
    /// Added.
    Plus,
    /// Subtracted.
    Minus,
}

impl Sign {
    fn factor(self) -> i32 {
        match self {
            Sign::Plus => 1,
            Sign::Minus => -1,
        }
    }
}

/// One part of a dice expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Term {
    /// `count` dice with `sides` faces.
    Dice {
        /// 1..=100.
        count: u32,
        /// 1..=1000.
        sides: u32,
        /// Added or subtracted.
        sign: Sign,
    },
    /// A signed modifier.
    Flat {
        /// The value, sign included.
        value: i32,
    },
}

impl Term {
    fn negative(&self) -> bool {
        match *self {
            Term::Dice { sign, .. } => sign == Sign::Minus,
            Term::Flat { value } => value < 0,
        }
    }

    fn text(&self) -> String {
        match *self {
            Term::Dice { count, sides, .. } => format!("{count}d{sides}"),
            Term::Flat { value } => value.unsigned_abs().to_string(),
        }
    }
}

/// A parsed dice expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiceExpr {
    /// The terms, summed.
    pub terms: Vec<Term>,
}

impl fmt::Display for DiceExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, t) in self.terms.iter().enumerate() {
            let sign = if t.negative() {
                "-"
            } else if i > 0 {
                "+"
            } else {
                ""
            };
            write!(f, "{sign}{}", t.text())?;
        }
        Ok(())
    }
}

enum Token {
    Op(Sign),
    Dice(Option<u64>, Option<u64>),
    Number(u64),
}

/// Reads digits; `None` when there are none. Saturates, so huge numbers fail the range checks.
fn digits(chars: &[char], at: &mut usize) -> Option<u64> {
    let start = *at;
    let mut v: u64 = 0;
    while let Some(d) = chars.get(*at).and_then(|c| c.to_digit(10)) {
        v = v.saturating_mul(10).saturating_add(u64::from(d));
        *at += 1;
    }
    (*at > start).then_some(v)
}

fn next_token(chars: &[char], at: &mut usize) -> Option<Token> {
    while chars.get(*at).is_some_and(|c| c.is_whitespace()) {
        *at += 1;
    }
    match chars.get(*at)? {
        '+' => {
            *at += 1;
            Some(Token::Op(Sign::Plus))
        }
        '-' => {
            *at += 1;
            Some(Token::Op(Sign::Minus))
        }
        _ => {
            let save = *at;
            let count = digits(chars, at);
            if matches!(chars.get(*at), Some('d' | 'D')) {
                *at += 1;
                let sides = digits(chars, at);
                Some(Token::Dice(count, sides))
            } else if let Some(n) = count {
                Some(Token::Number(n))
            } else {
                *at = save;
                None
            }
        }
    }
}

fn operand(token: Token, sign: Sign) -> Result<Term, DiceError> {
    match token {
        Token::Number(n) => {
            if n > MAX_FLAT as u64 {
                return Err(DiceError::Flat);
            }
            Ok(Term::Flat {
                value: sign.factor() * n as i32,
            })
        }
        Token::Dice(count, sides) => {
            let count = count.unwrap_or(1);
            let sides = sides.ok_or(DiceError::NoSides)?;
            if !(1..=u64::from(MAX_DICE)).contains(&count) {
                return Err(DiceError::DiceCount);
            }
            if !(1..=u64::from(MAX_SIDES)).contains(&sides) {
                return Err(DiceError::Sides);
            }
            Ok(Term::Dice {
                count: count as u32,
                sides: sides as u32,
                sign,
            })
        }
        Token::Op(_) => unreachable!("operators are handled by the caller"),
    }
}

/// Parses dice notation such as `1d8+3`, `2d6`, `d20` or `1d4+1d6+2`.
pub fn parse_notation(text: &str) -> Result<DiceExpr, DiceError> {
    if text.trim().is_empty() {
        return Err(DiceError::Empty);
    }
    if text.chars().count() > MAX_TEXT {
        return Err(DiceError::TooLong);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let mut terms = Vec::new();
    let mut sign = Sign::Plus;
    let mut expect_operand = true;
    let mut saw_operator = false;
    loop {
        let rest: String = chars[at..].iter().collect();
        if rest.trim().is_empty() {
            break;
        }
        let token = next_token(&chars, &mut at)
            .ok_or_else(|| DiceError::Unreadable(rest.trim().to_string()))?;
        if let Token::Op(s) = token {
            let leading = terms.is_empty() && !saw_operator;
            if expect_operand && !leading {
                return Err(DiceError::TwoOperators);
            }
            sign = s;
            saw_operator = true;
            expect_operand = true;
            continue;
        }
        if !expect_operand {
            return Err(DiceError::MissingOperator);
        }
        terms.push(operand(token, sign)?);
        if terms.len() > MAX_TERMS {
            return Err(DiceError::Terms);
        }
        sign = Sign::Plus;
        saw_operator = false;
        expect_operand = false;
    }
    if expect_operand {
        return Err(DiceError::TrailingOperator);
    }
    Ok(DiceExpr { terms })
}

/// [`parse_notation`], `None` on error.
#[must_use]
pub fn try_parse_notation(text: &str) -> Option<DiceExpr> {
    parse_notation(text).ok()
}

/// The dice and number tokens of a damage text, as the UI's `(?<![A-Za-z\d])(\d*d\d+|\d+)(?![A-Za-z\d])`:
/// each token's start, end and text.
fn damage_terms(text: &str) -> Vec<(usize, usize, &str)> {
    let b = text.as_bytes();
    let alnum = |i: usize| b.get(i).is_some_and(u8::is_ascii_alphanumeric);
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if i > 0 && alnum(i - 1) {
            i += 1;
            continue;
        }
        let mut j = i;
        while digit(j) {
            j += 1;
        }
        if matches!(b.get(j), Some(b'd' | b'D')) {
            let mut k = j + 1;
            while digit(k) {
                k += 1;
            }
            if k > j + 1 && !alnum(k) {
                out.push((i, k, &text[i..k]));
                i = k;
                continue;
            }
        }
        if j > i && !alnum(j) {
            out.push((i, j, &text[i..j]));
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

/// The dice of a damage text as a stat block prints it (`1d8+3 slashing + 1d6 fire` -> `1d8+3+1d6`). Damage types
/// are dropped; the sum stops at anything that is not a `+` or `-` join. `None` when the text starts with no dice
/// or number.
#[must_use]
pub fn damage_dice(text: &str) -> Option<DiceExpr> {
    let mut parts: Vec<String> = Vec::new();
    let mut end = 0;
    for (start, stop, term) in damage_terms(text) {
        let gap = &text[end..start];
        if parts.is_empty() {
            let g = gap.trim();
            if !(g.is_empty() || g == "+" || g == "-") {
                return None;
            }
            parts.push(format!("{g}{term}"));
        } else {
            // `^[\sA-Za-z]*([+-])\s*$`
            let trimmed = gap.trim_end();
            let Some(op) = trimmed.chars().last().filter(|c| *c == '+' || *c == '-') else {
                break;
            };
            let before = &trimmed[..trimmed.len() - 1];
            if !before
                .chars()
                .all(|c| c.is_whitespace() || c.is_ascii_alphabetic())
            {
                break;
            }
            parts.push(format!("{op}{term}"));
        }
        end = stop;
    }
    if parts.is_empty() {
        None
    } else {
        try_parse_notation(&parts.concat())
    }
}

/// A critical hit: every die rolled twice as many times; modifiers stay as they are.
#[must_use]
pub fn critical(expr: &DiceExpr) -> DiceExpr {
    DiceExpr {
        terms: expr
            .terms
            .iter()
            .map(|t| match *t {
                Term::Dice { count, sides, sign } => Term::Dice {
                    count: count * 2,
                    sides,
                    sign,
                },
                flat => flat,
            })
            .collect(),
    }
}

/// Where a result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollSource {
    /// Rolled by a [`DieRoller`].
    Rolled,
    /// Typed in from physical dice (D43).
    Typed,
}

/// One rolled term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolledTerm {
    /// The term.
    pub term: Term,
    /// Each die after the floor.
    pub rolls: Vec<u32>,
    /// The term's signed value.
    pub value: i32,
}

/// The result of a dice expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiceResult {
    /// Rolled or typed.
    pub source: RollSource,
    /// The notation (empty for a typed total).
    pub notation: String,
    /// The sum.
    pub total: i32,
    /// Each term (empty for a typed total).
    pub terms: Vec<RolledTerm>,
    /// The text a result card shows: `1d8 [6] +3 = 9`.
    pub breakdown: String,
}

/// Options for [`roll_expr`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollOptions {
    /// Dice that roll below this count as it (Great Weapon Fighting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<u32>,
    /// A note added to the breakdown ("critical").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn checked_roll(die: &mut dyn DieRoller, sides: u32) -> Result<u32, DiceError> {
    let value = die.roll(sides)?;
    if value < 1 || value > sides {
        return Err(DiceError::BadDie { sides, value });
    }
    Ok(value)
}

/// Rolls every term of `expr` and sums them.
pub fn roll_expr(
    expr: &DiceExpr,
    die: &mut dyn DieRoller,
    opts: &RollOptions,
) -> Result<DiceResult, DiceError> {
    let floor = opts.floor.unwrap_or(0);
    let mut floored = false;
    let mut terms = Vec::with_capacity(expr.terms.len());
    for term in &expr.terms {
        match *term {
            Term::Flat { value } => terms.push(RolledTerm {
                term: *term,
                rolls: vec![],
                value,
            }),
            Term::Dice { count, sides, sign } => {
                let mut rolls = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let raw = checked_roll(die, sides)?;
                    if raw < floor {
                        floored = true;
                    }
                    rolls.push(raw.max(floor));
                }
                let sum: u32 = rolls.iter().sum();
                terms.push(RolledTerm {
                    term: *term,
                    rolls,
                    value: sign.factor() * sum as i32,
                });
            }
        }
    }
    let total = terms.iter().map(|t| t.value).sum();
    let pieces: Vec<String> = terms
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let sign = if t.term.negative() {
                "-"
            } else if i > 0 {
                "+"
            } else {
                ""
            };
            let dice = match t.term {
                Term::Dice { .. } => format!(
                    " [{}]",
                    t.rolls
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Term::Flat { .. } => String::new(),
            };
            format!("{sign}{}{dice}", t.term.text())
        })
        .collect();
    let mut notes: Vec<String> = opts
        .label
        .iter()
        .filter(|l| !l.is_empty())
        .cloned()
        .collect();
    if floored {
        notes.push(format!("dice below {floor} count as {floor}"));
    }
    let tail = if notes.is_empty() {
        String::new()
    } else {
        format!(", {}", notes.join(", "))
    };
    Ok(DiceResult {
        source: RollSource::Rolled,
        notation: expr.to_string(),
        total,
        terms,
        breakdown: format!("{} = {total}{tail}", pieces.join(" ")),
    })
}

/// Normal, advantage or disadvantage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollMode {
    /// One d20.
    #[default]
    Normal,
    /// Two, keep the higher.
    Advantage,
    /// Two, keep the lower.
    Disadvantage,
}

impl RollMode {
    /// Advantage and disadvantage cancel out.
    #[must_use]
    pub fn combine(advantage: bool, disadvantage: bool) -> Self {
        match (advantage, disadvantage) {
            (true, false) => RollMode::Advantage,
            (false, true) => RollMode::Disadvantage,
            _ => RollMode::Normal,
        }
    }
}

/// The result of a d20 test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct D20Result {
    /// Rolled or typed.
    pub source: RollSource,
    /// The mode it was rolled in (`normal` for a typed result).
    pub mode: RollMode,
    /// The d20s rolled (the typed natural, if one was given).
    pub dice: Vec<u32>,
    /// The kept die; `None` for a typed total without its natural die.
    pub natural: Option<u32>,
    /// The modifier added (`None` for a typed total).
    pub modifier: Option<i32>,
    /// The total.
    pub total: i32,
    /// The kept die was a 20.
    pub is_nat20: bool,
    /// The kept die was a 1.
    pub is_nat1: bool,
    /// The kept die reached the crit range (attacks).
    pub is_crit: bool,
    /// `d20 [14, 7] +5 = 19, advantage`.
    pub breakdown: String,
}

fn signed(n: i32) -> String {
    if n < 0 {
        format!("-{}", n.unsigned_abs())
    } else {
        format!("+{n}")
    }
}

/// A d20 test: one d20, or two keeping the higher (advantage) or the lower (disadvantage), plus `modifier`.
/// `crit_range` is the lowest natural that crits (20 when `None`).
pub fn roll_d20(
    modifier: i32,
    mode: RollMode,
    die: &mut dyn DieRoller,
    crit_range: Option<u32>,
) -> Result<D20Result, DiceError> {
    let dice = if mode == RollMode::Normal {
        vec![checked_roll(die, 20)?]
    } else {
        vec![checked_roll(die, 20)?, checked_roll(die, 20)?]
    };
    let natural = if mode == RollMode::Disadvantage {
        *dice.iter().min().expect("one die")
    } else {
        *dice.iter().max().expect("one die")
    };
    let total = natural as i32 + modifier;
    let is_nat20 = natural == 20;
    let is_nat1 = natural == 1;
    let mut notes = Vec::new();
    match mode {
        RollMode::Normal => {}
        RollMode::Advantage => notes.push("advantage"),
        RollMode::Disadvantage => notes.push("disadvantage"),
    }
    if is_nat20 {
        notes.push("natural 20");
    }
    if is_nat1 {
        notes.push("natural 1");
    }
    let tail = if notes.is_empty() {
        String::new()
    } else {
        format!(", {}", notes.join(", "))
    };
    let list = dice
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(D20Result {
        source: RollSource::Rolled,
        mode,
        breakdown: format!("d20 [{list}] {} = {total}{tail}", signed(modifier)),
        dice,
        natural: Some(natural),
        modifier: Some(modifier),
        total,
        is_nat20,
        is_nat1,
        is_crit: natural >= crit_range.unwrap_or(20),
    })
}

/// A d20 test, either typed from physical dice or rolled (D43).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum D20Input {
    /// A result read off a physical die.
    Typed {
        /// The total.
        total: i32,
        /// The natural die, when the roller says it (needed for natural 20 and natural 1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        natural: Option<u32>,
    },
    /// Roll it with the injected die.
    Roll {
        /// Added to the kept die.
        #[serde(default)]
        modifier: i32,
        /// Normal, advantage or disadvantage.
        #[serde(default)]
        mode: RollMode,
        /// The lowest natural that crits (20 when absent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crit_range: Option<u32>,
    },
}

impl D20Input {
    /// A typed total.
    #[must_use]
    pub fn typed(total: i32) -> Self {
        D20Input::Typed {
            total,
            natural: None,
        }
    }

    /// A typed total with its natural die.
    #[must_use]
    pub fn typed_natural(total: i32, natural: u32) -> Self {
        D20Input::Typed {
            total,
            natural: Some(natural),
        }
    }

    /// A rolled test.
    #[must_use]
    pub fn roll(modifier: i32, mode: RollMode) -> Self {
        D20Input::Roll {
            modifier,
            mode,
            crit_range: None,
        }
    }
}

fn check_typed(total: i32) -> Result<(), DiceError> {
    if total.unsigned_abs() > MAX_TYPED as u32 {
        return Err(DiceError::TypedTotal);
    }
    Ok(())
}

/// Resolves a [`D20Input`]: a typed result as given, or a roll with `die`. The die is not touched for a typed result.
pub fn resolve_d20(input: &D20Input, die: &mut dyn DieRoller) -> Result<D20Result, DiceError> {
    match *input {
        D20Input::Roll {
            modifier,
            mode,
            crit_range,
        } => roll_d20(modifier, mode, die, crit_range),
        D20Input::Typed { total, natural } => {
            check_typed(total)?;
            if let Some(n) = natural {
                if !(1..=20).contains(&n) {
                    return Err(DiceError::TypedNatural);
                }
            }
            let breakdown = match natural {
                Some(n) => format!("typed {total} (natural {n})"),
                None => format!("typed {total}"),
            };
            Ok(D20Result {
                source: RollSource::Typed,
                mode: RollMode::Normal,
                dice: natural.into_iter().collect(),
                natural,
                modifier: None,
                total,
                is_nat20: natural == Some(20),
                is_nat1: natural == Some(1),
                is_crit: natural == Some(20),
                breakdown,
            })
        }
    }
}

/// A dice expression, either typed from physical dice or rolled (D43).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiceInput {
    /// A total read off physical dice.
    Typed {
        /// The total.
        total: i32,
    },
    /// Roll this notation with the injected die.
    Roll {
        /// Dice notation, as `2d6+3`.
        notation: String,
        /// Double the dice (a critical hit).
        #[serde(default)]
        critical: bool,
        /// Dice below this count as it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        floor: Option<u32>,
    },
}

impl DiceInput {
    /// A typed total.
    #[must_use]
    pub fn typed(total: i32) -> Self {
        DiceInput::Typed { total }
    }

    /// Roll `notation`.
    #[must_use]
    pub fn roll(notation: impl Into<String>) -> Self {
        DiceInput::Roll {
            notation: notation.into(),
            critical: false,
            floor: None,
        }
    }
}

/// Resolves a [`DiceInput`]: a typed total as given, or a roll with `die`.
pub fn resolve_dice(input: &DiceInput, die: &mut dyn DieRoller) -> Result<DiceResult, DiceError> {
    match input {
        DiceInput::Typed { total } => {
            check_typed(*total)?;
            Ok(DiceResult {
                source: RollSource::Typed,
                notation: String::new(),
                total: *total,
                terms: vec![],
                breakdown: format!("typed {total}"),
            })
        }
        DiceInput::Roll {
            notation,
            critical: crit,
            floor,
        } => {
            let expr = parse_notation(notation)?;
            let expr = if *crit { critical(&expr) } else { expr };
            let opts = RollOptions {
                floor: *floor,
                label: crit.then(|| "critical".to_string()),
            };
            roll_expr(&expr, die, &opts)
        }
    }
}

#[cfg(test)]
mod tests;
