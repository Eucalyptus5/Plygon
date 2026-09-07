# fuzzer/tools

Re-runnable acceptance script for the server-mode subprocess pool. Run from the
engine root.

## parity_check.js

Server-mode parity gate. Run after any change to `lib/subprocess_pool.js`,
the engine's stdout shape, the comparator's normalization, or before
flipping a runtime default.

```
node conformance/fuzzer/tools/parity_check.js                          # full run
node conformance/fuzzer/tools/parity_check.js --smoke                  # tiny counts
node conformance/fuzzer/tools/parity_check.js --pass preflights        # just the two pre-flights
node conformance/fuzzer/tools/parity_check.js --pass pass4 \
                                               --minimize-budget-seconds 60
node conformance/fuzzer/tools/parity_check.js --seeds N --start-seed N   # resize / shard a sweep
```

`--seeds` overrides the seed count of all three passes (pass *C* is still
capped at 1000). `--start-seed` moves the seed base; passes *B* and *C*
offset it by 100000 and 200000. Use the two together to resume or shard a
full sweep.

What it does:

1. **Pre-flights** (abort on either failure before the real sweep).
   - *canary-inversion* — server-mode response for the first seed is mutated
     by one byte; canonical hash MUST diverge. If it doesn't, the hash
     function is broken.
   - *self-validation* — legacy path twice on the same seed; canonical
     hashes MUST be byte-identical. If not, stdout is non-deterministic
     on this host and parity comparison is meaningless.
2. **Three-pass Dex-ID-stratified sweep** (each pass compares legacy vs
   server canonical-hash-by-canonical-hash):
   - *A* — `recycleAfter: Infinity`, 10 k seeds — strict no-recycle leak
     check.
   - *B* — `recycleAfter: 1000`, 10 k seeds — production knob.
   - *C* — `recycleAfter: 100`, 1 k seeds — recycle-boundary exerciser.
3. **Pass-4** — minimizer end-to-end on ≥ 20 synthesized divergent
   fixtures, 3 repeats per fixture per setting (legacy / server-r1000 /
   server-r1). Compares canonical hashes of `minResult.scenario` AND
   `minResult.signature`. **Use `--minimize-budget-seconds 60`** for
   parity testing — at 15 s the minimizer's wallclock-deadline races
   produce false-positive byte-divergences between runtimes. See the
   parent `README.md` section on the minimizer wallclock budget.

Output: `parity_report.json` next to this file.

### Bisect runbook

When a sweep diverges at canonical-hash index *i*:

1. Re-run scenario *i* alone in server mode and compare it to legacy on
   the same seed.
2. If it reproduces standalone, file it as a deterministic leak.
3. If it does not, the divergence is order-dependent: re-run scenarios
   *[i-50 .. i]*.
4. If the isolated runs all pass, the leak only shows under sustained
   load, and pass *A* (no recycling) is the right detector.
5. Report the first divergent seed, the scenario hash, and which pool
   drifted.

## Heartbeat instrumentation

The script runs with the production `subprocess_pool.js` instrumentation
visible. To see per-call stall logging on stderr, set:

```
POOL_HEARTBEAT_MS=5000 POOL_HEARTBEAT_STALL_MS=5000 \
  node conformance/fuzzer/tools/parity_check.js --smoke
```

Off by default (no overhead unless the env vars are set). Emits one line
per stuck in-flight call, throttled to once per heartbeat interval per
call:

```
pool=engine waiting_on=jx2k0pq3-127 elapsed=5421ms queue_depth=1 \
            lock_holder=jx2k0pq3-127 child_pid=12345 stdin_writable=true
```

Use this for any future soak that comes back with unexplained
`harness_error` rates or hangs.
