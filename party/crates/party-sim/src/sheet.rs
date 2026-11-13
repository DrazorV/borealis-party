//! What a combatant can do, as the simulation reads it from its stat block or character sheet: the numbers the
//! combat engine has no field for (saves, attacks, save effects, multiattack steps, a few traits), each action's
//! coverage, and its text (untrusted data, kept out of the engine state).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use borealis_content::monster_types::{Ability as CAbility, AttackKind, Feature};
use borealis_content::MonsterData;
use party_combat::party_dice::{self, DiceExpr, Term};
use party_combat::{AbilityKind, AbilityName, Combatant, Condition, DamageType};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::setup::PcInput;

/// How far the simulation runs an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// Rolled and applied by the engine from parsed numbers.
    Automated,
    /// Text only: an agent may adjudicate it with the closed schema (numbers by index).
    Adjudicable,
    /// Not run at all (reactions other than opportunity attacks, lair actions, auras, summons).
    NotSimulated,
}

/// One damage roll: dice notation and type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DamageRoll {
    pub dice: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage_type: Option<DamageType>,
}

impl DamageRoll {
    /// Twice the average (integer maths: no floats in decisions).
    #[must_use]
    pub fn avg2(&self) -> i32 {
        party_dice::try_parse_notation(&self.dice).map_or(0, |e| avg2(&e))
    }
}

/// Twice the average of an expression.
#[must_use]
pub fn avg2(e: &DiceExpr) -> i32 {
    e.terms
        .iter()
        .map(|t| match *t {
            Term::Dice { count, sides, sign } => {
                let v = i32::try_from(count * (sides + 1)).unwrap_or(0);
                if sign == party_dice::Sign::Minus {
                    -v
                } else {
                    v
                }
            }
            Term::Flat { value } => value * 2,
        })
        .sum()
}

/// Twice the average of a list of rolls.
#[must_use]
pub fn parts_avg2(parts: &[DamageRoll]) -> i32 {
    parts.iter().map(DamageRoll::avg2).sum()
}

/// A condition an attack's hit or a failed save applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rider {
    pub condition: Condition,
    /// The save that avoids it, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save: Option<(AbilityName, i32)>,
    /// Until the end of the target's next turn.
    #[serde(default)]
    pub one_turn: bool,
    /// The target repeats the save at the end of each of its turns.
    #[serde(default)]
    pub repeat: bool,
}

/// An attack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackDef {
    pub to_hit: i32,
    pub parts: Vec<DamageRoll>,
    pub melee: bool,
    pub ranged: bool,
    #[serde(default)]
    pub reach: u32,
    #[serde(default)]
    pub range: u32,
    #[serde(default)]
    pub long: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crit_range: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rider: Option<Rider>,
    /// Attacks per use (a PC's Extra Attack); 1 for a creature.
    #[serde(default = "one")]
    pub count: u32,
}

fn one() -> u32 {
    1
}

/// An area a save effect covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    pub shape: String,
    pub size: u32,
    /// The most targets the band model lets it catch (stated as an assumption).
    pub max_targets: u32,
}

/// A save effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveDef {
    pub ability: AbilityName,
    pub dc: i32,
    #[serde(default)]
    pub parts: Vec<DamageRoll>,
    #[serde(default)]
    pub half: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<Area>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rider: Option<Rider>,
    /// Range in feet for a single-target or placed effect (0: self or touch).
    #[serde(default)]
    pub range: u32,
    /// A spell (Magic Resistance applies).
    #[serde(default)]
    pub spell: bool,
}

/// One step of a multiattack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub key: String,
    pub count: u32,
}

/// A spell a sheet lists, adjudicated (its text is not in the snapshot).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpellDef {
    pub level: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dc: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack: Option<i32>,
    #[serde(default)]
    pub concentration: bool,
}

/// What an action does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "def", rename_all = "snake_case")]
pub enum Def {
    Attack(AttackDef),
    Save(SaveDef),
    Multiattack {
        steps: Vec<Step>,
        /// It moves first (a legendary Pounce: up to half its speed).
        #[serde(default)]
        moves: bool,
    },
    Spell(SpellDef),
    Heal {
        dice: String,
    },
    Text,
}

/// One action of a combatant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionDef {
    /// The engine ability key (spent with `UseAbility`).
    pub key: String,
    pub name: String,
    pub kind: AbilityKind,
    #[serde(flatten)]
    pub def: Def,
    pub coverage: Coverage,
    /// Its text: untrusted data. Never interpolated into instructions.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// The spell slot level it spends, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u8>,
    /// The resource it spends (a PC's Rage), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

/// The few traits the simulation applies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Traits {
    #[serde(default)]
    pub pack_tactics: bool,
    #[serde(default)]
    pub magic_resistance: bool,
    /// HP regained at the start of each of its turns (flagged in the report).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regeneration: Option<i32>,
    /// Trait names and texts, for an agent's advantage claims (untrusted data).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<(String, String)>,
}

/// What the engine lacks about a combatant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimSheet {
    pub saves: BTreeMap<AbilityName, i32>,
    pub con_save: i32,
    pub int: i32,
    pub creature_type: String,
    pub actions: Vec<ActionDef>,
    pub traits: Traits,
    /// Casts spells (target priority for clever foes).
    pub caster: bool,
    /// Never flees (undead, constructs, mindless).
    pub fearless: bool,
    /// Built from a card only: no attacks known.
    pub card_only: bool,
}

impl SimSheet {
    #[must_use]
    pub fn action(&self, key: &str) -> Option<&ActionDef> {
        self.actions.iter().find(|a| a.key == key)
    }

    #[must_use]
    pub fn save(&self, ability: AbilityName) -> i32 {
        self.saves.get(&ability).copied().unwrap_or(0)
    }

    /// Its best melee attack (opportunity attacks).
    #[must_use]
    pub fn best_melee(&self) -> Option<&ActionDef> {
        self.actions
            .iter()
            .filter(|a| a.coverage == Coverage::Automated)
            .filter(|a| matches!(&a.def, Def::Attack(x) if x.melee))
            .filter(|a| a.slot.is_none() && a.resource.is_none())
            .max_by_key(|a| match &a.def {
                Def::Attack(x) => parts_avg2(&x.parts),
                _ => 0,
            })
    }
}

fn ability_name(a: CAbility) -> AbilityName {
    match a {
        CAbility::Str => AbilityName::Str,
        CAbility::Dex => AbilityName::Dex,
        CAbility::Con => AbilityName::Con,
        CAbility::Int => AbilityName::Int,
        CAbility::Wis => AbilityName::Wis,
        CAbility::Cha => AbilityName::Cha,
    }
}

const ABILITIES: [(CAbility, AbilityName); 6] = [
    (CAbility::Str, AbilityName::Str),
    (CAbility::Dex, AbilityName::Dex),
    (CAbility::Con, AbilityName::Con),
    (CAbility::Int, AbilityName::Int),
    (CAbility::Wis, AbilityName::Wis),
    (CAbility::Cha, AbilityName::Cha),
];

/// An ability by its English name ("Dexterity").
#[must_use]
pub fn ability_word(word: &str) -> Option<AbilityName> {
    Some(match word.to_ascii_lowercase().as_str() {
        "strength" | "str" => AbilityName::Str,
        "dexterity" | "dex" => AbilityName::Dex,
        "constitution" | "con" => AbilityName::Con,
        "intelligence" | "int" => AbilityName::Int,
        "wisdom" | "wis" => AbilityName::Wis,
        "charisma" | "cha" => AbilityName::Cha,
        _ => return None,
    })
}

/// A damage type by its word.
#[must_use]
pub fn damage_word(word: &str) -> Option<DamageType> {
    let w = word.to_ascii_lowercase();
    DamageType::ALL.iter().copied().find(|t| t.as_str() == w)
}

fn content_damage(t: borealis_content::monster_types::DamageType) -> Option<DamageType> {
    damage_word(t.as_str())
}

/// A condition by its word.
#[must_use]
pub fn condition_word(word: &str) -> Option<Condition> {
    let w = word.to_ascii_lowercase();
    Condition::ALL.iter().copied().find(|c| c.as_str() == w)
}

const ABILITY_RE: &str = "(Strength|Dexterity|Constitution|Intelligence|Wisdom|Charisma)";

/// 2014: "... must make a DC 13 Dexterity saving throw, taking 22 (5d8) poison damage on a failed save, or half as
/// much damage on a successful one." The 5etools importer leaves `save` empty, so the text is read.
static SAVE_2014: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?i)\bDC (\d+) {ABILITY_RE} saving throw")).expect("regex")
});
/// 2024: "Dexterity Saving Throw: DC 17, each creature in a 30-foot Cone."
static SAVE_2024: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?i)\b{ABILITY_RE} Saving Throw: DC (\d+)")).expect("regex")
});
/// "56 (16d6) Fire damage" / "13 (2d8 + 4) bludgeoning damage".
static DAMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d+) \((\d+d\d+(?:\s*[+\-−]\s*\d+)?)\) ([A-Za-z]+) damage").expect("regex")
});
static HALF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)half as much damage|Success: Half damage|half damage on a success")
        .expect("regex")
});
static AREA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d+)-(?:foot|ft\.?)[- ](?:radius )?(cone|line|cube|sphere|emanation|cylinder|radius|square)")
        .expect("regex")
});
static RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bwithin (\d+) (?:feet|ft)").expect("regex"));
static REGENERATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)regains? (\d+) hit points? at the start of its turn").expect("regex")
});
static CONDITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)knocked (prone)|(?:has|have|gains?) the (\w+) condition|or (?:be|become) (?:\w+ )?(blinded|charmed|deafened|frightened|grappled|paralyzed|petrified|poisoned|prone|restrained|stunned|unconscious|incapacitated)\b|(?:target|creature) is (grappled|restrained|paralyzed|poisoned|stunned|blinded|frightened|charmed)\b",
    )
    .expect("regex")
});

/// The most targets an area catches under the band model (an assumption the report states).
#[must_use]
pub fn max_targets(shape: &str, size: u32) -> u32 {
    let n = match shape {
        "line" => size / 20,
        _ => size / 5,
    };
    n.clamp(1, 8)
}

fn notation(dice: &str) -> String {
    dice.replace([' ', '−'], "").replace("−", "-")
}

/// The save effect a text describes (2014 or 2024 wording), if it has one.
#[must_use]
pub fn parse_save(text: &str) -> Option<SaveDef> {
    let (ability, dc, at) = if let Some(c) = SAVE_2014.captures(text) {
        (
            ability_word(&c[2])?,
            c[1].parse().ok()?,
            c.get(0).map_or(0, |m| m.start()),
        )
    } else {
        let c = SAVE_2024.captures(text)?;
        (
            ability_word(&c[1])?,
            c[2].parse().ok()?,
            c.get(0).map_or(0, |m| m.start()),
        )
    };
    let tail = &text[at..];
    let parts: Vec<DamageRoll> = DAMAGE
        .captures_iter(tail)
        .take(3)
        .map(|c| DamageRoll {
            dice: notation(&c[2]),
            damage_type: damage_word(&c[3]),
        })
        .collect();
    let area = AREA.captures(text).and_then(|c| {
        let size: u32 = c[1].parse().ok()?;
        let shape = c[2].to_ascii_lowercase();
        Some(Area {
            max_targets: max_targets(&shape, size),
            shape,
            size,
        })
    });
    let range = RANGE
        .captures(text)
        .and_then(|c| c[1].parse().ok())
        .unwrap_or(0);
    let rider = parse_condition(tail).map(|condition| Rider {
        condition,
        save: Some((ability, dc)),
        one_turn: one_turn(tail),
        repeat: repeats(tail),
    });
    if parts.is_empty() && rider.is_none() {
        return None;
    }
    Some(SaveDef {
        ability,
        dc,
        half: HALF.is_match(tail) && !parts.is_empty(),
        parts,
        area,
        rider,
        range,
        spell: false,
    })
}

fn one_turn(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("until the end of its next turn")
        || t.contains("until the end of the target's next turn")
        || t.contains("until the start of")
}

fn repeats(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("repeat the saving throw") || t.contains("repeats the save")
}

/// The first condition a text applies, if any.
#[must_use]
pub fn parse_condition(text: &str) -> Option<Condition> {
    CONDITION.captures(text).and_then(|c| {
        (1..=4)
            .filter_map(|i| c.get(i))
            .find_map(|m| condition_word(m.as_str()))
    })
}

fn rider_of(text: &str) -> Option<Rider> {
    let after = text
        .find("Hit:")
        .or_else(|| text.find("Hit "))
        .map_or(text, |i| &text[i..]);
    let condition = parse_condition(after)?;
    let save = parse_save(after).map(|s| (s.ability, s.dc)).or_else(|| {
        SAVE_2014
            .captures(after)
            .and_then(|c| Some((ability_word(&c[2])?, c[1].parse().ok()?)))
    });
    Some(Rider {
        condition,
        save,
        one_turn: one_turn(after),
        repeat: repeats(after),
    })
}

fn attack_of(f: &Feature) -> Option<AttackDef> {
    let a = f.attack.as_ref()?;
    let (melee, ranged) = match a.kind {
        AttackKind::MeleeWeapon | AttackKind::MeleeSpell | AttackKind::Melee => (true, false),
        AttackKind::RangedWeapon | AttackKind::RangedSpell | AttackKind::Ranged => (false, true),
        AttackKind::MeleeOrRangedWeapon | AttackKind::MeleeOrRanged => (true, true),
    };
    let parts: Vec<DamageRoll> = a
        .damage
        .iter()
        .map(|d| DamageRoll {
            dice: d
                .dice
                .as_deref()
                .map_or_else(|| d.average.to_string(), notation),
            damage_type: content_damage(d.damage_type),
        })
        .collect();
    Some(AttackDef {
        to_hit: a.to_hit,
        parts,
        melee,
        ranged,
        reach: a.reach.unwrap_or(5),
        range: a.range.unwrap_or(0),
        long: a.long_range.or(a.range).unwrap_or(0),
        crit_range: None,
        rider: rider_of(&f.text),
        count: 1,
    })
}

fn save_of(f: &Feature) -> Option<SaveDef> {
    let parsed = parse_save(&f.text);
    let Some(s) = &f.save else {
        return parsed;
    };
    let parts = s
        .damage
        .iter()
        .map(|d| DamageRoll {
            dice: d
                .dice
                .as_deref()
                .map_or_else(|| d.average.to_string(), notation),
            damage_type: content_damage(d.damage_type),
        })
        .collect();
    Some(SaveDef {
        ability: ability_name(s.ability),
        dc: i32::from(s.dc),
        parts,
        half: s.half_on_success,
        area: parsed.as_ref().and_then(|p| p.area.clone()),
        rider: parsed.as_ref().and_then(|p| p.rider.clone()),
        range: parsed.as_ref().map_or(0, |p| p.range),
        spell: false,
    })
}

const NUMBERS: [(&str, u32); 8] = [
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("a", 1),
    ("an", 1),
];

fn number_word(w: &str) -> Option<u32> {
    NUMBERS
        .iter()
        .find(|(n, _)| *n == w)
        .map(|(_, v)| *v)
        .or_else(|| w.parse().ok())
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn stem(w: &str) -> &str {
    w.strip_suffix("es")
        .filter(|s| s.ends_with("sh") || s.ends_with("ch"))
        .or_else(|| w.strip_suffix('s'))
        .unwrap_or(w)
}

/// The attack count a multiattack text states ("makes two attacks"), as a cross-check.
#[must_use]
pub fn count_in_text(text: &str) -> Option<u32> {
    let w = words(text);
    w.iter().position(|x| x == "makes").and_then(|i| {
        w.get(i + 1)
            .and_then(|x| number_word(x))
            .filter(|_| w.get(i + 1).is_some_and(|x| x != "a" && x != "an"))
    })
}

/// The steps of a multiattack text over the creature's attack actions (`(key, name)`), or `None` when the text is
/// not one of the common phrasings (it then stays adjudicable).
#[must_use]
pub fn parse_multiattack(text: &str, attacks: &[(String, String)]) -> Option<Vec<Step>> {
    let total = count_in_text(text)?;
    let w = words(text);
    let mut steps: Vec<Step> = Vec::new();
    let mut seen_any = false;
    for (key, name) in attacks {
        let name_words = words(name);
        let Some(first) = name_words.first() else {
            continue;
        };
        let first = stem(first).to_string();
        let mut count = 0;
        let mut found = false;
        for (i, x) in w.iter().enumerate() {
            if stem(x) != first {
                continue;
            }
            found = true;
            // "two with its claws", "one with its bite", "two Greatclub attacks", "two Rend attacks".
            let mut n = None;
            for back in 1..=4 {
                let Some(j) = i.checked_sub(back) else { break };
                if w[j] == "makes" || w[j] == "and" && back > 1 {
                    break;
                }
                if let Some(v) = number_word(&w[j]) {
                    n = Some(v);
                    break;
                }
            }
            if let Some(v) = n {
                // "makes two Greatclub attacks": the leading count belongs to this name only.
                count += v;
            }
        }
        if found {
            seen_any = true;
            if count > 0 {
                steps.push(Step {
                    key: key.clone(),
                    count,
                });
            }
        }
    }
    if !seen_any && attacks.len() == 1 {
        let (key, _) = &attacks[0];
        return Some(vec![Step {
            key: key.clone(),
            count: total,
        }]);
    }
    let sum: u32 = steps.iter().map(|s| s.count).sum();
    if sum == total && !steps.is_empty() {
        return Some(steps);
    }
    // "makes two attacks, using Scimitar or Shortbow in any combination": the best one, `total` times.
    if steps.is_empty() && seen_any {
        let lower = text.to_lowercase();
        if lower.contains(" or ") {
            let named: Vec<&(String, String)> = attacks
                .iter()
                .filter(|(_, n)| {
                    words(n)
                        .first()
                        .is_some_and(|f| w.iter().any(|x| stem(x) == stem(f)))
                })
                .collect();
            if let Some((key, _)) = named.first() {
                return Some(vec![Step {
                    key: key.clone(),
                    count: total,
                }]);
            }
        }
    }
    None
}

/// The sheet of a creature built from its stat block (`combatant` from [`party_encounter::combatant`]).
#[must_use]
pub fn creature_sheet(c: &Combatant, data: Option<&MonsterData>) -> SimSheet {
    let mut sheet = SimSheet::default();
    let Some(d) = data else {
        sheet.int = 10;
        sheet.card_only = true;
        return sheet;
    };
    for (ca, name) in ABILITIES {
        let v = d.saves.get(&ca).copied().unwrap_or_else(|| d.modifier(ca));
        sheet.saves.insert(name, v);
    }
    sheet.con_save = sheet.save(AbilityName::Con);
    sheet.int = i32::from(d.abilities.int);
    sheet.creature_type = d.creature_type.as_str().to_string();
    sheet.fearless = matches!(
        sheet.creature_type.as_str(),
        "undead" | "construct" | "ooze"
    ) || sheet.int <= 2;
    let features: Vec<(&Feature, AbilityKind)> = d
        .traits
        .iter()
        .map(|f| (f, AbilityKind::Passive))
        .chain(d.actions.iter().map(|f| (f, AbilityKind::Action)))
        .chain(
            d.bonus_actions
                .iter()
                .map(|f| (f, AbilityKind::BonusAction)),
        )
        .chain(d.reactions.iter().map(|f| (f, AbilityKind::Reaction)))
        .chain(
            d.legendary
                .iter()
                .flat_map(|l| l.actions.iter().map(|f| (f, AbilityKind::Legendary))),
        )
        .collect();
    // The engine's abilities are in the same order as the features (party_encounter::combatant::build_creature).
    let keys: Vec<&str> = c.abilities.iter().map(|a| a.key.as_str()).collect();
    let mut defs: Vec<ActionDef> = Vec::new();
    for ((f, kind), key) in features.iter().zip(keys.iter()) {
        if *kind == AbilityKind::Passive {
            let lower = f.name.to_lowercase();
            if lower.starts_with("pack tactics") {
                sheet.traits.pack_tactics = true;
            }
            if lower.starts_with("magic resistance") {
                sheet.traits.magic_resistance = true;
            }
            if lower.starts_with("regeneration") {
                sheet.traits.regeneration = REGENERATION
                    .captures(&f.text)
                    .and_then(|c| c[1].parse().ok());
            }
            if f.spellcasting.is_some() || lower.contains("spellcasting") {
                sheet.caster = true;
            }
            sheet.traits.texts.push((f.name.clone(), f.text.clone()));
            continue;
        }
        if f.spellcasting.is_some() {
            sheet.caster = true;
        }
        let def = if let Some(a) = attack_of(f) {
            Def::Attack(a)
        } else if let Some(s) = save_of(f) {
            Def::Save(s)
        } else {
            Def::Text
        };
        let coverage = match (&def, kind) {
            (_, AbilityKind::Reaction) => Coverage::NotSimulated,
            (Def::Text, _) => Coverage::Adjudicable,
            _ => Coverage::Automated,
        };
        defs.push(ActionDef {
            key: (*key).to_string(),
            name: f.name.clone(),
            kind: *kind,
            def,
            coverage,
            text: f.text.clone(),
            slot: None,
            resource: None,
        });
    }
    // Multiattack, over the parsed actions.
    let attacks: Vec<(String, String)> = defs
        .iter()
        .filter(|a| a.kind == AbilityKind::Action && matches!(a.def, Def::Attack(_) | Def::Save(_)))
        .map(|a| (a.key.clone(), a.name.clone()))
        .collect();
    for a in &mut defs {
        let multi =
            a.kind == AbilityKind::Action && a.name.to_lowercase().starts_with("multiattack");
        // Legendary actions that make another action's attack ("makes one Rend attack", "makes a tail attack").
        let legendary = a.kind == AbilityKind::Legendary;
        if a.def != Def::Text || !(multi || legendary) {
            continue;
        }
        let text = if legendary {
            a.text
                .replace("makes a ", "makes one ")
                .replace("makes an ", "makes one ")
        } else {
            a.text.clone()
        };
        if let Some(steps) = parse_multiattack(&text, &attacks) {
            let lower = a.text.to_lowercase();
            a.def = Def::Multiattack {
                steps,
                moves: legendary && (lower.contains("moves up to") || lower.contains("move up to")),
            };
            a.coverage = Coverage::Automated;
        }
    }
    sheet.actions = defs;
    sheet
}

static RANGE_PAIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+)\s*/\s*(\d+)").expect("regex"));
static FIRST_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+)").expect("regex"));
static PC_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\d+d\d+|\d+)|([+\-])|([A-Za-z]+)").expect("regex"));

/// A PC attack row's damage text ("1d8+3 slashing + 1d6 fire") as rolls.
#[must_use]
pub fn pc_damage(text: &str) -> Vec<DamageRoll> {
    let mut out: Vec<DamageRoll> = Vec::new();
    let mut cur: Option<String> = None;
    let mut sign = '+';
    let close = |cur: &mut Option<String>, ty: Option<DamageType>, out: &mut Vec<DamageRoll>| {
        if let Some(d) = cur.take() {
            if party_dice::try_parse_notation(&d).is_some() && out.len() < 4 {
                out.push(DamageRoll {
                    dice: d,
                    damage_type: ty,
                });
            }
        }
    };
    for c in PC_TOKEN.captures_iter(&text.replace('−', "-")) {
        if let Some(n) = c.get(1) {
            let n = n.as_str();
            match &mut cur {
                // A flat number joins the dice before it ("1d8 + 3"); new dice start a new roll.
                Some(d) if !n.contains('d') => d.push_str(&format!("{sign}{n}")),
                _ => {
                    close(&mut cur, None, &mut out);
                    cur = Some(n.to_string());
                }
            }
            sign = '+';
        } else if let Some(s) = c.get(2) {
            sign = s.as_str().chars().next().unwrap_or('+');
        } else if let Some(w) = c.get(3) {
            if let Some(t) = damage_word(w.as_str()) {
                close(&mut cur, Some(t), &mut out);
            }
        }
    }
    close(&mut cur, None, &mut out);
    out
}

fn signed(text: &str) -> Option<i32> {
    let t = text.trim().replace('−', "-");
    let t = t.strip_prefix('+').unwrap_or(&t);
    t.parse().ok()
}

/// Attacks per Attack action from the sheet's features ("Extra Attack", "Extra Attack (2)").
fn attacks_per_action(features: &[String]) -> u32 {
    let mut n = 1;
    for f in features {
        let lower = f.to_lowercase();
        if lower.starts_with("extra attack") {
            let extra = FIRST_NUMBER
                .captures(&lower)
                .and_then(|c| c[1].parse::<u32>().ok())
                .unwrap_or(1);
            n = n.max(1 + extra);
        } else if lower.starts_with("three extra attacks") {
            n = n.max(4);
        }
    }
    n
}

/// The sheet of a PC from its snapshot subset. `keys` are the engine ability keys the setup gave its rows.
#[must_use]
pub fn pc_sheet(pc: &PcInput, keys: &[String], edition: party_combat::Edition) -> SimSheet {
    let mut sheet = SimSheet {
        int: pc.abilities.score(AbilityName::Int).unwrap_or(10),
        creature_type: "humanoid".into(),
        ..SimSheet::default()
    };
    for (_, name) in ABILITIES {
        sheet.saves.insert(name, pc.abilities.save_bonus(name));
    }
    sheet.con_save = sheet.save(AbilityName::Con);
    sheet.card_only = pc.card_only;
    sheet.caster = !pc.spellcasting.slots.is_empty() || !pc.spellcasting.classes.is_empty();
    let per_action = attacks_per_action(&pc.feature_names());
    let mut k = keys.iter();
    for row in &pc.attacks {
        let Some(key) = k.next() else { break };
        let range = row.range.to_lowercase();
        let pair = RANGE_PAIR.captures(&range);
        let near: u32 = FIRST_NUMBER
            .captures(&range)
            .and_then(|c| c[1].parse().ok())
            .unwrap_or(5);
        let thrown = row.description.to_lowercase().contains("thrown");
        let (melee, ranged, reach, r, long) = match &pair {
            Some(p) => {
                let a: u32 = p[1].parse().unwrap_or(0);
                let b: u32 = p[2].parse().unwrap_or(a);
                (thrown, true, 5, a, b)
            }
            None if near > 10 => (false, true, 5, near, near),
            None => (true, false, near.max(5), 0, 0),
        };
        let parts = pc_damage(&row.damage);
        let to_hit = row.to_hit.or_else(|| signed(&row.attack_bonus));
        let text = format!("{} {}", row.damage, row.description);
        let def = match to_hit {
            Some(to_hit) if !parts.is_empty() => Def::Attack(AttackDef {
                to_hit,
                parts: parts.clone(),
                melee,
                ranged,
                reach,
                range: r,
                long,
                crit_range: (row.crit_range > 0 && row.crit_range < 20)
                    .then_some(u32::from(row.crit_range)),
                rider: None,
                count: if melee || !row.is_spell() {
                    per_action
                } else {
                    1
                },
            }),
            _ => match parse_save(&text).or_else(|| pc_save(&text, &parts, &row.attack_bonus)) {
                Some(mut s) => {
                    s.range = if r > 0 { r } else { near };
                    s.spell = row.is_spell();
                    Def::Save(s)
                }
                None => Def::Text,
            },
        };
        let coverage = if def == Def::Text {
            Coverage::Adjudicable
        } else {
            Coverage::Automated
        };
        sheet.actions.push(ActionDef {
            key: key.clone(),
            name: row.name.clone(),
            kind: AbilityKind::Action,
            def,
            coverage,
            text: String::new(),
            slot: None,
            resource: None,
        });
    }
    // Spells: adjudicated (the snapshot has no spell text), one option per known combat spell.
    let dc = pc.spellcasting.classes.first().map(|c| c.save_dc);
    let atk = pc.spellcasting.classes.first().map(|c| c.attack_bonus);
    let modifier = dc.map_or(0, |d| d - 8 - pc.stats.proficiency_bonus.max(2));
    for (spell, key) in pc.spells().into_iter().zip(k.by_ref()) {
        let (def, kind, coverage) = match known_heal(&spell.name, edition, modifier) {
            Some((dice, kind)) => (Def::Heal { dice }, kind, Coverage::Automated),
            None => (
                Def::Spell(SpellDef {
                    level: u8::try_from(spell.level).unwrap_or(0),
                    dc,
                    attack: atk,
                    concentration: spell.concentration,
                }),
                AbilityKind::Action,
                Coverage::Adjudicable,
            ),
        };
        sheet.actions.push(ActionDef {
            key: key.clone(),
            name: spell.name.clone(),
            kind,
            def,
            coverage,
            text: String::new(),
            slot: (spell.level > 0).then(|| u8::try_from(spell.level).unwrap_or(1)),
            resource: None,
        });
    }
    for (res, key) in pc.play.resources.iter().zip(k) {
        sheet.actions.push(ActionDef {
            key: key.clone(),
            name: res.name.clone(),
            kind: AbilityKind::Free,
            def: Def::Text,
            coverage: Coverage::Adjudicable,
            text: String::new(),
            slot: None,
            resource: Some(format!("res_{}", res.id)),
        });
    }
    sheet
}

/// The two SRD healing spells the simulation knows without text (slot level 1): dice and casting time.
pub(crate) fn known_heal(
    name: &str,
    edition: party_combat::Edition,
    modifier: i32,
) -> Option<(String, AbilityKind)> {
    let new = edition == party_combat::Edition::Srd521;
    let m = modifier.max(0);
    match name.to_lowercase().as_str() {
        "cure wounds" => Some((
            format!("{}d8+{m}", if new { 2 } else { 1 }),
            AbilityKind::Action,
        )),
        "healing word" => Some((
            format!("{}d4+{m}", if new { 2 } else { 1 }),
            AbilityKind::BonusAction,
        )),
        _ => None,
    }
}

/// "DC 13 DEX" written in an attack row's bonus column (cantrips such as Sacred Flame).
fn pc_save(text: &str, parts: &[DamageRoll], bonus: &str) -> Option<SaveDef> {
    let joined = format!("{bonus} {text}");
    let re = Regex::new(r"(?i)DC\s*(\d+)\s*(STR|DEX|CON|INT|WIS|CHA|Strength|Dexterity|Constitution|Intelligence|Wisdom|Charisma)")
        .ok()?;
    let c = re.captures(&joined)?;
    if parts.is_empty() {
        return None;
    }
    Some(SaveDef {
        ability: ability_word(&c[2])?,
        dc: c[1].parse().ok()?,
        parts: parts.to_vec(),
        half: joined.to_lowercase().contains("half"),
        area: None,
        rider: None,
        range: 0,
        spell: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ab(names: &[&str]) -> Vec<(String, String)> {
        names
            .iter()
            .map(|n| (n.to_lowercase().replace(' ', "_"), (*n).to_string()))
            .collect()
    }

    #[test]
    fn multiattack_phrasings() {
        let s = parse_multiattack(
            "The ogre makes two Greatclub attacks.",
            &ab(&["Greatclub", "Javelin"]),
        )
        .unwrap();
        assert_eq!(
            s,
            vec![Step {
                key: "greatclub".into(),
                count: 2
            }]
        );
        let s = parse_multiattack(
            "The dragon makes three attacks: one with its bite and two with its claws.",
            &ab(&["Bite", "Claw"]),
        )
        .unwrap();
        assert_eq!(
            s,
            vec![
                Step {
                    key: "bite".into(),
                    count: 1
                },
                Step {
                    key: "claw".into(),
                    count: 2
                }
            ]
        );
        let s = parse_multiattack(
            "The dragon makes three Rend attacks.",
            &ab(&["Rend", "Fire Breath"]),
        )
        .unwrap();
        assert_eq!(
            s,
            vec![Step {
                key: "rend".into(),
                count: 3
            }]
        );
        let s = parse_multiattack(
            "The goblin makes two attacks, using Scimitar or Shortbow in any combination.",
            &ab(&["Scimitar", "Shortbow"]),
        )
        .unwrap();
        assert_eq!(
            s,
            vec![Step {
                key: "scimitar".into(),
                count: 2
            }]
        );
        let s = parse_multiattack(
            "The bandit captain makes three melee attacks.",
            &ab(&["Scimitar"]),
        )
        .unwrap();
        assert_eq!(
            s,
            vec![Step {
                key: "scimitar".into(),
                count: 3
            }]
        );
        assert_eq!(
            count_in_text("The veteran makes two longsword attacks."),
            Some(2)
        );
    }

    #[test]
    fn unparsed_multiattack_stays_adjudicable() {
        assert!(parse_multiattack(
            "The mage casts a spell and then does something unusual.",
            &ab(&["Bite"])
        )
        .is_none());
        assert!(parse_multiattack(
            "The creature makes four attacks: one with its bite and one with its tail.",
            &ab(&["Bite", "Tail"])
        )
        .is_none());
    }

    #[test]
    fn save_2014_and_2024() {
        let s = parse_save("The dragon exhales fire in a 30-foot cone. Each creature in that area must make a DC 17 Dexterity saving throw, taking 56 (16d6) fire damage on a failed save, or half as much damage on a successful one.").unwrap();
        assert_eq!((s.ability, s.dc, s.half), (AbilityName::Dex, 17, true));
        assert_eq!(s.parts[0].dice, "16d6");
        assert_eq!(s.parts[0].damage_type, Some(DamageType::Fire));
        assert_eq!(s.area.as_ref().unwrap().shape, "cone");
        assert_eq!(s.area.as_ref().unwrap().max_targets, 6);
        let s = parse_save("Dexterity Saving Throw: DC 17, each creature in a 30-foot Cone. Failure: 56 (16d6) Fire damage. Success: Half damage.").unwrap();
        assert_eq!((s.ability, s.dc, s.half), (AbilityName::Dex, 17, true));
        let s = parse_save(
            "The target must succeed on a DC 11 Strength saving throw or be knocked prone.",
        )
        .unwrap();
        assert_eq!(s.rider.unwrap().condition, Condition::Prone);
        assert!(parse_save("The goblin takes the Disengage or Hide action.").is_none());
    }

    #[test]
    fn pc_damage_rows() {
        let p = pc_damage("1d8+3 slashing + 1d6 fire");
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].dice, "1d8+3");
        assert_eq!(p[1].damage_type, Some(DamageType::Fire));
        assert_eq!(parts_avg2(&p), 2 * 3 + 9 + 7);
    }
}
