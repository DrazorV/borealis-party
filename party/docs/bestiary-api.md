# Bestiary API (0.3.0)

The bestiary layer answers two questions about an account's own installed content: did a book's bestiary come in
fully (`coverage`), and which creatures match a filter (`search`). It lists two kinds of creature (decision D17 b, the
user's choice on 2026-11-13):

- `monster`: typed `Monster` elements (`borealis_content::MonsterData`, see `docs/monster-model.md`): the bundled SRD,
  5etools imports, homebrew. Plus the details the 5etools import drops, kept party-side in `monster_meta` (D15 a).
- `companion`: Aurora `Companion` elements (familiars, beast companions, steeds): their setters (`ac`, `hp`, `speed`,
  the six scores, `type`, `size`, `alignment`, `challenge`, ...) and linked `Companion Trait` / `Companion Action` /
  `Companion Reaction` elements, read the way upstream's companion tab reads them. Only what is printed: a missing
  field is `null`, never filled in. There is no XP anywhere (the user's D16): the CR is a printed stat and a filter.

Server code: `party/crates/borealis-server/src/bestiary.rs`. Every route needs a signed-in account and reads that
account's own content only; another account's ids answer the same 404 as unknown ones.

## Routes

| Route | What |
|---|---|
| `GET /api/bestiary/coverage` | per-pack report: creatures, CR and type counts, what the import skipped or kept in part |
| `POST /api/bestiary/search` | filtered, paginated creature summaries (the encounter builder's picker) |
| `GET /api/bestiary/facets` | the values the filters can take, with counts (filter chips) |
| `POST /api/bestiary/facets` | the same, counted over `{ "enabledSources": [...] }` only (0.3.6) |
| `GET /api/bestiary/monsters/{id}` | one creature: its full stat block, its pack revision and its extra details |

Errors use the server's usual body `{ "code", "message" }`: 400 `invalid_argument` for a bad filter, 401 when signed
out, 404 `not_found` for an id that is not a typed Monster element of this account.

### `POST /api/bestiary/search`

Request (every field optional):

```json
{
  "text": "gob",
  "crMin": "1/8",
  "crMax": "5",
  "types": ["humanoid", "beast"],
  "sizes": ["small", "medium"],
  "environments": ["forest", "hill"],
  "sources": ["srd-5.2.1-monsters"],
  "ruleset": "dnd2024",
  "legendary": true,
  "kinds": ["monster", "companion"],
  "sort": "name",
  "limit": 50,
  "offset": 0
}
```

- `text`: case-insensitive part of the name (at most 100 characters). Names that start with it come first.
- `crMin` / `crMax`: challenge ratings as printed (`"0"`, `"1/8"`, `"1/4"`, `"1/2"`, `"1"` to `"30"`), inclusive.
  A creature without a CR (a summon, IM12) only matches when neither bound is given.
- `types`: creature types, lowercase singular (the 14 of `CreatureType`: `aberration` … `undead`).
- `sizes`: `tiny` `small` `medium` `large` `huge` `gargantuan`; a "Medium or Small" creature matches either.
- `environments`: lowercase 5etools environment words (`arctic`, `coastal`, `desert`, `forest`, `grassland`, `hill`,
  `mountain`, `swamp`, `underdark`, `underwater`, `urban`, …). Only creatures with captured environments can match;
  the bundled SRD packs have none.
- `sources`: pack ids (as `list_packs` names them) or source names (the element's `source`); either matches.
- `enabledSources` (0.3.5): the source names the account's new characters start on (the Sources page's saved default
  with its Restrict playtest / third-party toggles applied, as the app works it out). Creatures of any other source are
  never listed. Without it, the account's toggles apply (`content_rules.rs`). Sources unchecked on any of the
  account's own characters (0.3.6) are left out too, with or without it; another account's characters never count.
- `ruleset`: `dnd2014` or `dnd2024` (the edition). A creature marked `any` matches both.
- `legendary`: `true` only creatures with legendary actions, `false` only creatures without; absent for both.
  Companions never have legendary actions.
- `kinds`: `monster`, `companion`; empty or absent for both.
- `sort`: `name` (default) or `cr` (then name).
- `limit`: 1 to 200, default 50. `offset`: from 0.
- Each list takes at most 50 values. Unknown types, sizes, kinds, rulesets and CRs are a 400.
- A companion's type is its `type` setter lowercased (`Humanoid (goblinoid)` gives type `humanoid`, tags
  `["goblinoid"]`); one with no type setter has `type: null` and matches no type filter.

Response:

```json
{
  "total": 2,
  "offset": 0,
  "limit": 50,
  "items": [
    {
      "id": "ID_SRD521_MONSTER_GOBLIN_WARRIOR",
      "name": "Goblin Warrior",
      "kind": "monster",
      "cr": "1/4",
      "crValue": 0.25,
      "type": "fey",
      "tags": ["goblinoid"],
      "size": ["small"],
      "ac": 15,
      "hp": 10,
      "hpFormula": "3d6",
      "source": "System Reference Document 5.2.1",
      "packId": "srd-5.2.1-monsters",
      "packRev": "9f2c4a1e0b7d3c55",
      "ruleset": "dnd2024",
      "legendary": false,
      "mythic": false,
      "lair": false,
      "environments": [],
      "page": null
    }
  ]
}
```

- `cr`, `crValue` are `null` for a creature without a CR; `ac` / `hp` are `null` when printed as a note (IM12a)
  or, for a companion, not printed; `hpFormula` `null` for fixed HP. `type` is `null` only for a companion that
  prints none; `size` is empty then too.
- A companion's `ruleset` is its own, else its pack's (when the pack targets one), else `any`.
- `packRev` is the revision of the pack the creature came from. Snapshots (`encounter_creatures.pack_rev`) compare it
  for "Refresh stat block": a different value means the pack was imported or updated since.
- `mythic` comes from `monster_meta` (5etools data only).

### `GET /api/bestiary/facets`

Counts only the creatures the search would list (0.3.6): the account's content rules as above. `POST` takes
`{ "enabledSources": [...] }` as the search does; `GET` applies the toggles and the characters' sources only.

```json
{
  "total": 647,
  "kinds": [{ "value": "monster", "count": 647 }],
  "types": [{ "value": "humanoid", "count": 120 }],
  "sizes": [{ "value": "medium", "count": 300 }],
  "environments": [{ "value": "forest", "count": 0 }],
  "rulesets": [{ "value": "dnd2024", "count": 330 }],
  "crs": [{ "value": "1/4", "count": 40 }],
  "sources": [{ "value": "srd-5.2.1-monsters", "name": "SRD 5.2.1 Monsters", "count": 330 }]
}
```

Lists are sorted by value (CRs in CR order). `sources` lists packs that hold at least one creature of either kind.

### `GET /api/bestiary/monsters/{id}`

```json
{
  "id": "ID_SRD521_MONSTER_GOBLIN_WARRIOR",
  "name": "Goblin Warrior",
  "source": "System Reference Document 5.2.1",
  "packId": "srd-5.2.1-monsters",
  "packRev": "9f2c4a1e0b7d3c55",
  "kind": "monster",
  "ruleset": "dnd2024",
  "monster": { "ruleset": "dnd2024", "size": ["small"], "creature_type": "fey", "…": "MonsterData, snake_case" },
  "companion": null,
  "mapsTo": null,
  "meta": null
}
```

A companion answers `kind: "companion"`, `monster: null` and its block in `companion` (camelCase; every field as
printed, `null` / empty when not):

```json
{
  "ac": 11, "acText": "11", "hp": 1, "hpText": "1 (1d4 - 1)", "hpFormula": "1d4-1",
  "speed": "5 ft., fly 60 ft.", "speeds": { "walk": 5, "fly": 60 },
  "abilities": { "str": 3, "dex": 13, "con": 8, "int": 2, "wis": 12, "cha": 7 },
  "saves": {}, "proficiency": 2,
  "skills": "Perception +3, Stealth +3", "senses": "darkvision 120 ft., passive Perception 13", "languages": null,
  "challenge": "0", "size": "Tiny", "type": "Beast", "alignment": "unaligned",
  "defenses": [{ "name": "Damage Resistances", "text": "…" }],
  "traits": [{ "name": "Flyby", "text": "…" }],
  "actions": [{ "name": "Talons", "text": "Melee Weapon Attack: +3 to hit, …" }],
  "reactions": []
}
```

- `proficiency` is the printed one, else the one of its CR; `null` with neither.
- `saves` holds the printed (proficient) saving throws only; the others are the ability modifier.
- A companion's linked traits and actions are not creatures: their ids answer 404.

- `monster` is the element's `data` exactly as `MonsterData` serializes it; encounter snapshots store it as is
  (`statblock_json`) and can validate it with `MonsterData::validate()`. A companion has no `MonsterData`; snapshot its
  `companion` block instead (with `kind`), and compare `packRev` the same way.
- `meta` is `null` unless the 5etools import captured details for this pack revision (see below).

### `GET /api/bestiary/coverage`

```json
{
  "totals": { "packs": 3, "monsters": 647, "companions": 0 },
  "packs": [
    {
      "packId": "srd-5.2.1-monsters",
      "name": "SRD 5.2.1 Monsters",
      "kind": "bundled",
      "enabled": true,
      "packRev": "9f2c4a1e0b7d3c55",
      "status": "complete",
      "monsters": 330,
      "companions": 0,
      "inData": 330,
      "skipped": 0,
      "partial": 0,
      "skippedReasons": [],
      "partialNotes": [],
      "sources": [{ "name": "System Reference Document 5.2.1", "monsters": 330 }],
      "byCr": [{ "value": "0", "count": 20 }],
      "byType": [{ "value": "beast", "count": 80 }],
      "rulesets": [{ "value": "dnd2024", "count": 330 }],
      "extras": { "environment": 0, "page": 0, "mythic": 0, "covenCr": 0, "altSpeeds": 0, "variants": 0,
                  "reactionIntro": 0, "fluff": 0, "tokens": 0 },
      "captured": false
    }
  ]
}
```

- One entry per installed pack that holds a typed Monster or an Aurora `Companion`; packs with neither are left out.
- `monsters` and `companions` count the two kinds; `byCr`, `byType` and `rulesets` count both.
- `kind`: `bundled`, `5etools` (a local 5etools import), `index` (downloaded from an index), `custom`.
- `enabled`: the pack is turned on. A turned-off pack loads no creatures, so it is left out of the report and of
  search until it is turned on again.
- `status`:
  - `complete`: every creature in the data came in (`monsters == inData`, nothing skipped). Bundled SRD packs are
    complete by construction (317/317, 330/330).
  - `partial`: some creatures were skipped, or kept with details dropped (`partial > 0`); `skippedReasons` and
    `partialNotes` say what, with counts (no creature names).
  - `companions_only`: Aurora content: `Companion` creatures and no typed stat blocks.
  - `installed`: typed Monster elements from an index or a custom pack: there is no import record to check them
    against, so only the count is known.
  - `unknown`: a 5etools pack imported before 0.3.0 (no import record); import or update it again to fill
    `inData`, `skipped`, `partial`, `extras` and `captured`.
- `inData`: creatures the 5etools data had for this book (converted plus skipped); `null` when unknown.
- `extras` counts the creatures with each captured detail; `fluff` and `tokens` say whether the 5etools data has
  lore text or token images for them (neither is imported).
- `captured`: `monster_meta` holds this pack revision's details.

## Captured details (`monster_meta`, D15)

The 5etools import (`import_5etools`, and `update_5etools_url` when the address changed) is wrapped on the server:
after upstream's import succeeds, the same staged 5etools folder is read again and, for every creature that became a
Monster element, these are kept, keyed by account, element id and pack revision:

| Field | From 5etools | Shape in `meta` |
|---|---|---|
| `environments` | `environment` | `["forest", "hill"]` |
| `page` | `page` | number |
| `mythic` | `mythicHeader` + `mythic` | `{ "intro": "text", "actions": [{ "name", "text" }] }` (the mythic trait itself is already one of the stat block's traits) |
| `covenCr` | `cr.coven` | `{ "cr": "…" }` |
| `altSpeeds` | `speed.alternate` | `{ "walk": [{ "feet": 40, "condition": "…" }], "fly": […] }` |
| `variants` | `variant` | `[{ "name", "text" }]` (sidebars, as text) |
| `reactionIntro` | `reactionHeader` | text |
| `fluff`, `token` | `hasFluff` / `hasToken` | booleans |

A failed capture never fails the import; the import still works and coverage shows `captured: false`. Rows of a pack
are replaced whole on every import of it. Rows for an older pack revision are ignored (and replaced at the next
import), so a snapshot never mixes revisions.

## For the encounter builder (Rust)

`bestiary.rs` exposes the same calls for server code, run against the DM's own workspace:

```rust
pub async fn search(state: &AppState, owner: &str, query: &SearchQuery) -> ApiResult<SearchPage>;
pub async fn monster(state: &AppState, owner: &str, element_id: &str) -> ApiResult<Option<MonsterDetail>>;
```

`MonsterDetail { id, name, kind, source, pack_id, pack_rev, ruleset, monster: Option<MonsterData>,
companion: Option<CompanionDetail>, maps_to, meta }` is what an encounter snapshot needs: for a monster
`statblock_json` = `serde_json::to_string(&detail.monster)`, for a companion the `companion` block, plus `kind`,
`element_id` and `pack_rev`.

## Performance

The first call after the account's content loads builds an in-memory list of its creatures (one pass over the
content store, a few thousand entries for a full 5etools import) and keeps it until the content reloads or new
details are captured. Every later search filters that list, so a page answers in milliseconds; items are small
(no stat block) so a phone page of 50 is a few KB.
