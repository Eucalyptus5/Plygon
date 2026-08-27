# belief_fixtures_h1 — provenance

The HELD-OUT belief corpus. Not committed (377 MB); `.gitignore` holds the directory. Seed 8302,
disjoint from the seed-8301 corpus the belief fixes were developed against, so a soundness reading on
it is out of sample. The recorder is deterministic in its seed, so it regenerates byte-for-byte from:

```
node tools/belief_record.js --count 1000 --seed 8302 --out poke_mcts/data/belief_fixtures_h1
node tools/belief_record.js --count 1 --seed 8302 --bias de --out poke_mcts/data/belief_fixtures_h1
```

1,000 unbiased fixtures (`8302-0` .. `8302-1051`, 1,052 battles run) plus the 10 `de_*` scripted
profiles. Recorded with the checked-in randbats set table, like `belief_fixtures_r2`. The 10 `de_*`
files are also the `de_*` half of the committed `belief_fixtures_gate` corpus.
