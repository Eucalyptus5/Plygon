# belief_fixtures_r2 — provenance

Not committed (108 MB); `.gitignore` holds the directory. The recorder is deterministic in its seed,
so the corpus regenerates byte-for-byte from:

```
node tools/belief_record.js --count 300 --seed 8301 --out poke_mcts/data/belief_fixtures_r2
node tools/belief_record.js --count 1 --seed 8301 --bias de --out poke_mcts/data/belief_fixtures_r2
```

300 unbiased fixtures (`8301-0` .. `8301-311`, 312 battles run) plus the 10 `de_*` scripted profiles.
Recorded with the checked-in randbats set table, so the generated true sets come from the same data
the belief candidate pool is built from (`tools/tests/belief-record-sets.test.js` guards this).
