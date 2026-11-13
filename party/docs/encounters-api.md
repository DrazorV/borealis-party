# Encounters API (Borealis Party 0.3.0)

The encounter builder's server routes: a party's DM builds encounters from their own installed creatures (typed
Monster elements and Aurora Companions), gives villains pets and summons, and decides what players may see. Players
see a player-visible encounter as a **projection** only. The code is `party/crates/borealis-server/src/encounters.rs`;
the stat block copies, overrides and the player view are in `party/crates/party-encounter`. Plan:
`plans/borealis-party-roadmap-2026-11-12.md` 3.5, 3.6 (M-encounters), 4.3, 4.4 ("Controlled creatures"), 4.4b (C19,
C20, C23).

The user's decisions behind it (2026-11-13): **no XP or difficulty anywhere** (D16: CR and XP are not reliable; CR is
only a printed stat and a search filter); **hidden or revealed with the whole stat block**, no reveal levels (D20 b);
**the party's edition**, no per-encounter override (D23 b); **companions too** (D17 b); per-encounter **overrides** on
a copied block (D41 A); rules are a banner plus on/off flags (D42); average HP by default, rolled per encounter (D22);
every DM roll can be rolled in the app or typed (D21).

Everything here is JSON. Times are seconds since the epoch. Every response carries `Cache-Control: no-store, private`.
Errors have the server's usual shape `{ "code", "message" }`.

## Who may do what

| Caller | Draft (or ended, or not shown) encounter | Player-visible encounter (`ready` or `live`, and `playerVisible`) |
|---|---|---|
| The party's DM | everything | everything |
| A player in the party (owns a member character) | **404** | `GET` only, the **projection** |
| Anyone else, a wrong id, a malformed id | 404 | 404 |

- **One 404 for everything a caller may not see:** `{ "code": "not_found", "message": "There is no such encounter here." }`.
  The same answer, word for word, for a draft, a hidden creature's id, an id from another party, a random id, an id
  with odd characters, and any write by a player (whatever its body). Bodies are read only after the DM check.
- Only the party's DM (`parties.dm_id`, read from the database on every request) creates or changes anything.
- The routes exist only while parties are switched on in the server config (`BOREALIS__FEATURES__PARTIES`).

## The projection (`view_for`)

Every answer a player gets is built by one function, `view_for(viewer, …)`. For a player it is:

```json
{
  "id": "…uuid…",
  "partyId": "…",
  "publicName": "Goblin ambush",
  "status": "ready",
  "creatures": [
    { "id": "…uuid…", "publicName": "Big beast", "kind": "monster", "size": ["large"], "type": "Monstrosity",
      "conditions": [], "hp": { "current": 59, "max": 59 }, "statBlock": { "…MonsterData, overrides applied, no xp…": 0 } },
    { "id": "…", "publicName": "Bird", "kind": "companion", "size": ["tiny"], "type": "Beast", "conditions": [],
      "hp": { "current": 1, "max": 1 }, "statBlock": { "…CompanionDetail…": 0 }, "ownerId": "…only when its owner is revealed too…" }
  ]
}
```

- **Hidden creatures are absent:** no row, no count, no slot, no id. Revealed creatures keep the DM's order among
  themselves; positions and sort numbers are never sent.
- **Revealed = the whole stat block** (D20 b): the block the encounter uses (overrides applied), without `xp` /
  `xp_lair`. A monster's is `MonsterData` (snake_case); a companion's is the bestiary's `CompanionDetail` (camelCase,
  nulls where Aurora prints nothing). Rich text renders through upstream `SafeHtml` only.
- **Never sent to players:** the DM's `name` of the encounter, `dmNotes`, `hpMode`, `updatedAt`, warnings, creature
  `dmLabel`, `elementId`, `packId`, `packRev`, `overrides` (the DM's note included), `initMode`, effects and rules, and
  `ownerId` of a creature whose owner is hidden.
- Before combat there is no damage, so `hp.current` is the max. Combat (wave 2) feeds real HP and conditions into the
  same function (`party_encounter::creature::player_view`).

## Encounters

### `GET /api/parties/{party}/encounters`

- **DM:** `{ "role": "dm", "encounters": [Summary] }`, every encounter of the party, newest first.
  `Summary = { id, name, publicName, status, playerVisible, visibleToPlayers, creatureCount, hiddenCount, updatedAt }`.
- **Player:** `{ "role": "player", "encounters": [{ id, publicName, status }] }`, player-visible ones only (`live` first,
  then `ready`, newest first). The app shows the first as "Next: <publicName>".
- Not the DM or a player of the party: 404 (the parties' "There is no such party, invite or character here.").

### `POST /api/parties/{party}/encounters` (DM)

Body `{ name, publicName?, hpMode?, dmNotes? }`. Answers the DM view (below).
- `name`: 1–80 characters, the DM's own name for it. `publicName`: 1–60, what players see; defaults to `name`.
- `hpMode`: `average` (default, the printed average) or `rolled` (rolled from the hit dice when combat starts, D22).
- `dmNotes`: up to 4000 characters. At most 50 encounters per party (then 429 `too_many_requests`).
- `ruleset`, `status` and `playerVisible` are refused (400): an encounter uses its party's edition and starts as a draft.

### `GET /api/encounters/{encounter}`

The DM view for the DM; the projection for a player when it is player-visible; 404 otherwise. The DM view:

```json
{
  "id", "partyId", "name", "publicName",
  "ruleset": "dnd2024",            // the party's edition (D23 b); change it on the party
  "status": "draft",               // draft | ready | live | ended
  "playerVisible": false,
  "visibleToPlayers": false,       // playerVisible and status ready or live
  "hpMode": "average",
  "dmNotes": "",
  "createdAt", "updatedAt",
  "creatures": [Creature],         // in order; a controlled creature right after its owner
  "effects": [Effect],
  "rules": [Rule],
  "warnings": [Warning]            // other-edition creatures and characters only; nothing about difficulty
}
```

`Creature` (DM only):

```json
{
  "id", "sort",
  "kind": "monster",               // monster | companion (D17 b)
  "dmLabel": "Goblin Warrior 2",   // DM only; defaults to the creature's name, numbered
  "publicName": "Fey",             // what players see once revealed; defaults to the type word, never numbered
  "hidden": true,                  // new creatures start hidden
  "revealedAt": null,
  "ownerId": null,                 // the creature it is controlled by (pet, summon, mount), or null
  "initMode": "after_owner",       // after_owner | own (only matters with an owner)
  "elementId": "ID_SRD521_MONSTER_GOBLIN_WARRIOR", "packId": "monsters", "packRev": "16 hex digits",
  "name": "Goblin Warrior", "source": "SRD 5.2.1 Monsters", "ruleset": "dnd2024", "mapsTo": "ID_SRD51_MONSTER_GOBLIN",
  "cr": "1/4",                     // as printed; a stat, nothing rates with it
  "type": "Fey", "size": ["small"],
  "hp": { "average": 10, "formula": "3d6" },   // what the encounter uses (overrides applied)
  "hpMode": "encounter",           // encounter (follow the encounter's hpMode) | fixed (hpMax: rolled in the app or typed, D21)
  "hpMax": null,
  "ac": 15,
  "printed": { "hp": { "average", "formula" }, "ac": { "value", "source" }, "saves": { "str": -1, …, "cha": null } },
  "overrides": Overrides,
  "modified": false,               // the block differs from the copy (HP, AC, saves or traits)
  "statBlock": { …the block the encounter uses, no xp… }
}
```

`packRev` here is a fingerprint of the copied block itself; "Refresh" compares the block, not the pack. (The
bestiary's `packRev` is the pack's revision.)

### `PATCH /api/encounters/{encounter}` (DM)

Any of `{ name, publicName, status, playerVisible, hpMode, dmNotes }`. Answers the DM view.
- `status`: `draft`, `ready` or `ended`. `live` is set only by starting a combat (wave 2): 400 here.
- Players see it exactly while `playerVisible` is true **and** `status` is `ready` or `live`. Setting `status: draft`
  also clears `playerVisible`.

### `DELETE /api/encounters/{encounter}` (DM)

`{ "deleted": true }`. Its creatures, effects and rules go with it.

## Creatures (DM)

### `POST /api/encounters/{encounter}/creatures`

Body `{ elementId, count?, dmLabel?, publicName?, hidden?, ownerId?, initMode? }`. Answers the DM view.
- `elementId`: a **typed `Monster`** or an **Aurora `Companion`** element installed in the DM's own content (D17 b).
  Anything else is 400 "Only monsters and companions can be added to an encounter."; an unknown id is 400 "That
  creature isn't in your content."
- The block is **copied** (`statblock_json`: `{ kind, name, source, ruleset, mapsTo, data | companion }`). Later
  content changes don't touch it until "Refresh". A companion's copy holds only what it prints.
- `count`: 1–20 (default 1). At most 60 creatures per encounter. `hidden`: default `true`.
- `ownerId`: a creature of this encounter that isn't controlled itself; the new ones are controlled by it and sit
  right after it and its other controlled creatures. `initMode`: `after_owner` (default) or `own`.

### `PATCH /api/encounters/{encounter}/creatures/{creature}`

Any of `{ dmLabel, publicName, hidden, hpMode, hpMax, sort, ownerId, initMode, overrides }`. Answers the DM view.
- `dmLabel` 1–80, `publicName` 1–60 characters. `hpMax` 1–9999 with `hpMode: fixed`.
- `ownerId`: another top-level creature of this encounter, or `null` to stop being controlled. A creature that
  controls others can't become controlled; nothing is its own owner.
- **Visibility of controlled creatures:** each has its own `hidden`. Revealing an owner reveals none of its
  controlled creatures; a revealed pet of a hidden owner is shown without `ownerId`.
- `overrides` (D41 option A): `{ hp?, ac?, saves?, traits?, notes? }`. Each field absent = unchanged, `null` = drop it.
  - `hp: { average, formula? }` (1–9999; formula like `4d10+8`), `ac: { value, source? }` (1–40),
    `saves: { "dex": 4, "con": null }` (−10 to +30), `traits: [{ name, text }]` (replaces the list, at most 12),
    `notes: "…"` (the DM's, up to 4000).
  - The server records each overridden field's source value (`base`) from the copy; clients never send it.
  - They apply to a companion's block too (its `hp`, `hpFormula`, `ac`, `saves`, `traits`).

### `DELETE /api/encounters/{encounter}/creatures/{creature}`

Answers the DM view. The creatures it controls and its starting effects go with it; rules linked to it keep their text
and lose the link.

### `POST /api/encounters/{encounter}/creatures/{creature}/refresh`

"Refresh stat block": read the element again from the DM's content and compare.
Body `{ choices?: { "hp": "keep" | "source", "ac": …, "saves.dex": … } }`. Answers
`{ changed, conflicts: [{ field, was, source, value }], applied, encounter }`.
- `changed`: the source block differs from the copy.
- A **conflict** is a field the DM overrode that the source also changed since. With a choice for every conflict (or
  no conflicts) the new block is copied in and `applied` is true; otherwise nothing changes and `applied` is false.
- `keep` keeps the DM's value (its `base` moves to the new source value); `source` drops that override.
- The element gone from the DM's content: 409 "That creature isn't in your content any more."

### `POST /api/encounters/{encounter}/creatures/{creature}/swap`

D23: replace the copy with the matching-edition block. Body `{ elementId? }` (default: the creature's `mapsTo`).
Labels, public name, visibility and owner stay; overrides are dropped. Answers the DM view.

## Starting effects (C19, DM)

Effects on a creature before the fight ("precast" buffs). Stored now, applied when combat starts.

- `POST /api/encounters/{encounter}/effects` with
  `{ creatureId, name, detail?, sourceCreatureId?, durationTurns?, charges?, concentration?, slotLevel?, modifiers? }`.
- `PATCH /api/encounters/{encounter}/effects/{effect}` (any of those but `creatureId`), `DELETE …/effects/{effect}`.
- `Effect = { id, creatureId, sort, name, detail, sourceCreatureId, durationTurns, charges, concentration, slotLevel, modifiers }`.
  - `name` 1–80, `detail` up to 2000. `durationTurns` (in the *source's* turns, C12) and `charges` 1–1000 or null.
    `slotLevel` 1–9 or null (the slot it spent).
  - `modifiers = { ac?: -10..10, speedAdd?: -120..120, speedMult?: 0..4, speedZero?: bool }` (C13).
  - `creatureId` and `sourceCreatureId` must be creatures of this encounter. At most 40 effects.
- All answer the DM view.

## Encounter rules (C20, D42: a banner and flags, no scripting, DM)

- `POST /api/encounters/{encounter}/rules` with `{ name, text?, creatureId?, endsWhen?, active?, flags? }`.
- `PATCH /api/encounters/{encounter}/rules/{rule}`, `DELETE …/rules/{rule}`.
- `Rule = { id, sort, name, text, creatureId, endsWhen, active, flags: [{ creatureId, ability, whileActive }] }`.
  - `name` 1–80, `text` up to 2000 (the banner). `creatureId`: the creature the rule is tied to (or null).
  - `endsWhen`: `creature_down` (default; needs `creatureId`; ends for good at 0 HP) or `manual`.
  - `flags`: at most 20 `{ creatureId, ability (1–80 characters, as printed), whileActive: "on" | "off" }`.
  - At most 20 rules.
- All answer the DM view.

## Warnings (D23 b)

`warnings` in the DM view: `{ kind: "edition", creatureIds, creatures: [{ creatureId, ruleset, mapsTo }], text }` for
stat blocks of the other edition than the party's (offer **swap**), and `{ kind: "characters_edition", count, text }`
for member characters of the other edition. There is no XP, budget, band or `evaluate` route.

## Events

On the account's `/api/events` stream, as the party events (`party/docs/parties-api.md`):
`party://party` with `{ party, pseq, kind: "encounters", id: <encounter id> }`.
- The **DM** gets one on every change.
- **Players** get one only when what `view_for(player)` shows of that encounter changed (the projection is hashed
  before and after each write). Editing a draft, a hidden creature, an effect, a rule, notes or labels players don't
  see sends players **nothing**, and their `pseq` doesn't move.

## Storage (`M-encounters`)

Migration step 4 on this branch, **5** once `M-monster-meta` (step 4) merges first; marker table `encounters`.
- `encounters(id, party_id → parties CASCADE, name, public_name, status, player_visible, hp_mode, dm_notes, created_at,
  updated_at)`: no `dm_id` and no `ruleset` (both come from the party).
- `encounter_creatures(id, encounter_id → encounters CASCADE, sort, kind, dm_label, public_name, element_id, pack_id,
  pack_rev, statblock_json, overrides_json, automation_json, hp_mode, hp_max, visibility, revealed_at,
  owner_creature_id, init_mode, created_at, updated_at)`. `owner_creature_id` has no foreign key (so the copy between
  databases needs no row order); the server removes controlled creatures with their owner.
- `encounter_effects(…, creature_id → encounter_creatures CASCADE, source_creature_id → SET NULL, …)`,
  `encounter_rules(…, creature_id → SET NULL, flags_json, …)`.

## For wave 2 (combat)

- `party_encounter::creature::{player_view, Snapshot}` gives what players see of a revealed creature;
  `Snapshot::block(&overrides)` the block it fights with; `Snapshot::average_hp` its default HP.
- `encounters.status` `live` is set by `POST /api/encounters/{id}/start`; this API refuses to set it.
- Starting HP: `encounters.hp_mode` (`average` | `rolled`) and each creature's `hp_mode` / `hp_max` (`fixed` = a number
  the DM rolled in the app or typed). A `rolled` roll at start must also offer typed entry (D21).
- Controlled creatures: carry `owner_creature_id` and `init_mode` into the combatants (`after_owner`: right after the
  owner in a stable sub-order; `own`: its own initiative), and each creature's own `visibility`.
- Starting effects and rules are stored for the effects and rules work.
