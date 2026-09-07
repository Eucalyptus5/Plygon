# pkmn-fuzzer

Differential fuzzer for the Gen-9 engine. Drives Pokemon Showdown and
the Rust engine on identical scenarios, runs `compare_results.js`, classifies
outcomes, minimizes divergent scenarios, and routes findings into a
date-sharded `inbox/` tree.

This README documents operator-facing knobs and the cross-module
vocabulary. Implementation details for individual runtime modules live in
the modules themselves.

## CLI

```
node fuzz.js [--fixture PATH] [--max-scenarios N] [--seed HEXOR INT]
```

| Flag | Purpose |
|---|---|
| `--fixture PATH` | Run a single deterministic scenario from `PATH`. Used by `tests/plumbing_burn.json` for module-local plumbing verification. Bypasses the generator. |
| `--max-scenarios N` | Stop after N scenarios. Each scenario writes one line to `stats.jsonl`. |
| `--seed VALUE` | Seed the PRNG. Accepts decimal or `0x`-prefixed hex. **Determinism contract**: two consecutive invocations with the same `--seed` and `--max-scenarios` MUST produce byte-identical `stats.jsonl` outputs. Failure means the seeding path is not fully deterministic and is a hard bug, not a calibration issue. |

`stats.jsonl` line shape: `{seed, turns, result, duration_ms, oracle_coverage}`,
one line per scenario, parseable as JSON.

### Environment variables

| Var | Purpose |
|---|---|
| `FUZZ_USE_SERVER_MODE` | `1` forces server mode on, `0` forces legacy `spawnSync`. Overrides `config.json:use_server_mode` (default `true` since 2026-05-05). |
| `POOL_HEARTBEAT_MS` | Enables `subprocess_pool.js` heartbeat instrumentation; `0` disables (default). Recommended `5000` for triage. |
| `POOL_HEARTBEAT_STALL_MS` | Stall threshold for heartbeat output; default `5000`. Emits `pool=<label> waiting_on=<callId> elapsed=<ms> queue_depth=<n> lock_holder=<id>` to stderr per stuck call, throttled. |

## Locating Pokemon Showdown

The oracle is a Pokemon Showdown checkout that has been built (`node build` inside it, which
produces `dist/sim`). Showdown is not a declared dependency; `harness/lib/showdown_dir.js`
resolves it in this order and returns the first candidate that contains `dist/sim`:

1. `SHOWDOWN_DIR`, resolved against the current working directory.
2. `showdown_dir` in `fuzzer/config.json`, resolved against the config file's directory so a
   relative value survives a `cd`.
3. `<conformance>/../pokemon-showdown`.
4. `<conformance>/../../pokemon-showdown`.

The first two sources are authoritative: if either is set and the directory it names has no
`dist/sim`, resolution throws instead of falling through to the conventional locations. When
neither is set and neither conventional location has a built checkout, the error names both
paths tried.

## Cross-module vocabulary

The single source of truth for every cross-module constant is
`./constants.js`. No runtime module redeclares any of these:

- `CLASSIFICATIONS` — result strings + suppression levels.
- `SIGNATURE_CATEGORIES` — every canonical category the path-strip rule
  produces (32 entries; see `constants.js`).
- `REQUIRED_CATEGORIES` — the subset the coverage gate enforces (26
  entries; pinned in canonical path-tail namespace).
- `ORACLE_SCOPE_ALLOWLIST` — categories legitimately expected to be zero.
  See `oracle_scope.md` for prose + comparator-source citations.
- `ACTION_STRUGGLE = 255` — the dispatcher's Struggle byte.
- `path_to_category(path, showdown?, engine?)` — the canonical strip
  helper (turns + state_after / roll prefix, side, numeric team index).

`config.json` also carries a `required_categories` array identical to
`REQUIRED_CATEGORIES`. Nothing reads it and nothing checks the two
against each other, so treat `constants.js` as the source of truth and
expect the config copy to drift.

## Semantic name → canonical category mapping

External docs sometimes refer to category axes by
semantic names. The fuzzer's runtime code keys on the canonical
path-tail strings above. Translate as follows:

| Semantic name | Canonical `SIGNATURE_CATEGORIES` string(s) |
|---|---|
| `hp_decreased` | `team.current_hp_decreased` |
| `hp_increased` | `team.current_hp_increased` |
| `status_changed` | `team.status` |
| `fainted` | `team.is_fainted` |
| `boost_changed` | `active.boosts.atk`, `.def`, `.spa`, `.spd`, `.spe`, `.accuracy`, `.evasion` (seven tails, union) |
| `field_changed` | `field.weather`, `field.terrain` (union) |
| `side_condition_changed` | `side_conditions.stealth_rock`, `.spikes`, `.toxic_spikes`, `.sticky_web` (union) |
| `screen_toggled` | `side_conditions.reflect_turns`, `.light_screen_turns`, `.aurora_veil_turns` (union) |
| `item_changed` | `active.effective_item_id`, `team.item_id` (required category) |
| `ability_changed` | `active.effective_ability_id`, `team.ability_id` (required category) |
| `tera_changed` | `active.is_terastallized` (required category) |
| `types_changed` | `active.types` (required category) |

`active.effective_species_id` is in `SIGNATURE_CATEGORIES` but
deliberately NOT in `REQUIRED_CATEGORIES` — Transform/Illusion divergences
are rare and would trip the gate spuriously. Keep this asymmetry; it is
not an oversight.

## Generator skip lists, ban statuses, sampling rules

(Owned by the generator; surfaced here so operators know what is
and isn't sampled.)

- **Banned entities** are filtered from the sampling pool itself, never
  rejection-sampled. The generator never emits a banned species, ability,
  item, or move. This is an invariant.
- **Skip lists** at day 1 cover known-broken Gen-9 axes: Ogerpon formes
  are skipped entirely if the forme reverse map (`../id_maps/engine_forme_to_showdown.json`) is absent (no
  base-plus-mask emit fallback). Ubers-orb auto-transformers (Red Orb,
  Blue Orb, Griseous Orb, Rusted Sword, Rusted Shield) are skipped until
  Ubers expansion.
- **Distinct-engine-ID sampling rule.** When a Showdown entity collapses
  to multiple engine IDs (formes), the generator samples a distinct
  engine ID rather than the Showdown name. This keeps the engine-ID
  surface uniformly covered instead of being weighted by Showdown's
  forme-collapse rules.
- **Same-species opposing sides.** The toggle
  `same_species_opposing_sides_ban` defaults to `false` so mirror
  matchups are covered. Operators can flip it to `true` as an escape
  valve, never a default ban.
- **Action bytes.** Generator emits only the dispatcher-legal set
  `{0..10, 255}`. Anything else would be silently coerced to `'default'`
  by `showdown_runner.js:296` — that is an invariant.

## Oracle-coverage gate mechanics

The coverage gate runs **per-category** (never aggregated). Over a
sliding window of `window_size` scenarios, every category in
`REQUIRED_CATEGORIES` must have been **checked** at least once. "Checked"
means `path_to_category` resolved a path the comparator inspected — NOT
that the comparator emitted a diff. On PASS the diff list is empty; if
the gate keyed on diffs it would trip on every healthy window. The
counter is `categories_checked(sd_snapshot, eng_snapshot)`.

### Bootstrap period

The gate does NOT run before `window_size * window_multiplier` scenarios
have accumulated. With defaults (`window_size: 1000`,
`window_multiplier: 10`), the bootstrap is 10,000 scenarios. The bootstrap
exists because early generator output is biased by skip lists that haven't
yet rotated through their full pool. Tripping the gate during bootstrap
would manufacture spurious "coverage gap" findings. The driver counts
scenarios silently during bootstrap and only begins enforcing afterward.

### What the gate routes to

A category that has been silent for a full window after bootstrap is
written to `inbox/splash/<date>/<category>/<hash>/` (NOT `inbox/bugs/`)
— it is a coverage-machinery signal, not an engine-vs-Showdown
divergence. The corrective action is generator-side: tighten the
sampling distribution or revisit the category's skip list, not loosen
the gate's tolerance.

## Minimizer wallclock budget

The minimizer runs against a wallclock budget of
`minimize_budget_seconds` (default 15 s) per divergent scenario. When the
budget exhausts mid-shrink, the minimizer returns the best scenario found
so far with `minimize_verified: false`.

Note that minimizer output is **not** stable across re-runs at the
default 15 s budget on faster vs slower oracle implementations: when the
fixed-point loop runs longer than the budget, the return path switches
between `bestScenario` (wallclock-trip) and the original scenario
(post-check rejected), so two runtimes that produce semantically identical
oracle responses can return byte-distinct minimized scenarios. The 15 s
default is a triage-UX setting, not a parity-testing setting. To
reproduce a minimized scenario byte-exactly — for parity testing or
regression tracking — re-run with a generous budget (≥ 60 s) so the
fixed-point loop terminates on convergence rather than wallclock; or
read `minimize_verified: false` in `metadata.json` as the signal that
the budget tripped and the shrunk scenario is best-effort, not stable.

End-of-minimization correctness is checked **once**, not per shrink step:
the post-shrink scenario is re-run; if its signature is empty the shrink
is rejected and the pre-shrink scenario is kept (invariant).

The minimizer never mutates level / EV / IV / nature, and never
substitutes items (CLEAR only). This pins the divergence axis: an item
substitution would change the bug's repro shape and obscure the original
divergence's signal.

Each pass runs the five shrink steps in this fixed order, then repeats
until a pass accepts nothing:

1. repeated halving of the turn list, with a linear pop-from-tail finish,
2. back-to-front removal of team mons no surviving turn switches to,
3. per-slot move replacement with Splash,
4. per-slot item CLEAR,
5. `state_overrides` per-entry stripping.

Step 3 is the only step that consumes `turn_results`. The loop re-runs
the oracle after every accepted step, so each step measures against the
scenario the previous steps left behind.

Step 3 compares signatures modulo the replaced slot's
`effective_move_id_if_moved` component, but any other addition to or
removal from the category set rejects the shrink.

The minimizer never spawns a process. The driver injects a `runOracle`
callback that is the sole engine + Showdown + comparator entry point.

## Inbox layout

Findings land in a four-bucket date-sharded tree:

```
inbox/
├── bugs/<date>/<category>/<hash>/         # confirmed engine-vs-Showdown divergence
├── suppressed/<date>/<category>/<hash>/   # high-confidence-suppressed bug (already known)
├── rejected/<date>/<category>/<hash>/     # invalid choice / harness rejection
└── splash/<date>/<category>/<hash>/       # coverage-gate trip; not a divergence
```

Per-finding directory contents (driver- and minimizer-owned):

- `scenario.json` — the (minimized) scenario.
- `pre_minimize_scenario.json` — the original generator output (only if
  minimization shrank it).
- `signature.json` — the canonical category signature.
- `metadata.json` — classification, `minimize_verified`,
  `first_divergent_turn_preserved`, suppression `level` + `bug_ids`,
  timestamps.
- `showdown_out.json` / `engine_out.json` — raw harness outputs.
- `compare_results.json` — comparator output (for `bugs/`,
  `suppressed/`, and any `rejected/` case where the comparator did run).

The `rejected/` category enumeration is fixed: `harness_health`,
`trapped_switch`, `choice_locked`, `fainted_switch`, `tera_double`,
`disable_taunt_encore`, `engine_error`, `engine_crash`, `other`.

## Known divergences

`known_divergences.json` maps an engine entity id to the list of opaque
divergence ids attributed to it, under one key per axis (`abilities`,
`items`, `moves`, `species`). When every entity named by a minimized
finding is attributed, that finding is filed under `inbox/suppressed/`
instead of `inbox/bugs/`. The ids are opaque to the fuzzer — regenerate
the file from your own ledger, or edit it by hand.

Splash (engine move 150) is stripped from the file when it is generated,
so a finding whose minimized signature keys on Splash can never be
suppressed by the move axis.

The suppressor also guards move id 0 and item id 0, the empty-slot
fillers. Item 0 is never emitted into the file, but move 0 *is* bound
there; without the guard it would match nearly every scenario and
suppress real findings.

## Calibration criteria

The initial release ships with a deliberately conservative oracle and signature
vocabulary. The week-1 calibration loop should NOT widen suppression
confidence rules to silence noisy categories — that paper-overs the
signal. Instead, calibration tightens the **signature vocabulary**:

- If a category produces high false-positive volume, the corrective
  action is splitting that category into more specific tails (e.g. if
  `team.status` is noisy, split into per-status-letter sub-tails) — NOT
  bumping its suppression confidence.
- If two distinct bugs collapse onto the same signature, split the
  signature axis until they separate.
- If `splash/` is filling up faster than `bugs/`, the generator's
  sampling is biased — fix the sampling, not the gate.

Perf budget: 1,000 scenarios in under 3 minutes nominal under server
mode. Measured 2026-05-05 on Darwin/Apple-Silicon, debug engine binary,
full pipeline (probe loop + comparator-spawnSync): server **35.7 / min**,
legacy `spawnSync` **4.8 / min** — see *Server-mode default* below for
the flip rationale and citations.

## Known debt

- `min_roll` coverage slice for the `calc_damage` oracle path.
- `stats.jsonl` 50 MB rotation / ring buffer.
- Comparator (`compare_results.js`) is still `spawnSync`-per-call; it's
  the new floor (~130 ms/scenario, ~27 % of server-mode wall time at
  200/min). Server-mode-ifying it would lift the post-pool ceiling but
  is not the current bottleneck.

## Server-mode default (flipped 2026-05-05)

`use_server_mode: true` is now the `config.json` default. Engine and
Showdown each run as one persistent subprocess that reads NDJSON
scenarios on stdin; the per-call cold-spawn floor (~250 ms in
`spawnSync` mode) is amortized across the run. Measured speedup on this
host (Darwin 25.2.0, Apple Silicon, debug engine binary, full fuzzer
pipeline including comparator-spawnSync): **legacy 4.8 / min →
server 35.7 / min, 7.4×**. Two consecutive 10 000-scenario soaks at
heartbeat-instrumented `POOL_HEARTBEAT_MS=5000` /
`POOL_HEARTBEAT_STALL_MS=5000` ran clean (0 heartbeat stalls,
0 pool deaths, 0 uncaught events, 0 % harness_error). The legacy
`spawnSync` path is still available via `FUZZ_USE_SERVER_MODE=0` —
do not delete it. Heartbeat instrumentation is
**off by default in production** (no overhead unless the env vars are
set); enable it for triage with `POOL_HEARTBEAT_MS=5000`. One earlier
soak hit a single 39-minute scenario hang at seed 1601 that did not
reproduce in standalone or in either subsequent 10 k soak; the
underlying mechanism remains unidentified, but a fuzz.js main-loop
short-circuit on `pool.shuttingDown || !pool.child.stdin.writable`
caps the blast radius — if a similar transient ever recurs, the loop
exits within ≤ 2 trailing harness_error rows instead of cascading.
