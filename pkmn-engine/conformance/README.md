This tree holds a differential fuzzer and the test harness it drives: both run the engine and a Pokemon Showdown checkout on identical scenarios and diff the results. Showdown is located at runtime through `SHOWDOWN_DIR`, the fuzzer config, or a sibling checkout — see `fuzzer/README.md` for the resolution order.

# Commands
| Action | Command |
|--------|---------|
| Generate ID maps | `node conformance/harness/generate_maps.js` |
| Dump engine data | `conformance/harness/dump_data/target/debug/dump_data` |
| Run Showdown scenario | `node conformance/harness/showdown_runner.js < scenario.json` |
| Run engine scenario | `conformance/harness/run_scenario/target/debug/run_scenario < scenario.json` |
| Compare static data | `conformance/harness/dump_data/target/debug/dump_data \| node conformance/harness/compare_data.js` |
| Compare results | `node conformance/harness/compare_results.js showdown_out.json engine_out.json` |

# RNG Strategy
- Damage tests: run both engines for all 16 damage rolls (use `rng_mode: "all_rolls"` or loop with `specific` rolls 0-15) and compare the full set. Do NOT compare single rolls.
- Secondary effects / accuracy: `rng_mode: "force_all"` forces secondaries+accuracy on both engines. Use `rng_mode: "force_none"` to suppress.
- Crits: test crit and non-crit separately. Use `rng_overrides.crit: true/false` with `rng_mode: "specific"`.
- Never pass raw sequential integers as `sample_seed`. The engine's LCG is a
  truncated generator, so the low bit of the *first* draw has strong lattice
  structure across consecutive seeds (measured lag-3 P(equal) around 0.95). A
  50/50 mechanic read on the first draw would then be badly under-decorrelated.
  Mix the counter (splitmix64) before sending it. `adjacent_seeds_decorrelate`
  in `run_scenario` pins the two properties this relies on: the marginal first
  `rng(2)` is unbiased, and a multi-draw stream signature is lag-1 uncorrelated.

# ID Maps (conformance/id_maps/)
- Moves: Showdown `num` field = engine move ID
- Species: Showdown `num` field = engine species ID
- Items: Showdown `spritenum` field = engine item ID (index in GEN_ITEMS array)
- Abilities: Showdown `num` field = engine ability ID

# Scenario JSON Format

Both `showdown_runner.js` and `run_scenario` accept the same JSON on stdin.

## execute_turns mode
```json
{
  "name": "scenario-name",
  "mode": "execute_turns",
  "teams": {
    "p1": [{"species_id": 25, "ability_id": 9, "item_id": 0, "moves": [85, 150, 0, 0], "level": 50}],
    "p2": [{"species_id": 143, "ability_id": 47, "item_id": 0, "moves": [34, 150, 0, 0], "level": 50}]
  },
  "state_overrides": {
    "field": {"weather": 1, "terrain": 0},
    "p1": {"active_overrides": {"boosts": [2, 0, 0, 0, 0, 0, 0]}},
    "p2": {"side_conditions": {"stealth_rock": true, "spikes": 2}}
  },
  "turns": [
    {"p1_action": 0, "p2_action": 1, "rng_mode": "force_all"},
    {"p1_action": 0, "p2_action": 0, "rng_mode": "specific", "rng_overrides": {"damage_roll": 15, "crit": false}}
  ]
}
```

### `reject_resilient` (optional, default off)

Off, a Showdown reject re-throws and the runner emits
`success: false` with the reject message, and no other field is added.
On, the runner instead recovers by self-driving `default`/`default` from
the reject point and adds `winner`, `ended`, `reject_info` and
`terminal_via` to the result.

The default must stay byte-identical to the off case: consumers key on
`success: false` to detect a structural desync, so emitting the extra
fields unconditionally would change what every one of them sees.

## calc_damage mode
```json
{
  "name": "calc-tbolt-vs-gyarados",
  "mode": "calc_damage",
  "teams": {
    "p1": [{"species_id": 25, "ability_id": 9, "item_id": 0, "moves": [85, 0, 0, 0], "level": 50}],
    "p2": [{"species_id": 130, "ability_id": 22, "item_id": 0, "moves": [89, 0, 0, 0], "level": 50}]
  },
  "calc_damage_params": {"atk_side": 0, "move_id": 85, "include_crit": true}
}
```

## Mon fields
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| species_id | u16 | required | Showdown `num` |
| ability_id | u16 | required | Showdown `num` |
| item_id | u16 | 0 | Showdown `spritenum` (0=no item) |
| moves | [u16; 4] | required | Move `num` values (0=empty slot) |
| ivs | [u8; 6] | [31,31,31,31,31,31] | HP/Atk/Def/SpA/SpD/Spe |
| evs | [u8; 6] | [0,0,0,0,0,0] | HP/Atk/Def/SpA/SpD/Spe |
| nature | u8 | 0 | Engine nature ID (0=neutral). boosted=nature/5+1, reduced=nature%5+1 |
| level | u8 | 100 | 1-100 |
| tera_type | u8 | 0 | Type index (0=Normal, 9=Fire, 10=Water, ...) |
| is_female | bool | false | |

## Nature quick reference
| Nature | ID | +/- |
|--------|----|-----|
| Neutral | 0 | — |
| Adamant | 2 | +Atk/-SpA |
| Jolly | 22 | +Spe/-SpA |
| Timid | 20 | +Spe/-Atk |
| Modest | 10 | +SpA/-Atk |
| Bold | 5 | +Def/-Atk |
| Careful | 17 | +SpD/-SpA |
| Impish | 7 | +Def/-SpA |

## Actions
| Value | Meaning |
|-------|---------|
| 0-3 | Use move in slot 0-3 |
| 4-9 | Switch to team slot 0-5 |
| 10 | Terastallize + move slot 0 |

## RNG modes
| Mode | Damage | Crit | Secondary | Accuracy |
|------|--------|------|-----------|----------|
| force_all | max roll | no | yes | yes |
| force_none | max roll | no | no | yes |
| min_roll | min roll | no | yes | yes |
| max_roll | max roll | no | yes | yes |
| all_rolls | all 16 | no | yes | yes |
| specific | rng_overrides | rng_overrides | rng_overrides | rng_overrides |

## State overrides
| Field Path | Values |
|------------|--------|
| field.weather | 0=none, 1=sun, 2=rain, 3=sand, 4=snow |
| field.terrain | 0=none, 1=electric, 2=grassy, 3=psychic, 4=misty |
| p1/p2.active_overrides.boosts | [atk, def, spa, spd, spe, accuracy, evasion] — range -6 to +6 |
| p1/p2.active_overrides.status | 0=none, 1=burn, 2=paralysis, 3=poison, 4=bad_poison, 5=sleep, 6=freeze |
| p1/p2.side_conditions.spikes | 0-3 |
| p1/p2.side_conditions.stealth_rock | bool |

# CLI Binary Reference

## `run_scenario` — Engine scenario runner
Accepts scenario JSON on stdin, outputs result JSON on stdout.

**Real output example** (execute_turns, Pikachu Thunderbolt vs Snorlax Splash):
```json
{
  "name": "smoke-execute",
  "mode": "execute_turns",
  "success": true,
  "error": null,
  "initial_state": {
    "phase": "actions",
    "field": {"turn": 0, "weather": "none", "terrain": "none", ...},
    "p1": {
      "active_index": 0,
      "team": [{"slot": 0, "species_id": 25, "current_hp": 110, "max_hp": 110, "stats": [75,60,70,70,110], "status": "none", ...}],
      "active": {"boosts": [0,0,0,0,0,0,0], "volatile_flags": 0, ...},
      "side_conditions": {"spikes": 0, "stealth_rock": false, ...}
    },
    "p2": { ... }
  },
  "turns": [{
    "turn_number": 1,
    "state_after": { ... },
    "legal_actions_after": {"p1": [0,1], "p2": [0,1]}
  }]
}
```

## `dump_data` — Engine static data dumper
No stdin. Outputs all pkmn-engine static data as JSON to stdout.

**Output structure:**
```json
{
  "moves": [
    {"id": 85, "base_power": 90, "accuracy": 100, "category": "Special", "move_type": "Electric",
     "priority": 0, "pp": 15, "flags": 512, "flag_names": ["protect"], "crit_ratio": 0,
     "drain": 0, "multihit_lo": 0, "multihit_hi": 0, "secondary_chance": 10,
     "effect": "None", "var_power": "None", "target": "Normal"},
    ...
  ],
  "species": [
    {"id": 25, "hp": 35, "atk": 55, "def": 40, "spa": 50, "spd": 50, "spe": 90,
     "type1": "Electric", "type2": "Electric", "weight": 60},
    ...
  ],
  "items": [
    {"id": 68, "flags": 1, "flag_names": ["choice_atk"], "type_param": 255, ...},
    ...
  ],
  "type_chart": [[...], ...],
  "constants": {"total_moves": 1001, "total_species": 1454, "total_items": 762, "forme_offset": 1100}
}
```

# Common Pokemon IDs (for scenario construction)
| Pokemon | species_id | Common Ability (ability_id) |
|---------|-----------|---------------------------|
| Pikachu | 25 | Static (9) |
| Gyarados | 130 | Intimidate (22) |
| Snorlax | 143 | Immunity (17) / Thick Fat (47) |
| Garchomp | 445 | Rough Skin (24) |
| Blissey | 242 | Natural Cure (30) |
| Tyranitar | 248 | Sand Stream (45) |
| Ferrothorn | 598 | Iron Barbs (160) |
| Dragapult | 887 | Clear Body (29) |
| Corviknight | 823 | Pressure (46) / Mirror Armor (240) |
| Great Tusk | 984 | Protosynthesis (281) |

# Common Move IDs
| Move | num | BP | Type | Category |
|------|-----|-----|------|----------|
| Thunderbolt | 85 | 90 | Electric | Special |
| Earthquake | 89 | 100 | Ground | Physical |
| Splash | 150 | 0 | Normal | Status |
| Flamethrower | 53 | 90 | Fire | Special |
| Ice Beam | 58 | 90 | Ice | Special |
| Surf | 57 | 90 | Water | Special |
| Close Combat | 370 | 120 | Fighting | Physical |
| U-turn | 369 | 70 | Bug | Physical |
| Stealth Rock | 446 | 0 | Rock | Status |
| Swords Dance | 14 | 0 | Normal | Status |
| Dragon Dance | 349 | 0 | Dragon | Status |
| Body Slam | 34 | 85 | Normal | Physical |
| Knock Off | 282 | 65 | Dark | Physical |

# Common Item IDs (spritenum)
| Item | spritenum |
|------|-----------|
| Choice Band | 68 |
| Choice Specs | 69 |
| Choice Scarf | 70 |
| Life Orb | 247 |
| Leftovers | 234 |
| Focus Sash | 137 |
| Assault Vest | 581 |
| Rocky Helmet | 417 |
| Heavy-Duty Boots | 640 |
| Eviolite | 130 |

# Fuzzer scenario conventions
- Broad-random scope: any legal species/ability/move/item/nature/tera/level 1-100, no allowlist.
- Turns ≥2 use `legal_actions_after` from `execute_turns`; turn 1 uses `mode: "legal_actions"` against engine only.
  - `legal_actions` mode returns `response.both_sides: {p1: [u8], p2: [u8]}` with raw action bytes.
  - `legal_actions_params` is optional in `legal_actions` mode; absent → response emits `both_sides` only.
  - Consumers read `response.both_sides.p1` / `response.both_sides.p2`.
- Byte-255 dual-contract overload: noop vs Struggle, uniform sampling, empty-list → 255 rule.
- Banned configurations:
  - Same-side duplicate species forbidden.
  - Same-species opposing-sides ban is a default-off operator escape (`same_species_opposing_sides_ban`).
  - Species-mutating mons Zoroark/Zorua/Ditto/Imposter/Transform banned pending oracle fix.
  - Form-change items skipped.
- RNG mode distribution: 90% `force_all`, 10% `all_rolls`.
- Minimization signature rules: category-bucketed, set-preserved, post-shrink verified once-at-end with first-divergent-turn locality check.
- `showdown_rejected` is its own classification with dedicated inbox (not a PASS).
- Two-level suppression: `low_confidence` routes to main `inbox/bugs/`.
- Oracle-coverage is a hard per-category gate backed by `oracle_scope.md` as the expected-zero allowlist.
- Reverse-map path: `conformance/id_maps/engine_forme_to_showdown.json` (NOT `conformance/harness/id_maps/...`).
