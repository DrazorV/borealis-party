//! `tauri_plugin_single_instance`, inert: the server never runs `tauri::Builder`, so no second launch
//! ever reaches it. Upstream registers the plugin on Windows and Linux; this only has to compile.

use tauri::plugin::{inert, TauriPlugin};
use tauri::{AppHandle, Runtime};

/// An inert plugin; `on_second_instance` is never called.
#[must_use]
pub fn init<R: Runtime, F>(on_second_instance: F) -> TauriPlugin<R>
where
    F: FnMut(&AppHandle<R>, Vec<String>, String) + Send + Sync + 'static,
{
    drop(on_second_instance);
    inert()
}
