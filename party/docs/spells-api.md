# Spells API (0.3.2)

The server side of "Spells and mechanics in combat": an account's spells (one per edition), the DM's spell rules
(effect definitions, links and name choices), precast spells, and the cast actions of a live combat. Design:
`plans/borealis-party-spells-design-2026-11-13.md` (the user's decisions at its end override it).

Server code: `party/crates/borealis-server/src/spells.rs` (index, rules, file), `encounters.rs` (precast spells),
`combat.rs` (casts, start). Engine: `party-combat` (`spell.rs`, commands); data: `party-spells` (curated SRD rows,
the canonical resolver, drafts); spellbooks: `party-encounter/src/spellbook.rs`.

Every route needs a signed-in account and is that account's own; another account's ids answer the same 404 as unknown
ones. Nothing here reaches a player: a combat's player projection never carries definitions, links, casting numbers
or spell keys, only each player's own save rows (see "Players").

## Storage (migration step 9, M-spells)

| Table | What |
|---|---|
| `spell_mechanics` | the account's rows, by `kind`: `def` (an effect: the engine's closed `SpellMech`), `link` (`{"target": key}`), `alias` (a normalized name that means this spell) |
| `encounter_casts` | precast spells: caster, spell key, slot level, targets, choices |
| `spell_meta` | the raw 5etools data of imported spells (the tags the import drops), for drafts; replaced per pack on import |

`alias` rows are the DM's choices for the canonical resolver: an alias of the spell's own name is the DM's **pick** of
that copy; any other name is a **link** of that name to the spell (a monster spellbook line the DM resolved once).

## One spell per edition

The same spell from several sources (same normalized name, same ruleset, same level) is one spell (`party-spells`
`canonical::Catalog`). The chosen copy: the DM's pick, then the newest source when every copy has a date (SRD 5.1 is
2016, SRD 5.2.1 is 2025), then the SRD copy, then stable order. The other copies are hidden from new picks, never
deleted, and listed as `alsoIn`. 2014 and 2024 spells never merge; a copy of no edition (`any`) is its own spell.
Aurora XML content counts as 2014 rules.

Mechanics key on the one spell: a definition (or curated SRD row) of any copy applies to the chosen one.

| Route | What |
|---|---|
| `GET /api/spells?ruleset=&text=&level=&limit=&offset=&forRuleset=` | the spells, one per name/edition/level, each with `coverage` |
| `POST /api/spells/search` | the same with the query as a body, and `enabledSources` (0.3.5) |
| `GET /api/spells/{key}` | one spell from any copy's id: the spell, the element (its text), the account's rows, the mechanics combat would use, a suggestion |
| `POST /api/spells/{key}/choose` | this copy is the one spell (an alias row) |
| `DELETE /api/spells/{key}/choose` | back to the rule's choice |

A spell: `{ key, name, ruleset, level, school, concentration, ritual, source, packId, chosenBy, alsoIn: [{ key,
source, packId, packKind }], coverage }`. `chosenBy` is `only`, `dmPick`, `newest`, `srd`, `preferred` or `stable`.
`coverage` is `defined` or `linked` (the account's), the curated tier (`auto`, `partial`, `utility`, `ruling`), else
`ruling` (no definition: a cast spends the slot and the DM rules). `concentration` comes from the element's
`isConcentration` setter, never from its text.

**Content rules (0.3.5):** the list offers only what the account's characters may pick
(`crates/borealis-server/src/content_rules.rs`). `enabledSources` (body only) is the sources a new character starts on, as
the app works it out (the Sources page's saved default with the Restrict playtest / third-party toggles applied); every
other installed source is left out, as `set_source_restrictions` does. Without it, the account's toggles apply. A
source unchecked on any of the account's own characters is left out too (0.3.6). A
spell whose chosen copy is of a source that is off is not listed (its other copies stay hidden, as in the builder).
`forRuleset` (`dnd2014` / `dnd2024`): only the spells a new pick of a character of that ruleset may take (the
engine's edition rule: a 2014 pick takes no 2024 spells, a 2024 one may take 2014 ones). `alsoIn` keeps only copies
of enabled sources, and `alsoInSources` names their books once each, without the chosen copy's own.

`GET /api/spells/{key}` adds `mechanics: { def, origin: def|link|srd, fromKey, rev } | null`, `suggested: { mech,
checks[] } | null` (a draft from the user's own 5etools data, never bound by itself: saving it is a `def` with origin
`suggested`) and `sourceHash` (keep it on the saved def to flag a changed text).

## The DM's spell rules

| Route | What |
|---|---|
| `GET /api/spell-mechanics` | every row of the account |
| `PUT /api/spell-mechanics` | save one row (insert or replace) |
| `DELETE /api/spell-mechanics` | remove one: `{ kind, spellKey, ruleset, aliasName? }` |
| `GET /api/spell-mechanics/export` | the rows as a file (`spell-rules.borealis-spells.json`) |
| `POST /api/spell-mechanics/import?mode=merge\|replace` | a file back in; every row is checked first, nothing is written when one doesn't fit |

A row: `{ kind, spellKey, ruleset (dnd2014|dnd2024|any), aliasName?, def?, schemaV, rev, origin (dm|suggested|import),
sourceHash?, updatedAt }`. A `def` must be the engine's `SpellMech` for the row's edition (closed schema, no scripting,
at most 64 KiB); it is stored under the row's key. `rev` moves on every save. A spell is defined or linked, never both
(saving one replaces the other). At most 5,000 rows per account. The file: `{ format: "borealis-party-spell-mechanics",
v: 1, exportedAt, rows }`; imported rows get origin `import` (or keep `suggested`).

## Precast spells (encounters)

They replace "Starting effect"; legacy effects stay as "Custom effect" (and now spend their `slotLevel` too, from their
source, else the creature they are on).

| Route | What |
|---|---|
| `POST /api/encounters/{e}/casts` | `{ casterId, spellKey, slotLevel?, targets?: [creatureId \| "self"], choices? }` |
| `PATCH /api/encounters/{e}/casts/{cast}` | the same fields, each optional |
| `DELETE /api/encounters/{e}/casts/{cast}` | |

DM only (the encounter's 404 for anyone else). The spell must be installed in the DM's content and of the party's
edition (or `any`); `slotLevel` is 1 to 9, never below the spell's level, none for a cantrip; at most 20 targets and 40
precasts. The answer is the DM view, which lists `casts`; `castWarning` says when a caster now precasts two
concentration spells. Players' projections never list them.

When combat starts, each precast is cast with its real effect (compiled from the DM's mechanics, like a live cast) and
spends its slot. A precast that can't be cast (no such slot left, the spell gone) is skipped and logged.

## Casting in combat

All through `POST /api/combats/{combat}/{action}` with `{ requestId, expectSeq?, args }`. Every spell action is the
DM's except `resolve_cast_row`; a player gets the one generic 404 for the others, whatever the body.

| Action | Args (engine command, plus what the server adds) |
|---|---|
| `cast_spell` | `{ caster, ability?, spellKey?, slot?, targets, rows?, amounts?, crit_amounts?, choices?, force?, spec? }` |
| `resolve_cast_row` | `{ cast, target, ray?, roll: typed\|app\|outcome, use_lr? }` (a player: their own PC's row only) |
| `apply_cast` | `{ cast, amounts?, crit_amounts?, force? }` |
| `cancel_cast` | `{ cast, refund? }` |
| `apply_zone` | `{ effect, targets, rows?, amounts? }` |
| `perform_statblock` | `{ id, ability, spec?, targets, rows?, amounts?, ... }` |
| `standard_action` | `{ id, action: dodge\|dash\|disengage\|help, target?, bonus? }` |
| `bind_to_concentration` | `{ id }` |

For `cast_spell` the server compiles the definition: `spellKey` (or the ability's linked spell) is resolved through the
DM's mechanics (def, link, curated row, on any copy of the spell) and the caster's first spellcasting block (DC,
attack, modifier, cantrip tier); with none, the ability's own template, else a ruling (it spends the slot, takes
concentration from the element, and the DM rules). A DM's own `spec` is used as sent (the engine's closed schema). The
server always sets a random cast id (`apply_zone`'s `id`, `perform_statblock`'s `cast_id` too) and drops a client's
`precast`. Economy, slots, targets and the edition rules are the engine's checks (`force` overrides them, never a
missing slot); a refusal is 422 `refused`.

The DM's answer carries `notices`: a summon spell's `summon_options` (caster, spell, slot, the options, and whether the
caster concentrates) and `ruling_needed` questions. **Summons never spawn**: the toast's button opens Add creature.

`add_creature` takes `controlledBy` (a combatant id: the conjured creatures' owner; a PC's creature is its player's)
and `bindConcentration` (dismissed when the owner's current concentration ends; 422 when it concentrates on nothing).

### Players

A player's view has `casts`: the save rows of their own PCs only, `{ cast, target, ray, ability, dc, resolved }`, with
`spell` and `from` only while the caster is visible ("DEX save DC 15 from an unseen creature"). They answer with
`resolve_cast_row` (typed first: their own dice).

## Monster spellcasting

A 5etools import keeps each creature's raw `spellcasting` blocks (`monster_meta` extras, `meta.spellcasting` in
`GET /api/bestiary/monsters/{id}`). Adding the creature to an encounter copies them into its `automation_json`
(`{ v: 1, fivetools: { spellcasting } }`), and combat start reads them when the printed text gives no spell lists,
along with the DM's override spells; each unlinked spell is linked to the DM's chosen copy of that name.
