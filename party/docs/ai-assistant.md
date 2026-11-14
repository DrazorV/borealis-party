# The in-app assistant (removed in 0.3.3)

0.3.1 and 0.3.2 had a "bring your own AI" assistant: each account saved its own model and key under Account, Your AI;
an Assistant panel on the encounter, homebrew and Additional Content pages used it; and 0.3.2 added Connect OpenRouter.
The server called those models itself.

**Since 0.3.3, Borealis Party never calls an AI service.** AI works one way only: ChatGPT, Claude or a developer tool
connects TO Borealis Party over MCP, with our own OAuth or an API token, and the person's own plan pays. See
[ai-auth.md](ai-auth.md) (tokens and OAuth) and [mcp.md](mcp.md) (the endpoint and its tools). Account, AI apps shows
how to connect ChatGPT and Claude first, then the instructions connected apps get, then keys for developer tools.

A test keeps it that way: `no_vendor_calls_tests.rs` fails when code under `party/` or `ui/party/` names a model
vendor's API host or SDK (`openai.com`, `anthropic.com`, `generativelanguage`, `openrouter.ai`, …). Only the
connection guides' text may name the apps.

## What was removed

- UI: `ui/party/assistant/*` (the panel, its tool loop and zip), `ui/party/pages/YourAiCard.tsx`,
  `ui/party/api/overrides/assistant.ts`, and every Assistant button (encounter list and header, homebrew editor,
  Additional Content).
- Server: `ai_assistant.rs` (the `/api/ai*` routes, limits, usage), `ai_providers.rs` (adapters, key sealing),
  `ai_oauth.rs` (Connect OpenRouter), `ai_tools.rs`, `simulate_ai.rs` (the account model as the simulation's
  oracle), and the CSP's `connect-src` for models on the person's own device. `/api/ai*` now answers 404 like any
  unknown route.
- Rig specs `assistant.spec.ts` and `ai-signin.spec.ts`.

## The tables stay, unused

Migration 8 (`M-ai-providers`, MARKER `(8, "ai_providers")`) made `ai_providers` (one provider per account, its key
sealed with the server's old `[ai] secret_key`) and `ai_usage` (calls and tokens per account per day). **They are unused
since 0.3.3** and are not dropped: the database is production, and no migration is destructive. Nothing reads them, so
the sealed keys stay unreadable without the old secret. The migration, its MARKER, `migrate.rs`'s table and column
lists and the copy tests keep them, so a 0.3.0 to 0.3.2 database upgrades as before and the SQLite → PostgreSQL copy
still moves them. They go with an account when it is deleted (`ON DELETE CASCADE`).

## Settings

- `BOREALIS__AI__SECRET_KEY`, `BOREALIS__AI__REQUESTS_PER_MINUTE`, `BOREALIS__AI__TOKENS_PER_DAY` and an `[ai]` section
  in `config.toml` are accepted and ignored (values of any type); the server logs once at start, at info, that they
  are no longer used, and never fails to start because of them. They can be removed from the stack's environment.
- `BOREALIS__FEATURES__AI=false` still switches AI apps off: `/api/mcp`, API tokens and the OAuth server answer 404 and
  no token is accepted.
