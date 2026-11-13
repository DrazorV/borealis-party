# Encounters API (Borealis Party 0.3.0)

The encounter builder's server routes: a party's DM builds encounters from their own installed monsters, sees XP and
difficulty, and decides what players may see. Players see a player-visible encounter as a **projection** only. The
code is `party/crates/borealis-server/src/encounters.rs`; the maths and the stat block copies are in
`party/crates/party-encounter`. Plan: `plans/borealis-party-roadmap-2026-11-12.md` 3.5, 3.6 (M-encounters), 4.3, 4.4b
(C19, C20, C23) and decisions D16, D17, D20, D22, D23, D41, D42.

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
  with odd characters, and any write by a player (whatever its body). Bodies are read only after the DM check, so a
  malformed body from a player is the same 404.
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
    { "id": "…uuid…", "publicName": "Small fey", "revealLevel": "name", "size": ["small"], "type": "Fey", "conditions": [] },
    { "id": "…", "publicName": "Bear-like beast", "revealLevel": "band", "size": ["large"], "type": "Monstrosity", "conditions": [], "band": "unhurt" },
    { "id": "…", "publicName": "Owlbear", "revealLevel": "full", "size": ["large"], "type": "Monstrosity", "conditions": [], "band": "unhurt",
      "hp": { "current": 59, "max": 59 }, "statBlock": { "…MonsterData without xp / xp_lair…": 0 } }
  ]
}
```

- **Hidden creatures are absent:** no row, no count, no slot, no XP, no id. Revealed creatures keep the DM's order
  among themselves; positions and sort numbers are never sent.
- **Never sent to players:** the DM's `name` of the encounter, `dmNotes`, `ruleset`, `hpMode`, `updatedAt`, any XP or
  difficulty, creature `dmLabel`, `elementId`, `packId`, `packRev`, `overrides` (the DM's note included), effects and
  rules.
- **Reveal levels (D20, default `name`):**
  - `name`: public name, size, type word, conditions;
  - `band`: plus `band` (`unhurt` / `bloodied` / `down`);
  - `full`: plus `hp` and the stat block the encounter uses (overrides applied), without `xp` and `xp_lair`.
  Rich text in `statBlock` renders through upstream `SafeHtml` only.
- Before combat there is no damage, so `band` is `unhurt` and `hp.current` is the max. Combat (wave 2) feeds real HP and
  conditions into the same function (`party_encounter::creature::player_view`).

## Encounters

### `GET /api/parties/{party}/encounters`

- **DM:** `{ "role": "dm", "encounters": [Summary] }`, every encounter of the party, newest first.
  `Summary = { id, name, publicName, status, playerVisible, visibleToPlayers, creatureCount, hiddenCount, xp, band, updatedAt }`.
- **Player:** `{ "role": "player", "encounters": [{ id, publicName, status }] }`, player-visible ones only (`live` first,
  then `ready`, newest first). The app shows the first as "Next: <publicName>".
- Not the DM or a player of the party: 404 (the parties' "There is no such party, invite or character here.").

### `POST /api/parties/{party}/encounters` (DM)

Body `{ name, publicName?, ruleset?, hpMode?, dmNotes? }`. Answers the DM view (below).
- `name`: 1–80 characters, the DM's own name for it. `publicName`: 1–60, what players see; defaults to `name`.
- `ruleset`: `dnd2024`, `dnd2014` or `null`/absent = the party's (D23).
- `hpMode`: `average` (default, the printed average) or `rolled` (roll from the formula when combat starts, D22).
- `dmNotes`: up to 4000 characters.
- At most 50 encounters per party (then 429 `too_many_requests`).

### `GET /api/encounters/{encounter}`

The DM view for the DM; the projection for a player when it is player-visible; 404 otherwise. The DM view:

```json
{
  "id", "partyId", "name", "publicName",
  "ruleset": "dnd2024",            // the one used: the encounter's own, else the party's
  "rulesetSet": null,              // the encounter's own choice, null = the party's
  "partyRuleset": "dnd2024",
  "status": "draft",               // draft | ready | live | ended
  "playerVisible": false,
  "visibleToPlayers": false,       // playerVisible and status ready or live
  "hpMode": "average",
  "dmNotes": "",
  "createdAt", "updatedAt",
  "creatures": [Creature],
  "effects": [Effect],
  "rules": [Rule],
  "evaluation": Evaluation          // against the party's member characters (see evaluate)
}
```

`Creature` (DM only):

```json
{
  "id", "sort",
  "dmLabel": "Goblin Warrior 2",   // DM only; defaults to the monster's name, numbered
  "publicName": "Fey",             // what players see once revealed; defaults to the type word
  "hidden": true,                  // new creatures start hidden
  "revealLevel": "name",           // name | band | full
  "revealedAt": null,
  "elementId": "ID_SRD521_MONSTER_GOBLIN_WARRIOR", "packId": "monsters", "packRev": "16 hex digits",
  "name": "Goblin Warrior", "source": "SRD 5.2.1 Monsters", "ruleset": "dnd2024", "mapsTo": "ID_SRD51_MONSTER_GOBLIN",
  "cr": "1/4", "xp": 50,           // null for a creature with no CR (it counts as unrated)
  "type": "Fey", "size": ["small"],
  "hp": { "average": 10, "formula": "3d6" },  // what the encounter uses (overrides applied)
  "hpMode": "encounter",           // encounter (follow the encounter's hpMode) | fixed (hpMax, e.g. a typed roll, D43)
  "hpMax": null,
  "ac": 15,
  "printed": { "hp": {…}, "ac": 15, "saves": { "str": -1, … } },  // the snapshot's own numbers
  "overrides": Overrides,
  "modified": false,               // HP, AC, saves or traits overridden: difficulty still uses the CR, with a warning
  "statBlock": { …MonsterData, overrides applied… }
}
```

### `PATCH /api/encounters/{encounter}` (DM)

Any of `{ name, publicName, ruleset (null = the party's), status, playerVisible, hpMode, dmNotes }`. Answers the DM view.
- `status`: `draft`, `ready` or `ended`. `live` is set only by starting a combat (wave 2): 400 here.
- Players see it exactly while `playerVisible` is true **and** `status` is `ready` or `live`. Setting `status: draft`
  also clears `playerVisible`.

### `DELETE /api/encounters/{encounter}` (DM)

`{ "deleted": true }`. Its creatures, effects and rules go with it.

## Creatures (DM)

### `POST /api/encounters/{encounter}/creatures`

Body `{ elementId, count?, dmLabel?, publicName?, hidden? }`. Answers the DM view of the encounter.
- `elementId` must be a **typed `Monster` element** installed in the DM's own content (D17); anything else is 400
  "Only monsters can be added to an encounter." An unknown id is 400 "That monster isn't in your content."
  The bestiary search also lists Aurora `Companion` creatures (the user's D17 b, part A); the picker asks it for
  `kinds: ["monster"]` until companions get a stat block copy of their own (open item).
- The stat block is **copied** (`statblock_json`) with `elementId`, `packId` (the pack file's name) and `packRev` (a
  fingerprint of the block). Later content changes don't touch it until "Refresh".
- `count`: 1–20 (default 1). At most 60 creatures per encounter.
- `hidden`: default `true`.

### `PATCH /api/encounters/{encounter}/creatures/{creature}`

Any of `{ dmLabel, publicName, hidden, revealLevel, hpMode, hpMax, sort, overrides }`. Answers the DM view.
- `dmLabel` 1–80, `publicName` 1–60 characters. `hpMax` 1–9999 with `hpMode: fixed`.
- `overrides` (D41 option A): `{ hp?, ac?, saves?, traits?, notes? }`. Each field absent = unchanged, `null` = drop it.
  - `hp: { average, formula? }` (1–9999; formula like `4d10+8`), `ac: { value, source? }` (1–40),
    `saves: { "dex": 4, "con": null }` (−10 to +30), `traits: [{ name, text }]` (replaces the list, at most 12),
    `notes: "…"` (the DM's, up to 4000).
  - The server records each overridden field's source value (`base`) from the snapshot; clients never send it.
  - Stored `Overrides` look like `{ "hp": { "value": { "average": 80 }, "base": { "average": 59, "formula": "7d10+21" } }, "saves": { "dex": { "value": 4, "base": 1 } }, "traits": [...], "notes": "…" }`.

### `DELETE /api/encounters/{encounter}/creatures/{creature}`

Answers the DM view. Its starting effects go with it; rules linked to it keep their text and lose the link.

### `POST /api/encounters/{encounter}/creatures/{creature}/refresh`

"Refresh stat block": read the element again from the DM's content and compare.
Body `{ choices?: { "hp": "keep" | "source", "ac": …, "saves.dex": … } }`. Answers
`{ changed, conflicts: [{ field, was, source, value }], applied, encounter }`.
- `changed`: the source block differs from the copy (`packRev`).
- A **conflict** is a field the DM overrode that the source also changed since. With a choice for every conflict (or
  no conflicts) the new block is copied in and `applied` is true; otherwise nothing changes and `applied` is false, so
  the app can show the conflicts and ask.
- `keep` keeps the DM's value (its `base` moves to the new source value); `source` drops that override.
- The element gone from the DM's content: 409 "That monster isn't in your content any more."

### `POST /api/encounters/{encounter}/creatures/{creature}/swap`

D23: replace the copy with the matching-edition block. Body `{ elementId? }` (default: the creature's `mapsTo`).
Labels, public name, visibility and reveal level stay; overrides are dropped (the numbers belong to the other block).
Answers the DM view.

## Starting effects (C19, DM)

Effects on a creature before the fight ("precast" buffs). Stored now, applied when combat starts (0.4.1).

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
  - `name` 1–80, `text` up to 2000 (the banner).
  - `creatureId`: the creature the rule is tied to (or null).
  - `endsWhen`: `creature_down` (default; needs `creatureId`; ends for good at 0 HP) or `manual`.
  - `flags`: at most 20 `{ creatureId, ability (1–80 characters, an ability's name as printed), whileActive: "on" | "off" }`
    = "while this rule is active, that ability is usable (on) or not (off)".
  - At most 20 rules.
- All answer the DM view.

## `POST /api/encounters/{encounter}/evaluate` (DM)

XP and difficulty against the party (SRD 5.2.1 "XP Budget per Character"). Body `{ levels? }`: character levels
1–20 to rate against instead of the members (at most 12). Answers `Evaluation`:

```json
{
  "method": "srd521",
  "ruleset": "dnd2014",
  "note": "2014 encounters are rated with the SRD 5.2.1 XP budget.",   // only for 2014 (D16)
  "party": { "levels": [5, 5, 5, 4], "from": "members" },             // or "given"; unreadable members are skipped
  "totalXp": 900, "band": "low",                                       // trivial | low | moderate | high | extreme
  "thresholds": { "low": 2000, "moderate": 3000, "high": 4400 },
  "remaining": { "low": 1100, "moderate": 2100, "high": 3500 },
  "creatures": 5, "unrated": 0,
  "warnings": [{ "kind": "…", "text": "…", … }]
}
```

Warning kinds: `monster_above_party_level` (`cr`), `many_creatures`, `many_stat_blocks`, `zero_cr`,
`action_economy` (a rule of thumb, not SRD), `modified` (`creatureIds`: overridden creatures, rated by their CR),
`unrated` (`creatureIds`: no CR), `edition` (`creatureIds` whose block is the other edition, with their `mapsTo`),
`characters_edition` (members in the other edition), `no_party` (no readable member and no `levels`: thresholds are
0 and `band` is null).
Hidden creatures count: they are in the fight. Players never get this.

## Events

On the account's `/api/events` stream, as the party events (`party/docs/parties-api.md`):
`party://party` with `{ party, pseq, kind: "encounters", id: <encounter id> }`.
- The **DM** gets one on every change.
- **Players** get one only when what `view_for(player)` shows of that encounter changed (the projection is hashed
  before and after each write). Editing a draft, a hidden creature, an effect, a rule, notes or labels players don't
  see sends players **nothing**, and their `pseq` doesn't move. Then refetch `GET /api/parties/{party}/encounters` (and
  the encounter, if shown).

## Storage (`M-encounters`)

Migration step 4 on this branch (5 once `M-monster-meta` merges first; numbers are fixed at merge), marker table
`encounters`. Tables: `encounters`, `encounter_creatures` (`statblock_json`, `overrides_json`, `automation_json` for
0.5.1), `encounter_effects`, `encounter_rules`. No `dm_id`: authorization joins `parties.dm_id`. Everything cascades
from the party, so deleting a DM's account or a party removes its encounters.

## For wave 2 (combat)

- `party_encounter::creature::{player_view, health_band, RevealLevel}` is the reveal field matrix; feed it live HP and
  conditions. `party_encounter::overrides::Overrides::apply` gives the block a creature fights with.
- `encounters.status` `live` is set by `POST /api/encounters/{id}/start`; this API refuses to set it.
- `encounters.hp_mode` (`average` | `rolled`) and each creature's `hp_mode` / `hp_max` (`fixed` = a typed number)
  decide starting HP. Starting effects and rules are stored for 0.4.1 / 0.4.2.
