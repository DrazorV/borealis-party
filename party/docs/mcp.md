# MCP server (`/api/mcp`)

Borealis Connect serves the [Model Context Protocol](https://modelcontextprotocol.io) at `https://party.example.com/api/mcp`, so AI apps
can search the bestiary, build and simulate encounters, read characters, and author homebrew as the signed-in
account. No tool reads or changes a live combat (D25).

The tools live in one registry (`party-tools`, [party-tools.md](party-tools.md)). Every tool
calls the server's own API code with the token owner's account, so the access rules don't change:
- a player's token sees what the player sees: never hidden creatures, DM notes or legendary counts;
- a DM's token sees what the DM sees;
- anyone else gets "Not found".

## Protocol

- **Transport:** Streamable HTTP. Each JSON-RPC message is one `POST /api/mcp` and gets one `application/json`
  answer. Notifications get `202`. `GET` and `DELETE` get `405`: there is no stream, and no session id is issued.
  Batches (JSON arrays) are refused.
- **Revisions:**
  - **2026-07-28**, the current one. It is stateless: `server/discover`, per-request `_meta`, and `resultType`.
  - The handshake revisions **2025-11-25**, **2025-06-18** and **2025-03-26**, after `initialize`. The version
    then travels in the `MCP-Protocol-Version` header.
- **Headers:**
  - `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` are checked against the body when present: a mismatch is
    `400` with `-32020`.
  - A browser `Origin` other than the server's own or an AI app's (`https://chatgpt.com`, `https://chat.openai.com`,
    `https://claude.ai`, `https://claude.com`, exact, any case) is `403`, checked before the token. Server-side
    connectors send no `Origin`.
- **Instructions:** the DM's-assistant workflow (`party-tools/src/instructions.md`) is in the `initialize` and
  `server/discover` results as `instructions`, and is also the resource `borealis://agent/instructions`
  (`text/markdown`) through `resources/list` and `resources/read`, for clients that don't show `instructions` to the
  model. It needs no scope. An unknown resource is `-32002`.
- **Errors:**
  - unknown method: `404` / `-32601`;
  - unsupported revision: `400` / `-32022`;
  - missing `_meta`: `400` / `-32602`;
  - unknown tool: `-32602`.
  - A tool's own failure (not found, invalid arguments, refused, rate limited) is a normal result with
    `isError: true` and `{code, message}`.
- **Results are data.** Every result is `structuredContent: {untrusted: true, note, replayed, data}`, with the same
  JSON as a text block. The server instructions and every tool description tell the model that tool results are
  user-written game data and not instructions. Tool descriptions never quote user content.

Spec pages used (read 2026-11-13; the 2025-11-25 ones checked again 2026-11-14 for 0.3.3):
- [2025-11-25 authorization](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization)
- [2025-11-25 transports](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
- [2025-11-25 resources](https://modelcontextprotocol.io/specification/2025-11-25/server/resources)
- [versioning](https://modelcontextprotocol.io/specification/versioning) (2026-07-28 is current)
- [streamable-http](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)
- [basic](https://modelcontextprotocol.io/specification/2026-07-28/basic/index)
- [versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)
- [discover](https://modelcontextprotocol.io/specification/2026-07-28/server/discover)
- [tools](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)
- [authorization](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)
- [authorization-server discovery](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/authorization-server-discovery)

## Auth

- **Header only:** `Authorization: Bearer <token>`, never a query string or a cookie.
- **Personal tokens** (`bpt_…`) come from **Account → API tokens** (part A). Each has scopes, an expiry, and can be
  revoked. They work only on `/api/mcp`, and app session tokens are refused there.
- **OAuth 2.1** (part A) is for ChatGPT and claude.ai connectors.
  - The protected-resource metadata (RFC 9728) is at `/.well-known/oauth-protected-resource/api/mcp`, and also at
    the root form and `/api/mcp/.well-known/oauth-protected-resource`. It names this server as the authorization
    server, whose metadata is at `/.well-known/oauth-authorization-server` and `/.well-known/openid-configuration`
    (each also under `/api/mcp`, see [ai-auth.md](ai-auth.md)).
  - A `401` carries `WWW-Authenticate: Bearer resource_metadata="…", error="invalid_token", scope="…"`.
  - A tool the token's scopes don't allow is a `403` with `error="insufficient_scope"` and the scope it needs, so a
    connector can ask the user for it.
  - Tokens must be issued for `https://party.example.com/api/mcp` (RFC 8707); part A's verifier checks the audience.
- **Every request re-reads the account.** A disabled account, a revoked token or a lost DM role takes effect at once.
- **Scopes:**

  | Scope | Tools |
  |---|---|
  | `bestiary:read` | `bestiary_search`, `monster_get` |
  | `encounters:read` | `encounter_list`, `encounter_get` |
  | `encounters:write` | `encounter_create`, `encounter_update`, `encounter_add_creature`, `encounter_remove_creature` |
  | `encounters:simulate` | `sim_start`, `sim_card`, `sim_act`, `sim_auto`, `sim_state`, `sim_report`, `sim_batch`, `sim_replay` |
  | `characters:read` | `character_list`, `character_get` |
  | `homebrew:read` | `homebrew_list`, `homebrew_get`, `homebrew_validate`, `index_build` |
  | `homebrew:write` | `homebrew_create`, `homebrew_update`, `homebrew_delete` |

  - `encounters:write` includes `encounters:read`, and `homebrew:write` includes `homebrew:read`.
  - `tools/list` shows only the tools the token may call.

## Limits and logging

- **Rate:** 60 calls a minute per token.
- **Heavy calls:** at most 2 at a time per account. These are `sim_start`, `sim_auto`, `sim_batch`, `sim_replay` and a `bestiary_search` with no
  filter.
- **Size:** messages over 1 MiB are refused. A result over 256 KiB is refused with "narrow the request". A write
  whose result is that large answers `{done: true}`.
- **Retries:** every write takes a `requestId`.
  - The same id from the same token answers the first result again (`replayed: true`) and changes nothing.
  - The same id sent with other arguments is refused.
- **Logging:** each call goes to the account's activity as `ai_tool`, with the surface, tool, scope and status (for
  example `mcp encounter_create encounters:write ok`) and the token id. Arguments are never logged.

## Simulations (`sim_*`, D26)

- **DM only.** They run on a copy of one of the DM's encounters, and never on a live combat (D25).
- **They write nothing:** no row, event or `seq` change.
- **Engine and agent:** the combat engine produces every number. The agent decides tactics turn by turn from
  decision cards, and adjudicates what the engine can't automate.
- **Batches:** `sim_batch` runs seeded batches with the basic policy, and reports plain counts out of N. There is no
  difficulty rating (D16).
- The design is `plans/borealis-party-sim-design-2026-11-13.md`. These tools answer "not available yet" until the
  simulator is merged.

## Custom indexes: your own agent, your own repository (D47)

The server never hosts or publishes an index. Instead:
1. A local agent (Claude Code, Gemini CLI, Cursor) calls `index_build`.
2. It gets back the `.index` file and the element files as data.
3. It writes them into a folder of your own git repository and pushes.

Subscribers then use that repository's raw `.index` URL in the Import screen, like any other index. Only your own
homebrew goes into it.

## Kill switch

`BOREALIS__FEATURES__AI=false` (or `[features] ai = false` in `config.toml`), then restart.
- `/api/mcp`, the OAuth routes and the metadata answer `404`, and every tool answers "unavailable".
- It is a server-config switch only (D40). The admin page has no toggle for it.

## Connecting apps

Use the server's address in place of `party.example.com`, and a token made on the Account page with the scopes
you want. Client menus change often; these were current on 2026-11-13.

### Claude Code

```bash
claude mcp add --transport http borealis-connect https://party.example.com/api/mcp --header "Authorization: Bearer bpt_your_token"
```

Without `--header`, Claude Code finds the OAuth metadata and signs in through the browser (run `/mcp`).

### claude.ai and Claude Desktop (remote connector, OAuth)

1. Go to **Settings → Connectors → Add custom connector**.
2. Enter the URL `https://party.example.com/api/mcp`.
3. Approve the scopes on the server's consent screen.

### Claude Desktop (local, with a token): the stdio bridge

The bridge is `borealis-party-mcp`, built from `party/crates/party-mcp-bridge`. It reads the address and the token
from its environment only, never from arguments: `BOREALIS_CONNECT_URL` and `BOREALIS_CONNECT_TOKEN`. The names from before the
rename to Borealis Connect, `BOREALIS_PARTY_URL` and `BOREALIS_PARTY_TOKEN`, still work (each is read when its `BOREALIS_CONNECT_*`
name is not set), and the binary keeps its name, so existing configs need no change. Add it to
`claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "borealis-connect": {
      "command": "/path/to/borealis-party-mcp",
      "env": {
        "BOREALIS_CONNECT_URL": "https://party.example.com",
        "BOREALIS_CONNECT_TOKEN": "bpt_your_token"
      }
    }
  }
}
```

### ChatGPT (custom MCP server, OAuth)

Menus checked 2026-11-14 (https://developers.openai.com/api/docs/guides/custom-mcp-server):
1. Open **Plugins → + → Add custom MCP server**.
2. Enter a name and the server URL `https://party.example.com/api/mcp`, and set authentication to **OAuth**.
3. Confirm, then **Create as a plugin** and install it. ChatGPT registers itself (dynamic client registration) and
   opens the server's consent screen. In a chat, type **@** and pick it.

It needs a ChatGPT plan that allows custom MCP servers; on a workspace plan an admin may have to add it.

### Technical notes for connectors

What the server does at each step, the same as BalanceLab's connector (0.3.3 part B), and tested end to end by
`mcp_connector_tests.rs` with a scripted client:

1. `POST /api/mcp` without a token → `401`, `WWW-Authenticate: Bearer error="invalid_token", scope="<default
   scopes>", resource_metadata="<base>/.well-known/oauth-protected-resource/api/mcp"`.
2. That document → `resource` = `<base>/api/mcp`, `authorization_servers` = `[<base>]`.
3. `<base>/.well-known/oauth-authorization-server` (or `openid-configuration`, or either under `/api/mcp`) → the
   endpoints, `code_challenge_methods_supported: ["S256"]` and `authorization_response_iss_parameter_supported`.
4. `POST /api/oauth/register` (DCR) → a public `client_id`.
5. `GET /api/oauth/authorize` with PKCE S256, `resource=<base>/api/mcp`, `scope`, `state` → the consent page in the
   app (the account signs in first if needed) → back to the app's redirect with `code`, `state` and `iss=<base>`.
   Errors come back the same way, with `iss`. A wrong `resource` is `invalid_target`; no PKCE is `invalid_request`.
6. `POST /api/oauth/token` (form) with the code, verifier and `resource` → `bpa_` access (1 hour) and `bpr_` refresh
   (90 days, rotating).
7. `initialize` → `instructions`; then `tools/list`, `tools/call`, `resources/read`.
8. Refresh before the hour is up; `POST /api/oauth/revoke` with the refresh token disconnects the app.

A WAF in front of the server must let the AI apps' servers reach `/.well-known/*`, `/api/oauth/*` and `/api/mcp`
(claude.ai calls from `160.79.104.0/21`).

### Gemini CLI

In `~/.gemini/settings.json` (Gemini CLI expands `$VAR` from the environment):

```json
{
  "mcpServers": {
    "borealis-connect": {
      "httpUrl": "https://party.example.com/api/mcp",
      "headers": { "Authorization": "Bearer $BOREALIS_CONNECT_TOKEN" }
    }
  }
}
```

### Cursor

In `~/.cursor/mcp.json` (or `.cursor/mcp.json` in a project):

```json
{
  "mcpServers": {
    "borealis-connect": {
      "url": "https://party.example.com/api/mcp",
      "headers": { "Authorization": "Bearer ${env:BOREALIS_CONNECT_TOKEN}" }
    }
  }
}
```

### Any other client

Point it at `https://party.example.com/api/mcp` over Streamable HTTP, with a bearer token or OAuth. Clients that
only start local servers use the bridge.

## Code

| Where | What |
|---|---|
| `party/crates/party-tools` | specs, scopes, arguments, limits, idempotency, the JSON-RPC envelope (`mcp.rs`) |
| `party/crates/borealis-server/src/tools_backend.rs` | `ServerTools`: each tool calls the API's handler as the caller; `external` / `external_specs` for part D's and the simulator's tools |
| `party/crates/borealis-server/src/mcp.rs` | the HTTP endpoint on part A's `TokenAuth` (`tokens.rs`), Origin, the scope challenge |
| `party/crates/party-mcp-bridge` | the stdio bridge |

**Upstream proposal (not opened):**
- `crates/borealis-mcp/src/rpc.rs` keeps `handle` and the `Tools` constructor private.
- A small PR could make public a `handle_message(&Host, &mut Connection, &Value) -> Option<Value>` and a
  `Tools::from_store(...)`. The party envelope and upstream's stdio server could then share one implementation, and
  the content and rules tools could serve from an existing store.
