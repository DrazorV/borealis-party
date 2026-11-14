//! Monster spellbooks (release 0.3.2): the spells a creature's Spellcasting and Innate Spellcasting entries list,
//! with how often each can be cast, read from four places (plan "spells in combat", section 2):
//!
//! * (a) the structured `Feature.spellcasting` lists of an SRD 5.2.1 block (spell ids included);
//! * (b) the list grammar the SRD 5.1 text and our importer write: "Nth level (k slots):", "k/day each:",
//!   "At will:", "Cantrips (at will):", the DC and to-hit, "Nth-level spellcaster"; a line that doesn't parse is
//!   kept as a chip, never dropped;
//! * (c) a raw 5etools `spellcasting` array kept with an imported monster (the bestiary's capture);
//! * (d) the DM's own spells for this encounter (`Overrides.spells`).
//!
//! [`seed`] fills a combatant's spell slots and per-day pools from the books, so a precast or cast spell spends a
//! real slot. Linking each name to an installed spell element is [`link`], through the canonical-spell catalog of
//! `party-spells` (one spell per name and ruleset).

use std::collections::BTreeMap;

use borealis_content::monster_types::{Feature, SpellFrequency, Usage};
use borealis_content::MonsterData;
use party_combat::{Combatant, Pool, Reset, Resource};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most spells one book may list (a guard against runaway text).
pub const MAX_SPELLS: usize = 80;

/// How often the spells of one list can be cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Frequency {
    /// "At will", "Cantrips (at will)".
    AtWill,
    /// "2/day each" (`each`: every spell gets that many) or "1/day" (one pool shared by the list).
    #[serde(rename_all = "camelCase")]
    PerDay {
        /// Uses per day.
        uses: u8,
        /// True if every spell of the list has its own uses.
        each: bool,
    },
    /// "3rd level (3 slots)": the spells of that level, cast with the book's slots.
    #[serde(rename_all = "camelCase")]
    Slots {
        /// The spell level.
        level: u8,
        /// Slots of that level.
        slots: u8,
    },
}

/// One spell a book lists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookSpell {
    /// The name as printed ("fireball", "Acid Arrow").
    pub name: String,
    /// The content id the block gives (SRD 5.2.1 structured lists) or the DM picked (overrides).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The printed parenthetical ("self only", "level 3 version").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// "level N version": always cast at that level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_level: Option<u8>,
    /// "included in AC": the printed AC already counts it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub baked_in: bool,
    /// Marked as cast before combat (an asterisk and its footnote, or "cast before combat").
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub precast: bool,
}

/// One list of a book: a frequency and its spells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookList {
    /// How often.
    pub frequency: Frequency,
    /// The spells, in printed order.
    pub spells: Vec<BookSpell>,
}

/// Where a book was read from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BookOrigin {
    /// The block's structured spell lists (SRD 5.2.1).
    Structured,
    /// The feature's printed text.
    #[default]
    Text,
    /// A raw 5etools `spellcasting` entry.
    FiveEtools,
    /// The DM's own spells for this encounter.
    Override,
}

/// A creature's spellbook from one Spellcasting entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spellbook {
    /// The entry's name ("Spellcasting", "Innate Spellcasting").
    pub name: String,
    /// Where it came from.
    pub origin: BookOrigin,
    /// The entry's kind of action in a 2024 block (cast with an action); `None` for a trait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// The spellcasting ability, short (`int`, `wis`, `cha`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ability: Option<String>,
    /// Spell save DC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_dc: Option<u8>,
    /// Spell attack bonus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack_bonus: Option<i32>,
    /// "an 18th-level spellcaster" (2014 blocks); sets cantrip damage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caster_level: Option<u8>,
    /// "the following wizard spells" (2014 blocks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// The lists.
    pub lists: Vec<BookList>,
    /// Text that didn't parse (a line, a footnote, trait text run into the list): shown as chips.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chips: Vec<String>,
}

impl Spellbook {
    /// Every spell with its list's frequency.
    pub fn spells(&self) -> impl Iterator<Item = (Frequency, &BookSpell)> {
        self.lists
            .iter()
            .flat_map(|l| l.spells.iter().map(move |s| (l.frequency, s)))
    }

    /// Slots by spell level.
    #[must_use]
    pub fn slots(&self) -> BTreeMap<u8, u8> {
        let mut out = BTreeMap::new();
        for l in &self.lists {
            if let Frequency::Slots { level, slots } = l.frequency {
                let e = out.entry(level).or_insert(0u8);
                *e = (*e).max(slots);
            }
        }
        out
    }
}

/// True for the name of a Spellcasting entry ("Spellcasting", "Innate Spellcasting", "Shared Spellcasting").
#[must_use]
pub fn is_spellcasting(name: &str) -> bool {
    name.to_lowercase().contains("spellcasting")
}

const ABILITIES: [(&str, &str); 6] = [
    ("strength", "str"),
    ("dexterity", "dex"),
    ("constitution", "con"),
    ("intelligence", "int"),
    ("wisdom", "wis"),
    ("charisma", "cha"),
];

/// The number right after `marker` in `text` ("DC 17" with "DC ").
fn number_after(text: &str, marker: &str) -> Option<u32> {
    let i = text.find(marker)? + marker.len();
    let digits: String = text[i..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// "+9 to hit" anywhere in the text.
fn to_hit(text: &str) -> Option<i32> {
    let i = text.find(" to hit")?;
    let head = &text[..i];
    let start = head.rfind(['+', '-'])?;
    let n: i32 = head[start + 1..].trim().parse().ok()?;
    Some(if head[start..].starts_with('-') {
        -n
    } else {
        n
    })
}

/// "an 18th-level spellcaster" or "a 1st-level spellcaster".
fn caster_level(text: &str) -> Option<u8> {
    let i = text.find("-level spellcaster")?;
    let head = &text[..i];
    let word = head.rsplit(' ').next()?;
    let digits: String = word.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|n| (1..=20).contains(n))
}

/// The spellcasting ability: "spellcasting ability is Charisma", "using Wisdom as the spellcasting ability".
fn ability_of(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let at = |marker: &str| -> Option<String> {
        let i = lower.find(marker)? + marker.len();
        let word: String = lower[i..]
            .trim_start()
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect();
        ABILITIES
            .iter()
            .find(|(long, _)| *long == word)
            .map(|(_, short)| (*short).to_string())
    };
    at("spellcasting ability is").or_else(|| at("using"))
}

/// "the following wizard spells" → `wizard`.
fn class_of(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let i = lower.find("following ")? + "following ".len();
    let word: String = lower[i..]
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    let rest = lower[i + word.len()..].trim_start();
    (rest.starts_with("spells") && word != "spells").then_some(word)
}

/// The frequency a list line's prefix (before ':') gives.
fn frequency_of(prefix: &str) -> Option<Frequency> {
    let p = prefix.trim().to_lowercase();
    if p == "at will" || p.starts_with("cantrips") {
        return Some(Frequency::AtWill);
    }
    if let Some((n, rest)) = p.split_once("/day") {
        let uses: u8 = n.trim().parse().ok()?;
        let rest = rest.trim();
        return (rest.is_empty() || rest == "each").then_some(Frequency::PerDay {
            uses,
            each: rest == "each",
        });
    }
    // "3rd level (3 slots)", "1st level (1 slot)"
    let (head, tail) = p.split_once('(')?;
    let head = head.trim();
    let level_word = head.strip_suffix("level")?.trim();
    let level: u8 = level_word
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .parse()
        .ok()?;
    let slots: u8 = tail.split_whitespace().next()?.parse().ok()?;
    let tail_ok = tail.contains("slot");
    ((1..=9).contains(&level) && tail_ok).then_some(Frequency::Slots { level, slots })
}

/// Split at commas that are not inside parentheses.
fn split_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in text.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if ch == ',' && depth <= 0 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

/// "level 3 version" → 3.
fn fixed_level(note: &str) -> Option<u8> {
    let lower = note.to_lowercase();
    let i = lower.find("level ")? + "level ".len();
    let digits: String = lower[i..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let rest = &lower[i + digits.len()..];
    if rest.trim_start().starts_with("version") {
        digits.parse().ok()
    } else {
        None
    }
}

/// A spell entry with its note and flags, from a printed name with its parenthetical.
fn spell_from(name: &str, note: Option<String>) -> BookSpell {
    let mut s = BookSpell {
        name: name.trim().to_string(),
        ..BookSpell::default()
    };
    if let Some(n) = note.filter(|n| !n.trim().is_empty()) {
        let lower = n.to_lowercase();
        s.fixed_level = fixed_level(&n);
        s.baked_in = lower.contains("included in ac");
        s.precast = lower.contains("cast before combat");
        s.note = Some(n.trim().to_string());
    }
    s
}

/// One printed list segment: "mage armor", "invisibility (self only)", "stoneskin*", "sleep Magic Resistance. The
/// hag ..." (the next trait run into the list). Returns the spell (if any), whether it is starred, and leftover text.
fn parse_segment(raw: &str) -> (Option<BookSpell>, bool, Option<String>) {
    let mut text = raw.trim().to_string();
    let mut leftover = None;
    let mut starred = false;
    // A star ends the name; text after it is a footnote ("time stop *The archmage casts ...").
    if let Some(i) = text.find('*') {
        starred = true;
        let after = text[i + 1..].trim().to_string();
        if after.chars().any(char::is_alphabetic) {
            leftover = Some(after);
        }
        text.truncate(i);
    }
    // The parenthetical note.
    let mut note = None;
    if let (Some(open), Some(close)) = (text.find('('), text.rfind(')')) {
        if open < close {
            note = Some(text[open + 1..close].to_string());
            let tail = text[close + 1..].trim().to_string();
            text = text[..open].to_string();
            if !tail.is_empty() {
                text.push(' ');
                text.push_str(&tail);
            }
        }
    }
    let mut name = text.trim().trim_end_matches('.').trim().to_string();
    // Text run into the list: cut at the first sentence end, and for a lowercase list (SRD 5.1 prints spell names in
    // lowercase) at the first capitalised word.
    if let Some(i) = name.find(". ") {
        let rest = name[i + 2..].trim().to_string();
        leftover = Some(match leftover {
            Some(l) => format!("{rest} {l}"),
            None => rest,
        });
        name.truncate(i);
    }
    if name.chars().next().is_some_and(char::is_lowercase) {
        let words: Vec<&str> = name.split(' ').collect();
        if let Some(cut) = words
            .iter()
            .position(|w| w.chars().next().is_some_and(char::is_uppercase))
        {
            let rest = words[cut..].join(" ");
            leftover = Some(match leftover {
                Some(l) => format!("{rest}. {l}"),
                None => rest,
            });
            name = words[..cut].join(" ");
        }
    }
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 60 {
        let all = raw.trim().to_string();
        return (None, starred, (!all.is_empty()).then_some(all));
    }
    (Some(spell_from(&name, note)), starred, leftover)
}

/// The names of one list, with the star rules: a segment starting with `*` stars the previous spell
/// ("mage armor,* magic missile").
fn parse_names(text: &str, chips: &mut Vec<String>) -> (Vec<BookSpell>, bool) {
    let mut out: Vec<BookSpell> = Vec::new();
    let mut any_star = false;
    for seg in split_names(text) {
        let mut seg = seg.trim().to_string();
        while let Some(rest) = seg.strip_prefix('*') {
            if let Some(prev) = out.last_mut() {
                prev.precast = true;
                any_star = true;
            }
            seg = rest.trim().to_string();
        }
        if seg.is_empty() {
            continue;
        }
        let (spell, starred, leftover) = parse_segment(&seg);
        if let Some(mut s) = spell {
            if starred {
                s.precast = true;
                any_star = true;
            }
            if out.len() < MAX_SPELLS {
                out.push(s);
            }
        }
        if let Some(l) = leftover {
            chips.push(l);
        }
    }
    (out, any_star)
}

/// The header numbers (ability, DC, to-hit, caster level, class) into `book`.
fn read_header(book: &mut Spellbook, header: &str) {
    book.ability = ability_of(header);
    book.save_dc = number_after(header, "DC ").and_then(|n| u8::try_from(n).ok());
    book.attack_bonus = to_hit(header);
    book.caster_level = caster_level(header);
    book.class = class_of(header);
}

/// "The mephit can innately cast heat metal (spell save DC 10), requiring ..." and "The archmage can cast disguise
/// self and invisibility at will": spells named in the header itself.
fn header_spells(header: &str, usage: Option<&Usage>, chips: &mut Vec<String>) -> Vec<BookList> {
    let lower = header.to_lowercase();
    let mut out = Vec::new();
    let at = [" can innately cast ", " can cast "]
        .iter()
        .find_map(|m| lower.find(m).map(|i| i + m.len()));
    if let Some(i) = at {
        let after = &header[i..];
        let after_lower = after.to_lowercase();
        if ["the following", "one of", "its ", "spells", "these "]
            .iter()
            .any(|p| after_lower.starts_with(p))
        {
            return out;
        }
        if let Some(end) = after_lower.find(" at will") {
            let names = after[..end].replace(" and ", ", ");
            let (spells, _) = parse_names(&names, chips);
            if !spells.is_empty() {
                out.push(BookList {
                    frequency: Frequency::AtWill,
                    spells,
                });
            }
            return out;
        }
        // "innately cast <spell>, requiring ..." / "... cast <spell>. Its innate ..."
        let end = after.find([',', '.']).map_or(after.len(), |e| {
            // keep a parenthetical with its comma inside ("heat metal (spell save DC 10)")
            let open = after[..e].rfind('(');
            let close = after[..e].rfind(')');
            match (open, close) {
                (Some(o), c) if c.is_none_or(|c| c < o) => {
                    after[e..].find(')').map_or(e, |x| e + x + 1)
                }
                _ => e,
            }
        });
        let name_text = after[..end].trim();
        // A DC in the parenthetical is the book's, not a spell note.
        let (spells, _) = parse_names(name_text, chips);
        let frequency = match usage {
            Some(Usage::PerDay(n) | Usage::PerDayInLair { normal: n, .. }) => Frequency::PerDay {
                uses: *n,
                each: false,
            },
            _ => Frequency::AtWill,
        };
        if spells.len() == 1 {
            let mut spells = spells;
            if spells[0]
                .note
                .as_deref()
                .is_some_and(|n| n.to_lowercase().contains("dc"))
            {
                spells[0].note = None;
            }
            out.push(BookList { frequency, spells });
        }
    }
    out
}

/// A book from a feature's printed text (grammar (b)). `None` when the feature isn't a Spellcasting entry.
#[must_use]
pub fn parse_text(name: &str, text: &str, usage: Option<&Usage>) -> Option<Spellbook> {
    if !is_spellcasting(name) {
        return None;
    }
    let mut book = Spellbook {
        name: name.to_string(),
        origin: BookOrigin::Text,
        ..Spellbook::default()
    };
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("");
    read_header(&mut book, header);
    let mut chips = Vec::new();
    book.lists = header_spells(header, usage, &mut chips);
    let mut starred = false;
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed = line
            .split_once(':')
            .and_then(|(p, rest)| frequency_of(p).map(|f| (f, rest)));
        match parsed {
            Some((frequency, rest)) => {
                let (spells, star) = parse_names(rest, &mut chips);
                starred |= star;
                book.lists.push(BookList { frequency, spells });
            }
            None => chips.push(line.to_string()),
        }
    }
    // A starred spell is precast only when the footnote says so; otherwise the star is just a mark.
    if starred {
        let says = chips
            .iter()
            .any(|c| c.to_lowercase().contains("before combat"));
        if !says {
            for l in &mut book.lists {
                for s in &mut l.spells {
                    s.precast = s
                        .note
                        .as_deref()
                        .is_some_and(|n| n.to_lowercase().contains("cast before combat"));
                }
            }
        }
    }
    book.chips = chips;
    Some(book)
}

/// A book from a feature: its structured lists (grammar (a)) when the block has them, else its text.
#[must_use]
pub fn of_feature(f: &Feature, section: Option<&str>) -> Option<Spellbook> {
    let mut book = parse_text(&f.name, &f.text, f.usage.as_ref())?;
    book.section = section.map(str::to_string);
    if let Some(sc) = &f.spellcasting {
        book.origin = BookOrigin::Structured;
        book.ability = sc.ability.map(|a| a.as_str().to_string()).or(book.ability);
        book.save_dc = sc.save_dc.or(book.save_dc);
        book.attack_bonus = sc.attack_bonus.or(book.attack_bonus);
        book.lists = sc
            .lists
            .iter()
            .map(|l| BookList {
                frequency: match l.frequency {
                    SpellFrequency::AtWill => Frequency::AtWill,
                    SpellFrequency::PerDay { uses, each } => Frequency::PerDay { uses, each },
                },
                spells: l
                    .spells
                    .iter()
                    .take(MAX_SPELLS)
                    .map(|s| {
                        let mut b = spell_from(&s.name, s.note.clone());
                        b.id.clone_from(&s.id);
                        b
                    })
                    .collect(),
            })
            .collect();
        // The text's own lines are now covered by the structured lists.
        book.chips
            .retain(|c| frequency_of(c.split(':').next().unwrap_or("")).is_none());
    }
    Some(book)
}

/// "{@spell fire bolt}", "{@spell mage armor|phb}*", "{@spell invisibility} (self only)" → name and note.
fn five_etools_spell(entry: &str) -> BookSpell {
    let mut name = entry.to_string();
    let mut note = None;
    let mut star = false;
    if let Some(start) = entry.find("{@spell ") {
        let body_start = start + "{@spell ".len();
        if let Some(len) = entry[body_start..].find('}') {
            let body = &entry[body_start..body_start + len];
            name = body.split('|').next().unwrap_or(body).to_string();
            let tail = entry[body_start + len + 1..].trim();
            star = tail.contains('*') || entry[..start].contains('*');
            let tail = tail.replace('*', "");
            let tail = tail.trim();
            if let (Some(o), Some(c)) = (tail.find('('), tail.rfind(')')) {
                if o < c {
                    note = Some(strip_tags(&tail[o + 1..c]));
                }
            }
        }
    } else {
        name = strip_tags(entry);
    }
    let mut s = spell_from(&name, note);
    s.precast |= star;
    s
}

/// The text of a 5etools string with its `{@tag text|...}` tags reduced to their text.
#[must_use]
pub fn strip_tags(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("{@") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let inner = &after[..end];
        let body = inner.split_once(' ').map_or("", |(_, b)| b);
        out.push_str(body.split('|').next().unwrap_or(body));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Books from a raw 5etools `spellcasting` array (grammar (c)): `will`, `daily` (`"2e"` = 2/day each), `spells` by
/// level with `slots`, the header's DC, to-hit and caster level. Entries that are not spell lists give no book.
#[must_use]
pub fn from_5etools(value: &Value) -> Vec<Spellbook> {
    let Some(list) = value.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in list {
        let name = entry["name"].as_str().unwrap_or("Spellcasting").to_string();
        let mut book = Spellbook {
            name,
            origin: BookOrigin::FiveEtools,
            ..Spellbook::default()
        };
        let header: String = entry["headerEntries"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(strip_tags)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        read_header(&mut book, &header);
        // {@dc 14} and {@hit 6} lose their markers when stripped: read them from the raw header.
        let raw: String = entry["headerEntries"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        if let Some(dc) = number_after(&raw, "{@dc ") {
            book.save_dc = u8::try_from(dc).ok();
        }
        if let Some(i) = raw.find("{@hit ") {
            let n: String = raw[i + 6..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-')
                .collect();
            book.attack_bonus = n.parse().ok();
        }
        if let Some(a) = entry["ability"].as_str() {
            book.ability = Some(a.to_string());
        }
        book.section = entry["displayAs"].as_str().map(str::to_string);
        let names = |v: &Value| -> Vec<BookSpell> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| {
                            s.as_str()
                                .map(str::to_string)
                                .or_else(|| s["entry"].as_str().map(str::to_string))
                        })
                        .take(MAX_SPELLS)
                        .map(|s| five_etools_spell(&s))
                        .collect()
                })
                .unwrap_or_default()
        };
        if entry["will"].is_array() {
            book.lists.push(BookList {
                frequency: Frequency::AtWill,
                spells: names(&entry["will"]),
            });
        }
        if let Some(daily) = entry["daily"].as_object() {
            let mut keys: Vec<&String> = daily.keys().collect();
            keys.sort_by(|a, b| b.cmp(a));
            for k in keys {
                let each = k.ends_with('e');
                let Ok(uses) = k.trim_end_matches('e').parse::<u8>() else {
                    book.chips.push(format!("{k}/day"));
                    continue;
                };
                book.lists.push(BookList {
                    frequency: Frequency::PerDay { uses, each },
                    spells: names(&daily[k]),
                });
            }
        }
        if let Some(levels) = entry["spells"].as_object() {
            let mut keys: Vec<(u8, &Value)> = levels
                .iter()
                .filter_map(|(k, v)| k.parse::<u8>().ok().map(|n| (n, v)))
                .collect();
            keys.sort_by_key(|(n, _)| *n);
            for (level, v) in keys {
                let frequency = if level == 0 {
                    Frequency::AtWill
                } else {
                    let slots = v["slots"]
                        .as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .unwrap_or(0);
                    Frequency::Slots { level, slots }
                };
                book.lists.push(BookList {
                    frequency,
                    spells: names(&v["spells"]),
                });
            }
        }
        for foot in entry["footerEntries"].as_array().into_iter().flatten() {
            if let Some(t) = foot.as_str() {
                book.chips.push(strip_tags(t));
            }
        }
        if !book.lists.is_empty() {
            out.push(book);
        }
    }
    out
}

/// The DM's own spells for one creature in this encounter (`Overrides.spells`, plan 4.5b item 3): spells picked from
/// installed elements, with a frequency each, the slots by level, and the casting numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpellsOverride {
    /// The spells.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spells: Vec<OverrideSpell>,
    /// Slots by spell level ("1" → 4), replacing the printed ones for the levels given.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub slots: BTreeMap<u8, u8>,
    /// Spellcasting ability, short.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ability: Option<String>,
    /// Spell save DC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_dc: Option<u8>,
    /// Spell attack bonus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack_bonus: Option<i32>,
}

/// One spell the DM added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverrideSpell {
    /// The installed spell element's id.
    pub id: String,
    /// Its name, for the list (the element's name when it was picked).
    pub name: String,
    /// How often.
    pub frequency: Frequency,
}

/// Most spells and highest numbers an override may set.
pub const MAX_OVERRIDE_SPELLS: usize = 60;

impl SpellsOverride {
    /// True when it adds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Check the numbers and lengths.
    ///
    /// # Errors
    /// A message for the DM.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.spells.len() > MAX_OVERRIDE_SPELLS {
            return Err("A creature can have 60 added spells.");
        }
        for s in &self.spells {
            if s.id.is_empty() || s.id.len() > 200 || s.name.trim().is_empty() || s.name.len() > 120
            {
                return Err("Each added spell needs a spell from your content.");
            }
            match s.frequency {
                Frequency::PerDay { uses, .. } if !(1..=9).contains(&uses) => {
                    return Err("Uses per day are 1 to 9.");
                }
                Frequency::Slots { level, .. } if !(1..=9).contains(&level) => {
                    return Err("Spell levels are 1 to 9.");
                }
                _ => {}
            }
        }
        if self
            .slots
            .iter()
            .any(|(l, n)| !(1..=9).contains(l) || *n > 9)
        {
            return Err("Spell slots are 0 to 9 for levels 1 to 9.");
        }
        if self.save_dc.is_some_and(|d| !(1..=40).contains(&d)) {
            return Err("A spell save DC is 1 to 40.");
        }
        if self.attack_bonus.is_some_and(|a| !(-10..=30).contains(&a)) {
            return Err("A spell attack bonus is -10 to +30.");
        }
        if self
            .ability
            .as_deref()
            .is_some_and(|a| !ABILITIES.iter().any(|(_, s)| *s == a))
        {
            return Err("The spellcasting ability is str, dex, con, int, wis or cha.");
        }
        Ok(())
    }

    /// The book it adds.
    #[must_use]
    pub fn book(&self) -> Spellbook {
        let mut lists: Vec<BookList> = Vec::new();
        for s in &self.spells {
            let spell = BookSpell {
                name: s.name.clone(),
                id: Some(s.id.clone()),
                ..BookSpell::default()
            };
            let frequency = match s.frequency {
                Frequency::Slots { level, .. } => Frequency::Slots {
                    level,
                    slots: self.slots.get(&level).copied().unwrap_or(0),
                },
                f => f,
            };
            match lists.iter_mut().find(|l| l.frequency == frequency) {
                Some(l) => l.spells.push(spell),
                None => lists.push(BookList {
                    frequency,
                    spells: vec![spell],
                }),
            }
        }
        // Slot levels the DM set without a spell of that level still count.
        for (level, slots) in &self.slots {
            if !lists
                .iter()
                .any(|l| matches!(l.frequency, Frequency::Slots { level: x, .. } if x == *level))
            {
                lists.push(BookList {
                    frequency: Frequency::Slots {
                        level: *level,
                        slots: *slots,
                    },
                    spells: vec![],
                });
            }
        }
        Spellbook {
            name: "Spells (this encounter)".into(),
            origin: BookOrigin::Override,
            ability: self.ability.clone(),
            save_dc: self.save_dc,
            attack_bonus: self.attack_bonus,
            lists,
            ..Spellbook::default()
        }
    }
}

/// Every book of a monster block: its Spellcasting traits and actions (structured or text), then a raw 5etools
/// spellcasting array when the block's text gave none, then the DM's override spells.
#[must_use]
pub fn of_monster(
    data: &MonsterData,
    five_etools: Option<&Value>,
    overrides: Option<&SpellsOverride>,
) -> Vec<Spellbook> {
    let mut out = Vec::new();
    let sections: [(&[Feature], Option<&str>); 4] = [
        (&data.traits, None),
        (&data.actions, Some("action")),
        (&data.bonus_actions, Some("bonus")),
        (&data.reactions, Some("reaction")),
    ];
    for (features, section) in sections {
        for f in features {
            if let Some(b) = of_feature(f, section) {
                out.push(b);
            }
        }
    }
    let text_empty = out
        .iter()
        .all(|b| b.lists.iter().all(|l| l.spells.is_empty()));
    if text_empty {
        if let Some(v) = five_etools {
            let books = from_5etools(v);
            if !books.is_empty() {
                out.retain(|b| b.lists.iter().any(|l| !l.spells.is_empty()));
                out.extend(books);
            }
        }
    }
    if let Some(o) = overrides.filter(|o| !o.is_empty()) {
        out.push(o.book());
    }
    out
}

/// The resource key of a per-day pool: one per spell for "each", one per list otherwise.
#[must_use]
pub fn pool_key(book: usize, list: usize, spell: Option<usize>) -> String {
    match spell {
        Some(s) => format!("spell_b{book}_l{list}_s{s}"),
        None => format!("spell_b{book}_l{list}"),
    }
}

/// Fill a combatant's spell slots and per-day spell pools from its books (the precast-slot fix, 0.3.2): slots by
/// level (the highest any book gives; an override's slots replace the printed ones of that level), and a
/// [`Resource`] per "N/day" list (shared) or per spell ("N/day each"), never refilled during combat.
pub fn seed(c: &mut Combatant, books: &[Spellbook]) {
    let mut slots: BTreeMap<u8, u8> = BTreeMap::new();
    for b in books.iter().filter(|b| b.origin != BookOrigin::Override) {
        for (level, n) in b.slots() {
            let e = slots.entry(level).or_insert(0);
            *e = (*e).max(n);
        }
    }
    for b in books.iter().filter(|b| b.origin == BookOrigin::Override) {
        for (level, n) in b.slots() {
            slots.insert(level, n);
        }
    }
    for (level, n) in slots {
        if n > 0 {
            c.spell_slots.insert(level, Pool::full(u32::from(n)));
        }
    }
    for (bi, b) in books.iter().enumerate() {
        for (li, l) in b.lists.iter().enumerate() {
            let Frequency::PerDay { uses, each } = l.frequency else {
                continue;
            };
            if l.spells.is_empty() {
                continue;
            }
            if each {
                for (si, s) in l.spells.iter().enumerate() {
                    c.resources.insert(
                        pool_key(bi, li, Some(si)),
                        Resource {
                            label: format!("{} ({uses}/day)", s.name),
                            pool: Pool::full(u32::from(uses)),
                            reset: Reset::Never,
                        },
                    );
                }
            } else {
                let names: Vec<&str> = l.spells.iter().map(|s| s.name.as_str()).collect();
                c.resources.insert(
                    pool_key(bi, li, None),
                    Resource {
                        label: format!("{} ({uses}/day)", names.join(", ")),
                        pool: Pool::full(u32::from(uses)),
                        reset: Reset::Never,
                    },
                );
            }
        }
    }
}

/// Seed a creature from its encounter overrides as well: the printed books plus the DM's added spells, whose slots
/// replace the printed ones of the levels they set. Call after [`crate::combatant::creature_of`] when the creature
/// comes from an encounter row (the start of combat); a copy of a combatant keeps the copied pools instead.
pub fn seed_overrides(
    c: &mut Combatant,
    data: Option<&MonsterData>,
    overrides: &crate::overrides::Overrides,
) {
    let Some(spells) = overrides.spells.as_ref().filter(|s| !s.is_empty()) else {
        return;
    };
    let (books, edition) = match data {
        Some(d) => (of_monster(d, None, Some(spells)), edition_of(d)),
        None => (vec![spells.book()], party_combat::Edition::Srd51),
    };
    equip(c, &books, edition);
}

/// One book spell linked to an installed spell element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedSpell {
    /// The book (an index into the books).
    pub book: usize,
    /// The list.
    pub list: usize,
    /// The spell.
    pub spell: usize,
    /// The printed name.
    pub name: String,
    /// How it resolved.
    pub link: party_spells::canonical::Link,
}

/// Link every spell of `books` to installed spells through the canonical catalog (the block's id, then the DM's
/// link, then the name in the creature's ruleset). Unresolved names stay, as
/// [`party_spells::canonical::Link::Unlinked`].
#[must_use]
pub fn link(
    books: &[Spellbook],
    catalog: &party_spells::canonical::Catalog,
    ruleset: &str,
) -> Vec<LinkedSpell> {
    let mut out = Vec::new();
    for (bi, b) in books.iter().enumerate() {
        for (li, l) in b.lists.iter().enumerate() {
            for (si, s) in l.spells.iter().enumerate() {
                out.push(LinkedSpell {
                    book: bi,
                    list: li,
                    spell: si,
                    name: s.name.clone(),
                    link: catalog.resolve(&s.name, ruleset, s.id.as_deref()),
                });
            }
        }
    }
    out
}

/// The engine's ability name of a short ability key (`int`).
fn ability_name(short: &str) -> Option<party_combat::AbilityName> {
    serde_json::from_value(Value::String(short.to_string())).ok()
}

/// The combat numbers of a monster block (0.3.2): ability modifiers and save bonuses, Magic Resistance, the creature
/// type, and what its printed AC comes from.
#[must_use]
pub fn stats_of(d: &MonsterData) -> party_combat::CombatStats {
    use borealis_content::monster::Ability as A;
    let order = [A::Str, A::Dex, A::Con, A::Int, A::Wis, A::Cha];
    party_combat::CombatStats {
        mods: order.map(|a| d.abilities.modifier(a)),
        saves: order.map(|a| d.save_bonus(a)),
        magic_resistance: d
            .traits
            .iter()
            .any(|t| t.name.to_lowercase().starts_with("magic resistance")),
        creature_type: Some(d.creature_type.as_str().to_string()),
        armor: d.ac.first().and_then(|a| a.source.clone()),
    }
}

/// The edition of a monster block.
#[must_use]
pub fn edition_of(d: &MonsterData) -> party_combat::Edition {
    match d.ruleset {
        borealis_content::Ruleset::Dnd2024 => party_combat::Edition::Srd521,
        _ => party_combat::Edition::Srd51,
    }
}

/// The SRD element a book spell names: the block's id, else the bundled SRD spell of that name in the creature's
/// edition. The server relinks through the account's own catalog ([`link`]) when it has one.
fn srd_link(s: &BookSpell, edition: party_combat::Edition) -> Option<(String, u8)> {
    if let Some(id) = &s.id {
        let level = party_spells::data::curated(id)
            .map(|m| m.level)
            .or_else(|| {
                party_spells::data::srd_spell(&s.name, edition)
                    .filter(|(i, _)| i == id)
                    .map(|(_, l)| l)
            });
        return Some((id.clone(), level.unwrap_or(0)));
    }
    party_spells::data::srd_spell(&s.name, edition).map(|(id, l)| (id.to_string(), l))
}

/// Give a combatant its spellcasting from `books` (0.3.2): slots and per-day pools ([`seed`]), one [`Casting`] per
/// book, and one ability per spell, linked to its spell element (`spell`) and carrying the curated SRD definition as
/// its cast template (`mech`) when there is one. Spells already on the combatant (same book position) are not added
/// twice.
///
/// [`Casting`]: party_combat::Casting
pub fn equip(c: &mut Combatant, books: &[Spellbook], edition: party_combat::Edition) {
    use party_combat::spell::{
        cantrip_tier, CastSpec, CastTime, MechOrigin, SpellFrequency, SpellLink,
    };
    use party_combat::{Ability, AbilityKind, Casting};
    seed(c, books);
    let mut keys: std::collections::BTreeSet<String> =
        c.abilities.iter().map(|a| a.key.clone()).collect();
    for (bi, b) in books.iter().enumerate() {
        let ability = b.ability.as_deref().and_then(ability_name);
        if let Some(ab) = ability {
            let casting = Casting {
                ability: ab,
                dc: b.save_dc.map(u32::from),
                attack: b.attack_bonus,
                caster_level: b.caster_level,
                cantrip_tier: b.caster_level.map(cantrip_tier),
            };
            if !c.casting.contains(&casting) {
                c.casting.push(casting);
            }
        }
        let modifier = match (ability, &c.stats) {
            (Some(ab), Some(st)) => st.modifier(ab),
            _ => 0,
        };
        for (li, l) in b.lists.iter().enumerate() {
            for (si, s) in l.spells.iter().enumerate() {
                let marker = format!("b{bi}_l{li}_s{si}");
                let key = format!("spell_{marker}");
                if !keys.insert(key.clone()) {
                    continue;
                }
                let link = srd_link(s, edition);
                let level = link.as_ref().map_or_else(
                    || match l.frequency {
                        Frequency::Slots { level, .. } => level,
                        _ => 0,
                    },
                    |(_, lv)| *lv,
                );
                let curated = link
                    .as_ref()
                    .and_then(|(id, _)| party_spells::data::curated(id));
                let kind = match curated.map(|m| m.cast) {
                    Some(CastTime::Bonus) => AbilityKind::BonusAction,
                    Some(CastTime::Reaction) => AbilityKind::Reaction,
                    _ => AbilityKind::Action,
                };
                let mut a = Ability::new(key.clone(), s.name.clone(), kind);
                a.text = s.note.clone().unwrap_or_default();
                let frequency = match l.frequency {
                    Frequency::AtWill => SpellFrequency::AtWill,
                    Frequency::PerDay { each, .. } => {
                        a.uses = Some(pool_key(bi, li, each.then_some(si)));
                        SpellFrequency::PerDay
                    }
                    Frequency::Slots { .. } => SpellFrequency::Slot,
                };
                a.spell = Some(SpellLink {
                    key: link.as_ref().map(|(id, _)| id.clone()),
                    level,
                    frequency,
                    fixed_slot: s.fixed_level,
                    baked_in: s.baked_in,
                });
                a.mech = curated.map(|m| {
                    Box::new(CastSpec {
                        v: party_combat::spell::MECH_VERSION,
                        origin: MechOrigin::Srd {
                            key: m.key.clone(),
                            rev: m.rev,
                        },
                        mech: m.clone(),
                        dc: b.save_dc.map(u32::from),
                        attack: b.attack_bonus,
                        modifier,
                        cantrip_tier: b.caster_level.map_or(1, cantrip_tier),
                        baked_in: s.baked_in,
                    })
                });
                c.abilities.push(a);
            }
        }
    }
}

/// The resolver's candidate of an installed Spell element; `ruleset` is the element's effective ruleset (its own,
/// else its file's: `dnd2014`, `dnd2024` or `any`). `None` for any other element type. `date` is the source's
/// publication date when the caller knows it.
#[must_use]
pub fn candidate_of(
    element: &borealis_content::Element,
    ruleset: &str,
    date: Option<String>,
) -> Option<party_spells::canonical::Candidate> {
    if element.ty != "Spell" {
        return None;
    }
    Some(party_spells::canonical::Candidate {
        id: element.id.clone(),
        name: element.name.clone(),
        ruleset: ruleset.to_string(),
        source: element.source.clone(),
        level: element
            .setters
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case("level"))
            .and_then(|s| s.value.trim().parse().ok()),
        date: date
            .or_else(|| party_spells::canonical::srd_date(&element.source).map(str::to_string)),
    })
}

/// The raw 5etools `spellcasting` array an encounter creature's `automation_json` keeps
/// (`{"v":1,"fivetools":{"spellcasting":[...]}}`).
#[must_use]
pub fn fivetools_of(automation_json: Option<&str>) -> Option<Value> {
    let v: Value = serde_json::from_str(automation_json?).ok()?;
    let sc = v.get("fivetools")?.get("spellcasting")?;
    sc.is_array().then(|| sc.clone())
}

/// At the start of combat, for a creature from an encounter row: its full spellcasting from the block, the raw
/// 5etools spellcasting kept in `automation_json` (when the block's text lists none) and the DM's added spells. Call
/// after [`crate::combatant::creature_of`]; what `build_creature` already added is not added twice.
pub fn equip_start(
    c: &mut Combatant,
    data: Option<&MonsterData>,
    automation_json: Option<&str>,
    overrides: &crate::overrides::Overrides,
) {
    let five = fivetools_of(automation_json);
    let spells = overrides.spells.as_ref().filter(|s| !s.is_empty());
    match data {
        Some(d) => equip(c, &of_monster(d, five.as_ref(), spells), edition_of(d)),
        None => {
            let mut books = five.as_ref().map(from_5etools).unwrap_or_default();
            if let Some(s) = spells {
                books.push(s.book());
            }
            equip(c, &books, party_combat::Edition::Srd51);
        }
    }
}
