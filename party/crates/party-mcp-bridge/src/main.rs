//! `borealis-party-mcp`: relays MCP over stdio (one JSON-RPC message per line) to a Borealis Party server's
//! Streamable HTTP endpoint, for clients that only start local MCP servers (Claude Desktop, older clients).
//!
//! Configuration, from the environment only (never a command-line argument, which other users can list):
//! * `BOREALIS_PARTY_URL`: the server, e.g. `https://party.example.com` (`/api/mcp` is added);
//! * `BOREALIS_PARTY_TOKEN`: a personal `bpt_` token from the server's Account page.
//!
//! It keeps no state but the protocol version an `initialize` agreed on, which it sends as `MCP-Protocol-Version`.
//! Everything else is the server's: the bridge adds nothing to and removes nothing from the messages.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{json, Value};

const MAX_LINE: usize = 1024 * 1024;

fn error_line(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

fn main() {
    let url = match std::env::var("BOREALIS_PARTY_URL") {
        Ok(u)
            if u.starts_with("https://")
                || u.starts_with("http://localhost")
                || u.starts_with("http://127.0.0.1") =>
        {
            format!(
                "{}/api/mcp",
                u.trim_end_matches('/').trim_end_matches("/api/mcp")
            )
        }
        _ => {
            eprintln!(
                "borealis-party-mcp: set BOREALIS_PARTY_URL to the server's https:// address"
            );
            std::process::exit(2);
        }
    };
    let Ok(token) = std::env::var("BOREALIS_PARTY_TOKEN") else {
        eprintln!("borealis-party-mcp: set BOREALIS_PARTY_TOKEN to a token from the server's Account page");
        std::process::exit(2);
    };
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("borealis-party-mcp/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("borealis-party-mcp: {e}");
            std::process::exit(1);
        }
    };
    let mut version: Option<String> = None;
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > MAX_LINE {
            let _ = writeln!(
                out,
                "{}",
                error_line(&Value::Null, -32600, "message too large")
            );
            let _ = out.flush();
            continue;
        }
        let msg: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let modern = msg
            .pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut req = client
            .post(&url)
            .bearer_auth(&token)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(line.clone());
        if let Some(v) = modern.as_ref().or(version.as_ref()) {
            req = req.header("mcp-protocol-version", v);
        }
        if !method.is_empty() {
            req = req.header("mcp-method", &method);
        }
        if let Some(name) = msg.pointer("/params/name").and_then(Value::as_str) {
            if name.is_ascii() {
                req = req.header("mcp-name", name);
            }
        }
        let reply = match req.send() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("borealis-party-mcp: {e}");
                if !id.is_null() {
                    let _ = writeln!(
                        out,
                        "{}",
                        error_line(&id, -32603, "the Borealis Party server can't be reached")
                    );
                    let _ = out.flush();
                }
                continue;
            }
        };
        let status = reply.status();
        let body = reply.text().unwrap_or_default();
        if status.as_u16() == 202 || id.is_null() && body.trim().is_empty() {
            continue;
        }
        let parsed: Option<Value> = serde_json::from_str(&body).ok();
        let line_out = match parsed {
            Some(v) => {
                if method == "initialize" {
                    if let Some(p) = v.pointer("/result/protocolVersion").and_then(Value::as_str) {
                        version = Some(p.to_string());
                    }
                }
                v.to_string()
            }
            None if status.as_u16() == 401 => {
                error_line(&id, -32001, "the token was refused (expired or revoked?)")
            }
            None => error_line(&id, -32603, &format!("the server answered {status}")),
        };
        let _ = writeln!(out, "{line_out}");
        let _ = out.flush();
    }
}
