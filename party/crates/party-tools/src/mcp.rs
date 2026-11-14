//! The MCP envelope for Streamable HTTP: one JSON-RPC message per POST, answered with one JSON object. No HTTP
//! here (the server's `mcp.rs` does auth, Origin and headers); this is the protocol, so it can be tested alone.
//!
//! Spec (read 2026-11-13): <https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http>,
//! `.../basic/index`, `.../basic/versioning`, `.../server/discover`, `.../server/tools`, `.../basic/authorization`.
//!
//! * **Modern (2026-07-28):** stateless. Every request carries `_meta["io.modelcontextprotocol/protocolVersion"]`
//!   and `clientCapabilities`; `server/discover` replaces `initialize`; results carry `resultType`.
//! * **Legacy (2025-11-25, 2025-06-18, 2025-03-26):** `initialize` answers the newest of these the client asked
//!   for. We mint no `Mcp-Session-Id` (sessions are optional there), so every later request is answered on its own,
//!   its era read from the `MCP-Protocol-Version` header.
//! * Batches (JSON arrays) are refused: the body is a single message.
//! * Same shape as upstream's stdio server (`crates/borealis-mcp/src/rpc.rs`), whose `handle` is private; an
//!   upstream PR could make it public so both share one implementation.

use serde_json::{json, Value};

use crate::{Backend, Caller, Registry, Scope, ToolError, DATA_NOTE};

/// The modern revision.
pub const MODERN_VERSION: &str = "2026-07-28";
/// Handshake revisions served after `initialize`, newest first.
pub const LEGACY_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const HEADER_MISMATCH: i64 = -32020;
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
/// The list depends on the token's scopes, so it is cached per client only, briefly.
const TTL_MS: u64 = 300_000;

/// What the server knows of the HTTP request besides its body.
#[derive(Debug, Clone, Copy, Default)]
pub struct Headers<'a> {
    /// `MCP-Protocol-Version`.
    pub protocol_version: Option<&'a str>,
    /// `Mcp-Method`.
    pub method: Option<&'a str>,
    /// `Mcp-Name`.
    pub name: Option<&'a str>,
}

/// The answer: an HTTP status and a JSON body (none for 202).
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Option<Value>,
    /// A `tools/call` refused for this scope: the server answers 403 with an `insufficient_scope` challenge, so a
    /// connector can ask the user for it (step-up).
    pub insufficient_scope: Option<Scope>,
}

impl Reply {
    fn ok(body: Value) -> Self {
        Self {
            status: 200,
            body: Some(body),
            insufficient_scope: None,
        }
    }
}

fn error_body(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

fn fail(status: u16, id: &Value, code: i64, message: &str, data: Option<Value>) -> Reply {
    Reply {
        status,
        body: Some(error_body(id, code, message, data)),
        insufficient_scope: None,
    }
}

/// The server's name and version.
#[must_use]
pub fn server_info() -> Value {
    json!({ "name": "borealis-party", "title": "Borealis Party", "version": env!("CARGO_PKG_VERSION") })
}

/// What a connected app is told about using these tools (`initialize` and `server/discover`): a short workflow for a
/// DM's assistant. The one source of the text: the Account page shows it (`GET /api/me/ai`), and MCP clients also read
/// it as the resource [`INSTRUCTIONS_URI`].
pub const INSTRUCTIONS: &str = include_str!("instructions.md");

/// The same text as an MCP resource, for clients that don't show `instructions` to the model (or drop it after a
/// while): an agent can list and read it like a file.
pub const INSTRUCTIONS_URI: &str = "borealis://agent/instructions";

/// `resources/read`'s "not found" (MCP 2025-11-25 server/resources, Error Handling).
pub const RESOURCE_NOT_FOUND: i64 = -32002;

/// What the server offers. Resources are fixed (no subscriptions, no list changes); tools depend on the scopes.
fn capabilities() -> Value {
    json!({
        "tools": { "listChanged": false },
        "resources": { "subscribe": false, "listChanged": false },
    })
}

fn instructions_resource() -> Value {
    json!({
        "uri": INSTRUCTIONS_URI,
        "name": "instructions",
        "title": "How to use Borealis Party",
        "description": "The workflow for a DM's assistant: what the tools do, in which order, and the rules (read this first).",
        "mimeType": "text/markdown",
        "size": INSTRUCTIONS.len(),
    })
}

enum Era {
    Modern,
    Legacy,
}

/// Handle one POSTed body for `caller`.
pub async fn handle(
    registry: &Registry,
    backend: &dyn Backend,
    caller: &Caller,
    headers: Headers<'_>,
    body: &[u8],
) -> Reply {
    let null = Value::Null;
    let msg: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return fail(400, &null, PARSE_ERROR, "parse error", None),
    };
    let Some(obj) = msg.as_object() else {
        return fail(
            400,
            &null,
            INVALID_REQUEST,
            "send one JSON-RPC message per request (no batches)",
            None,
        );
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return fail(400, &null, INVALID_REQUEST, "jsonrpc must be \"2.0\"", None);
    }
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return fail(
            400,
            &null,
            INVALID_REQUEST,
            "clients send requests or notifications only",
            None,
        );
    };
    let id = match obj.get("id") {
        // A notification (notifications/initialized and the like): accepted, nothing to answer.
        None => {
            return Reply {
                status: 202,
                body: None,
                insufficient_scope: None,
            }
        }
        Some(id) if id.is_string() || id.is_i64() || id.is_u64() => id.clone(),
        Some(_) => {
            return fail(
                400,
                &null,
                INVALID_REQUEST,
                "id must be a string or an integer",
                None,
            )
        }
    };
    let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return fail(400, &id, INVALID_PARAMS, "params must be an object", None);
    }
    if let Some(h) = headers.method {
        if h != method {
            return fail(
                400,
                &id,
                HEADER_MISMATCH,
                "Mcp-Method doesn't match the body",
                None,
            );
        }
    }
    if method == "initialize" {
        let asked = params.get("protocolVersion").and_then(Value::as_str);
        let version = asked
            .filter(|v| LEGACY_VERSIONS.contains(v))
            .unwrap_or(LEGACY_VERSIONS[0]);
        return Reply::ok(json!({ "jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": version,
            "capabilities": capabilities(),
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        }}));
    }
    let era = match era_of(&id, &params, headers) {
        Ok(e) => e,
        Err(r) => return r,
    };
    let legacy = matches!(era, Era::Legacy);
    let result = match method {
        "server/discover" if !legacy => json!({
            "supportedVersions": [MODERN_VERSION],
            // No stream in this revision, so nothing to subscribe to or be told about.
            "capabilities": { "tools": {}, "resources": {} },
            "instructions": INSTRUCTIONS,
            "ttlMs": TTL_MS,
            "cacheScope": "private",
        }),
        "ping" if legacy => json!({}),
        // The instructions resource: the same for every caller, so no scope is needed.
        "resources/list" => json!({ "resources": [instructions_resource()] }),
        "resources/templates/list" => json!({ "resourceTemplates": [] }),
        "resources/read" => {
            let Some(uri) = params.get("uri").and_then(Value::as_str) else {
                return fail(
                    400,
                    &id,
                    INVALID_PARAMS,
                    "resources/read needs a string uri",
                    None,
                );
            };
            if uri != INSTRUCTIONS_URI {
                return fail(
                    if legacy { 200 } else { 404 },
                    &id,
                    RESOURCE_NOT_FOUND,
                    "Resource not found",
                    Some(json!({ "uri": uri })),
                );
            }
            json!({ "contents": [{ "uri": INSTRUCTIONS_URI, "mimeType": "text/markdown", "text": INSTRUCTIONS }] })
        }
        "tools/list" => {
            let tools: Vec<Value> = registry.list(caller).iter().map(|s| s.mcp()).collect();
            let mut r = json!({ "tools": tools });
            if !legacy {
                r["ttlMs"] = json!(TTL_MS);
                r["cacheScope"] = json!("private");
            }
            r
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return fail(
                    400,
                    &id,
                    INVALID_PARAMS,
                    "tools/call needs a string name",
                    None,
                );
            };
            if let Some(h) = headers.name {
                if h != name {
                    return fail(
                        400,
                        &id,
                        HEADER_MISMATCH,
                        "Mcp-Name doesn't match the body",
                        None,
                    );
                }
            }
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match registry.call(backend, caller, name, args).await {
                Ok(out) => {
                    let data = json!({
                        "untrusted": true,
                        "note": DATA_NOTE,
                        "replayed": out.replayed,
                        "data": out.value,
                    });
                    json!({ "content": [{ "type": "text", "text": data.to_string() }], "structuredContent": data, "isError": false })
                }
                Err(ToolError::UnknownTool) => {
                    return fail(
                        if legacy { 200 } else { 400 },
                        &id,
                        INVALID_PARAMS,
                        &format!("Unknown tool: {name}"),
                        None,
                    )
                }
                Err(ToolError::MissingScope(scope)) => {
                    let mut r = fail(
                        403,
                        &id,
                        INVALID_REQUEST,
                        &ToolError::MissingScope(scope).to_string(),
                        Some(json!({ "code": "insufficient_scope", "scope": scope.as_str() })),
                    );
                    r.insufficient_scope = Some(scope);
                    return r;
                }
                Err(e) => {
                    json!({ "content": [{ "type": "text", "text": e.to_value().to_string() }], "isError": true })
                }
            }
        }
        _ => {
            return fail(
                if legacy { 200 } else { 404 },
                &id,
                METHOD_NOT_FOUND,
                &format!("method not found: {method}"),
                None,
            )
        }
    };
    let result = if legacy {
        result
    } else {
        let mut r = result;
        r["resultType"] = json!("complete");
        r["_meta"] = json!({ META_SERVER_INFO: server_info() });
        r
    };
    Reply::ok(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn era_of(id: &Value, params: &Value, headers: Headers<'_>) -> Result<Era, Reply> {
    let meta = params.get("_meta").and_then(Value::as_object);
    if let Some(version) = meta.and_then(|m| m.get(META_VERSION)) {
        let Some(v) = version.as_str().filter(|v| *v == MODERN_VERSION) else {
            return Err(fail(
                400,
                id,
                UNSUPPORTED_PROTOCOL_VERSION,
                "Unsupported protocol version",
                Some(json!({ "supported": [MODERN_VERSION], "requested": version })),
            ));
        };
        if headers.protocol_version.is_some_and(|h| h != v) {
            return Err(fail(
                400,
                id,
                HEADER_MISMATCH,
                "MCP-Protocol-Version doesn't match _meta",
                None,
            ));
        }
        if !meta
            .and_then(|m| m.get(META_CAPABILITIES))
            .is_some_and(Value::is_object)
        {
            return Err(fail(
                400,
                id,
                INVALID_PARAMS,
                "missing required _meta[\"io.modelcontextprotocol/clientCapabilities\"]",
                None,
            ));
        }
        return Ok(Era::Modern);
    }
    match headers.protocol_version {
        Some(v) if LEGACY_VERSIONS.contains(&v) => Ok(Era::Legacy),
        Some(v) if v == MODERN_VERSION => Err(fail(
            400,
            id,
            INVALID_PARAMS,
            "missing required _meta[\"io.modelcontextprotocol/protocolVersion\"]",
            None,
        )),
        Some(v) => Err(fail(
            400,
            id,
            UNSUPPORTED_PROTOCOL_VERSION,
            "Unsupported protocol version",
            Some(
                json!({ "supported": [MODERN_VERSION, LEGACY_VERSIONS[0], LEGACY_VERSIONS[1], LEGACY_VERSIONS[2]], "requested": v }),
            ),
        )),
        // No header: a 2025-03-26 client (the spec lets servers assume that version).
        None => Ok(Era::Legacy),
    }
}
