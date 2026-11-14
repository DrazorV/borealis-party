# Authentication for AI apps (0.3.1, part A)

How AI apps and MCP clients sign in to a Borealis Party server, and the Rust API the MCP server (part B) and later
HTTP APIs use to check them. Code: `party/crates/borealis-server/src/tokens.rs` and `oauth.rs`. Schema: migration
step 7 (`M-ai-auth`, marker table `api_tokens`).

## Two ways in

| Client | How it signs in |
| --- | --- |
| Claude Code, Codex CLI, Gemini CLI, Cursor, Claude Desktop (through the stdio bridge) | A **personal API token** `bpt_…` from Account → AI apps & API tokens, sent as `Authorization: Bearer bpt_…`. Clients that do OAuth (Claude Code, Cursor) can use OAuth instead. |
| ChatGPT (developer-mode connectors), claude.ai custom connectors | **OAuth 2.1**: the app registers itself, the account approves it on a consent screen, and the app gets short-lived access tokens `bpa_…` plus rotating refresh tokens `bpr_…`. |

Both kinds work **only** where a route takes `TokenAuth` (the MCP endpoint `/api/mcp`). They are not sessions, so every
session route (`/api/rpc`, `/api/events`, party routes, uploads, files, admin) refuses them with 401. Session tokens are
refused by `TokenAuth`.

## Rust API (`crate::tokens`)

```rust
use crate::tokens::{Scope, Scopes, TokenAuth, TokenClient, TokenRejection};

pub struct TokenAuth {
    pub account: User,       // the active account, read from the DB on this request
    pub scopes: Scopes,      // what the token may be used for
    pub client: TokenClient, // which token or app
}

pub enum TokenClient {
    Personal { token_id: String, name: String },
    OAuth { grant_id: String, client_id: String, client_name: String },
}

impl TokenAuth {
    pub fn has(&self, scope: Scope) -> bool;
    pub fn require(&self, scope: Scope) -> Result<(), TokenRejection>; // 403 insufficient_scope; write includes read
}

impl TokenAuth { pub fn scope_names(&self) -> Vec<&'static str> } // ["bestiary:read", …]
impl TokenClient {
    pub fn id(&self) -> &str;      // token id or grant id: a stable key for your own limits; never a secret
    pub fn label(&self) -> String; // "api token <id>" / "app <grant>": for logs and activity
}

// TokenRejection: IntoResponse. 401 invalid_token / 403 insufficient_scope carry
//   WWW-Authenticate: Bearer error="…", scope="…", resource_metadata="<public_url>/.well-known/oauth-protected-resource/api/mcp"
// (MCP clients start OAuth from that header); 404 when BOREALIS__FEATURES__AI=false; 429 over the rate limit.

pub fn mcp_resource(state: &AppState) -> Option<String>;          // "<public_url>/api/mcp", the tokens' audience
pub fn resource_metadata_url(state: &AppState) -> Option<String>; // for your own 401s
```

`TokenAuth` is an axum extractor (`FromRequestParts`, rejection `TokenRejection`):

```rust
async fn mcp(State(state): State<AppState>, auth: TokenAuth, body: Bytes) -> Response {
    // Pick the scope from the tool, then call the SAME handler or permission code the session routes use,
    // with auth.account as the User. That is what keeps a token from doing more than its account.
    if let Err(refusal) = auth.require(Scope::EncountersRead) { return refusal.into_response(); }
    crate::encounters::read(State(state), auth.account.clone(), Path(id)).await.into_response()
}
```

What the extractor does on every call:

1. `BOREALIS__FEATURES__AI=false` → 404.
2. `Authorization: Bearer` must start `bpt_` (personal) or `bpa_` (OAuth access). Anything else → 401.
3. Looks up the peppered hash; the token must be unexpired and its account `active` (pending, disabled and rejected
   accounts are refused at once). An OAuth token's audience must be this server's `/api/mcp` (RFC 8707).
4. An account that must change its password → 403 `password_change_required`.
5. Rate limit: **60 calls a minute per personal token or per connected app** (429). Add your own per-tool budgets on
   top (simulation rounds, wall clock).
6. `last_used_at` is written at most once a minute.

It does **not** check party roles: those are per resource, so the party code's own checks (DM of P, member of P,
owner) apply when you call it with `auth.account`, and the player projections hide what players can't see. Losing DM
status, or leaving a party, takes effect on the next call. Outsiders still get the generic 404.

## Scopes and the role checks they sit on

A scope is necessary, never sufficient. A write scope includes its read scope (`Scopes::has`). Every scope is in
the default set for new tokens and on the consent screen; the account can untick any. There is **no scope for live
combat** (D25: AI apps get no access to active combats in 0.3.1, neither read nor actions) and **none for
publishing** (D47: the app never hosts or publishes indexes).

| Scope | Allows (if the account itself may) | Role check that still applies |
| --- | --- | --- |
| `bestiary:read` | Search and read the bestiary | The account's own bestiary only (`bestiary::search`/`monster` with `account.id`) |
| `encounters:read` | Read encounters | `encounters::access`/`party_access`: DM view as the party's DM, player projection as a member (hidden creatures, DM labels and notes never shown), 404 otherwise |
| `encounters:write` (includes read) | Create and edit drafts | `run_by`: DM of that party only |
| `encounters:simulate` | Dry-run simulations (D26): on copies, never writing | DM of that party only |
| `homebrew:read` | Read own homebrew | Own workspace only |
| `homebrew:write` (includes read) | Write own homebrew | Own workspace only |
| `characters:read` | Own characters; rosters the account may see | Owner, or `parties::viewed` for rosters |

Tool results are data, not instructions: wrap user content as marked data (part B).

## Personal API tokens

- Format `bpt_` + 32 random bytes in base64url (47 characters). Shown **once**, in the create answer.
- Stored as HMAC-SHA256 under the server pepper; the list shows a hint (`bpt_abcd…`), name, scopes, created, expires
  and last used.
- Expiry 1–365 days, default 90, not sliding. At most 25 live tokens per account; 20 creations an hour.
- Routes (session auth, the Account page): `GET /api/me/ai` (scope catalog, MCP URL), `GET/POST /api/me/ai/tokens`,
  `DELETE /api/me/ai/tokens/{id}` (revokes at once).

## OAuth 2.1 server

Issuer: `server.public_url` exactly (no trailing slash). Without `public_url` the OAuth routes answer 404 and only
personal tokens work.

| Endpoint | |
| --- | --- |
| `GET /.well-known/oauth-authorization-server` | RFC 8414 metadata. The same document is at `/.well-known/openid-configuration` (OIDC discovery form; no ID tokens, no `openid` scope), and at `/.well-known/oauth-authorization-server/api/mcp`, `/.well-known/openid-configuration/api/mcp`, `/api/mcp/.well-known/oauth-authorization-server` and `/api/mcp/.well-known/openid-configuration`, every place an MCP client may look. |
| `GET /.well-known/oauth-protected-resource/api/mcp` (and at the root, and `/api/mcp/.well-known/oauth-protected-resource`) | RFC 9728: `resource` = `<issuer>/api/mcp`, `authorization_servers` = `[<issuer>]` |
| `POST /api/oauth/register` | Dynamic client registration (RFC 7591), JSON. Auth methods `none`, `client_secret_post`, `client_secret_basic` (RFC default). 20 per client address per hour. Clients never used are removed after 30 days. |
| `GET /api/oauth/authorize` | Code flow. **PKCE S256 required.** Exact redirect match (loopback http may change port, RFC 8252). `resource` must be `<issuer>/api/mcp` or absent. Unknown scopes → `invalid_scope`; `offline_access` is ignored. Repeated parameters are refused. Then 303 to the app's consent screen `#/oauth/consent/<id>` (10 minutes). |
| `GET /api/oauth/requests/{id}`, `POST …/approve {scopes}`, `POST …/deny` | The consent screen (session auth in a header, so no other site can approve). The account may untick scopes. The answer is the redirect with `code`, `state` and `iss` (RFC 9207), or `error=access_denied`. |
| `POST /api/oauth/token` | Form-encoded. `authorization_code` (code once, 5 minutes, verifier checked, redirect and resource must match) or `refresh_token` (rotates; may narrow `scope`). Access 1 hour, refresh 90 days (30 days before 0.3.3; refresh tokens issued then keep their 30 days, and the next refresh gets 90). 60 per client address per minute. |
| `POST /api/oauth/revoke` | RFC 7009. A refresh token ends the whole grant; an access token only itself. Always 200. |
| `GET /api/me/ai/apps`, `DELETE /api/me/ai/apps/{grant}` | Connected apps on the Account page; disconnect revokes every token of the grant at once. |

Theft detection: a second use of a code or a refresh token deletes the grant, so every token issued from it stops
working.

Redirect URIs: https anywhere; http only on `127.0.0.1`, `localhost` or `[::1]`; an app's own scheme
(`cursor://…`) is allowed, while `javascript:`, `data:`, `file:` and similar never are. No fragments or credentials.

### What the big clients need (checked 2026-11-13)

- **claude.ai** (web, desktop, mobile): DCR, PKCE S256, `code_challenge_methods_supported` in the metadata, RFC 9728
  `resource` equal to the MCP URL exactly, uses the first `authorization_servers` entry, form-encoded token endpoint,
  `invalid_grant` for dead refresh tokens, rotation for public clients. Callback
  `https://claude.ai/api/mcp/auth_callback` (may move to `https://claude.com/api/mcp/auth_callback`). Requests come
  from `160.79.104.0/21`, so a WAF in front of the server must let that range reach `/.well-known/*` and
  `/api/oauth/*`. Claude Code uses a loopback redirect on any port.
  Docs: https://claude.com/docs/connectors/building/authentication, https://support.claude.com/en/articles/11503834
- **ChatGPT** (developer mode): DCR or CIMD, PKCE S256 (refuses without `S256` in the metadata), sends `resource` on
  authorize and token, `WWW-Authenticate` with `resource_metadata` on 401. With `iss` in every authorization answer
  and `authorization_response_iss_parameter_supported: true` (we do both) it uses the stable redirect
  `https://chatgpt.com/connector_platform_oauth_redirect`; otherwise `https://chatgpt.com/connector/oauth/{id}`.
  Both are just registered redirect URIs here.
  Docs: https://developers.openai.com/apps-sdk/build/auth
- **MCP authorization spec 2025-11-25**: https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization

### Parity with BalanceLab (0.3.3 part B, 2026-11-14)

The user's other app, BalanceLab, has the connector that works smoothly with ChatGPT and Claude. Audit of this server
against it and the MCP spec (2025-11-25 authorization and transports, read 2026-11-14):

| BalanceLab point | Before 0.3.3 | Now |
| --- | --- | --- |
| Streamable HTTP, one JSON reply per POST, no SSE or session, GET/DELETE 405 | had it | same |
| Revisions 2025-11-25, 2025-06-18, 2025-03-26 | had them (plus 2026-07-28) | same |
| DCR | had it (`/api/oauth/register`) | same |
| Authorize: S256 required, RFC 8707 `resource` | had them | tested end to end |
| Token: code + refresh grants, access 1 h, refresh 90 d | refresh 30 d | **refresh 90 d** |
| RFC 9207 `iss` on every authorization answer | had it (also on errors) | tested, plus a two-issuer mix-up test |
| Protected-resource metadata, root and under the MCP path | had both | **also `/api/mcp/.well-known/…`** |
| AS metadata, also under the MCP path, plus `openid-configuration` | root only | **all five more forms** |
| 401 `WWW-Authenticate: Bearer resource_metadata, scope` | had it | tested |
| Consent page after sign-in | had it | same |
| Instructions in `initialize` and as an MCP resource | `initialize` only | **`borealis://agent/instructions` resource** |
| Personal keys for developer tools | had `bpt_` | same |
| Browser origins chatgpt.com, chat.openai.com, claude.ai, claude.com | own origin only | **allowed** |

Not done in 0.3.1: Client ID Metadata Documents (`client_id_metadata_document_supported: false`). Both big clients
fall back to DCR. Adding CIMD needs an SSRF-guarded fetch (`net_guard.rs`).

## Storage and secrets

- Every token, code, client secret and consent id is stored as hex HMAC-SHA256 under a 32-byte pepper in
  `<config dir>/ai-token-pepper` (made on first start, mode 0600). A database dump alone holds nothing usable. Losing
  the file signs every AI app out, but nothing else. Back it up with `config.toml`.
- Lookups are by hash on a unique index; client secrets and PKCE are compared in constant time.
- Nothing secret is logged. Activity entries (`api_token_created`, `api_token_revoked`, `ai_app_approved`,
  `ai_app_disconnected`) hold scope names only.
- Deleting an account removes its tokens and grants (foreign keys cascade). A disabled account's tokens stop working
  at once and work again if it is re-enabled before they expire.
- **Revoke everything (0.3.3):** `tokens::revoke_all` deletes the account's personal tokens, pending codes and OAuth
  grants (their access and refresh tokens cascade) in one transaction. It runs on:
  - a password change (`POST /api/me/password`; the session that changed it stays);
  - **Sign out everywhere** on the Account page (`POST /api/me/sign-out-everywhere`, answers
    `{ sessions, apiTokens, apps }`; this session stays, every other one ends);
  - the admin's password reset and **Sign out everywhere** (`/api/admin/accounts/{id}/password` and `/sign-out`,
    every session ends too).

  Connected apps get a 401 with the usual challenge on their next call and go through OAuth again; personal tokens
  have to be made again. Activity entries hold counts only.

## Emergency switch (D40)

`BOREALIS__FEATURES__AI=false` (or `[features] ai = false` in `config.toml`) removes every route above and makes
`TokenAuth` answer 404. There is no admin-page switch. Tokens are kept and work again when it is turned back on.
