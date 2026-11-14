//! Scopes a token or an OAuth grant gives. Part A issues exactly these strings.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// One scope. A scope never widens a role: the handlers still apply the API's DM, player and party checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// `bestiary:read`
    #[serde(rename = "bestiary:read")]
    BestiaryRead,
    /// `encounters:read`
    #[serde(rename = "encounters:read")]
    EncountersRead,
    /// `encounters:write` (implies `encounters:read`)
    #[serde(rename = "encounters:write")]
    EncountersWrite,
    /// `encounters:simulate`: dry-run simulations on copies of the DM's encounters (D26). Never touches combats.
    #[serde(rename = "encounters:simulate")]
    EncountersSimulate,
    /// `characters:read`
    #[serde(rename = "characters:read")]
    CharactersRead,
    /// `homebrew:read`
    #[serde(rename = "homebrew:read")]
    HomebrewRead,
    /// `homebrew:write` (implies `homebrew:read`)
    #[serde(rename = "homebrew:write")]
    HomebrewWrite,
}

impl Scope {
    /// Every scope, in a fixed order.
    pub const ALL: [Self; 7] = [
        Self::BestiaryRead,
        Self::EncountersRead,
        Self::EncountersWrite,
        Self::EncountersSimulate,
        Self::CharactersRead,
        Self::HomebrewRead,
        Self::HomebrewWrite,
    ];

    /// The wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BestiaryRead => "bestiary:read",
            Self::EncountersRead => "encounters:read",
            Self::EncountersWrite => "encounters:write",
            Self::EncountersSimulate => "encounters:simulate",
            Self::CharactersRead => "characters:read",
            Self::HomebrewRead => "homebrew:read",
            Self::HomebrewWrite => "homebrew:write",
        }
    }

    /// A wire name, or `None`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == text)
    }

    /// A space- or comma-separated list. Unknown names are an error (a typo never silently grants less or more).
    ///
    /// # Errors
    /// The first unknown name.
    pub fn parse_list(text: &str) -> Result<BTreeSet<Self>, String> {
        text.split([' ', ','])
            .filter(|s| !s.is_empty())
            .map(|s| Self::parse(s).ok_or_else(|| format!("unknown scope: {s}")))
            .collect()
    }

    /// Writes something (its tools take a `requestId`).
    #[must_use]
    pub fn writes(self) -> bool {
        matches!(self, Self::EncountersWrite | Self::HomebrewWrite)
    }

    /// The scopes this one also grants.
    #[must_use]
    pub fn implied(self) -> &'static [Self] {
        match self {
            Self::EncountersWrite => &[Self::EncountersRead],
            Self::HomebrewWrite => &[Self::HomebrewRead],
            _ => &[],
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a set of granted scopes allows, implications included.
#[must_use]
pub fn grants(granted: &BTreeSet<Scope>, needed: Scope) -> bool {
    granted.contains(&needed) || granted.iter().any(|g| g.implied().contains(&needed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_unknown_names_are_refused() {
        for s in Scope::ALL {
            assert_eq!(Scope::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_str());
        }
        assert!(Scope::parse_list("bestiary:read combat:read").is_err());
        let list = Scope::parse_list("bestiary:read,encounters:write").unwrap();
        assert!(grants(&list, Scope::EncountersRead));
        assert!(!grants(&list, Scope::EncountersSimulate));
        assert!(Scope::parse("combat:run").is_none() && Scope::parse("index:publish").is_none());
    }
}
