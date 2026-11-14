//! One spell per edition (the user's rule, 2026-11-13): "we should not have duplicate spells, if a spell comes from
//! more sources we just select one, if we have the data to know which book is more recent we pick this (from same
//! game version)".
//!
//! The same normalized name within the same ruleset is ONE spell. Its copies from other sources are hidden from new
//! picks, never deleted (content files stay unchanged, Aurora indexes still parse), and the chosen copy says where
//! else it is printed ("also in: ..."). 2014 and 2024 spells never merge, and neither do different spells: a copy
//! printed at another level is a different spell (a homebrew "Fireball" at level 2 is never folded into the SRD one).
//!
//! The chosen copy, in order:
//! 1. the DM's pick for that spell;
//! 2. when every copy has a publication date: the most recent (SRD versions have theirs, see [`srd_date`]);
//! 3. an SRD copy;
//! 4. the first of the DM's preferred sources;
//! 5. stable: source A-Z, then id.
//!
//! Pure: the caller gathers the account's Spell elements and dates.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// One installed Spell element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// Element id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// `dnd2014`, `dnd2024` or `any` (the element's, else its file's).
    pub ruleset: String,
    /// Source name.
    pub source: String,
    /// Spell level, when its setters give one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    /// The source's publication date (`2024-09-17`, `2016`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// The DM's choices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    /// Sources to prefer when no date decides, first wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred_sources: Vec<String>,
    /// The chosen copy per spell: element id of the copy, for any id of that spell's group.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub picks: BTreeMap<String, String>,
    /// Names the DM linked to a spell ("Link..." on an unlinked monster spell): `ruleset:normalized name` → id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, String>,
}

/// Why a copy is the chosen one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// The only copy.
    Only,
    /// The DM picked it.
    DmPick,
    /// The most recent source.
    Newest,
    /// The SRD copy.
    Srd,
    /// A preferred source.
    Preferred,
    /// Stable order (source A-Z).
    Stable,
}

/// One spell and all its copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// `ruleset:normalized name` (with `@level` when copies differ in level).
    pub key: String,
    /// The chosen copy's id.
    pub canonical: String,
    /// Why.
    pub reason: Reason,
    /// Every copy, chosen first: (id, source).
    pub members: Vec<(String, String)>,
}

/// How a printed spell name resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Link {
    /// One spell.
    Linked {
        /// Its chosen copy.
        id: String,
    },
    /// Several different spells by that name (different levels): the DM picks once.
    Ambiguous {
        /// The chosen copy of each.
        ids: Vec<String>,
    },
    /// No installed spell of that name in this ruleset.
    Unlinked {
        /// The same name in the other edition, offered, never used silently.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        other_edition: Vec<String>,
    },
}

/// A name as the resolver compares it: lowercase, apostrophes dropped, other punctuation and spaces collapsed.
#[must_use]
pub fn normalize(name: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for ch in name.chars() {
        if matches!(ch, '\'' | '’' | '‘' | '`') {
            continue;
        }
        if ch.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.extend(ch.to_lowercase());
        } else {
            space = true;
        }
    }
    out
}

/// The group key of a name in a ruleset.
#[must_use]
pub fn key(ruleset: &str, name: &str) -> String {
    format!("{ruleset}:{}", normalize(name))
}

/// True for an SRD source name ("SRD 5.1", "SRD 5.2.1", "System Reference Document 5.1").
#[must_use]
pub fn is_srd(source: &str) -> bool {
    let s = source.trim().to_lowercase();
    s.starts_with("srd") || s.starts_with("system reference document")
}

/// The publication date of an SRD version: 5.1 (2016) and 5.2.1 (2025), as year-month text compared as strings.
#[must_use]
pub fn srd_date(source: &str) -> Option<&'static str> {
    if !is_srd(source) {
        return None;
    }
    let s = source.to_lowercase();
    if s.contains("5.2") {
        Some("2025-05")
    } else if s.contains("5.1") {
        Some("2016-01")
    } else {
        None
    }
}

/// Every spell group of an account's installed spells.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    groups: BTreeMap<String, Group>,
    /// id → group key.
    of_id: BTreeMap<String, String>,
    /// `ruleset:name` → group keys (several when levels differ).
    by_name: BTreeMap<String, Vec<String>>,
    links: BTreeMap<String, String>,
}

fn choose(members: &[&Candidate], prefs: &Preferences) -> (usize, Reason) {
    if members.len() == 1 {
        return (0, Reason::Only);
    }
    if let Some(i) = members.iter().position(|m| {
        members
            .iter()
            .any(|o| prefs.picks.get(&o.id).is_some_and(|p| *p == m.id))
    }) {
        return (i, Reason::DmPick);
    }
    let date = |m: &Candidate| {
        m.date
            .clone()
            .or_else(|| srd_date(&m.source).map(str::to_string))
    };
    if members.iter().all(|m| date(m).is_some()) {
        let best = members.iter().map(|m| date(m)).max().flatten();
        let newest: Vec<usize> = (0..members.len())
            .filter(|i| date(members[*i]) == best)
            .collect();
        if newest.len() == 1 {
            return (newest[0], Reason::Newest);
        }
    }
    if let Some(i) = members.iter().position(|m| is_srd(&m.source)) {
        return (i, Reason::Srd);
    }
    for p in &prefs.preferred_sources {
        if let Some(i) = members
            .iter()
            .position(|m| m.source.eq_ignore_ascii_case(p))
        {
            return (i, Reason::Preferred);
        }
    }
    (0, Reason::Stable)
}

impl Catalog {
    /// Group the candidates and choose each spell's copy.
    #[must_use]
    pub fn build(candidates: &[Candidate], prefs: &Preferences) -> Self {
        // name key → level → copies (unknown level last).
        let mut by_name: BTreeMap<String, BTreeMap<Option<u8>, Vec<&Candidate>>> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for c in candidates {
            if !seen.insert(c.id.as_str()) {
                continue;
            }
            by_name
                .entry(key(&c.ruleset, &c.name))
                .or_default()
                .entry(c.level)
                .or_default()
                .push(c);
        }
        let mut out = Self {
            links: prefs.links.clone(),
            ..Self::default()
        };
        for (name_key, mut levels) in by_name {
            // Copies without a level join the one levelled spell of that name, if there is exactly one.
            let known: Vec<Option<u8>> = levels.keys().copied().filter(Option::is_some).collect();
            if known.len() == 1 {
                if let Some(unknown) = levels.remove(&None) {
                    levels.entry(known[0]).or_default().extend(unknown);
                }
            }
            let split = levels.len() > 1;
            for (level, mut members) in levels {
                members.sort_by(|a, b| {
                    a.source
                        .to_lowercase()
                        .cmp(&b.source.to_lowercase())
                        .then(a.id.cmp(&b.id))
                });
                let (i, reason) = choose(&members, prefs);
                let chosen = members.remove(i);
                members.insert(0, chosen);
                let group_key = match (split, level) {
                    (true, Some(l)) => format!("{name_key}@{l}"),
                    (true, None) => format!("{name_key}@?"),
                    _ => name_key.clone(),
                };
                for m in &members {
                    out.of_id.insert(m.id.clone(), group_key.clone());
                }
                out.by_name
                    .entry(name_key.clone())
                    .or_default()
                    .push(group_key.clone());
                out.groups.insert(
                    group_key.clone(),
                    Group {
                        key: group_key,
                        canonical: chosen.id.clone(),
                        reason,
                        members: members
                            .iter()
                            .map(|m| (m.id.clone(), m.source.clone()))
                            .collect(),
                    },
                );
            }
        }
        out
    }

    /// Every spell group.
    pub fn groups(&self) -> impl Iterator<Item = &Group> {
        self.groups.values()
    }

    /// The group of an installed spell.
    #[must_use]
    pub fn group(&self, id: &str) -> Option<&Group> {
        self.groups.get(self.of_id.get(id)?)
    }

    /// The chosen copy of the spell `id` is a copy of (itself when it is the chosen one).
    #[must_use]
    pub fn canonical<'a>(&'a self, id: &'a str) -> &'a str {
        self.group(id).map_or(id, |g| g.canonical.as_str())
    }

    /// True when `id` is a copy hidden from new picks.
    #[must_use]
    pub fn is_hidden(&self, id: &str) -> bool {
        self.group(id).is_some_and(|g| g.canonical != id)
    }

    /// Every hidden copy.
    #[must_use]
    pub fn hidden(&self) -> BTreeSet<String> {
        self.groups
            .values()
            .flat_map(|g| g.members.iter().skip(1).map(|(id, _)| id.clone()))
            .collect()
    }

    /// Every hidden copy with the chosen copy that answers for it where content names the hidden
    /// copy by id (an `<extend>` of a class list, a select of named spells).
    #[must_use]
    pub fn stand_ins(&self) -> BTreeMap<String, String> {
        self.groups
            .values()
            .flat_map(|g| {
                g.members
                    .iter()
                    .skip(1)
                    .map(|(id, _)| (id.clone(), g.canonical.clone()))
            })
            .collect()
    }

    /// The other sources of the spell `id` ("also in: ...").
    #[must_use]
    pub fn also_in(&self, id: &str) -> Vec<String> {
        self.group(id)
            .map(|g| {
                g.members
                    .iter()
                    .filter(|(m, _)| m != id)
                    .map(|(_, s)| s.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolve a printed spell name in a ruleset: the id the block gives, then the DM's link, then the name (in the
    /// ruleset, then `any`). Never crosses editions by itself: the other edition's spell is only offered.
    #[must_use]
    pub fn resolve(&self, name: &str, ruleset: &str, id: Option<&str>) -> Link {
        if let Some(id) = id.filter(|id| self.of_id.contains_key(*id)) {
            return Link::Linked {
                id: self.canonical(id).to_string(),
            };
        }
        let k = key(ruleset, name);
        if let Some(linked) = self.links.get(&k) {
            if self.of_id.contains_key(linked) {
                return Link::Linked {
                    id: self.canonical(linked).to_string(),
                };
            }
        }
        for r in [ruleset, "any"] {
            let ids: Vec<String> = self
                .by_name
                .get(&key(r, name))
                .into_iter()
                .flatten()
                .filter_map(|g| self.groups.get(g))
                .map(|g| g.canonical.clone())
                .collect();
            match ids.len() {
                0 => {}
                1 => {
                    return Link::Linked {
                        id: ids.into_iter().next().unwrap_or_default(),
                    }
                }
                _ => return Link::Ambiguous { ids },
            }
        }
        let other = match ruleset {
            "dnd2014" => "dnd2024",
            "dnd2024" => "dnd2014",
            _ => "",
        };
        Link::Unlinked {
            other_edition: self
                .by_name
                .get(&key(other, name))
                .into_iter()
                .flatten()
                .filter_map(|g| self.groups.get(g))
                .map(|g| g.canonical.clone())
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, name: &str, ruleset: &str, source: &str, level: Option<u8>) -> Candidate {
        Candidate {
            id: id.into(),
            name: name.into(),
            ruleset: ruleset.into(),
            source: source.into(),
            level,
            date: None,
        }
    }

    #[test]
    fn normalizes() {
        assert_eq!(
            normalize("  Tasha’s  Hideous-Laughter "),
            "tashas hideous laughter"
        );
        assert_eq!(normalize("Detect Evil and Good"), "detect evil and good");
        assert_ne!(normalize("Fireball"), normalize("Delayed Blast Fireball"));
    }

    #[test]
    fn one_spell_per_edition() {
        let mut fiveetools = c(
            "ID_X_FIREBALL",
            "Fireball",
            "dnd2024",
            "Sample Book 2024",
            Some(3),
        );
        fiveetools.date = Some("2024-09-17".into());
        let all = vec![
            c(
                "ID_PHB_SPELL_FIREBALL",
                "Fireball",
                "dnd2014",
                "SRD 5.1",
                Some(3),
            ),
            c(
                "ID_AURORA_FIREBALL",
                "fireball",
                "dnd2014",
                "Aurora Book",
                Some(3),
            ),
            c(
                "ID_2024_SPELL_FIREBALL",
                "Fireball",
                "dnd2024",
                "SRD 5.2.1",
                Some(3),
            ),
            fiveetools,
            c(
                "ID_DBF",
                "Delayed Blast Fireball",
                "dnd2014",
                "SRD 5.1",
                Some(7),
            ),
        ];
        let cat = Catalog::build(&all, &Preferences::default());
        // 2014: no date on the Aurora copy, so SRD first.
        assert_eq!(cat.canonical("ID_AURORA_FIREBALL"), "ID_PHB_SPELL_FIREBALL");
        assert_eq!(cat.group("ID_AURORA_FIREBALL").unwrap().reason, Reason::Srd);
        // 2024: both dated, the most recent wins.
        assert_eq!(cat.canonical("ID_X_FIREBALL"), "ID_2024_SPELL_FIREBALL");
        assert_eq!(cat.group("ID_X_FIREBALL").unwrap().reason, Reason::Newest);
        // Editions never merge; different spells never merge.
        assert_ne!(
            cat.canonical("ID_PHB_SPELL_FIREBALL"),
            cat.canonical("ID_2024_SPELL_FIREBALL")
        );
        assert_eq!(cat.canonical("ID_DBF"), "ID_DBF");
        assert_eq!(
            cat.hidden(),
            ["ID_AURORA_FIREBALL", "ID_X_FIREBALL"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        assert_eq!(cat.also_in("ID_PHB_SPELL_FIREBALL"), ["Aurora Book"]);
    }

    #[test]
    fn newest_dated_source_wins() {
        let mut a = c("A", "Bless", "dnd2014", "Old Book", Some(1));
        a.date = Some("2014-08-19".into());
        let mut b = c("B", "Bless", "dnd2014", "New Book", Some(1));
        b.date = Some("2020-11-17".into());
        let cat = Catalog::build(&[a, b], &Preferences::default());
        assert_eq!(cat.canonical("A"), "B");
    }

    #[test]
    fn dm_pick_and_preferred_sources() {
        let all = vec![
            c("A", "Bless", "dnd2014", "Book A", Some(1)),
            c("B", "Bless", "dnd2014", "Book B", Some(1)),
        ];
        let cat = Catalog::build(&all, &Preferences::default());
        assert_eq!(
            (cat.canonical("B"), cat.group("B").unwrap().reason),
            ("A", Reason::Stable)
        );
        let prefs = Preferences {
            preferred_sources: vec!["book b".into()],
            ..Preferences::default()
        };
        assert_eq!(Catalog::build(&all, &prefs).canonical("A"), "B");
        let mut prefs = Preferences::default();
        prefs.picks.insert("A".into(), "B".into());
        let cat = Catalog::build(&all, &prefs);
        assert_eq!(cat.canonical("A"), "B");
        assert_eq!(cat.group("A").unwrap().reason, Reason::DmPick);
    }

    #[test]
    fn other_level_is_another_spell() {
        let all = vec![
            c(
                "ID_PHB_SPELL_FIREBALL",
                "Fireball",
                "dnd2014",
                "SRD 5.1",
                Some(3),
            ),
            c("HB", "Fireball", "dnd2014", "My Homebrew", Some(2)),
        ];
        let cat = Catalog::build(&all, &Preferences::default());
        assert!(cat.hidden().is_empty());
        assert!(matches!(
            cat.resolve("fireball", "dnd2014", None),
            Link::Ambiguous { ids } if ids.len() == 2
        ));
        // The block's own id decides.
        assert_eq!(
            cat.resolve("fireball", "dnd2014", Some("ID_PHB_SPELL_FIREBALL")),
            Link::Linked {
                id: "ID_PHB_SPELL_FIREBALL".into()
            }
        );
    }

    #[test]
    fn resolve_links_and_editions() {
        let all = vec![
            c(
                "ID_PHB_SPELL_SHIELD",
                "Shield",
                "dnd2014",
                "SRD 5.1",
                Some(1),
            ),
            c("ID_ANY_LIGHT", "Light", "any", "Some Pack", Some(0)),
        ];
        let mut prefs = Preferences::default();
        prefs
            .links
            .insert(key("dnd2024", "Shield Spell"), "ID_PHB_SPELL_SHIELD".into());
        let cat = Catalog::build(&all, &prefs);
        assert_eq!(
            cat.resolve("Light", "dnd2014", None),
            Link::Linked {
                id: "ID_ANY_LIGHT".into()
            }
        );
        assert_eq!(
            cat.resolve("shield", "dnd2024", None),
            Link::Unlinked {
                other_edition: vec!["ID_PHB_SPELL_SHIELD".into()]
            }
        );
        assert_eq!(
            cat.resolve("Shield Spell", "dnd2024", None),
            Link::Linked {
                id: "ID_PHB_SPELL_SHIELD".into()
            }
        );
        assert_eq!(
            cat.resolve("Wish", "dnd2014", None),
            Link::Unlinked {
                other_edition: vec![]
            }
        );
    }
}
