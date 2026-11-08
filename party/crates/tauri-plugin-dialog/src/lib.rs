//! `tauri_plugin_dialog`, inert: a server has nobody to show a dialog to, so every dialog is cancelled at once.
//!
//! None of the commands that open one are reachable over the network (`backend.rs`); this only has to compile.

use std::path::PathBuf;

use tauri::plugin::{inert, TauriPlugin};
use tauri::{AppHandle, Runtime};

/// An inert plugin.
#[must_use]
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    inert()
}

/// `app.dialog()`.
pub trait DialogExt {
    /// The dialog entry point.
    fn dialog(&self) -> Dialog;
}

impl<R: Runtime> DialogExt for AppHandle<R> {
    fn dialog(&self) -> Dialog {
        Dialog
    }
}

/// `tauri_plugin_dialog::Dialog`.
#[derive(Debug, Clone, Copy)]
pub struct Dialog;

impl Dialog {
    /// A file dialog.
    #[must_use]
    pub fn file(&self) -> FileDialog {
        FileDialog
    }

    /// A message dialog.
    #[must_use]
    pub fn message(&self, _: impl Into<String>) -> MessageDialog {
        MessageDialog
    }
}

/// A picked path; never produced here.
#[derive(Debug)]
pub struct FilePath;

impl FilePath {
    /// The path.
    ///
    /// # Errors
    /// Always, since nothing is ever picked.
    pub fn into_path(self) -> Result<PathBuf, std::io::Error> {
        Err(std::io::Error::other("no dialogs on the server"))
    }
}

/// A file dialog, cancelled as soon as it is shown.
#[derive(Debug)]
pub struct FileDialog;

impl FileDialog {
    /// Ignored.
    #[must_use]
    pub fn add_filter(self, _: impl AsRef<str>, _: &[&str]) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn set_file_name(self, _: impl AsRef<str>) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn set_directory(self, _: impl AsRef<std::path::Path>) -> Self {
        self
    }

    /// Cancelled.
    #[must_use]
    pub fn blocking_save_file(self) -> Option<FilePath> {
        None
    }

    /// Cancelled.
    #[must_use]
    pub fn blocking_pick_file(self) -> Option<FilePath> {
        None
    }

    /// Cancelled.
    #[must_use]
    pub fn blocking_pick_files(self) -> Option<Vec<FilePath>> {
        None
    }

    /// Cancelled.
    #[must_use]
    pub fn blocking_pick_folder(self) -> Option<FilePath> {
        None
    }
}

/// `MessageDialogKind`.
#[derive(Debug, Clone, Copy)]
pub enum MessageDialogKind {
    /// A warning.
    Warning,
}

/// `MessageDialogButtons`.
#[derive(Debug, Clone)]
pub enum MessageDialogButtons {
    /// Two labelled buttons.
    OkCancelCustom(String, String),
}

/// A message dialog, answered "no".
#[derive(Debug)]
pub struct MessageDialog;

impl MessageDialog {
    /// Ignored.
    #[must_use]
    pub fn title(self, _: impl Into<String>) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn kind(self, _: MessageDialogKind) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn buttons(self, _: MessageDialogButtons) -> Self {
        self
    }

    /// "No".
    #[must_use]
    pub fn blocking_show(self) -> bool {
        false
    }
}
