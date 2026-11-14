# Role views: the DM's screen and the player's screen (Borealis Party 0.3.3)

The server side of the user's rules of 2026-11-14:

- "DM should never know players HP and resources";
- "when players cast spell it should be shown to dm if it applies an effect";
- "having different screens for DM and player helps with deciding what each one sees". The **Combat** tab is the
  DM's screen; the **Play** tab is the player's.

Later the same day (quoted): "DM can have the same information as players, blodied, healthy, down, etc, players do
not need to know much about monsters in their tab only the name and if bloodied", "when the dm starts a combat with
the party the play tab appears for characters that are in the initiative", and "the DM can see how much max hp
characters have when creating the party and he can also have access to the players build like it is now, while in
combat we want to hide it because realistically in a fight you cant know how healthy someone is". Party members
always see each other's health bands, in and out of combat (decision 1, accepted).

This replaces D6 ("the DM and the owner see the full card"; "the band only during combat") and narrows D20 b
("revealed = full stat block"). Code: `party/crates/party-combat/src/visibility.rs`
(the DM half: `dm_view_of`, `dm_may_see`, `cast_private`, `redacted_for_dm`), `party/crates/party-combat/src/engine.rs`
(`dm_may`, `player_may`, prompts), `party/crates/borealis-server/src/combat.rs`, `hub.rs` (cards) and `parties.rs`
(the DM's sheet view). The routes are the ones in `combat-api.md`; this file lists what 0.3.3 changes in them.

## Words

- **The player's side:** a PC, and every creature a player controls (summons, pets, companions:
  `controller: { "player": { "pc": … } }`). `visibility::player_owned`.
- **The player's numbers:** HP (current, max, temp), hit dice, death saves, spell slots, class resources, rolled HP,
  concentration checks, ability uses and recharges, casting data, defenses and save bonuses from the sheet.

## What the DM gets of the player's side

### In the combat (`GET /api/combats/{id}`, every write's answer, `notices`)

`state.combatants[]` for a player's combatant is an **allowlist** (`COMBATANT_DM_OF_PLAYER`):

```json
{ "id", "kind", "label", "public_name", "visible", "ever_revealed", "removed",
  "initiative", "initiative_bonus", "initiative_advantage", "tiebreak", "absent", "linked_to", "surprised",
  "owner_id", "controller", "initiative_mode",
  "ac",                  // the DM resolves attacks against it (open point 2)
  "speed", "exhaustion", "economy", "group", "notes",
  "concentration": true, // yes/no only (the effects it holds say which spell)
  "band": "bloodied",    // "unhurt" | "bloodied" | "down" | null (HP not known yet): the band the party sees
  "life_state": "alive", // "alive" | "dying" | "stable" | "dead", never the death-save counts
  "down": false }        // life_state isn't "alive"
```

Never there: `hp`, `hp_authority`, `zero_hp`, `life`, `defenses`, `pending_concentration`, `abilities`,
`legendary_resistance`, `spell_slots`, `resources`, `stats`, `casting`, `spell_turn` (`COMBATANT_PLAYER_PRIVATE`).
A test classifies every combatant key into exactly one of the two lists, so a new field stays hidden until someone
classifies it.

So the DM tracker **must not read `hp`, `life` or slots of a PC or a player's creature**: they aren't there. A DM
creature's entry is unchanged (all of it).

- `state.effects`: all of them, the ones **on** PCs included (conditions and active effects on a PC are the DM's to
  see: Bless, Shield, Hold Person, the dying Unconscious while `down` is shown).
- `state.casts`: the open casts, except a player's private cast (below).
- `creatures[id]` of a player's creature: `statBlock` is `null` (its printed HP and uses are the player's); `name` and
  `origin` stay.
- `offers[]` (a PC's sheet companions not in the fight): `{ pc, key, name, ac, elementId }`, no `hp` or `initiative`.
- `prompts[]`: what the DM sent and what became of it, **never the HP after it**:
  `{ id, combatantId, kind: "damage"|"heal", amount, parts, criticalHit, at, status: "open"|"accepted"|"declined" }`;
  open ones first, then the last 30 answered, newest first.
- `feed[]` (new): the last 30 log entries the DM may see, newest first, `{ seq, at, author, event }` (`event` is a
  `DmEvent`, as in the log). This is where the DM sees what players did that the DM sees or resolves: a player's cast
  (`cast_started`, `cast_row`, `cast_applied`, `summon_options`, `ruling_needed`), attacks, effects added.
- `undo[]`: a player's step reads `"player action"` (its label could name a slot or HP change).
- `characters[id]`: unchanged (`ownerId`, `characterId`, `member`, `online`).

### DM writes on the player's side

- `damage` / `heal` on a PC **or a player's creature**: no HP change; the player gets a prompt ("Ogre hits you:
  13 bludgeoning", already through the PC's or creature's resistances). The DM's answer shows it in `prompts` with
  `status: "open"`, later `"accepted"` or `"declined"`. Spell damage from a DM's cast works the same.
- Refused with 422 `refused` ("That number is on the player's side: …"): `set_hp`, `grant_temp_hp`, `roll_hp`,
  `adjust_slot`, `set_slots`, `adjust_resource`, `death_save`, `set_death_saves`, `resolve_concentration` for a
  player's combatant (`engine::dm_may`). Conditions, effects, concentration end, initiative, reveal, absent, notes:
  unchanged.
- `resolve_cast_row` for a DM creature's save against a player's spell: the DM's (as for any cast).

### The DM's live stream

`party://combat` `{ party, combat, seq }` reaches the DM only when **the DM view changed** (a hash kept per combat in
`combat_views`, the DM's own row) or the DM wrote. A sync of a PC's HP from the sheet, a player's private cast or a
change of their creature's numbers sends the DM nothing. `seq` still moves on every write (it's the CAS counter), so
a DM write with an old `expectSeq` can get 409 `stale` after such a write: refetch and retry, as today.

### The log and the export

- `GET /api/combats/{id}/log`: only entries the DM may see (`visibility::dm_may_see`): never a private event (the
  engine flags them, `DmEvent.private`), and never an HP, temp HP, death save, slot/resource, roll, concentration
  check, recharge or applied damage/heal event about the player's side (this also filters logs from before 0.3.3).
  `limit` counts shown entries.
- `export`: `state` is `redacted_for_dm`: the player's side's numbers reset to empty values (HP 0/0, no slots,
  resources, abilities, casting data; a down PC is `stable`), private casts left out, and the log filtered as above.
  It still imports; the PCs' HP comes back from their sheets. `server.creatures[id].statBlock` is `null` for a
  player's creature.

### The roster and the sheet view

- **Outside combat the DM's roster card is the whole card, as before** (HP x/y, max HP, AC, initiative, passive
  Perception, DCs, the build): the DM sees max HP when building the party.
- **While the party is in a combat** (`parties::in_combat`), the DM's card (`GET /api/parties/{p}/roster`) has
  `stats` cut to `hub::DM_STATS` (`armorClass`, `initiative`, `speed`, `passivePerception`, `spellDcs`, `exhaustion`,
  `concentration` as **`true`/`false`**, `inspiration`) plus `band`; no `hp`, `hpMax`, `tempHp`, no `readAt`.
- **Other members always** get `band` (`unhurt` / `bloodied` / `down` / null), in and out of combat, never numbers.
  The owner gets everything.
- Events: during a combat the DM hears `party://party` `kind: "card"` only when the DM's card changed (a new band,
  the AC; never HP within a band); the members when their band changed. A change only the owner sees moves nothing
  the DM or the members can count or time.
- `POST /api/parties/{p}/members/{owner}/{character}/view/snapshot` (the DM's read-only sheet): the whole build
  outside combat. **During a combat** `play` is cut to `speeds, exhaustion, inspiration, defenses, senses,
  advantages, initiativeAdvantage, saveDcs, masteries, concentrationSpells, compatibility, concentration` (yes/no);
  no `stats.hpMax`, no `spellcasting.slots`, `multiclassSlots` or `classes[].pact`, no companion `hp` / `hpNote`
  (`parties::dm_sheet`), and its `card` is the in-combat card.
- A homebrew save or delete, added share links, and an import job **when it ends** (`update_indexes`, the 5etools
  URL jobs) now refresh the account's party cards too (they were stale on the DM's roster).

## The player's screen (Play tab)

The projection (`GET /api/combats/{id}` as a player): `me[]` keeps the player's own HP, death saves, prompts and
rolls. New in 0.3.3:

### Monsters are a name and a band

A DM creature's row in `order` is exactly `{ "id", "kind": "creature", "name", "mine": false, "band" }`
(`combat::CREATURE_FOR_PLAYERS`): the public name and `unhurt` / `bloodied` / `down` / null. No `view`, no
`statBlock`, AC, HP, conditions, abilities, legendary counts or owner. Hidden creatures stay absent as before.
The same holds before the fight: a shown encounter's revealed creatures (`GET /api/encounters/{id}` as a player) are
`{ id, publicName, band }` (`encounters::CREATURE_FOR_PLAYERS`; `encounters-api.md`), and the builder says "Players see
its name".
Another player's PC or creature: `{ id, kind, name, mine, band, life, conditions, exhaustion, concentration }` (+
`initiative` for a PC, `ownerId` when visible), no `view`. My PC and my creatures: in full (`view`, and `statBlock`
for my creatures). A save request (`casts[]`) still names a revealed caster's spell.

### Which characters are in the fight

- `me[].inInitiative`: the PC (or creature) is in this fight (the DM started it with it and hasn't set it absent).
  The Play tab shows its combat section only for a character with `inInitiative: true`.
- `party://combat` to a player now carries `{ party, combat, pseq, characters: ["c_…"] }`: their member
  characters in the fight.
- `GET /api/parties/{party}/combat` answers `{ combat, characters }` (`characters` empty for the DM or with no
  live combat).

### Casting from the character

`POST /api/combats/{combat}/cast_spell`, as the PC's player (or for a creature it controls):

```json
{ "requestId": "…",
  "args": { "caster": "<my PC's or my creature's combatant id>",
            "spellKey": "ID_PHB_SPELL_FIREBALL",   // a spell of the player's own content
            "slot": 3,                              // the level spent on the sheet (null for a cantrip)
            "targets": ["…", "…"],                  // visible combatants only
            "rows": [ … ], "amounts": [ … ], "crit_amounts": [ … ], "choices": { … },   // optional, as the DM's
            "dc": 15, "attack": 7, "mod": 4, "cantripTier": 2,   // a PC's numbers from its sheet (optional)
            "ability": "…" } }                      // a creature's stat block entry, instead of spellKey
```

- The server compiles the definition on **any installed content**, in this order:
  1. the party DM's own definition of that spell (a DM-defined or confirmed draft);
  2. the player's library: their own definitions, then the curated SRD rows (any copy of the same spell counts);
  3. the same-named spell of the party's edition that has mechanics (a sheet id from another book or edition);
  4. a draft from the player's own copy (its 5etools data), `spec.origin: { "kind": "draft", "key" }`, unreviewed: the
     DM sees the cast and confirms the results;
  5. only then a **ruling**: the slot is spent on the sheet and the DM gets `ruling_needed` to apply it by hand.

  A client `spec`, `precast` or `force` is never used. Send the sheet's spell element id as `spellKey`.
- **The slot is spent on the sheet** by the app (the player's own `expend_spell_slot`-style write on the Play tab),
  never in the combat: the engine checks only that the level fits. A player's creature spends its own combat slots.
- **The player's open casts** are in the projection's new `myCasts[]` (casts by their PC or its creatures,
  `visibility::player_own_casts`):

  ```json
  { "cast": "…", "caster": "<combatant id>", "spell": "Fireball", "slot": 3,
    "delivery": "auto" | "attack" | "save",
    "rows": [{ "target": "…", "ray": 0, "outcome": null | "lands" | "hit" | "crit" | "miss" | "saved" | "failed" }],
    "amounts": [{ "kind": "damage" | "heal" | "temp_hp", "dice": "8d6", "damage_type": "fire", "choice": [],
                  "add_mod": false }],
    "needs_apply": true }   // every row has an outcome and the amounts are still to roll: send apply_cast
  ```

  Rows on a hidden combatant are left out. `amounts` are in `apply_cast`'s order. A cast that applied at once
  (every roll sent with `cast_spell`) is never listed.
- Rolls: typed first (table dice), or `app`. A player answers: the **attack rows of their own cast** (their attack
  roll) and their PC's **save rows** (as before). The saves of DM creatures are the DM's (they come up in the DM's
  `state.casts` and `feed`). Damage: `apply_cast { cast, amounts }` by the caster's player (or the DM).
  `cancel_cast { cast }` drops their own cast. `standard_action` (Dodge, Dash, Disengage, Help) is theirs too.
- **What the DM sees:** a cast that targets a DM creature, leaves any effect or condition (on anyone, Shield on
  yourself included), concentrates, summons, makes a zone or needs a ruling is shown (`feed`, `state.casts`, the
  effect chips). A cast **only on the player's side, with nothing but damage, healing or temporary HP** (Cure Wounds
  on an ally, Healing Word) is **private**: every event of it is flagged and the DM sees nothing of it
  (`visibility::cast_private`). The healed ally's player gets the heal prompt as usual.
- Answers: the projection. Refusals, in fixed words: 404 (not yours, a hidden target, malformed, no spell named),
  400 "That spell isn't in your content." / "That spell can't be cast here." / "That cast isn't valid." / "That roll
  isn't valid.", 409 "Action spent." / "Bonus action spent." / "Reaction spent." / "Choose a slot." / "That slot is
  too low for this spell." / "A spell slot was already used this turn." / "Bonus action spell this turn: only an
  action cantrip." / "You're down." / "Incapacitated: not now." / "No uses left." / "Needs recharge." / "Not
  available now.".

### Prompts for a player's creature

The DM's damage or healing on a creature the player controls is a prompt in that creature's `me[].prompts` (as for
the PC). `resolve_prompt { promptId, applied: true }` applies it to the creature (the amount as sent; no `hp`
needed); `applied: false` declines it. The player can still `damage` / `heal` / `set_hp` their creature directly.

### No compatibility shims

The legacy player Combat sub-tab had no routes of its own: it read the same projection, whose shape changed above
(monster rows, `inInitiative`). Nothing is kept for the old UI (the user's "no compatibility shims", 2026-11-14);
part G replaces it.

## Open points for the user

1. "Down" is answered (the DM sees the bands, `down` and `life_state`). `visibility::DM_SEES_PLAYER_DOWN` stays as
   the one switch.
2. **AC stays visible** to the DM (needed to resolve attacks), and so do initiative bonus, speed, exhaustion, action
   economy, spell save DCs, passive Perception and Heroic Inspiration on the card. A cast's slot level is in the
   DM's `cast_started` (the damage dice show it anyway); remaining slots never are.
3. The spell a player casts isn't checked against the character's spellbook (the server has the character's card,
   not its spell list): installed content is the limit.
4. **Decided (orchestrator, 2026-11-14):** the simulation tools (`sim_*`, DM-only MCP) may show the PCs' simulated
   HP, slots and resources to the DM's agent. A simulation is DM prep on a copy, never a live fight, and outside
   combat the DM may see the players' HP and build. They stay DM-only and never touch live combats.
5. Monsters' conditions are no longer shown to players (only the name and the band, as the user said). Players may
   want "prone" or "restrained" on a target; one key to add back if the user asks.
