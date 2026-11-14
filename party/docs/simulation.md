# Encounter simulation (0.3.1, D26; spells 0.3.3)

A dry-run fight on a **copy** of an encounter with the party, for the party's DM. The combat engine
(`party_combat::apply` with one seeded die) produces every number; an AI only picks ids from engine-built menus.
Nothing is written: no row, no combat, no event (D25). Reports describe what happened; there is no XP, CR,
difficulty or rating anywhere (D16). A batch gives plain counts out of N.

The server never calls an AI model (0.3.3). The oracles are the MCP **Agent** (the client's own model, through
`sim_act`), **Basic** (the deterministic heuristic) and **Transcript** (replay).

Code: `party/crates/party-sim` (pure: setup, sheet, rules, space, menu, runner, basic, adjudicate, report, spells),
`borealis-server/src/simulate.rs` (sessions, SELECT-only loading, the tool table) and `simulate_tools.rs` (the
`party-tools` adapter).

## Tools (scope `encounters:simulate`, DM only)

| Tool | What it does |
|---|---|
| `sim_start {encounterId, seed?, opts?}` | Builds the copy (PCs `c1..`, then creatures), rolls initiative, returns `simId`, roster, coverage, assumptions, the first card. |
| `sim_card {simId}` | The open card (idempotent); `card: null` when it ended. |
| `sim_act {simId, step, d}` | Answers the card: `d = {o, t?, mv?, adj?, why?, memo?}` or up to 3 for one turn; `o` may be `repeat` or `basic`. A stale `step` returns the current card and changes nothing; an invalid answer returns `error` (ids only) and the same card. |
| `sim_auto {simId, step, until, who?}` | Basic plays until `turn`/`round`/`end`; `who` hands only those combatants to Basic. |
| `sim_state {simId}` | The unfogged board for the DM: HP, positions, conditions, concentration, slots, resources, the log. |
| `sim_report {simId}` | The report and transcript; ends the session. |
| `sim_batch {encounterId, runs?, seed?, opts?}` | N runs (default 30, at most 100), Basic only, 3 s budget (`truncated` if it runs out); counts out of N. |
| `sim_replay {encounterId, seed, opts?, transcript, fingerprints?}` | `{identical, report}` or the first divergence (a changed creature, PC or spell definition shows in `fingerprints`). |

`opts`: `pcHp` (`full`/`current`), `maxRounds` (1-20, default 10), `start {partyBand, foesBand, surprise}`,
`control {agent: "foes"|"party"|"all"|"none"|[ids]}`, `fog` (default true), `profile` (`small` skips text-only
options instead of adjudicating them; for clients on small models).

Sessions are in memory: 30 idle minutes, 2 per account, 600 steps. A server update loses them; the transcript
replays.

## Cards

`{step, round, window, actor{id, name, side, hp, conditions, engaged_with, economy, resources, int, type, memo},
seen[{id, name, side, steps_away, health, down, conditions, concentrating}], morale[...], recent[...],
options[{o, name, kind, cost, summary, targets, max_targets, mode_hint, slot?, repeat_targets?}], adjudicate?[{o,
numbers, text: {untrusted_stat_block_text}}]}`. With fog, enemies' health is `unhurt`/`bloodied`/`down`.

## Spells (0.3.3)

The sim casts with the combat tracker's own command and definitions:

- **Definitions.** The server reads the DM's mechanics for the party's edition once per setup (`spells::Library`:
  the account's definition or link, then the curated SRD row of `party-spells`) for every spell of the creatures'
  spellbooks (the printed block, the raw 5etools spellcasting kept in `automation_json`, the DM's added spells) and
  of the PCs' sheets, and the DM's chosen copy of each unlinked printed name. They travel in `SimSetup.spells`, so a
  replay never depends on later edits; each definition's revision is in the fingerprints (`spell:<key>` →
  `srd:<rev>`, `def:<id>:<rev>` or `ruling`).
- **Compile.** `party_sim::spells::compile` builds the caster's `CastSpec` like the tracker's server: the caster's
  first spellcasting block gives the DC, attack bonus, modifier and cantrip tier. PCs get their casting from the
  sheet's class DC and attack bonus.
- **Coverage.** Tier `auto` and `partial`: automated. `utility` (Light, Mage Hand): not simulated. `ruling` and
  uncurated: adjudicated (the cast still spends its slot, economy and concentration through the engine first).
  Summons give their options only; nothing spawns (as in the tracker). Shield-like reactions (`ac_bonus`, self,
  reaction) are cast by the runner when they turn a hit into a miss; other reaction spells are not simulated.
- **Menu.** One option per slot level the caster can use now: `s3` at the spell's level, `s3@4` with a 4th-level
  slot. Targets follow the definition (self, enemies, allies, creature types, areas within one band up to the
  area's size limit, rays that may repeat a target). Spells reach 60 feet (an assumption in every report).
- **Casting.** `CastSpell` with every save, attack and amount rolled in the app on the run's seeded die: one
  command. A save of a creature with Legendary Resistance left is rolled first (typed from the same die), so the
  policy can spend it on a failure that would apply a condition or cost half its HP; then `ApplyCast`.
- **Upkeep.** Concentration checks, repeated saves (Hold Person) and the end of concentration's effects come from
  the engine; saves use the engine's `save_mod` (Bless dice, Magic Resistance, automatic failures), attacks its
  `attack_mode` (`rules.rs` re-exports both).
- **Basic.** Expected value from the definition's primitives: damage and conditions through the save or attack
  chance, healing only for a dying ally (or itself at a quarter of its HP), pure buffs 0. A new concentration spell
  while concentrating is skipped; a higher slot costs a little, so Basic upcasts only when it pays. Slot spells
  count as limited uses (the 1.3× rule).
- **Drafts.** An adjudicated spell appears in the report's `drafts` as an effect-builder draft (tier `partial`,
  revision 0, "unreviewed"). The simulation never saves it; the DM checks and saves it in the builder.

## Realism model and limits

The engine's turn loop and action economy; bands of 30 ft on one line (same step = within 5 ft, engaged);
movement, Dash, Disengage, opportunity attacks; the engine's advantage table (conditions, ranged while engaged,
long range, prone, Dodge, Pack Tactics, spell effects, Magic Resistance, auto-crits, auto-failed STR/DEX saves);
save effects with one damage roll and half on a success, read from 2014 or 2024 text when `save` is empty;
multiattack phrasings; legendary actions in windows after other turns, each creature with its own pool; Legendary
Resistance; recharge; death saves; concentration; repeated saves; spells (above); morale (flee, surrender). Not
simulated: lair actions, reactions other than opportunity attacks and Shield-like spells, auras, zones, summons'
creatures, and class features (adjudicable). Coverage and assumptions are in every report.

## Spell read tools (scope `encounters:read`)

`spell_search`, `spell_get` and `creature_spellbook` (`borealis-server/src/spell_tools.rs`) read the caller's own
content: one copy per spell per edition, a spell's mechanics, tier and source (its text only as an
`untrusted_spell_text` data leaf), and a creature's spellbook (a bestiary creature, or the DM's encounter creature
with its overrides). They never read a combat.
