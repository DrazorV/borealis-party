//! What the DM tools may offer from the account's content (0.3.5): the same rules as the account's character
//! building, so the encounter builder's spell picker, the bestiary and the creature pickers never list content its
//! characters can't pick.
//!
//! * **Sources:** the client sends the sources its new characters start on (`enabledSources`: the Sources page's
//!   saved default with the Restrict playtest / third-party toggles applied, by upstream's own `withToggles`, so the
//!   rule lives in one place). The server turns it into the engine's [`SourceRestriction`] exactly as
//!   `set_source_restrictions` does (every installed source not in the list). A caller that sends none (the MCP
//!   tools, an older app) gets the account's toggles only, as a new character with no saved default has.
//! * **Edition:** the engine's own edition status of each element in the universe of a ruleset (accepted, not
//!   superseded, and foreign only where that ruleset's profile allows the other edition by default), as a new pick
//!   of a character of that ruleset ([`borealis_engine`] `offers_edition`).
//! * **One copy per spell:** the spell list's [`crate::spells::Book`] already offers only the resolver's chosen copy.

use std::collections::HashSet;

use borealis_content::{is_unrestrictable_source, Ruleset, SourceRestriction};
use borealis_lib::state::AppState as Backend;
use tauri::Manager;

use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// Most source names a client may send.
pub const MAX_SOURCES: usize = 5_000;

/// The account's rules for one listing.
pub struct PickRules {
    restriction: SourceRestriction,
    /// Ids of the elements of the asked type a character of the asked ruleset may pick; `None` = any edition.
    offered: Option<HashSet<String>>,
}

impl PickRules {
    /// No rules (every source, every edition).
    #[cfg(test)]
    #[must_use]
    pub fn none() -> Self {
        Self {
            restriction: SourceRestriction::none(),
            offered: None,
        }
    }

    /// May content of this source be listed?
    #[must_use]
    pub fn source_on(&self, source: &str) -> bool {
        !self.restriction.is_restricted(source)
    }

    /// May the element `id` of `source` be offered?
    #[must_use]
    pub fn offers(&self, id: &str, source: &str) -> bool {
        self.source_on(source) && self.offered.as_ref().is_none_or(|o| o.contains(id))
    }
}

fn upstream(e: &borealis_lib::error::ApiError) -> ApiError {
    ApiError::Internal(format!("content: {e:?}"))
}

/// A ruleset name as the engine takes it; `None` for none or `any`.
///
/// # Errors
/// An unknown name.
pub fn ruleset(name: Option<&str>) -> ApiResult<Option<Ruleset>> {
    match name.map(str::trim) {
        None | Some("" | "any") => Ok(None),
        Some("dnd2014") => Ok(Some(Ruleset::Dnd2014)),
        Some("dnd2024") => Ok(Some(Ruleset::Dnd2024)),
        Some(other) => Err(ApiError::Invalid(format!("Unknown ruleset: {other}."))),
    }
}

/// The sources left out: those not in `enabled`, else (none sent) the books the account's toggles restrict.
fn restricted(backend: &Backend, enabled: Option<&[String]>) -> ApiResult<Vec<String>> {
    let hub = backend.hub().map_err(|e| upstream(&e))?;
    if let Some(on) = enabled {
        // As upstream's `restricted_from` (manage.rs): every installed source the list leaves out.
        return Ok(hub
            .store
            .source_names()
            .into_iter()
            .filter(|s| !on.iter().any(|e| e == s) && !is_unrestrictable_source(s))
            .map(str::to_string)
            .collect());
    }
    let settings = backend.settings().map_err(|e| upstream(&e))?;
    if !settings.restrict_playtest && !settings.restrict_third_party {
        return Ok(Vec::new());
    }
    Ok(backend
        .list_sources()
        .map_err(|e| upstream(&e))?
        .into_iter()
        .filter(|s| {
            (settings.restrict_playtest && s.playtest)
                || (settings.restrict_third_party && s.third_party)
        })
        .map(|s| s.id)
        .collect())
}

/// The rules for listing elements of type `ty` to the account `owner`: its sources (`enabled`, the client's, or
/// its toggles), and with `for_ruleset` the edition rule of a new pick in that ruleset.
///
/// # Errors
/// Too many sources sent, or the workspace fails.
pub async fn rules(
    state: &AppState,
    owner: &str,
    enabled: Option<Vec<String>>,
    for_ruleset: Option<Ruleset>,
    ty: &'static str,
) -> ApiResult<PickRules> {
    if enabled.as_ref().is_some_and(|e| e.len() > MAX_SOURCES) {
        return Err(ApiError::Invalid("Too many sources.".into()));
    }
    let workspace = state.workspaces.get(owner).await?;
    workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            let restriction =
                SourceRestriction::excluding(restricted(&backend, enabled.as_deref())?);
            let offered = match for_ruleset {
                None => None,
                Some(rs) => {
                    let hub = backend.hub().map_err(|e| upstream(&e))?;
                    let cross = borealis_engine::profile_for(rs).default_allow_cross_edition();
                    Some(
                        hub.engine
                            .universe(rs)
                            .by_type(ty)
                            .filter(|c| {
                                c.edition.is_accepted()
                                    && !c.superseded
                                    && (cross || !c.edition.is_foreign())
                            })
                            .map(|c| c.el.id.clone())
                            .collect(),
                    )
                }
            };
            Ok(PickRules {
                restriction,
                offered,
            })
        })
        .await
}

/// Source names once each (ignoring case), in order, leaving out `own` and the sources that are off.
#[must_use]
pub fn distinct_sources<'a>(
    rules: &PickRules,
    own: &str,
    names: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::from([own.to_lowercase()]);
    names
        .into_iter()
        .filter(|n| rules.source_on(n) && seen.insert(n.to_lowercase()))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_note_names_each_enabled_book_once() {
        let rules = PickRules {
            restriction: SourceRestriction::excluding(["Off Book"]),
            offered: None,
        };
        assert_eq!(
            distinct_sources(
                &rules,
                "SRD 5.1",
                ["Made Up", "made up", "Off Book", "SRD 5.1", "Other", "Made Up"]
            ),
            ["Made Up", "Other"]
        );
        assert!(PickRules::none().offers("X", "Off Book"));
        assert!(!rules.offers("X", "Off Book"));
    }
}
