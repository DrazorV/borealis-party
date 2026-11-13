//! Why a tool call failed. Every one of these is a tool result with `isError: true` for the model; only protocol
//! faults (bad JSON-RPC, an unknown method) are JSON-RPC errors.

use std::fmt;

use serde_json::{json, Value};

use crate::scope::Scope;

/// The one text for anything that doesn't exist or isn't the caller's to see: the API's fixed 404, so a model (or
/// whoever reads its transcript) learns nothing from the difference.
pub const NOT_FOUND: &str = "Not found.";

/// A failed tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// No such tool (or one the caller's scopes don't list).
    UnknownTool,
    /// The token lacks this scope.
    MissingScope(Scope),
    /// The arguments don't fit the tool.
    InvalidArgs(String),
    /// Doesn't exist, or isn't the caller's: one answer for both.
    NotFound,
    /// Over the per-token call rate.
    RateLimited,
    /// Too many heavy calls of this account at once.
    Busy,
    /// Not available on this server (not built yet, or switched off).
    Unavailable(String),
    /// The server refused it with its own code (the combat engine's `refused`, a `conflict`, ...).
    Refused { code: String, message: String },
    /// Something went wrong on the server; logged there, never shown.
    Internal,
}

impl ToolError {
    /// The stable code (the API's codes where they exist).
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::UnknownTool => "unknown_tool",
            Self::MissingScope(_) => "insufficient_scope",
            Self::InvalidArgs(_) => "invalid_argument",
            Self::NotFound => "not_found",
            Self::RateLimited => "too_many_requests",
            Self::Busy => "busy",
            Self::Unavailable(_) => "unavailable",
            Self::Refused { code, .. } => code,
            Self::Internal => "internal",
        }
    }

    /// The status word written to the activity log.
    #[must_use]
    pub fn status(&self) -> &str {
        self.code()
    }

    /// `{code, message}` for the model.
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({ "code": self.code(), "message": self.to_string() })
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool => f.write_str("There is no such tool."),
            Self::MissingScope(s) => write!(f, "This token isn't allowed to do that (needs {s})."),
            Self::InvalidArgs(m) => write!(f, "Invalid arguments: {m}"),
            Self::NotFound => f.write_str(NOT_FOUND),
            Self::RateLimited => f.write_str("Too many calls. Wait a minute and try again."),
            Self::Busy => f.write_str("Another long call of this account is running. Try again."),
            Self::Unavailable(m) => f.write_str(m),
            Self::Refused { message, .. } => f.write_str(message),
            Self::Internal => f.write_str("Something went wrong on the server."),
        }
    }
}

impl std::error::Error for ToolError {}
