//! Windows, the app builder and plugins: surfaces a server does not have.
//!
//! Upstream's shell code (`run()`, the splash window, the plugins) has to compile, but the server never calls it. These
//! are inert placeholders with just enough shape for that.

use std::marker::PhantomData;

use serde_json::Value;

use crate::{AppHandle, Error, Headless, Invocation, Manager, Result, Runtime};

/// `tauri::Config`.
#[derive(Debug, Default)]
pub struct Config {
    /// Application settings.
    pub app: AppConfig,
    /// The app's version, for the updater code.
    pub version: Option<String>,
    /// The bundle section.
    pub bundle: BundleConfig,
}

/// `tauri::Config::app`.
#[derive(Debug, Default)]
pub struct AppConfig {
    /// Windows from the configuration.
    pub windows: Vec<WindowConfig>,
}

/// One configured window.
#[derive(Debug, Default)]
pub struct WindowConfig {
    /// Extra browser arguments.
    pub additional_browser_args: Option<String>,
}

/// `tauri::Config::bundle`.
#[derive(Debug, Default)]
pub struct BundleConfig {
    /// Whether updater artifacts are built.
    pub create_updater_artifacts: bool,
}

/// `tauri::Context`.
pub struct Context<R: Runtime = Headless> {
    config: Config,
    runtime: PhantomData<R>,
}

impl<R: Runtime> Default for Context<R> {
    fn default() -> Self {
        Self {
            config: Config::default(),
            runtime: PhantomData,
        }
    }
}

impl<R: Runtime> Context<R> {
    /// The configuration.
    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }
}

/// `tauri::generate_context!`.
#[macro_export]
macro_rules! generate_context {
    () => {
        $crate::Context::default()
    };
}

/// `tauri::App`.
pub struct App<R: Runtime = Headless> {
    handle: AppHandle<R>,
}

impl<R: Runtime> App<R> {
    /// The app's handle.
    pub fn handle(&self) -> &AppHandle<R> {
        &self.handle
    }

    /// Never runs on the server.
    pub fn run<F: FnMut(&AppHandle<R>, RunEvent)>(self, _: F) {
        unreachable!("the server has no window loop");
    }
}

impl<R: Runtime> Manager<R> for App<R> {
    fn app_handle(&self) -> &AppHandle<R> {
        &self.handle
    }
}

/// `tauri::RunEvent`.
#[derive(Debug)]
pub enum RunEvent {
    /// Nothing happens on a server.
    Ready,
}

/// `tauri::Builder`.
pub struct Builder<R: Runtime = Headless>(PhantomData<R>);

impl<R: Runtime> Default for Builder<R> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

type SetupResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Outcome = Option<std::result::Result<Value, Value>>;

impl Builder<Headless> {
    /// Ignored.
    #[must_use]
    pub fn plugin(self, _: plugin::TauriPlugin<Headless>) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn manage<T: Send + Sync + 'static>(self, _: T) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn on_window_event<F: Fn(&Window, &WindowEvent)>(self, _: F) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn setup<F: FnOnce(&mut App) -> SetupResult>(self, _: F) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn invoke_handler<F: Fn(&str, &Invocation) -> Outcome>(self, _: F) -> Self {
        self
    }

    /// Always fails: the server opens workspaces itself.
    ///
    /// # Errors
    /// Always.
    pub fn build(self, _: Context) -> Result<App> {
        Err(Error::Anyhow(anyhow::anyhow!("no window system")))
    }
}

/// `tauri::Window` and `tauri::WebviewWindow`.
pub struct Window<R: Runtime = Headless> {
    handle: AppHandle<R>,
}

impl<R: Runtime> Manager<R> for Window<R> {
    fn app_handle(&self) -> &AppHandle<R> {
        &self.handle
    }
}

impl<R: Runtime> Window<R> {
    /// The window's label.
    #[must_use]
    pub fn label(&self) -> &str {
        ""
    }

    /// No window to show.
    ///
    /// # Errors
    /// Never.
    pub fn show(&self) -> Result<()> {
        Ok(())
    }

    /// No window to focus.
    ///
    /// # Errors
    /// Never.
    pub fn set_focus(&self) -> Result<()> {
        Ok(())
    }

    /// No window to close.
    ///
    /// # Errors
    /// Never.
    pub fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// `tauri::WindowEvent`.
#[derive(Debug)]
pub enum WindowEvent {
    /// Closed.
    Destroyed,
    /// Backgrounded.
    Suspended,
    /// Foregrounded.
    Resumed,
    /// Files dropped.
    DragDrop(DragDropEvent),
}

/// `tauri::DragDropEvent`.
#[derive(Debug)]
pub enum DragDropEvent {
    /// A drop.
    Drop {
        /// The files.
        paths: Vec<std::path::PathBuf>,
    },
}

/// `tauri::WebviewUrl`.
#[derive(Debug)]
pub enum WebviewUrl {
    /// A page of the app.
    App(std::path::PathBuf),
}

/// `tauri::WebviewWindowBuilder`.
pub struct WebviewWindowBuilder<R: Runtime = Headless>(AppHandle<R>);

impl<R: Runtime> WebviewWindowBuilder<R> {
    /// Ignored.
    #[must_use]
    pub fn new(handle: &AppHandle<R>, _: &str, _: WebviewUrl) -> Self {
        Self(handle.clone())
    }

    /// Ignored.
    #[must_use]
    pub fn additional_browser_args(self, _: &str) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn title(self, _: &str) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn inner_size(self, _: f64, _: f64) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn resizable(self, _: bool) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn maximizable(self, _: bool) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn minimizable(self, _: bool) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn decorations(self, _: bool) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn shadow(self, _: bool) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn center(self) -> Self {
        self
    }

    /// Ignored.
    #[must_use]
    pub fn background_color(self, _: window::Color) -> Self {
        self
    }

    /// No window to build.
    ///
    /// # Errors
    /// Never.
    pub fn build(self) -> Result<Window<R>> {
        Ok(Window { handle: self.0 })
    }
}

/// `tauri::window`.
pub mod window {
    /// RGBA.
    #[derive(Debug, Clone, Copy)]
    pub struct Color(pub u8, pub u8, pub u8, pub u8);
}

/// `tauri::Listener`.
pub trait Listener<R: Runtime> {
    /// Nothing is ever emitted to the backend here.
    fn listen<F: Fn(Event) + Send + 'static>(&self, _: &str, _: F) {}
}

impl<R: Runtime> Listener<R> for App<R> {}
impl<R: Runtime> Listener<R> for AppHandle<R> {}

/// `tauri::Event`.
#[derive(Debug)]
pub struct Event;

/// `tauri::plugin`.
pub mod plugin {
    use std::marker::PhantomData;

    use crate::{AppHandle, Headless, Runtime};

    /// `tauri::plugin::TauriPlugin`.
    pub struct TauriPlugin<R: Runtime = Headless, C = ()>(PhantomData<(R, C)>);

    /// `tauri::plugin::PluginApi`.
    pub struct PluginApi<R: Runtime = Headless, C = ()>(PhantomData<(R, C)>);

    /// `tauri::plugin::PluginHandle`.
    pub struct PluginHandle<R: Runtime = Headless>(PhantomData<R>);

    /// `tauri::plugin::Builder`.
    pub struct Builder<R: Runtime = Headless, C = ()>(PhantomData<(R, C)>);

    impl<R: Runtime, C> Builder<R, C> {
        /// Ignored.
        #[must_use]
        pub fn new(_: &'static str) -> Self {
            Self(PhantomData)
        }

        /// Ignored.
        #[must_use]
        pub fn setup<F>(self, _: F) -> Self
        where
            F: FnOnce(&AppHandle<R>, PluginApi<R, C>) -> Result<(), Box<dyn std::error::Error>>
                + Send
                + 'static,
        {
            self
        }

        /// An inert plugin.
        #[must_use]
        pub fn build(self) -> TauriPlugin<R, C> {
            TauriPlugin(PhantomData)
        }
    }

    /// An inert plugin, for the plugin crates.
    #[must_use]
    pub fn inert<R: Runtime, C>() -> TauriPlugin<R, C> {
        TauriPlugin(PhantomData)
    }
}

/// `tauri::utils`.
pub mod utils {
    /// `tauri::utils::config`.
    pub mod config {
        /// How the app was packaged.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum BundleType {
            /// A Debian package.
            Deb,
            /// An RPM.
            Rpm,
            /// An AppImage.
            AppImage,
            /// A Windows MSI.
            Msi,
            /// A Windows NSIS installer.
            Nsis,
            /// A macOS app.
            App,
            /// A macOS disk image.
            Dmg,
        }
    }

    /// `tauri::utils::platform`.
    pub mod platform {
        /// The server is not a bundled desktop app.
        #[must_use]
        pub fn bundle_type() -> Option<super::config::BundleType> {
            None
        }
    }
}
