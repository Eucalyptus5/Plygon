# Oracle scope allowlist

This document is **prose documentation**. The runtime source of truth is
`conformance/fuzzer/constants.js` — the driver reads `ORACLE_SCOPE_ALLOWLIST`
from that module, NOT from this file. This file enumerates the same strings
in a designated fenced block below, with exact comparator-source line
citations.

## What "oracle scope" means

The fuzzer's coverage gate enforces that, over a window of scenarios, the
comparator was actually given the chance to assert on every category in
`REQUIRED_CATEGORIES`. Some category-axes are **legitimately expected to be
zero** in healthy traffic — the comparator inspects them, sees identical
state on both sides, and emits no diff. We must not let those silent paths
trigger the coverage gate. The whitelist below names them.

## Allowlist contents (cite-mapped to comparator source)

Source file references point at
`conformance/harness/compare_results.js` (the comparator the driver
spawns and parses). Each entry is a category string identical to a value
exported by `constants.js`'s `ORACLE_SCOPE_ALLOWLIST` set.

```text
pp_changed                  — compare_results.js compareSideHP body 95-119
                              (PP comparison is grouped with the per-mon HP /
                              status / item / ability comparisons; pp diff
                              emission lives in this region today and is the
                              forward-compatible slot for any future
                              `*.pp` path that path_to_category would map
                              to `pp_changed`).
active.substitute_hp        — compare_results.js compareActive 141-167
                              (sub-volatile block; substitute_hp emit).
active.confusion_turns      — compare_results.js compareActive 141-167
                              (sub-volatile block; confusion_turns emit).
active.taunt_turns          — compare_results.js compareActive 141-167
                              (sub-volatile block; taunt_turns emit).
active.encore_turns         — compare_results.js compareActive 141-167
                              (sub-volatile block; encore_turns emit).
```

## Why these and not others

`item_changed`, `ability_changed`, `tera_changed`, `types_changed` are
**in `REQUIRED_CATEGORIES`, not in the allowlist.**
Their canonical strings are `active.effective_item_id`,
`active.effective_ability_id`, `active.is_terastallized`, and
`active.types` respectively (with `team.item_id` / `team.ability_id`
covering the per-mon emit family). They are now first-class oracle axes,
not allowlisted-zero.

The aggregate label `volatile_changed` does NOT appear in the comparator
output and **must not** appear in this allowlist. Each sub-volatile is
individually tracked here (never aggregated under one "volatile" bucket).

## Manual review on unknown-category trip

If the driver's unknown-category fail-closed branch fires on a path whose
canonical category is not in `SIGNATURE_CATEGORIES`, the operator should:

1. Inspect the `compare_results.js` source line that emitted the path.
2. Decide whether the new axis belongs in `SIGNATURE_CATEGORIES`,
   `REQUIRED_CATEGORIES`, this allowlist, or stays as a true bug.
3. Promote into both `constants.js` (code) and this file (prose) before
   relying on it.

Until promoted, the path continues to route through the fail-closed
branch — no silent normalization.

