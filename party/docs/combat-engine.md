# Combat engine (`party-dice`, `party-combat`)

Two pure Rust crates for Borealis Party combat. They have no server, database or UI code and do no I/O. The combat
server, the DM tracker and the player Combat sub-tab are built on them. The rustdoc on every type is the
reference; this page gives the shape.

## `party-dice`

A port of `ui/src/lib/dice.ts`: the same notation, breakdown text and error messages.

| API | What it does |
|---|---|
| `parse_notation("2d6+3")`, `damage_dice("1d8+3 slashing + 1d6 fire")` | Parse dice notation, or the dice of a printed damage text |
| `roll_expr(expr, die, opts)` | Roll with a die floor (`opts.floor`) and a label |
| `critical(expr)` | Double the dice, not the modifiers |
| `roll_d20(modifier, mode, die, crit_range)` | A d20 test with advantage or disadvantage |
| `D20Input::{Typed, Roll}`, `resolve_d20` | **A typed result from physical dice, or a roll (D43)** |
| `DiceInput::{Typed, Roll}`, `resolve_dice` | The same for damage, healing and recharge dice |
| `DieRoller` | The injectable die (closures work too) |
| `SeededDie` | mulberry32, serializable; the same rolls as the UI's `seededDie` for the same seed |
| `SequenceDie` | Scripted rolls for tests |

A typed result never touches the die. Inputs serialize with a `kind` tag:
`{"kind":"typed","total":15,"natural":12}` or `{"kind":"roll","modifier":2,"mode":"advantage"}`.

## `party-combat`

### The flow

```rust
let mut combat = Combat::new(Edition::Srd521, seed);   // the server picks the seed (D21)
let events = combat.execute(&Actor::Dm, &Command::Start)?;
// events.dm      -> the DM-only log and the DM's live view
// events.players -> safe for players: ids of visible combatants and PCs only, never a creature's numbers
combat.undo(&Actor::Dm, false)?;                        // C8: back to the exact previous state
```

- `apply(&state, &actor, &cmd, &mut die) -> Result<Outcome { state, events }, CombatError>` is the pure core.
  `Combat` wraps it with the die, the undo journal (50 steps) and the log.
- A refused command returns `Err` and changes nothing.
- `Combat` serializes whole (state, die, journal, log, `seq`), so the server can store it as one JSON value and
  use `seq` for CAS.

### State (`CombatState`)

| Field | Meaning |
|---|---|
| `edition` | `"2014"` (SRD 5.1) or `"2024"` (SRD 5.2.1) |
| `settings` | `show_enemy_turn` (D5 opt-in) and `auto_recharge` (D21; off means the DM types the d6) |
| `status`, `round`, `current` | `setup` / `active` / `ended`; whose turn it is |
| `last_visible` | The last turn players saw (D5) |
| `combatants` | In the order they were added. Removed ones stay as tombstones (`removed: true`) and are never shown |
| `effects` | Active effects and conditions (a condition is an effect that gives it) |
| `legendary_pools` | Pools by id, each with `members` (several creatures can share one, C16) |
| `rules` | Encounter rules (C20, D42) |
| `group_limits` | Per-round limits by group tag (C21) |

A `Combatant` holds:
- **identity:** `id`; `kind` (`creature`, or `character` with an `owner` account); `label` (DM only); `public_name`;
  `visible` / `ever_revealed`;
- **initiative:** `initiative`, `initiative_bonus`, `initiative_advantage`, `tiebreak`, `absent` (C1),
  `linked_to` (C3), `surprised`;
- **HP:** `hp {current, max, temp}`; `hp_authority` (`engine` or `player`, D7); `zero_hp` (`dies` or
  `death_saves`); `life` (`alive`, `dying {successes, failures}`, `stable`, `dead`);
- **combat stats:** `ac`; `speed`; `defenses` (damage clauses shaped like upstream's `DamageClause`, plus condition
  immunities); `exhaustion`;
- **concentration:** `concentration`, `pending_concentration`;
- **resources:** `economy` (C5); `abilities` (C14: `kind`, `recharge`, `uses`, `legendary_cost`, `group_limit`,
  `requires_effect`); `legendary_resistance` (C17); `spell_slots`; `resources` (C18);
- **other:** `group` (C21) and `notes`.

Build them with `Combatant::creature(id, label, hp, ac, init_bonus)` or
`Combatant::character(id, owner, name, hp, ac, init_bonus)`. Ids come from the server (random UUIDs).

### Commands (`Command`, tagged `"command"`)

| Area | Commands |
|---|---|
| Roster (C1, C7) | `add_combatant {combatant, roll_initiative}`, `remove_combatant`, `rename`, `set_public_name`, `reveal`, `hide`, `set_absent`, `set_notes`, `set_settings` |
| Initiative and turns (C2–C4) | `roll_initiative {ids, only_missing}`, `set_initiative {id, roll: D20Input}`, `clear_initiative`, `swap_tie`, `link {id, to}`, `start`, `next_turn`, `previous_turn`, `end` |
| Economy (C5) | `set_economy {id, action?, bonus_action?, reaction?}` |
| HP (C6) | `damage {target, parts: [{amount: DiceInput, damage_type?}], magical, apply_qualified, critical_hit}`, `heal`, `set_hp`, `grant_temp_hp`, `sync_player_hp {id, current, max, temp, damage?}` |
| Death saves (D18) | `death_save {id, roll}`, `stabilize`, `set_death_saves` |
| Conditions (C10) | `add_condition {effect}`, `remove_condition {target, condition}`, `set_exhaustion` |
| Concentration (C11) | `set_concentration`, `end_concentration`, `resolve_concentration {id, roll}` |
| Effects (C12, C13) | `add_effect {effect: EffectSpec}`, `remove_effect {effect, skip_follow_up}`, `use_charge`, `resolve_effect_save {effect, roll}` |
| Abilities (C14–C18) | `use_ability {id, ability, slot?}`, `roll_recharge {id, ability, roll: DiceInput}`, `set_recharge`, `set_legendary_pool`, `adjust_legendary`, `adjust_legendary_resistance`, `adjust_slot`, `set_slots`, `adjust_resource` |
| Rules (C20, C21) | `set_rule`, `set_rule_active`, `remove_rule`, `set_group_limit` |

**Who may issue what.**
- `Actor::Dm` and `Actor::System` may issue anything.
- `Actor::Player { account }` may issue only `set_initiative`, `death_save`, `set_concentration`,
  `end_concentration` and `resolve_concentration`, and only for a PC whose `owner` is that account. The server
  still checks membership in the same transaction.
- Undo is DM-only. If the last write was a player's, undo needs `force`.

### The rules, in short
- **Turn order:**
  - initiative descending, then `initiative_bonus` descending, then `tiebreak` (the DM can `swap_tie`);
  - a linked follower acts right after its leader, at the leader's initiative, and unlinks when the leader goes
    down (C3);
  - `next_turn` skips the dead and the removed, and steps from the active combatant's own place, so a defeated
    active creature never restarts the round (C4);
  - a 2014 surprised creature has no actions on its round 1 turn; under 2024, surprise means disadvantage on
    initiative.
- **Turn start:**
  - the economy resets, and so do resources with `reset: turn_start`;
  - a legendary pool refills at the start of the turn of its first member that isn't defeated;
  - recharges are rolled (auto) or reminded (`recharge_due`);
  - effect durations and repeated saves at `start` are counted.
- **Turn end:** durations and saves at `end` are counted. **Round start:** group limits and `round_start`
  resources reset.
- **Damage:**
  - per part: immunity makes it 0; otherwise resistance halves it (rounded down), then vulnerability doubles it;
  - a clause with "nonmagical" in its text applies unless `magical` is set; other qualified clauses apply only
    with `apply_qualified`, and every qualified clause's text comes back for the DM (`ClauseNote`);
  - then temp HP, then HP. For a PC (`hp_authority: player`) nothing changes: the player gets a `damage_prompt`,
    and the server reports the result back with `sync_player_hp {damage}`.
- **0 HP:**
  - `dies` means dead;
  - `death_saves` means dying, with an `unconscious` "Dying" effect;
  - damage at 0 HP is one failure, two on a crit;
  - damage at least max HP past 0 kills outright.
- **Death saves:** a natural 20 gives 1 HP; a natural 1 is two failures; 10 or more is a success; three successes
  mean stable, three failures mean dead. A typed total without its natural counts 20 as a natural 20 and 1 as a
  natural 1.
- **Concentration:**
  - the DC is max(10, half the damage), capped at 30 under 2024;
  - it ends at 0 HP, on an incapacitating condition, on a failed save, or when the creature starts concentrating
    on something else;
  - ending it ends every `concentration: true` effect it holds, and their follow-ups are added.
- **Effects:**
  - `duration` is `manual`, `source_turns {remaining, at}` or `target_turns {remaining, at}`;
  - `charges` end the effect when spent; `repeat_save` ends it on a success; `ends_on_defeat_of` ends it when that
    combatant goes down (C22: a mount's cover);
  - `follow_up` is added when the effect ends on its own;
  - `modifiers`: `ac`, `speed_add`, `speed_multiply`, `speed_halve`, `speed_zero` and `cover` (the best cover
    counts). `derive::effective_ac` and `derive::effective_speed` give the base and the effective values.
- **Abilities:** `use_ability` checks, then spends, the economy, the recharge, the resource, the slot, the group
  limit and the legendary pool. The refusal reason is an `Unavailable` ("Needs recharge", "Bonus action spent",
  "Requires X active", "Switched off by Y", …). `derive::ability_status` tells the UI the same without spending.
- **Encounter rules:** a banner (`name`, `text_active`, `text_ended`) plus ability flags. A rule tied to a combatant
  ends for good when it goes down. It is not a rules engine (D42).

### Events
- `DmEvent { round, subject, event, text }`:
  - `event` is a `DmEventKind`, for example `damage {result, prompted}`, `death_save {roll}`,
    `concentration_check {dc}`, `recharge {roll, ready}`, `hidden {was_seen}` or `undone`;
  - `text` is the log line;
  - DM events name hidden creatures, so **they never reach players**. `Combat::log` keeps them (D32: the server
    deletes an ended combat's log after `LOG_RETENTION_DAYS` = 90 days).
- `PlayerEvent`:
  - `started`, `ended`, `turn {pointer}`, `order`, `appeared {id}`, `disappeared {id}`, `changed {id}`;
  - `damage_prompt`, `heal_prompt`, `concentration_check`, `death_save_due` and `your_turn`, each with the PC's
    `owner`.
  - They are derived by comparing what players could see before and after the command. **A DM-only write (anything
    about a hidden creature, labels, notes, legendary pools…) produces no player events.**

### Visibility (for part D's `view_for`)
- `visible_order(&state, ViewerKind::Player)`: the turn order without hidden or removed creatures.
- `turn_pointer(&state, ViewerKind::Player)` gives one of:
  - `combatant {id, round}`: the current combatant when visible. While hidden creatures act, it stays on the last
    visible turn, with **that turn's** round, so players can't count hidden turns (D5);
  - `enemy_turn {round}`: only with `settings.show_enemy_turn`;
  - `waiting {round}`, `not_started`, `ended`.
- `is_visible(&state, id, viewer)`.
- **The projection must not serialize:** `label`, `notes`, `tiebreak`, effect ids, `next_effect`, raw positions,
  or an effect's `source` when that source is hidden.

### Export (D45)
- `export_json(&combat)` writes `{format: "borealis-party-combat", schema_version: 1, state, log}`. It is DM only.
- `import_json(text, seed)` makes a **new** combat: a fresh die, an empty journal and `seq` 0. It refuses:
  - a newer schema;
  - another format;
  - a file over 8 MiB;
  - a state whose references don't resolve.
- Older schemas go through `export::migrate`.

### Not in the engine
- The offline buffer (D44) belongs to the server and the UI.
- The player projection belongs to part D.
- Stat-block parsing into `abilities` belongs to the server or the builder.
- Lair actions on initiative 20, mythic actions and automatic attack rolls (0.5.1) are not modelled yet.
