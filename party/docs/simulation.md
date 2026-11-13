# Encounter simulation (0.3.1, D26)

A dry-run fight on a **copy** of an encounter with the party, for the party's DM. The combat engine
(`party_combat::apply` with one seeded die) produces every number; an AI only picks ids from engine-built menus.
Nothing is written: no row, no combat, no event (D25). Reports describe what happened; there is no XP, CR,
difficulty or rating anywhere (D16). A batch gives plain counts out of N.

Code: `party/crates/party-sim` (pure: setup, sheet, rules, space, menu, runner, basic, adjudicate, report, oracle),
`borealis-server/src/simulate.rs` (sessions, SELECT-only loading, the tool table), `simulate_tools.rs` (the
`party-tools` adapter) and `simulate_ai.rs` (the bring-your-own model as oracle).

## Tools (scope `encounters:simulate`, DM only)

| Tool | What it does |
|---|---|
| `sim_start {encounterId, seed?, opts?}` | Builds the copy (PCs `c1..`, then creatures), rolls initiative, returns `simId`, roster, coverage, assumptions, the first card. |
| `sim_card {simId}` | The open card (idempotent); `card: null` when it ended. |
| `sim_act {simId, step, d}` | Answers the card: `d = {o, t?, mv?, adj?, why?, memo?}` or up to 3 for one turn; `o` may be `repeat` or `basic`. A stale `step` returns the current card and changes nothing; an invalid answer returns `error` (ids only) and the same card. |
| `sim_auto {simId, step, until, who?, oracle?}` | Basic plays until `turn`/`round`/`end`; `who` hands only those combatants to Basic; `oracle: "assistant"` lets the account's own hosted model play in the background. |
| `sim_state {simId}` | The unfogged board for the DM, the log, the assistant's progress. |
| `sim_report {simId}` | The report and transcript; ends the session. |
| `sim_batch {encounterId, runs?, seed?, opts?}` | N runs (default 30, at most 100), Basic only, 3 s budget (`truncated` if it runs out); counts out of N. |
| `sim_replay {encounterId, seed, opts?, transcript, fingerprints?}` | `{identical, report}` or the first divergence. |

`opts`: `pcHp` (`full`/`current`), `maxRounds` (1-20, default 10), `start {partyBand, foesBand, surprise}`,
`control {agent: "foes"|"party"|"all"|"none"|[ids]}`, `fog` (default true), `profile` (`small`/`frontier`).

Sessions are in memory: 30 idle minutes, 2 per account, 600 steps. A server update loses them; the transcript
replays.

## Cards

`{step, round, window, actor{id, name, side, hp, conditions, engaged_with, economy, resources, int, type, memo},
seen[{id, name, side, steps_away, health, down, conditions, concentrating}], morale[...], recent[...],
options[{o, name, kind, cost, summary, targets, max_targets, mode_hint}], adjudicate?[{o, numbers, text:
{untrusted_stat_block_text}}]}`. With fog, enemies' health is `unhurt`/`bloodied`/`down`. About 700 tokens; the
small profile sends a compact one (about 500).

## The assistant (BYO model)

One structured completion per card: a fixed system prompt plus the card, no history, no tools, temperature 0, the
answer forced to a schema built for that card (`o` an enum of its option ids, `t` of its targets). A refused
answer gets one repair; then Basic plays that turn (`fallback`); three fallbacks in a row hand the rest to Basic.
`small` (default for unknown models): no adjudication, compact cards. `frontier`: adjudication by index and a memo.
Only server-reachable (hosted) models play here; device-mode models (Ollama, LM Studio on the LAN) are not called
by the server.

## Realism model and limits

The engine's turn loop and action economy; bands of 30 ft on one line (same step = within 5 ft, engaged);
movement, Dash, Disengage, opportunity attacks; the advantage table (conditions, ranged while engaged, long range,
prone, Dodge, Pack Tactics, Magic Resistance, auto-crits, auto-failed STR/DEX saves); save effects with one damage
roll and half on a success, read from 2014 or 2024 text when `save` is empty; multiattack phrasings; legendary
actions in windows after other turns, each creature with its own pool; Legendary Resistance; recharge; death
saves; concentration; repeated saves; morale (flee, surrender). Not simulated in 0.3.1: lair actions, reactions
other than opportunity attacks, auras, summons, most spells and class features (adjudicable in `frontier`).
Coverage and assumptions are in every report.

## Merge wiring

- `tools_backend.rs`: `external_specs()` adds `simulate_tools::tools()`; `external()` sends `sim_*` to
  `simulate_tools::call_tool(state, user, tool, args)`. main.rs: `mod simulate_tools;`.
- main.rs: `mod simulate_ai;` and `simulate_ai::install();` at start-up (needs part C's `ai_assistant` and
  `ai_providers::complete_json`).
- Then the `#![allow(dead_code)]` in simulate.rs can go.

## Testing the small profile against a real model

`SIM_OLLAMA=http://127.0.0.1:11434 SIM_MODEL=qwen2.5:7b-instruct cargo test -p party-sim --test live_model -- --ignored --nocapture`
