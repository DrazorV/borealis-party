//! The small profile against a real local model (design: "it must finish a run with structured output").
//!
//! Ignored by default. Run with an Ollama server (plain HTTP, `format` = the per-card JSON schema, temperature 0):
//!
//! ```text
//! SIM_OLLAMA=http://127.0.0.1:11434 SIM_MODEL=qwen2.5:7b-instruct cargo test -p party-sim --test live_model -- --ignored --nocapture
//! ```
//!
//! It plays the foes of the Ogre fixture with the model and Basic plays the party, then prints the steering and
//! the assistant's counts. No extra dependency: a minimal HTTP/1.1 client over a TCP stream (loopback or LAN).

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use party_sim::oracle::{run_assistant, AssistantStats, Profile};
use party_sim::{report, Sim};
use serde_json::{json, Value};

fn post(base: &str, path: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
    let rest = base
        .strip_prefix("http://")
        .ok_or("SIM_OLLAMA must be http://host:port")?;
    let host = rest.trim_end_matches('/');
    let mut s = TcpStream::connect(host).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let text = body.to_string();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| format!("read: {e}"))?;
    let raw = String::from_utf8_lossy(&buf);
    let (head, body) = raw.split_once("\r\n\r\n").ok_or("no HTTP body")?;
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        // De-chunk.
        let mut out = String::new();
        let mut rest = body;
        while let Some((size, tail)) = rest.split_once("\r\n") {
            let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            if n == 0 || tail.len() < n {
                break;
            }
            out.push_str(&tail[..n]);
            rest = tail[n..].trim_start_matches("\r\n");
        }
        out
    } else {
        body.to_string()
    };
    serde_json::from_str(&body).map_err(|e| format!("json: {e}"))
}

#[test]
#[ignore = "needs a local Ollama server: SIM_OLLAMA and SIM_MODEL"]
fn the_small_profile_finishes_a_run_with_a_real_model() {
    let base = std::env::var("SIM_OLLAMA").expect("SIM_OLLAMA");
    let model = std::env::var("SIM_MODEL").expect("SIM_MODEL");
    let profile = Profile::for_model(&model, true);
    assert_eq!(
        profile,
        Profile::Small,
        "local models get the small profile"
    );
    let mut setup = common::ogre_setup(17, "foes");
    setup.opts.adjudicate = false;
    setup.opts.max_rounds = 6;
    let mut sim = Sim::new(&setup).unwrap();
    let mut stats = AssistantStats::default();
    let mut slowest = Duration::ZERO;
    let mut ask = |system: &str, user: &str, schema: &Value| -> Result<Value, String> {
        let t = Instant::now();
        let body = json!({
            "model": model,
            "stream": false,
            "format": schema,
            "options": { "temperature": 0 },
            "messages": [ { "role": "system", "content": system }, { "role": "user", "content": user } ],
        });
        let out = post(&base, "/api/chat", &body, Duration::from_secs(60))?;
        slowest = slowest.max(t.elapsed());
        let content = out["message"]["content"]
            .as_str()
            .ok_or_else(|| format!("no content: {out}"))?;
        serde_json::from_str(content).map_err(|e| format!("not JSON ({e}): {content}"))
    };
    let t = Instant::now();
    let outcome = run_assistant(&mut sim, profile, &mut ask, &mut stats);
    let r = report(&sim);
    println!(
        "model {model}: outcome {outcome:?} in {} rounds, {:?}",
        r.rounds,
        t.elapsed()
    );
    println!("assistant: {stats:?}; slowest answer {slowest:?}");
    println!("steering: {}", serde_json::to_string(&r.steering).unwrap());
    for d in r
        .decisions
        .iter()
        .filter(|d| d.by == party_sim::By::Assistant)
        .take(12)
    {
        println!("  R{} {} -> {} {:?}", d.round, d.actor, d.option, d.targets);
    }
    assert!(r.steering.assistant > 0, "the model's answers were used");
    assert!(sim.outcome.is_some(), "the run finished");
}
