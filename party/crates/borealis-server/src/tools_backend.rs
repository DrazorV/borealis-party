//! The server side of `party-tools`: every tool calls the HTTP API's own handler with the caller's [`User`], so the
//! account, DM/player and party checks, the one 404 and the player projection are exactly what the app gets.
//! Used by the MCP server (`mcp.rs`), never anything else.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::Json;
use party_tools::args::{BestiarySearchArgs, CharacterGetArgs};
use party_tools::{BoxFuture, CallRecord, Caller, ToolCall, ToolError, ToolSpec};
use serde_json::{json, Map, Value};

use crate::auth::User;

use crate::db::Activity;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// The tools' backend for one request: the server state and the caller's account, read from the database by the
/// auth extractor for this very request (so a lost DM role or a disabled account counts at once).
pub struct ServerTools {
    pub state: AppState,
    pub user: User,
    /// The token or app the calls came through, as the activity log names it (`TokenClient::label`); none when
    /// unknown.
    pub client: Option<String>,
}

impl ServerTools {
    #[must_use]
    pub fn new(state: AppState, user: User) -> Self {
        Self {
            state,
            user,
            client: None,
        }
    }

    /// Calls through this token or app (`TokenClient::label`).
    #[must_use]
    pub fn via_client(mut self, label: String) -> Self {
        self.client = Some(label);
        self
    }
}

/// The API's error as a tool error. Its messages are the API's own fixed texts.
pub(crate) fn from_api(e: ApiError) -> ToolError {
    match e {
        ApiError::NotFound(_) => ToolError::NotFound,
        ApiError::Invalid(m) => ToolError::InvalidArgs(m),
        ApiError::TooMany => ToolError::RateLimited,
        ApiError::Unauthorized(m) | ApiError::Forbidden(m) => ToolError::Refused {
            code: "forbidden".into(),
            message: m,
        },
        ApiError::Refused(code, m) => ToolError::Refused {
            code: code.into(),
            message: m,
        },
        ApiError::Conflict(m) => ToolError::Refused {
            code: "conflict".into(),
            message: m,
        },
        ApiError::Internal(detail) => {
            tracing::error!("tools: internal error: {detail}");
            ToolError::Internal
        }
        ApiError::Command(v) => command_error(v),
    }
}

/// A command's refusal `{code, message, ...}`: everything besides code and message (a validation report, the
/// element as sent) goes to the model as `details`, so it can fix the call and retry.
fn command_error(v: Value) -> ToolError {
    let code = v
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("refused")
        .to_string();
    if code == "not_found" {
        return ToolError::NotFound;
    }
    let message = v
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("The server refused it.")
        .to_string();
    let mut details = v;
    if let Some(map) = details.as_object_mut() {
        map.remove("code");
        map.remove("message");
        if !map.is_empty() {
            return ToolError::RefusedWith {
                code,
                message,
                details,
            };
        }
    }
    ToolError::Refused { code, message }
}

fn json_of(r: ApiResult<Json<Value>>) -> Result<Value, ToolError> {
    r.map(|Json(v)| v).map_err(from_api)
}

fn body_of(v: Value) -> Bytes {
    Bytes::from(v.to_string())
}

/// `Some(x)` keys only.
fn object(pairs: &[(&str, Option<Value>)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if let Some(v) = v {
            m.insert((*k).to_string(), v.clone());
        }
    }
    Value::Object(m)
}

/// XP never leaves the server through a tool (D16), wherever it sits in a block.
fn without_xp(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for key in party_encounter::creature::HIDDEN_KEYS {
                map.remove(key);
            }
            map.remove("xpLair");
            map.values_mut().for_each(without_xp);
        }
        Value::Array(items) => items.iter_mut().for_each(without_xp),
        _ => {}
    }
}

impl ServerTools {
    async fn bestiary_search(&self, a: BestiarySearchArgs) -> Result<Value, ToolError> {
        let query: crate::bestiary::SearchQuery =
            serde_json::from_value(serde_json::to_value(&a).map_err(|_| ToolError::Internal)?)
                .map_err(|e| ToolError::InvalidArgs(e.to_string()))?;
        let page = crate::bestiary::search(&self.state, &self.user.id, &query)
            .await
            .map_err(from_api)?;
        let mut v = serde_json::to_value(page).map_err(|_| ToolError::Internal)?;
        without_xp(&mut v);
        Ok(v)
    }

    async fn monster_get(&self, id: &str) -> Result<Value, ToolError> {
        let found = crate::bestiary::monster(&self.state, &self.user.id, id)
            .await
            .map_err(from_api)?
            .ok_or(ToolError::NotFound)?;
        let mut v = serde_json::to_value(found).map_err(|_| ToolError::Internal)?;
        without_xp(&mut v);
        Ok(v)
    }

    async fn character_list(&self) -> Result<Value, ToolError> {
        let st = State(self.state.clone());
        let own = json_of(
            crate::rpc::command(
                st.clone(),
                self.user.clone(),
                Path("list_characters".into()),
                Json(Map::new()),
            )
            .await,
        )?;
        let parties = json_of(crate::parties::list(st.clone(), self.user.clone()).await)?;
        let mut run = Vec::new();
        for p in parties["parties"].as_array().into_iter().flatten() {
            if p["role"] != "dm" {
                continue;
            }
            let Some(id) = p["id"].as_str() else { continue };
            let roster = json_of(
                crate::parties::roster(st.clone(), self.user.clone(), Path(id.to_string())).await,
            )?;
            run.push(json!({ "partyId": id, "name": p["name"], "roster": roster }));
        }
        Ok(json!({ "own": own, "partiesYouRun": run }))
    }

    async fn character_get(&self, a: CharacterGetArgs) -> Result<Value, ToolError> {
        let st = State(self.state.clone());
        match (a.party_id, a.owner_id) {
            (Some(party), Some(owner)) => json_of(
                crate::parties::view_snapshot(
                    st,
                    self.user.clone(),
                    Path((party, owner, a.character_id)),
                )
                .await,
            ),
            (None, None) => {
                let mut args = Map::new();
                args.insert("characterId".into(), json!(a.character_id));
                json_of(
                    crate::rpc::command(
                        st,
                        self.user.clone(),
                        Path("get_snapshot".into()),
                        Json(args),
                    )
                    .await,
                )
                .map_err(|e| match e {
                    // An id that isn't one of the caller's characters: the same answer as anything else unknown.
                    ToolError::Refused { .. } | ToolError::InvalidArgs(_) => ToolError::NotFound,
                    other => other,
                })
            }
            _ => Err(ToolError::InvalidArgs(
                "partyId and ownerId go together".into(),
            )),
        }
    }

    async fn run(&self, call: ToolCall) -> Result<Value, ToolError> {
        let st = State(self.state.clone());
        let user = self.user.clone();
        match call {
            ToolCall::BestiarySearch(a) => self.bestiary_search(a).await,
            ToolCall::MonsterGet(a) => self.monster_get(&a.id).await,
            ToolCall::EncounterList(a) => {
                json_of(crate::encounters::list(st, user, Path(a.party_id)).await)
            }
            ToolCall::EncounterGet(a) => {
                json_of(crate::encounters::read(st, user, Path(a.encounter_id)).await)
            }
            ToolCall::EncounterCreate(a) => {
                let body = object(&[
                    ("name", Some(json!(a.name))),
                    ("publicName", a.public_name.map(Value::from)),
                    ("hpMode", a.hp_mode.map(Value::from)),
                    ("dmNotes", a.notes.map(Value::from)),
                ]);
                json_of(crate::encounters::create(st, user, Path(a.party_id), body_of(body)).await)
            }
            ToolCall::EncounterUpdate(a) => {
                let body = object(&[
                    ("name", a.name.map(Value::from)),
                    ("publicName", a.public_name.map(Value::from)),
                    ("status", a.status.map(Value::from)),
                    ("playerVisible", a.player_visible.map(Value::from)),
                    ("hpMode", a.hp_mode.map(Value::from)),
                    ("dmNotes", a.notes.map(Value::from)),
                ]);
                json_of(
                    crate::encounters::update(st, user, Path(a.encounter_id), body_of(body)).await,
                )
            }
            ToolCall::EncounterAddCreature(a) => {
                let body = object(&[
                    ("elementId", Some(json!(a.element_id))),
                    ("count", a.count.map(Value::from)),
                    ("dmLabel", a.dm_label.map(Value::from)),
                    ("publicName", a.public_name.map(Value::from)),
                    ("hidden", a.hidden.map(Value::from)),
                    ("ownerId", a.owner_creature_id.map(Value::from)),
                    ("initMode", a.init_mode.map(Value::from)),
                ]);
                json_of(
                    crate::encounters::add_creatures(st, user, Path(a.encounter_id), body_of(body))
                        .await,
                )
            }
            ToolCall::EncounterRemoveCreature(a) => json_of(
                crate::encounters::remove_creature(st, user, Path((a.encounter_id, a.creature_id)))
                    .await,
            ),
            ToolCall::CharacterList(_) => self.character_list().await,
            ToolCall::CharacterGet(a) => self.character_get(a).await,
            ToolCall::External { tool, args } => {
                external(&self.state, &self.user, tool, args).await
            }
        }
    }
}

/// Tools other modules serve, registered in [`crate::mcp::registry`] through [`external_specs`].
///
/// * part D (`homebrew.rs`): `homebrew_*` and `index_build` (D47: the index comes back as data; nothing is hosted);
/// * the simulator (`simulate.rs`, D26): `sim_*`, on copies of encounters only (D25: no combat is read or written).
///
/// The registry has already checked the scope and the request id (which is removed here: it is the registry's).
/// The module re-derives the account's rights itself, as its HTTP routes do.
async fn external(
    state: &AppState,
    user: &User,
    tool: &'static str,
    mut args: Map<String, Value>,
) -> Result<Value, ToolError> {
    args.remove("requestId");
    if tool.starts_with("sim_") {
        return crate::simulate_tools::call_tool(state, user, tool, args).await;
    }
    crate::homebrew::call_tool(state, user, tool, Value::Object(args))
        .await
        .map_err(from_api)
}

/// The specs of [`external`]'s tools, as their modules define them (replacing the registry's placeholders):
/// part D's homebrew tools and `index_build`, and the simulator's `sim_*`.
pub fn external_specs() -> Vec<ToolSpec> {
    let mut specs: Vec<ToolSpec> = crate::homebrew::tools()
        .into_iter()
        .map(|t| {
            let scope = match t.scope {
                crate::homebrew::ToolScope::HomebrewRead => party_tools::Scope::HomebrewRead,
                crate::homebrew::ToolScope::HomebrewWrite => party_tools::Scope::HomebrewWrite,
            };
            ToolSpec::external(t.name, t.title, t.description, scope, t.input_schema)
        })
        .collect();
    specs.extend(crate::simulate_tools::tools());
    specs
}

impl party_tools::Backend for ServerTools {
    fn call<'a>(
        &'a self,
        caller: &'a Caller,
        call: ToolCall,
    ) -> BoxFuture<'a, Result<Value, ToolError>> {
        Box::pin(async move {
            if caller.account != self.user.id {
                // A backend is built for one account; a mismatch is a bug, never a way in.
                tracing::error!("tools: a caller for another account reached the backend");
                return Err(ToolError::Internal);
            }
            if !crate::config::features().ai {
                return Err(ToolError::Unavailable(
                    "AI tools are switched off on this server.".into(),
                ));
            }
            self.run(call).await
        })
    }

    fn record<'a>(&'a self, caller: &'a Caller, record: CallRecord) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // Name, scope, account and status only: never the arguments.
            let detail = format!(
                "{} {} {} {}",
                record.via.as_str(),
                record.tool,
                record.scope,
                record.status
            );
            let mut entry = Activity::new("ai_tool").by(&self.user).detail(detail);
            if let Some(token) = &caller.token {
                entry = entry.on(token, self.client.as_deref().unwrap_or("token"));
            }
            crate::admin::record(&self.state, entry).await;
        })
    }
}
