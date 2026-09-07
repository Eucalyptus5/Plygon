# Benchmarks

## The headline number

One complete six-on-six battle, simulated start to finish, takes about **13.9 microseconds** on one core. That is roughly **72,000 battles per second**.

```
cargo bench --bench engine_benches -- "MCTS Simulation"
```

## What that actually measures

The number comes from a benchmark called `Rollout (Vanilla, 200 turns)` in [`pkmn-engine/benches/engine_benches.rs`](../pkmn-engine/benches/engine_benches.rs). It is worth being precise about what it does and does not include, because a battle-simulation benchmark can mean almost anything.

Both sides get the same six Pokemon. Slot one has 300 HP, the other five have 200 HP each. Everyone has four moves with 24 PP, at level 100. Nobody has an ability or a held item, and there is no weather, terrain, or entry hazard.

Each turn, both sides take the *first* legal action available to them. This is not a search and not even a heuristic. It is a fixed script, chosen so the benchmark measures the engine rather than a policy.

Randomness comes from a fixed linear congruential generator seeded at 12345, so every iteration replays exactly the same battle.

The "200 turns" in the name is a cap, not a turn count. The loop stops when the battle actually ends, and this matchup finishes well before turn 50. The 50-turn and 200-turn variants measure within about 0.3 microseconds of each other, which confirms the extra 150 turns are never simulated.

So the honest one-line version is: **13.9 microseconds to play one complete battle with no abilities, items, or field effects, taking the first legal action each turn.**

## A more realistic number

There is a second benchmark, `Rollout (Complex, 50 turns)`, which sets up an attack-boosting ability and a recoil item against a damage-reducing ability and a defensive item, with a burn, a stat boost, rain, and hazards on the field. It reads about **5.8 to 6.2 microseconds**, because the extra modifiers make the battle end sooner rather than because each turn is cheaper.

Neither number is "the" speed of the engine. They bracket it.

## Methodology

Benchmark numbers are easy to fake by accident, so here is how these were taken.

Every measurement is a **cold median** of three runs for a routine check, or five for anything that decides a design question. Each run is a fresh `cargo bench` process. Machine load is recorded alongside every run.

When two options are compared, the runs are **interleaved** (A, B, A, B, ...) rather than batched (A, A, A, then B, B, B). Batching lets a gradual change in machine load masquerade as a real difference.

If the spread between runs exceeds about 3 percent, the machine is too noisy and the measurement is repeated. It is never averaged across sessions. Drift of nearly 4 percent has been observed between sessions with no code change at all, so only same-session comparisons are treated as meaningful.

The noise is real and worth quoting: three consecutive cold runs of the vanilla benchmark on a moderately loaded machine read 14.382, 14.024, and 13.890 microseconds, a spread of 3.5 percent.

## Two caveats on the numbers

**Link-time optimization changes what the benchmark means.** The release profile uses `lto = "fat"`. That setting makes the production search binary about 1.7 percent *faster*, because it lets the compiler inline engine code into the search crate. But it makes this engine-only benchmark about 2.5 percent *slower*, because the benchmark lives in the same crate as the engine and never sees the cross-crate inlining that pays for it. Numbers taken before and after that switch are not comparable, and I do not present them as a single series.

`target-cpu=native` was tried and made no measurable difference, so it was removed.

**The benchmark was renamed partway through the project's history.** Early figures are labelled as a 50-turn rollout and later ones as 200-turn. Since the two endpoints measure within 0.3 microseconds of each other the series is roughly continuous, but it is not literally the same benchmark start to finish.

## Determinism check

Separately from timing, [`examples/golden_dump.rs`](../pkmn-engine/examples/golden_dump.rs) replays 20,000 battles and hashes the results into a single 64-bit digest. Any change that alters behaviour changes the digest. This caught several optimizations that were fast and wrong.

The digest is deliberately blind to code paths the vanilla corpus never reaches, since those battles have no items, abilities, or Terastallization. Changes in that territory are gated by unit tests instead.

## Where the time goes

Profiling the engine on a realistic corpus (real abilities and items, not the vanilla setup) puts the move-dispatch chain at roughly 14 percent of engine time, and the cost grows as the battles get more realistic.

One number worth keeping in mind before optimizing anything: the engine accounts for about 45 percent of wall time in the parallel production search, and about 70 percent single-threaded. So making the engine 10 percent faster buys roughly 4.5 percent end to end.
