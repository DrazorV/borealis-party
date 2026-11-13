//! Damage typing: immunity, then resistance, then vulnerability, with each clause's text kept for the DM.

use crate::model::{ClauseScope, DamageClause, DamageType, Defenses, Edition};
use party_dice::DiceResult;
use serde::{Deserialize, Serialize};

/// Which list a clause is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefenseKind {
    /// Immunity.
    Immunity,
    /// Resistance.
    Resistance,
    /// Vulnerability.
    Vulnerability,
}

/// A clause that names the damage type and has qualifier text: shown to the DM with whether it was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClauseNote {
    /// Which list.
    pub kind: DefenseKind,
    /// The clause text ("from nonmagical attacks").
    pub text: String,
    /// When it applies.
    pub scope: ClauseScope,
    /// It was applied to this damage.
    pub applied: bool,
}

/// The typing of one part of a hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartResult {
    /// The damage type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage_type: Option<DamageType>,
    /// The roll or typed total.
    pub roll: DiceResult,
    /// The damage after typing.
    pub amount: i32,
    /// Immune: 0.
    pub immune: bool,
    /// Halved (rounded down).
    pub resisted: bool,
    /// Doubled.
    pub vulnerable: bool,
    /// Qualified clauses for the DM to read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clauses: Vec<ClauseNote>,
}

/// The typing of a whole hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DamageResult {
    /// Each part.
    pub parts: Vec<PartResult>,
    /// The sum after typing.
    pub total: i32,
}

/// How one amount of one type is typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typed {
    /// The damage after typing.
    pub amount: i32,
    /// Immune.
    pub immune: bool,
    /// Resisted.
    pub resisted: bool,
    /// Vulnerable.
    pub vulnerable: bool,
    /// Qualified clauses.
    pub clauses: Vec<ClauseNote>,
}

fn applies(
    list: &[DamageClause],
    kind: DefenseKind,
    t: DamageType,
    magical: bool,
    apply_qualified: bool,
    notes: &mut Vec<ClauseNote>,
) -> bool {
    let mut any = false;
    for clause in list.iter().filter(|c| c.types.contains(&t)) {
        let scope = clause.scope();
        let on = match scope {
            ClauseScope::Always => true,
            ClauseScope::Nonmagical => !magical,
            ClauseScope::Qualified => apply_qualified,
        };
        if let Some(text) = &clause.text {
            notes.push(ClauseNote {
                kind,
                text: text.clone(),
                scope,
                applied: on,
            });
        }
        any |= on;
    }
    any
}

/// Types `amount` of `damage_type` against `defenses`: immunity makes it 0; otherwise resistance halves it
/// (rounded down) and then vulnerability doubles it. Several resistances to one type count once. Untyped damage
/// is never adjusted.
#[must_use]
pub fn type_damage(
    defenses: &Defenses,
    damage_type: Option<DamageType>,
    amount: i32,
    magical: bool,
    apply_qualified: bool,
) -> Typed {
    let Some(t) = damage_type else {
        return Typed {
            amount,
            immune: false,
            resisted: false,
            vulnerable: false,
            clauses: vec![],
        };
    };
    let mut clauses = Vec::new();
    let immune = applies(
        &defenses.immunities,
        DefenseKind::Immunity,
        t,
        magical,
        apply_qualified,
        &mut clauses,
    );
    let resisted = applies(
        &defenses.resistances,
        DefenseKind::Resistance,
        t,
        magical,
        apply_qualified,
        &mut clauses,
    );
    let vulnerable = applies(
        &defenses.vulnerabilities,
        DefenseKind::Vulnerability,
        t,
        magical,
        apply_qualified,
        &mut clauses,
    );
    if immune {
        return Typed {
            amount: 0,
            immune,
            resisted: false,
            vulnerable: false,
            clauses,
        };
    }
    let mut out = amount;
    if resisted {
        out /= 2;
    }
    if vulnerable {
        out = out.saturating_mul(2);
    }
    Typed {
        amount: out,
        immune,
        resisted,
        vulnerable,
        clauses,
    }
}

/// The concentration save DC for `damage` taken: max(10, half the damage, rounded down), capped at 30 under 2024
/// rules.
#[must_use]
pub fn concentration_dc(edition: Edition, damage: i32) -> u32 {
    let dc = (damage.max(0) / 2).max(10) as u32;
    match edition {
        Edition::Srd521 => dc.min(30),
        Edition::Srd51 => dc,
    }
}
