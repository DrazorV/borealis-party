# Combat API (Borealis Connect 0.3.0)

The combat server's routes and live events: a party's DM runs a combat built from an encounter, and the party's
players see a **projection** of it in which hidden creatures don't exist. The code is
`party/crates/borealis-server/src/combat.rs`; the rules are the pure engine in `party/crates/party-combat`
(`party/docs/combat-engine.md`, which lists every command and its fields). Plan:
`plans/borealis-party-roadmap-2026-11-12.md` 3.0, 3.5, 3.6 (M-combat), 3.7, 4.4, 4.4b, and the user's decisions D5 a,
D6, D7 a, D16, D20 b, D21, D23 b, D32 a, D43, D44, D45.

**0.3.3:** the DM view no longer holds any HP or resource of a PC or a player-controlled creature, players cast
from their own character, and the log, export, roster and sheet view follow. See `party/docs/role-views.md`, which
takes precedence over this file where they differ.

Everything is JSON. Times are seconds since the epoch. Every response carries `Cache-Control: no-store, private`.
Errors have the server's usual shape `{ "code", "message" }`. The routes exist only while parties are switched on in
the server config (`BOREALIS__FEATURES__PARTIES`).

## Who may do what

| Caller | What they get |
|---|---|
| The party's DM (`parties.dm_id`, read on every request) | the **DM view**, and every action |
| A player in the party (owns a member character, checked on every request) | the **projection** only, and the player actions below, for **their own PC and the creatures it controls** only |
| Anyone else, a wrong id, a malformed id | 404 |

- **One 404 for players and outsiders:** `{ "code": "not_found", "message": "There is no such combat here." }`, word
  for word, for an id that doesn't exist, a hidden creature's id, a random id, an id with odd characters, another
  player's PC, a DM-only action, and a malformed body. A player can't tell any of these apart.
- Players never see the DM's `seq`, the engine's state, labels, notes, element ids, stat blocks of hidden creatures,
  or the DM log.

## Starting and finding a combat

### `POST /api/encounters/{encounter}/start` (DM)

Body `{ "requestId": "…" }` (see [Writes](#writes)). Starts a combat from the encounter and answers the **DM view**.

- The edition is always the party's (D23 b).
- Creatures: one combatant per encounter creature (monsters and Aurora companions, D17 b), with a **new random
  UUID** (never the encounter creature's id). Hidden creatures start hidden; revealed ones start revealed. HP: the
  creature's fixed HP if set (a typed roll, D43), otherwise the printed average (D22 a), or rolled from the formula
  when the encounter's `hpMode` is `rolled`. AC: the printed (or overridden) one; none for a thin companion block.
  The stat block (overrides applied) is copied into the combat, so later encounter edits don't change a running fight.
- Controlled creatures (pets, mounts, summons): an encounter creature with an `ownerId` comes in with `owner_id` set to
  its owner's combatant, `controller` DM, and its `initMode` (`after_owner`: right after the owner; `own`). Owners are
  added before their pets. A pet keeps its own visibility (a hidden owner's pet usually starts hidden too).
- PCs: every member character of the party, from its roster card (HP, AC, initiative bonus). A PC's HP stays the
  player's own (D7 a): the server never writes a player's file.
- The encounter's status becomes `live`; the combat is in `setup` until the DM sends `start`.
- 409 `conflict` "This party already has a live combat." when one is live. 404 when the caller isn't the DM.

### `GET /api/parties/{party}/combat`

The party's live combat, if any: `{ "combat": "<id>" | null }`. DM and players alike. Players use it (and the party
event below) to show the Combat sub-tab only while a combat is live.

### `GET /api/combats/{combat}`

The DM view for the DM; the projection for a player of the party; 404 otherwise. An ended combat stays readable.

### `GET /api/combats/{combat}/log?before=<seq>&limit=<n>` (DM)

The DM-only log, newest first: `{ "entries": [{ "seq", "at", "author": "dm"|"player"|"system", "event": DmEvent }],
"more": bool }`. `limit` 1–200 (default 100). An ended combat's log and undo journal are deleted 90 days after it ended
(D32 a).

## The projection (`view_for`)

Every answer a player gets, every player event and every player error is built by one function,
`view_for(viewer, combat)`. For a player it is:

```json
{
  "id": "…combat uuid…",
  "partyId": "…",
  "encounter": { "id": "…", "publicName": "Goblin ambush" },
  "status": "setup",                       // setup | active | ended
  "pseq": 4,                               // this player's own sequence (see Live)
  "turn": { "kind": "combatant", "id": "…", "round": 2 },
                                           // or { "kind": "not_started" } | { "kind": "ended" }
                                           //  | { "kind": "waiting", "round": 1 } | { "kind": "enemy_turn", "round": 2 }
  "showEnemyTurn": false,
  "order": [
    { "id": "…", "kind": "character", "name": "Aria", "mine": true, "initiative": 17,
      "band": "unhurt", "life": { "state": "alive" }, "conditions": [], "exhaustion": 0, "concentration": "Bless",
      "view": { …player_view_of: everything of my own PC… } },
    { "id": "…", "kind": "character", "name": "Bran", "mine": false, "initiative": 12,
      "band": "bloodied", "life": { "state": "alive" }, "conditions": ["prone"], "exhaustion": 0, "concentration": null },
    { "id": "…", "kind": "creature", "name": "Owlbear", "mine": false, "band": "unhurt", "life": { "state": "alive" },
      "conditions": [], "exhaustion": 0, "concentration": null,
      "view": { …player_view_of: exact hp, ac, speed, defenses, effects, abilities with their text, economy… },
      "statBlock": { …MonsterData, overrides applied, without xp / xp_lair… } },
    { "id": "…", "kind": "creature", "name": "Owl", "mine": true, "ownerId": "…Aria's combatant id…", "…": "…" }
  ],
  "me": [
    { "id": "…my PC, or a creature it controls…", "kind": "character",
      "characterId": "c_…",                // my PC's character id (null for a creature): which `me` is the open sheet
      "offers": [{ "key": "granted", "name": "Owl", "hp": 1, "ac": 11, "initiative": 3 }],
                                           // companions on this PC's sheet not in the fight yet (summon with `companion`)
      "hp": { "current": 20, "max": 24, "temp": 0 },
      "life": { "state": "alive" },        // the engine's Life: alive | stable | dead | { "state": "dying", "successes": 1, "failures": 0 }
      "absent": false,                     // C1: not in this fight
      "concentration": "Bless", "concentrationChecks": [{ "dc": 10 }],
      "prompts": [{ "id": "…", "kind": "damage" | "heal", "amount": 7, "parts": [{ "amount": 7, "damage_type": "slashing" }],
                    "criticalHit": false, "at": 1763040000 }],
      "rolls": [{ "id": "…", "name": "Initiative", "total": 16, "natural": 12, "source": "app" | "typed",
                  "breakdown": "1d20 [12] +4 = 16", "hit": null, "critical": false }] }   // newest first, 10 kept
  ]
}
```

- **Hidden creatures are absent:** no row, no count, no slot, no id, no turn. `order` is the visible turn order
  (`visible_order`); positions, tiebreaks and the DM's `seq` are never sent. Like the engine's turn order, it lists
  only combatants with an initiative (and not absent): before initiative is in, a player's own PC is in `me` only.
- **Hidden turns are skipped (D5 a):** while hidden creatures act, `turn` stays on the last visible combatant with
  *that* turn's round. `enemy_turn` appears only when the DM switched on `show_enemy_turn` for this combat (it reveals
  that something hidden acts).
- **`view`** is the engine's `player_view_of` (`combat-engine.md`, snake_case fields): it already leaves out labels,
  notes, tiebreaks, effect ids, legendary-action and Legendary Resistance counts (DM only, user 2026-11-13), and every
  reference to a hidden combatant (a hidden effect source and its duration, a hidden owner or link).
- **Revealed creatures (D20 b):** `view` plus the full `statBlock`. There are no reveal levels. A creature's
  initiative isn't sent (a follower's would give away a hidden leader's), and neither is a PC's owning account
  (`view.kind`) except for my own.
- **`ownerId`** (any row): the combatant a creature acts for and right after (a pet, a summon), only when that owner
  is visible. A hidden villain's hidden pet leaves no trace at all: no row, no id, no turn, no count.
- **Other PCs:** name, band (`unhurt` / `bloodied` / `down`), life, conditions, exhaustion, concentration, initiative;
  no `view`, no HP numbers (D6). **Mine** (`mine: true`, and the `me` list): my PCs that are members of the party and
  the creatures they control (summons, pets, companions): full HP, death saves, concentration checks waiting, open
  prompts and my latest roll results (D21: every roll for them, whoever made it, in the app or typed).
- A creature's `life` is only `alive` or `dead`. `band` is null while a PC's HP isn't known yet (its card hasn't been
  read).
- Rich text in `statBlock` renders through upstream `SafeHtml` only.

### The DM view

```json
{
  "id", "partyId", "encounter": { "id", "name", "publicName" },
  "seq": 12,                     // the CAS counter: send it back as expectSeq
  "status": "active",
  "state": { …party-combat CombatState, as combat-engine.md describes… },
  "creatures": { "<combatant id>": { "encounterCreatureId": "…" | null, "statBlock": { …MonsterData or companion… },
                                       "name": "Goblin Warrior",        // the element's name (null for a typed summon)
                                       "origin": "encounter" | "copy" | "element" | "companion" | "summon" } },
  "characters": { "<combatant id>": { "ownerId": "…", "characterId": "c_…", "member": true, "online": true } },
  "prompts": [{ "id", "combatantId", "kind": "damage" | "heal", "amount", "parts", "criticalHit", "at" }],
  "offers": [{ "pc": "<combatant id>", "key", "name", "hp", "ac", "initiative", "elementId" }],   // PCs' sheet companions
  "joinable": [{ "characterId": "c_…", "name": "…" }],   // 0.3.5: party characters not fighting (joined late, absent, no initiative); names only
  "undo": ["damage", "next turn", …],      // newest first (the engine's labels)
  "startedAt", "endedAt"
}
```

`member: false` marks a PC whose character left the party mid-fight: it stays in the fight for the DM; its owner
no longer sees the combat.

## Writes

`POST /api/combats/{combat}/{action}` with a JSON body:

```json
{ "requestId": "8f0c…", "expectSeq": 12, "args": { … } }
```

- **`requestId`** (required, 1–64 characters of `A-Z a-z 0-9 _ -`): one per user action. Sending the same id again
  (a retry after a timeout, or the ~60 s offline buffer replaying, D44) does the write **at most once** and answers
  **the answer the first try got**, with `"replayed": true`. Ids are kept for a day.
- **`expectSeq`** (DM only, optional): the `seq` the DM's view showed. When it isn't the current `seq` the write is
  refused with 409 `{ "code": "stale", "message": "The combat changed. Refresh and try again.", "seq": <current> }`.
  Without it, the write applies to the newest state (commands are intents like "7 slashing to X", so a queued write
  stays meaningful). Every write is a compare-and-set on `seq` inside one transaction either way.
- **`args`**: the engine command's own fields, with the engine's names (snake_case), as `party/docs/combat-engine.md`
  lists them. Every roll field takes `{ "kind": "typed", "total": 15, "natural": 12 }` (a table roll, always accepted,
  D43) or `{ "kind": "app", … }` (the server's seeded roller, D21). A typed value is always accepted.
- **Answer:** the caller's view (DM view, or the projection) after the write.
- Engine refusals answer 422 `{ "code": "refused", "message": "<the engine's reason>" }` to the DM (for example
  "Needs recharge", "Bonus action spent"). For players see [Player actions](#player-actions).
- **Limits:** 240 writes a minute per account (then 429 `too_many_requests`); at most 2 blocking jobs (start, import,
  export) at a time per account and 16 on the server.

### DM actions

Every engine command by its tag (`set_initiative`, `damage`, `heal`, `set_hp`, `grant_temp_hp`, `death_save`,
`stabilize`, `set_death_saves`, `add_condition`, `remove_condition`, `set_exhaustion`, `set_concentration`,
`end_concentration`, `resolve_concentration`, `add_effect`, `remove_effect`, `use_charge`, `resolve_effect_save`,
`use_ability`, `roll_recharge`, `set_recharge`, `set_legendary_pool`, `adjust_legendary`,
`adjust_legendary_resistance`, `adjust_slot`, `set_slots`, `adjust_resource`, `set_rule`, `set_rule_active`,
`remove_rule`, `set_group_limit`, `set_economy`, `roll_initiative`, `clear_initiative`, `swap_tie`, `link`, `start`,
`next_turn`, `previous_turn`, `rename`, `set_public_name`, `reveal`, `hide`, `set_absent`, `set_notes`,
`set_settings`), plus these aliases and server actions:

| Action | Args | What it does |
|---|---|---|
| `prev_turn` | – | `previous_turn` |
| `condition_add` / `condition_remove` | as `add_condition` / `remove_condition` | aliases |
| `remove_creature` | `{ id }` | `remove_combatant` (a tombstone; undo restores it) |
| `add_creature` | `{ copyOf: "<combatant id>" }` or `{ encounterCreatureId }`, plus `count` (1–10, default 1), `rollInitiative` (default true), `hidden` (default: like the source) | C7: new copies with new random ids, the stat block copied, labels numbered. 0.3.5 (A24): a copy keeps its source's abilities and spells (override spells too), spellcasting, stats and defenses, with full slots, resources, recharges and Legendary Resistance |
| `add_member` | `{ characterId: "c_…", initiative: D20Input }` | 0.3.5 (A2): a party member's character not in the fight (`joinable`) joins: a new PC combatant from its roster card, or an absent one back; its initiative is typed, or `{ kind: "app" }` rolled with the card's own bonus. 400 when it's in the fight already |
| `multi_damage` | `{ targets: [{ id, save?: RowRoll }], amount: DiceInput, damage_type?, save?: { ability, dc, half (default true) }, magical?, apply_qualified? }` | 0.3.5 (D13): the engine's `MultiDamage`: one roll for every target; each target's save typed, `outcome`, or `app` (creatures only); half (rounded down, before defenses) or none on a success; a PC gets a damage prompt. One undo step |
| `undo` | `{ force?: bool }` | C8: revert the last write (50 steps). When it was a player's: 409 `{ "code": "undo_player" }` unless `force` |
| `end` | – | ends the combat; the encounter becomes `ended`; the D6 health band switches off |
| `export` | – | D45: answers `{ "format": "borealis-party-combat", "schema_version": 1, "state", "log", "server": { "creatures", "characters" } }` (no `requestId` needed; it writes nothing) |

The engine's newer commands work the same way: `roll`, `attack`, `roll_hp`, `dismiss`, `set_initiative_mode`.
`add_combatant` and `sync_player_hp` are not offered: the server builds combatants itself and syncs PC HP itself.

### Summons (DM and players)

`summon` takes `{ owner, initiative?, visible? }` (as the engine) plus **one** of:

| Field | The creature |
|---|---|
| `companion: "<key>"` | a companion on the owning PC's sheet (`offers`); offered once while it's in the fight. 0.3.5 (A24): the cards of members whose workspace is open are read again at combat start, and a card change with the same HP still rebuilds the views, so a companion added mid-fight is offered |
| `elementId` | a typed Monster or an Aurora Companion element of the **caller's own** content |
| `copyOf: "<combatant id>"` (DM) | a fresh copy of a creature in this fight (with its spells and full pools, as `add_creature`) |
| `combatant` | an engine-shaped block the client typed: `{ "kind": "creature", "label", "public_name", "hp": { "current", "max" }, "ac"? }` |

Optional `name` / `publicName` rename an `elementId` or `copyOf` summon. The server **always** mints the id (a random
UUID), builds the block, and adds the creature to the DM view's `creatures`. A PC's summon is controlled by that PC's
player and is always visible; a DM creature's pet starts as its owner is.

### Player actions

A player acts only for **their own PC and the creatures it controls** (summons, pets, companions). The engine decides
which commands that allows (`engine::player_may`, listed in `combat-engine.md` under "Who may issue what": for the PC
`set_initiative`, `death_save`, the concentration commands, `resolve_effect_save`, `roll`, `attack` and `summon`; for
its creatures also HP, conditions, effects, abilities, economy, recharges, slots, resources, `set_initiative_mode` and
`dismiss`). The server adds, in the write's own transaction, that every PC the command acts through is the caller's
and still a member of the party. Plus:

| Action | Args |
|---|---|
| `resolve_prompt` | `{ promptId, applied: bool, hp?: { current, max, temp } }` |

- `summon` for their own PC (see [Summons](#summons-dm-and-players)); not `copyOf`.
- Anything else, another player's PC or creature, a DM creature, a hidden or unknown id, a server action (`undo`,
  `end`, `add_creature`, `add_member`, `multi_damage`, `remove_creature`, `export`), a malformed body: the one 404.
- A refusal about the player's own PC answers 409 with a fixed message: "Not making death saves.", "Dead: set HP to
  revive.", "No concentration check is waiting.", "The combat isn't running.", "The combat has ended.", or 400 "That
  roll isn't valid.". Nothing else ever reaches a player.
- **The damage prompt (D7 a):** when the DM damages or heals a PC, the owner's projection gets a prompt ("Take 7
  slashing?"). The app applies it on the player's own Play tab (`take_damage` / `regain_hit_points` through the
  normal RPC, the player's own write), then sends `resolve_prompt` with `applied: true` and, ideally, the sheet's HP
  after it (`hp`; otherwise the server reads the roster card). `applied: false` dismisses it. The server then syncs
  the PC's HP into the combat (with the damage, for concentration and death saves). While a damage prompt is open, a
  card refresh doesn't sync that PC's HP, so the damage counts once.

## Import (D45)

`POST /api/parties/{party}/combats/import` (DM), body `{ "requestId", "file": <an export, as JSON> }`, at most 8 MiB.
It always makes a **new** live combat, in the state the file holds (round, turn, HP, effects), with a fresh die, an
empty undo journal, an empty log and `seq` 0; it never overwrites one. It answers the DM view of the new combat
(with `"replayed": true` for a retried `requestId`). Every combatant gets a new random id. PCs whose character isn't a member of the
party any more are marked removed. Refuses a newer schema, another format, or a state whose references don't
resolve (400). 409 while the party has a live combat.

## Live (`/api/events`)

On the account's existing event stream (`party/docs/parties-api.md`), never a second connection:

- `party://combat` with `{ "party", "combat", "seq" }` to the **DM**, after every write.
- `party://combat` with `{ "party", "combat", "pseq" }` to **each player**, only when **that player's** projection
  changed. The server hashes each player's projection after every write and compares it with the hash it kept;
  `pseq` is that player's own counter. A DM-only write (anything about a hidden creature, labels, notes, legendary
  pools, …) sends players **nothing** and moves no `pseq`.
- `party://party` with `{ "party", "pseq", "kind": "combat", "id": "<combat id>" }` to the DM and the players when a
  combat starts or ends: refetch `GET /api/parties/{party}/combat` and the roster (the health band, D6, shows only
  while a combat is live).
- `party://ping` with `{}` and no id, every 15 s on every stream (plus SSE keep-alive comments for proxies;
  Cloudflare drops an idle stream at about 100 s). No ping for about 45 s means the stream is dead: reconnect.
- Payloads are invalidations only: refetch `GET /api/combats/{combat}`. "Your turn" is in the projection (`turn.id`
  is one of my `me` ids: a PC or a creature it controls); vibrate when it becomes so.

### Guarantees (tested)

- **Latency:** a write reaches every connected DM and player stream within 300 ms locally (tested: p95 is logged; on
  SQLite about 30 ms).
- **Gapless, resync in one request:** the DM's `seq` goes up by exactly 1 per write; a player's `pseq` by exactly 1
  per change of their view. A client that sees a jump, reconnects (`?after=<last event id>` gives a
  `party://resync` when it missed anything), or comes back from the background fetches `GET /api/combats/{combat}`
  once: that answer is the whole current state, with its `seq` / `pseq`.
- **Compare-and-set:** two writers never overwrite each other; a stale `expectSeq` gets 409 `stale` with the current
  `seq` (two-writer tests on SQLite and PostgreSQL).
- **Idempotent:** a repeated `requestId` applies nothing and answers the first answer.
- **Never blocked by content work:** combat writes and reads use only the database and the in-memory roster cards,
  never the account's workspace, so a slow content reload or index job doesn't hold them up (tested with a 3 s job
  running in both the DM's and the player's workspaces).

## Storage (`M-combat`)

Migration step 6 (after 4 = monster_meta and 5 = encounters), marker table `combats`.

- `combats`: the party, the encounter, `status`, `seq` (CAS), the engine state and die as JSON, times.
- `combatants`: one row per combatant id: `kind`, the encounter creature it came from, the PC's owner and character,
  the stat block a creature fights with, its element's `name` and its `origin`.
- `combat_journal`: undo steps (the state before each write, 50 kept per combat).
- `combat_log`: the DM-only log.
- `combat_views`: each player's last projection hash and `pseq`.
- `combat_prompts`: open and resolved damage and heal prompts, and players' roll results.
- `combat_requests`: request ids already done, with the first answer (a day).

Everything cascades from the party. Log, journal and requests of combats ended more than 90 days ago are deleted
(D32 a).
