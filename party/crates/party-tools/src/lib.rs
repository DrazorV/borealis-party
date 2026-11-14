//! The one tool registry for the AI apps that connect to Borealis Connect over MCP (`/api/mcp`). See
//! `party/docs/party-tools.md`.
//!
//! * A tool is a [`ToolSpec`]: a name, a JSON schema and one [`Scope`].
//! * [`Registry::call`] checks the scope, parses the arguments ([`args`]), applies the [`Limits`] (per-token rate,
//!   per-account heavy calls, idempotent writes) and hands a typed [`ToolCall`] to a [`Backend`].
//! * The server's backend calls the same functions as its HTTP API, with the caller's own account, so the DM,
//!   player and party checks and the player projection are exactly the API's. Nothing here decides who may see what.
//! * Results are data ([`DATA_NOTE`]), never instructions.

pub mod args;
pub mod error;
pub mod guard;
pub mod mcp;
pub mod scope;
pub mod spec;

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use sha2::{Digest, Sha256};

pub use args::ToolCall;
pub use error::ToolError;
pub use guard::Limits;
pub use scope::Scope;
pub use spec::{ToolSpec, DATA_NOTE};

/// A boxed future, for the object-safe [`Backend`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Which surface the call came through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// `/api/mcp`, with a token or an OAuth grant (the only surface since 0.3.3 removed the in-app assistant).
    Mcp,
}

impl Via {
    /// For the activity log.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
        }
    }
}

/// Who is calling.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The account the tool acts as.
    pub account: String,
    /// The token or OAuth grant id; `None` for a call made without one (tests).
    pub token: Option<String>,
    /// What the token was granted.
    pub scopes: BTreeSet<Scope>,
    /// Which surface.
    pub via: Via,
}

impl Caller {
    /// The key limits and the idempotency cache count by: the token, else the account.
    #[must_use]
    pub fn key(&self) -> String {
        match &self.token {
            Some(t) => format!("token:{t}"),
            None => format!("account:{}", self.account),
        }
    }

    /// Whether the scopes allow `scope` (implications included).
    #[must_use]
    pub fn may(&self, scope: Scope) -> bool {
        scope::grants(&self.scopes, scope)
    }
}

/// What the activity log gets of a call: never its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRecord {
    pub tool: &'static str,
    pub scope: Scope,
    /// `ok`, `replayed` or the error code.
    pub status: String,
    pub via: Via,
}

/// The server side of the tools.
pub trait Backend: Send + Sync {
    /// Run one checked call as `caller.account`.
    fn call<'a>(
        &'a self,
        caller: &'a Caller,
        call: ToolCall,
    ) -> BoxFuture<'a, Result<Value, ToolError>>;
    /// Write one finished call to the account's activity.
    fn record<'a>(&'a self, caller: &'a Caller, record: CallRecord) -> BoxFuture<'a, ()>;
}

/// A successful call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// The result: DATA.
    pub value: Value,
    /// The first answer to an earlier call with the same request id.
    pub replayed: bool,
}

/// The registry. Cheap to share (`Arc`); one per server.
#[derive(Debug, Default)]
pub struct Registry {
    limits: Limits,
    /// Tools other modules serve, by [`Registry::with_tools`]; one of the same name replaces a built-in placeholder.
    extra: Vec<ToolSpec>,
    rate: guard::Rate,
    heavy: guard::Heavy,
    done: guard::Idempotency,
}

fn fingerprint(tool: &str, args: &Value) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(tool.as_bytes());
    h.update([0]);
    h.update(args.to_string().as_bytes());
    h.finalize().into()
}

impl Registry {
    /// A registry with these limits.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Add tools another module serves (part D's `homebrew_*` and `index_build`, the simulator's `sim_*`). A tool of
    /// the same name as a built-in one replaces it; a name already added once is replaced too.
    #[must_use]
    pub fn with_tools(mut self, specs: impl IntoIterator<Item = ToolSpec>) -> Self {
        for spec in specs {
            self.extra.retain(|s| s.name != spec.name);
            self.extra.push(spec);
        }
        self
    }

    /// Every tool, in a fixed order: the built-in ones (replaced where a module registered its own), then the added
    /// ones.
    #[must_use]
    pub fn all(&self) -> Vec<&ToolSpec> {
        let mut out: Vec<&ToolSpec> = spec::all()
            .iter()
            .map(|s| self.extra.iter().find(|e| e.name == s.name).unwrap_or(s))
            .collect();
        out.extend(self.extra.iter().filter(|e| spec::find(e.name).is_none()));
        out
    }

    /// One tool by name.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&ToolSpec> {
        self.extra
            .iter()
            .find(|s| s.name == name)
            .or_else(|| spec::find(name))
    }

    /// The tools the caller's scopes allow, in a fixed order.
    #[must_use]
    pub fn list(&self, caller: &Caller) -> Vec<&ToolSpec> {
        self.all()
            .into_iter()
            .filter(|s| caller.may(s.scope))
            .collect()
    }

    /// Run one call. Every outcome, failures included, is recorded with [`Backend::record`].
    ///
    /// # Errors
    /// A [`ToolError`]; the model gets it as an error result.
    pub async fn call(
        &self,
        backend: &dyn Backend,
        caller: &Caller,
        name: &str,
        args: Value,
    ) -> Result<ToolOutput, ToolError> {
        let Some(spec) = self.find(name) else {
            return Err(ToolError::UnknownTool);
        };
        let out = self.run(backend, caller, spec, args).await;
        let status = match &out {
            Ok(o) if o.replayed => "replayed".to_string(),
            Ok(_) => "ok".to_string(),
            Err(e) => e.status().to_string(),
        };
        backend
            .record(
                caller,
                CallRecord {
                    tool: spec.name,
                    scope: spec.scope,
                    status,
                    via: caller.via,
                },
            )
            .await;
        out
    }

    async fn run(
        &self,
        backend: &dyn Backend,
        caller: &Caller,
        spec: &ToolSpec,
        args: Value,
    ) -> Result<ToolOutput, ToolError> {
        if !caller.may(spec.scope) {
            return Err(ToolError::MissingScope(spec.scope));
        }
        let call = args::parse(spec.name, &args)?;
        if spec.write && call.request_id().is_none() {
            return Err(ToolError::InvalidArgs("send a requestId".into()));
        }
        let key = caller.key();
        if !self.rate.take(&key, self.limits.calls_per_minute) {
            return Err(ToolError::RateLimited);
        }
        let heavy = spec.heavy || matches!(&call, ToolCall::BestiarySearch(a) if a.unfiltered());
        let _permit = if heavy {
            Some(
                self.heavy
                    .try_take(&caller.account, self.limits.heavy_per_account)
                    .ok_or(ToolError::Busy)?,
            )
        } else {
            None
        };
        let Some(request) = call.request_id().map(str::to_string) else {
            let value = backend.call(caller, call).await?;
            return self.sized(value, false);
        };
        let done_key = format!("{key}\u{0}{}\u{0}{request}", spec.name);
        let print = fingerprint(spec.name, &args);
        let lock = self.done.lock_of(&done_key);
        let _held = lock.lock().await;
        match self
            .done
            .seen(&done_key, &print, self.limits.idempotency_ttl)
        {
            guard::Seen::Done(value) => return self.sized(value, true),
            guard::Seen::Reused => {
                return Err(ToolError::InvalidArgs(
                    "this requestId was already used for a different change; send a new one".into(),
                ))
            }
            guard::Seen::New => {}
        }
        let mut value = backend.call(caller, call).await?;
        if value.to_string().len() > self.limits.max_result_bytes {
            // The write happened: say so, rather than an error a model would retry.
            value = serde_json::json!({
                "done": true,
                "note": "The change was made; its result is too large to return. Read it with the matching get tool.",
            });
        }
        self.done
            .keep(&done_key, print, value.clone(), self.limits.idempotency_max);
        Ok(ToolOutput {
            value,
            replayed: false,
        })
    }

    fn sized(&self, value: Value, replayed: bool) -> Result<ToolOutput, ToolError> {
        if value.to_string().len() > self.limits.max_result_bytes {
            return Err(ToolError::InvalidArgs(
                "the result is too large; narrow the request (a smaller limit or a filter)".into(),
            ));
        }
        Ok(ToolOutput { value, replayed })
    }
}
