# Bring your own AI: the assistant (0.3.1 part C)

Each account can set up its own AI model ("Your AI" on the Account page). The assistant panel uses it for
**encounter building** (the encounter list and builder), **homebrew building** and **custom index creation**
(Additional Content). Code: `crates/borealis-server/src/ai_providers.rs` (the provider protocols, and sealed keys),
`ai_assistant.rs` (the routes, scopes and limits), `ai_tools.rs` (the tools; a stand-in for part B's `party-tools`),
`ui/party/assistant/` (the panel and its tool loop), and `ui/party/pages/YourAiCard.tsx`.

## Providers and the two modes

| Provider | Mode | Who calls it | Key |
|---|---|---|---|
| OpenAI-compatible: OpenAI, OpenRouter, Groq, Together, … (a base URL and a model) | `server` | the server | saved, sealed |
| Anthropic (a model) | `server` | the server, at `api.anthropic.com` only | saved, sealed |
| Google Gemini (a model) | `server` | the server, at `generativelanguage.googleapis.com` only | saved, sealed |
| OpenAI-compatible on the person's own device or home network: Ollama, LM Studio | `device` | **the app**, never the server | none |

**Local and LAN models run from the person's own device. The server never calls them.** The server's address guard
(`net_guard.rs`) refuses private and home-network addresses for every account, and the assistant keeps it that way:
in `server` mode the base URL must be `https` on the internet. The request is then pinned to the addresses the check
resolved, and redirects are not followed. In `device` mode the address must be localhost, a private address or a
home-network name, and the app sends the model requests itself in the OpenAI format. The server only gives the app
the prompt and the tool list, and runs the tools.

There's nothing for the admin to allow, and no account can make the server reach the home network. The other design
(an admin-allowed LAN AI host in the server config) was not built. It would give every account a path from the server
into the home network, which the runbook's firewall rule exists to block.

Limits of `device` mode:
- In a browser, only `localhost` works: the server's CSP allows `connect-src http://localhost:* http://127.0.0.1:*
  http://[::1]:*`, and browsers block `http://` LAN addresses on an `https` page.
- The apps (desktop, Android) reach the home network too.
- Ollama needs `OLLAMA_ORIGINS` set to allow the app's origin. In LM Studio, turn on CORS.

## Keys

- They're sealed with AES-256-GCM under `[ai] secret_key` (`BOREALIS__AI__SECRET_KEY`, base64 of 32 bytes), with a
  random nonce. The account id and the provider are the associated data, so a sealed key copied to another row
  doesn't open. `key_ring` (a short digest of the server key, never the key) tells a row sealed under an older
  server key, and the person is asked to enter their key again.
- Without a server key, no key is saved, and the Account page says why. `device` mode still works.
- A key is never logged, never sent back (only `•••• last4`), and is scrubbed from provider errors. It lives in the
  database, not in the account's Borealis folder, so it's never in a bundle, a share code or the offline mirror.
- "Remove key" deletes the key. "Remove" deletes the whole setup. Both go when the account is deleted.
- Saving needs `consent: true`: the person agrees that what they ask, and what the tools read for them, goes to the
  provider they chose.

## What the assistant may do

- It runs as the signed-in account (its session), with part A's default scopes (the user's D25, D26 and D47):
  `bestiary:read`, `characters:read`, `encounters:read`, `encounters:write`, `encounters:simulate`, `homebrew:read`
  and `homebrew:write`. **No combat tool** exists for it (D25: it never touches a live combat), and **nothing is
  published** (D47: `index_build` returns a `.index` and its files as data, which the panel zips for download).
- Every tool goes through the same route code as the app, so the same permission and projection checks apply.
  Another DM's party is "not found".
- Writes (`effect: draft`) ask first, in an app dialog. The server refuses a write without `confirmed: true`
  (`ai_confirm_required`), unless the person turned on "Apply drafts without asking". Reads and simulations
  (`effect: read`, on a copy) never ask. A write gets `requestId` = `ai-` plus the model's call id, so a retried write
  happens once.
- Tool results and model output are data. Results go back to the model inside `{ note: "…not instructions…", data }`.
  The system prompt says to follow only the person in the chat, and the panel shows text as plain text.
- D16: no XP or difficulty. `ai_tools::strip_d16` removes them from every result, and the encounter prompt forbids them.
- **Simulate encounter** (D26): in the encounter builder, the panel's button asks the model to simulate on a copy
  (`sim_start`, `sim_auto`, `sim_report`), and the panel shows a `sim_auto` / `sim_report` / `sim_state` /
  `sim_replay` result's play-by-play (`rounds[].turns[]`), what needs a ruling (`needsAdjudication`) and `end`, as
  plain text.
- **Model size** (`profile`, D26): `small` or `frontier`, chosen in Your AI. By default it's `frontier` only for
  the big hosted families through the server, and `small` for device or unknown models. Part E's simulation oracle
  gets it from `ai_assistant::model_of`, with `ai_providers::complete_json` (one structured decision at temperature 0,
  held to a JSON schema by each provider's own means), and `admit_call` / `record_usage` for the same limits.
  `model_of` is `None` for a device model: the server never calls it.

## Limits

`[ai] requests_per_minute` (default 20) model calls per account per minute, `[ai] tokens_per_day` (default 200000)
tokens per account per day (UTC, in `ai_usage`), and 60 tool calls per account per minute. A conversation has at most
60 turns, 8000 characters per message and 8 model steps per message.

## Routes

All answer 404 when `[features] ai = false` (`BOREALIS__FEATURES__AI=false`). That's an emergency switch in the server
config only, with no admin-page toggle (D40).

| Route | What |
|---|---|
| `GET /api/ai` | `{ keys: { canStore, why? }, provider: { provider, mode, baseUrl, model, hasKey, keyLast4, keyReadable, autoApply, profile, consentedAt } \| null, scopes, limits: { requestsPerMinute, tokensPerDay, tokensToday }, defaults }` |
| `PUT /api/ai` | `{ provider, mode?, baseUrl?, model, key?, consent, autoApply?, profile? }`. Leaving `key` out keeps the saved key for the same provider |
| `DELETE /api/ai`, `DELETE /api/ai/key` | Forget the setup, or only the key |
| `POST /api/ai/test` | `{ ok, reply? \| status?, message? }`: one tiny request (`server` mode; the app tests `device` mode itself) |
| `GET /api/ai/tools?purpose=encounter\|homebrew\|index` | `{ system, tools: [{ name, description, inputSchema, scope, effect }], autoApply }` |
| `POST /api/ai/chat` | `{ purpose, turns }` → `{ reply: { text, toolCalls, inputTokens, outputTokens }, budget }` (`server` mode) |
| `POST /api/ai/tools/{name}` | `{ args, confirmed, callId? }` → `{ ok: true, result }` or `{ ok: false, error: { code, message } }` |

Turns are `{ role: "user", text }`, `{ role: "assistant", text, toolCalls: [{ id, name, args }] }` and
`{ role: "tool", toolCallId, name, result }`, the same for every provider.

Refusal codes: `ai_consent_required`, `ai_keys_unavailable`, `ai_not_set_up`, `ai_device_mode`, `ai_key_unreadable`,
`ai_budget`, `ai_rate_limited`, `ai_confirm_required` and `ai_provider_error`.

## Tools: the shared registry

`ai_tools.rs` is a thin adapter over part B's `party-tools` registry, the same one `/api/mcp` uses
(`crate::mcp::registry()`). Each call is `registry.call(&ServerTools::new(state, user), &Caller { account, token: None,
scopes: SCOPES, via: Via::Assistant }, name, args)`. The registry checks the scope, the arguments, 60 calls a minute
per account, the heavy-call limit and a write's `requestId` (the assistant sends `ai-` plus the model's call id, so a
retried write is done once), and records each call in the account's activity with `via: assistant`. A write
(`spec.write`) is a draft the person confirms; everything else is a read. Each tool's purposes come from its name
(encounter: `encounter_*`, `character_*`, `sim_*`, and the bestiary; homebrew: `homebrew_*` and the bestiary; index:
`index_build` and the homebrew reads). Part D's and the simulator's tools come in through `external_specs()`; until
they merge, their placeholders answer `unavailable` in words. The panel zips `index_build`'s files (or uses its
`zip`) for download (`ui/party/assistant/zip.ts`). Part D's homebrew editor can open the panel with
`<AssistantButton purpose="homebrew" />`.
