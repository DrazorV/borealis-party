//! Borealis Party never calls an AI service itself (0.3.3, roadmap 4.5c): ChatGPT and Claude connect TO it over MCP
//! and the user's own plan pays. This test reads the fork's own code, `party/` and `ui/party/`, and fails on any model
//! vendor's API address or SDK, so the removed in-app assistant can't come back by accident.
//!
//! Prose is not code: `party/docs/` and READMEs may name vendors. The connection guides (how to add Borealis Party
//! to ChatGPT or Claude) are allowed to name the apps, and so are the allowed origins of those apps, which are the
//! apps' own sites (chatgpt.com, claude.ai), not their APIs.

use std::path::{Path, PathBuf};

/// Model vendors' API hosts, and their SDKs as a dependency or an import names them.
const NEEDLES: [&str; 13] = [
    "openai.com",
    "anthropic.com",
    "generativelanguage",
    "openrouter.ai",
    "api.mistral.ai",
    "api.groq.com",
    "@anthropic-ai/",
    "@google/genai",
    "@google/generative-ai",
    "async-openai",
    "async_openai",
    "anthropic-sdk",
    "\"openai\":",
];

/// Files that may name a vendor: the connection guides' text, and this test.
const ALLOWED: [&str; 2] = [
    "ui/party/pages/AiAppsGuides.tsx",
    "party/crates/borealis-server/src/no_vendor_calls_tests.rs",
];

const CODE: [&str; 13] = [
    "rs", "ts", "tsx", "js", "mjs", "cjs", "jsx", "toml", "json", "css", "html", "yml", "yaml",
];

const SKIP_DIRS: [&str; 10] = [
    "build",
    ".gradle",
    "target",
    "node_modules",
    "dist",
    "docs",
    "test-results",
    "playwright-report",
    ".git",
    "gen",
];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(&path, out);
            }
        } else if kind.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| CODE.contains(&e))
        {
            out.push(path);
        }
    }
}

#[test]
fn no_code_calls_a_model_vendor() {
    // party/crates/borealis-server → the repository root.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let root = root.canonicalize().unwrap();
    let mut files = Vec::new();
    for top in ["party", "ui/party"] {
        walk(&root.join(top), &mut files);
    }
    assert!(
        files.len() > 100,
        "the walk found only {} files under party/ and ui/party/",
        files.len()
    );
    let mut found = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWED.contains(&rel.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            for needle in NEEDLES {
                if lower.contains(needle) {
                    found.push(format!("{rel}:{}: {needle}", n + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "Borealis Party must not call an AI service itself (AI apps connect to it over MCP):\n{}",
        found.join("\n")
    );
}
