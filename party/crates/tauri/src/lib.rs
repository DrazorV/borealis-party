//! The part of Tauri the Borealis backend touches, without a webview.
//!
//! `src-tauri` compiles against this crate under the name `tauri` when its `headless` feature is on, so the upstream
//! command code builds unchanged. Each [`AppHandle`] is one server workspace: its own managed state and its own event
//! sink. Native surfaces (dialogs, windows, the OS opener) do not exist on a server.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::de::DeserializeOwned;
use serde::Serialize;
pub use serde_json;
use serde_json::{json, Map, Value};

pub use tauri_macros::{command, generate_handler};
pub use url::Url;

mod native;
pub use native::*;

/// Stand-in for `tauri::Error`.
#[derive(Debug)]
pub enum Error {
    /// A failure.
    Anyhow(anyhow::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self::Anyhow(m) = self;
        std::fmt::Display::fmt(m, f)
    }
}

impl std::error::Error for Error {}

/// `tauri::Result`.
pub type Result<T> = std::result::Result<T, Error>;

/// Marker for `tauri::Runtime`.
pub trait Runtime: Send + Sync + 'static {}

/// The only runtime here.
#[derive(Debug, Clone, Copy)]
pub struct Headless;
impl Runtime for Headless {}

type EventSink = Arc<dyn Fn(&str, Value) + Send + Sync>;

struct Inner {
    state: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    events: EventSink,
    data_dir: PathBuf,
    resource_dir: PathBuf,
    config: Config,
}

/// One workspace.
pub struct AppHandle<R: Runtime = Headless> {
    inner: Arc<Inner>,
    runtime: PhantomData<R>,
}

impl<R: Runtime> Clone for AppHandle<R> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            runtime: PhantomData,
        }
    }
}

impl AppHandle {
    /// A workspace whose events go to `events`.
    #[must_use]
    pub fn new(
        data_dir: PathBuf,
        resource_dir: PathBuf,
        events: impl Fn(&str, Value) + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: RwLock::new(HashMap::new()),
                events: Arc::new(events),
                data_dir,
                resource_dir,
                config: Config::default(),
            }),
            runtime: PhantomData,
        }
    }
}

impl<R: Runtime> AppHandle<R> {
    /// The (empty) configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// A server has no windows.
    #[must_use]
    pub fn get_webview_window(&self, _: &str) -> Option<Window<R>> {
        None
    }

    /// Never called on the server.
    pub fn exit(&self, _: i32) {
        unreachable!("the server does not exit on request");
    }

    /// Never called on the server.
    pub fn restart(&self) -> ! {
        unreachable!("the server does not restart itself");
    }
}

/// Reference to managed state.
pub struct State<'r, T: Send + Sync + 'static>(&'r T);

impl<'r, T: Send + Sync + 'static> State<'r, T> {
    /// The value, for as long as the handle it came from.
    #[must_use]
    pub fn inner(&self) -> &'r T {
        self.0
    }
}

impl<T: Send + Sync + 'static> Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
    }
}

impl<T: Send + Sync + 'static> Clone for State<'_, T> {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

/// `tauri::Manager`.
pub trait Manager<R: Runtime> {
    /// The handle behind this object.
    fn app_handle(&self) -> &AppHandle<R>;

    /// Add managed state; the first value of a type stays.
    fn manage<T: Send + Sync + 'static>(&self, value: T) -> bool {
        let mut map = self
            .app_handle()
            .inner
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = TypeId::of::<T>();
        if map.contains_key(&key) {
            return false;
        }
        map.insert(key, Box::new(value));
        true
    }

    /// Managed state, when there is any of this type.
    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>> {
        let map = self
            .app_handle()
            .inner
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = map.get(&TypeId::of::<T>())?.downcast_ref::<T>()?;
        // SAFETY: entries are boxed and never removed or replaced (`manage` keeps the first value), and `Inner` owns the
        // map, so the value's address is stable for as long as the `&self` borrow keeps the handle, hence `Inner`, alive.
        Some(State(unsafe { &*std::ptr::from_ref(value) }))
    }

    /// Managed state.
    ///
    /// # Panics
    /// When nothing of this type is managed (a programming error, as in Tauri).
    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
        self.try_state().expect("state not managed")
    }

    /// `app.path()`.
    fn path(&self) -> PathResolver {
        let handle = self.app_handle();
        PathResolver {
            data_dir: handle.inner.data_dir.clone(),
            resource_dir: handle.inner.resource_dir.clone(),
        }
    }
}

impl<R: Runtime> Manager<R> for AppHandle<R> {
    fn app_handle(&self) -> &AppHandle<R> {
        self
    }
}

/// `app.path()`.
#[derive(Debug, Clone)]
pub struct PathResolver {
    data_dir: PathBuf,
    resource_dir: PathBuf,
}

impl PathResolver {
    /// The workspace's data folder.
    ///
    /// # Errors
    /// Never; matches Tauri's signature.
    pub fn app_data_dir(&self) -> Result<PathBuf> {
        Ok(self.data_dir.clone())
    }

    /// The folder holding the bundled content.
    ///
    /// # Errors
    /// Never; matches Tauri's signature.
    pub fn resource_dir(&self) -> Result<PathBuf> {
        Ok(self.resource_dir.clone())
    }
}

/// `tauri::Emitter`.
pub trait Emitter<R: Runtime> {
    /// Send an event to this workspace's listeners.
    ///
    /// # Errors
    /// When the payload does not serialise.
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()>;

    /// As `emit`: a workspace has one audience.
    ///
    /// # Errors
    /// When the payload does not serialise.
    fn emit_to<S: Serialize + Clone>(&self, _target: &str, event: &str, payload: S) -> Result<()> {
        self.emit(event, payload)
    }
}

impl<R: Runtime> Emitter<R> for Window<R> {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()> {
        self.app_handle().emit(event, payload)
    }
}

impl<R: Runtime> Emitter<R> for AppHandle<R> {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()> {
        let value = serde_json::to_value(payload).map_err(|e| Error::Anyhow(e.into()))?;
        (self.inner.events)(event, value);
        Ok(())
    }
}

/// One command call.
pub struct Invocation {
    /// The workspace it runs in.
    pub app: AppHandle,
    /// The JSON arguments, by the UI's camelCase names.
    pub args: Map<String, Value>,
    /// Only asks whether the command exists: the wrapper returns before reading anything.
    pub probe: bool,
}

/// Something a command can take as a parameter.
pub trait CommandArg<'a>: Sized {
    /// Read the parameter `key`.
    ///
    /// # Errors
    /// An `invalid_argument` API error when the JSON does not fit.
    fn from_invocation(
        key: &'static str,
        invocation: &'a Invocation,
    ) -> std::result::Result<Self, Value>;
}

impl<'a, D: DeserializeOwned> CommandArg<'a> for D {
    fn from_invocation(
        key: &'static str,
        invocation: &'a Invocation,
    ) -> std::result::Result<Self, Value> {
        let value = invocation.args.get(key).cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(|e| {
            json!({ "code": "invalid_argument", "message": format!("The argument {key} is not valid: {e}") })
        })
    }
}

impl<'a, T: Send + Sync + 'static> CommandArg<'a> for State<'a, T> {
    fn from_invocation(
        _: &'static str,
        invocation: &'a Invocation,
    ) -> std::result::Result<Self, Value> {
        invocation.app.try_state::<T>().ok_or_else(|| {
            json!({ "code": "internal", "message": "This feature is not available on the server." })
        })
    }
}

impl<'a> CommandArg<'a> for AppHandle {
    fn from_invocation(
        _: &'static str,
        invocation: &'a Invocation,
    ) -> std::result::Result<Self, Value> {
        Ok(invocation.app.clone())
    }
}

fn serialized<T: Serialize>(value: &T) -> std::result::Result<Value, Value> {
    serde_json::to_value(value).map_err(|e| json!({ "code": "internal", "message": e.to_string() }))
}

/// The JSON outcome of a command returning a `Result`.
///
/// # Errors
/// The command's error, as JSON.
pub fn finish_result<T: Serialize, E: Serialize>(
    outcome: std::result::Result<T, E>,
) -> std::result::Result<Value, Value> {
    match outcome {
        Ok(value) => serialized(&value),
        Err(error) => Err(serialized(&error)?),
    }
}

/// The JSON outcome of a command returning a plain value.
///
/// # Errors
/// When the value does not serialise.
pub fn finish_value<T: Serialize>(value: T) -> std::result::Result<Value, Value> {
    serialized(&value)
}

/// Run an async command to completion from a blocking worker thread.
///
/// # Panics
/// Outside a tokio runtime.
pub fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Handle::current().block_on(future)
}

/// `tauri::async_runtime`.
pub mod async_runtime {
    pub use tokio::task::JoinHandle;

    /// Spawn a future on the server's runtime.
    pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(future)
    }

    /// Run blocking work on the server's blocking pool.
    pub fn spawn_blocking<F, T>(work: F) -> JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(work)
    }
}
