//! `tauri_build`, doing nothing.

/// `tauri_build::AppManifest`.
#[derive(Debug, Default)]
pub struct AppManifest;

impl AppManifest {
    /// A manifest.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Ignored.
    #[must_use]
    pub fn commands(self, _: &'static [&'static str]) -> Self {
        self
    }
}

/// `tauri_build::Attributes`.
#[derive(Debug, Default)]
pub struct Attributes;

impl Attributes {
    /// Attributes.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Ignored.
    #[must_use]
    pub fn app_manifest(self, _: AppManifest) -> Self {
        self
    }
}

/// Nothing to generate.
///
/// # Errors
/// Never.
pub fn try_build(_: Attributes) -> Result<(), std::convert::Infallible> {
    // As Tauri's own build crate does. Upstream's code assumes every build is either `desktop` or a phone.
    println!("cargo:rustc-check-cfg=cfg(desktop)");
    println!("cargo:rustc-check-cfg=cfg(mobile)");
    println!("cargo:rustc-cfg=desktop");
    Ok(())
}

/// Nothing to generate.
pub fn build() {}
