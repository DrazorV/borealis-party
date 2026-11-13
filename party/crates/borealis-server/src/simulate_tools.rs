//! The `sim_*` tools in the shared tool registry (`party-tools`, 0.3.1 part B), for `/api/mcp` and the in-app
//! assistant alike, under the scope `encounters:simulate`.
//!
//! Wiring at merge (two lines in `tools_backend.rs`): `external_specs()` returns `simulate_tools::tools()` (plus
//! the other modules' specs), and `external()` sends every `sim_*` call to `simulate_tools::call_tool`. The
//! simulator re-derives that the caller is the encounter's DM on every call, as its module docs say.

use party_tools::{Scope, ToolError, ToolSpec};
use serde_json::{Map, Value};

use crate::auth::User;
use crate::simulate::{self, SimError};
use crate::AppState;

/// The specs of the sim tools.
#[must_use]
pub fn tools() -> Vec<ToolSpec> {
    simulate::tool_defs()
        .iter()
        .map(|d| {
            let spec = ToolSpec::external(
                d.name,
                d.title,
                &d.description,
                Scope::EncountersSimulate,
                d.schema.clone(),
            );
            if d.heavy {
                spec.heavy()
            } else {
                spec
            }
        })
        .collect()
}

/// Run one sim tool.
///
/// # Errors
/// The registry's errors: not found (also for anything not the caller's), invalid arguments, busy, unavailable.
pub async fn call_tool(
    state: &AppState,
    user: &User,
    tool: &str,
    args: Map<String, Value>,
) -> Result<Value, ToolError> {
    simulate::call(state, user, tool, args)
        .await
        .map_err(|e| match e {
            SimError::NotFound => ToolError::NotFound,
            SimError::Invalid(m) => ToolError::InvalidArgs(m),
            SimError::Busy(m) => ToolError::Refused {
                code: "busy".into(),
                message: m,
            },
            SimError::Unavailable(m) => ToolError::Unavailable(m),
            SimError::Internal => ToolError::Internal,
        })
}
