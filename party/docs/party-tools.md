# Party tools (`party/crates/party-tools`)

Borealis Connect has ONE tool registry for every AI client of the server: the MCP server at `/api/mcp`
(`borealis-server/src/mcp.rs`, [mcp.md](mcp.md)). ChatGPT, Claude and developer tools connect to it; the server
never calls a model vendor itself (the in-app assistant was removed in 0.3.3).

A tool has a name, a JSON schema, one required scope and a handler. The built-in handlers
(`borealis-server/src/tools_backend.rs`) call the HTTP API's own axum handlers with the caller's `User`. So the
account, DM/player and party checks, the one 404 and the player projection are the ones the app gets. Tools that
other modules serve register their specs with the registry and handle their own calls (see below).

This is the 0.3.1 contract, on branch `feat/mcp`. Tell part B before changing it.

**The user's decisions applied:**
- D25: no tool reads or changes a live combat. There are no `combat_*` tools or `combat:*` scopes.
- D47: the server never hosts or publishes an index. `index_build` returns files as data.
- D26: simulations run on copies of encounters only (`sim_*`, scope `encounters:simulate`).

## Rust API

```rust
use party_tools::{Registry, Limits, Caller, Scope, Via, ToolSpec, ToolError, ToolOutput, Backend};

// The one registry: crate::mcp::registry() (a static). Part C uses it too, so the limits and the idempotency cache
// are shared.
let registry: &Registry = crate::mcp::registry();

// Who is calling. Build it from the auth result for THIS request (the account is re-read from the DB every time).
let caller = Caller {
    account: user.id.clone(),
    token: Some(auth.client.id().into()), // the personal token id or the OAuth grant id
    scopes: Scope::parse_list("bestiary:read encounters:write")?, // what the token grants
    via: Via::Mcp,
};

// What the model may call: tools the scopes allow, in a fixed order. `spec.input_schema` is a JSON schema
// (type object), usable as the tool's parameters for OpenAI, Anthropic and Gemini tool calling. `spec.mcp()` is
// the MCP Tool object.
let specs: Vec<&ToolSpec> = registry.list(&caller);

// One call. Checks, in order: the tool exists, the scope, the arguments (typed, unknown keys refused), requestId
// for writes, the per-token (else per-account) rate, the per-account heavy-call limit, the idempotency cache, and
// then the backend. Every outcome is recorded in the account's activity.
let backend = crate::tools_backend::ServerTools::new(state.clone(), user.clone());
let out: Result<ToolOutput, ToolError> = registry.call(&backend, &caller, "encounter_create", args).await;
```

- `ToolOutput { value, replayed }`. `value` is DATA.
  - The MCP surface sends `{untrusted: true, note: DATA_NOTE, replayed, data}`.
- `ToolError`:
  - the cases: `UnknownTool`, `MissingScope(Scope)`, `InvalidArgs(String)`, `NotFound`, `RateLimited`, `Busy`,
    `Unavailable(String)`, `Refused { code, message }`, `Internal`;
  - `.code()` and `.to_value()` give `{code, message}` for the model. The message text never quotes user content.
- `Backend::record` writes `ai_tool` activity: the surface, tool, scope and status, plus the token id. Arguments
  are never recorded.

## Scopes

| Scope | Tools |
|---|---|
| `bestiary:read` | `bestiary_search`, `monster_get`, `item_search` |
| `encounters:read` | `encounter_list`, `encounter_get`, `spell_search`, `spell_get`, `creature_spellbook` |
| `encounters:write` (implies `encounters:read`) | `encounter_create`, `encounter_update`, `encounter_add_creature`, `encounter_remove_creature` |
| `encounters:simulate` | `sim_start`, `sim_card`, `sim_act`, `sim_auto`, `sim_state`, `sim_report`, `sim_batch`, `sim_replay` |
| `characters:read` | `character_list`, `character_get` |
| `homebrew:read` | `homebrew_list`, `homebrew_get`, `homebrew_validate`, `index_build` |
| `homebrew:write` (implies `homebrew:read`) | `homebrew_create`, `homebrew_update`, `homebrew_delete` |

- Part A issues exactly these strings. `Scope::parse_list` refuses unknown names.
- A scope never widens a role. For example, a player's token with `encounters:write` still gets the API's 404 on
  every write to the DM's encounters.

## Built-in tools

**Request ids.** Every write takes a `requestId`: 8 to 64 characters from `[A-Za-z0-9_-]`.
- The same id from the same token on the same tool, within 24 h, returns
  the first answer with `replayed: true` and changes nothing.
- The same id sent with other arguments is refused.

| Tool | Same as |
|---|---|
| `bestiary_search {text?, crMin?, crMax?, types?, sizes?, environments?, sources?, ruleset?, legendary?, kinds?, sort?, limit?, offset?}` | `POST /api/bestiary/search` (no XP) |
| `monster_get {id}` | `GET /api/bestiary/monsters/{id}` (no XP, D16) |
| `encounter_list {partyId}` | `GET /api/parties/{party}/encounters` |
| `encounter_get {encounterId}` | `GET /api/encounters/{id}`: the DM's full view, a player's projection |
| `encounter_create {requestId, partyId, name, publicName?, hpMode?, notes?}` | `POST /api/parties/{party}/encounters` |
| `encounter_update {requestId, encounterId, name?, publicName?, status?, playerVisible?, hpMode?, notes?}` | `PATCH /api/encounters/{id}` |
| `encounter_add_creature {requestId, encounterId, elementId, count?, dmLabel?, publicName?, hidden?, ownerCreatureId?, initMode?}` | `POST /api/encounters/{id}/creatures` (`ownerCreatureId`: pets, summons) |
| `encounter_remove_creature {requestId, encounterId, creatureId}` | `DELETE /api/encounters/{id}/creatures/{cid}` |
| `character_list {}` | `list_characters` for the caller's own characters, plus the rosters of the parties the caller runs |
| `character_get {characterId}` / `{characterId, partyId, ownerId}` | own `get_snapshot` / the DM's read-only `.../view/snapshot` |

## Tools other modules serve (part D, the simulator)

A module defines its tools with `ToolSpec::external(name, title, description, scope, input_schema)`:
- a write scope adds a required `requestId` to the schema;
- `.heavy()` counts the tool against the heavy-call limit.

It plugs in with two lines in `borealis-server/src/tools_backend.rs` at merge:

```rust
pub fn external_specs() -> Vec<ToolSpec> {
    let mut v = crate::homebrew::tools();     // part D
    v.extend(crate::simulate::tools());       // the simulator
    v
}

async fn external(state, user, tool, args) -> Result<Value, ToolError> {
    // requestId has been removed already: idempotency is the registry's.
    match tool {
        t if t.starts_with("sim_") => crate::simulate::call_tool(state, user, t, args).await,
        t => crate::homebrew::call_tool(state, user, t, args).await,
    }
    .map_err(crate::tools_backend::from_api)
}
```

- The registry has already checked the scope, the request id, the rate and the heavy-call limit.
- The module re-derives the account's rights itself, as its HTTP routes do. For example, `sim_*` checks that the
  caller is the `parties.dm_id` of the encounter's party, and answers not_found otherwise.
- Until a module is merged, the registry lists placeholders under these names, and they answer `Unavailable`. A
  registered spec replaces the placeholder of the same name.
- Placeholders today:
  - `homebrew_list`, `homebrew_get`, `homebrew_validate` (homebrew:read);
  - `homebrew_create`, `homebrew_update`, `homebrew_delete` (homebrew:write);
  - `index_build` (homebrew:read). It returns the `.index` and element files as data for the caller's own agent to
    write and push to its own git repository. Nothing is hosted (D47).
  - `sim_start`, `sim_card`, `sim_act`, `sim_auto`, `sim_state`, `sim_report`, `sim_batch`, `sim_replay`
    (encounters:simulate; `sim_start`, `sim_auto`, `sim_batch` and `sim_replay` are heavy).
- The simulator's design is `plans/borealis-party-sim-design-2026-11-13.md`.
  - Decisions come from the connected agent (decision cards through `sim_*`) or the Basic policy; the server-side
    Assistant oracle was removed in 0.3.3.

## Limits (`Limits::default()`)

- 60 calls a minute per token (per personal token or per connected app).
- 2 heavy calls at a time per account. A third answers `Busy`; it never waits.
- Results over 256 KiB are refused ("narrow the request"). A write's oversized result becomes `{done: true}`.

## Kill switch

`BOREALIS__FEATURES__AI=false` (`[features] ai`, server config only, D40):
- `/api/mcp` and its metadata answer 404;
- every backend call answers `Unavailable`.
