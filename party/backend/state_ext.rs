//! Party server hooks into `AppState`, pulled in by the single `#[path]` line in `src-tauri/src/state.rs`.
//!
//! This has to be a child module of `state` to reach `AppState`'s private content slot. Keeping the code here means
//! upstream's file gains one line rather than a block that could conflict on every merge.

use super::{AppPaths, AppState, ContentHub};
use crate::error::ApiResult;
use std::path::PathBuf;
use std::sync::Arc;

impl AppState {
    /// A state over a content snapshot other states are using too.
    ///
    /// # Errors
    /// `io_error` if the data folders cannot be created.
    pub fn party_with_shared_hub(paths: AppPaths, hub: Arc<ContentHub>) -> ApiResult<Self> {
        let state = Self::new_degraded(paths, "")?;
        *state.content.write()? = hub;
        Ok(state)
    }

    /// Swap in a snapshot other states are using too; search follows, as with `set_hub`.
    ///
    /// # Errors
    /// `internal` if the lock is poisoned.
    pub fn party_set_shared_hub(&self, hub: Arc<ContentHub>) -> ApiResult<()> {
        *self.content.write()? = Arc::clone(&hub);
        self.search.content_changed(Arc::clone(&hub));
        self.refresh_semantic_index_with(hub);
        Ok(())
    }

    /// The pack folders in this state's library.
    #[must_use]
    pub fn party_library_packs(&self) -> Vec<PathBuf> {
        super::user_pack_dirs(&self.paths.library())
    }

    /// The pack folders in the library at `paths`, before a state exists.
    #[must_use]
    pub fn party_packs_at(paths: &AppPaths) -> Vec<PathBuf> {
        super::user_pack_dirs(&paths.library())
    }
}
