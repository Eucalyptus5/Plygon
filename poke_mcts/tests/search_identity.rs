use poke_mcts::audit_snapshot::{read_search_states, SearchStatePair};
use poke_mcts::chance::OpenLoop;
use poke_mcts::eval::Evaluator;
use poke_mcts::eval_learned::{artifacts_required, LearnedValueV2, INT8_SCOPE_ENV, LVV2_WEIGHTS_PATH};
use poke_mcts::search::{search_world, ArmStat, SearchParams, SearchResult};
use rayon::prelude::*;

const STATES_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/search_states.bin");
const STATES: usize = 8;
const ITERS: u64 = 4096;
const SEED: u64 = 0x5EA4_C11D;

fn arms_identical(a: &[ArmStat], b: &[ArmStat]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.action == y.action
                && x.visits == y.visits
                && x.avg_score.to_bits() == y.avg_score.to_bits()
        })
}

fn identical(a: &SearchResult, b: &SearchResult) -> bool {
    arms_identical(&a.s1, &b.s1)
        && arms_identical(&a.s2, &b.s2)
        && a.iterations == b.iterations
        && a.guard_hits == b.guard_hits
        && a.depth_sum == b.depth_sum
}

fn run_all<E: Evaluator + Sync>(
    pool: &rayon::ThreadPool,
    states: &[SearchStatePair],
    ev: &E,
) -> Vec<SearchResult> {
    // no wall cap: the clock is polled every 1024 iters, so a faster candidate would run
    // past the reference and move `iterations`, which is itself a compared field
    let params = SearchParams { time_ms: u64::MAX, max_iters: ITERS, ..Default::default() };
    pool.install(|| {
        states
            .par_iter()
            .enumerate()
            .map(|(i, (s, t))| {
                search_world(s, t, ev, &OpenLoop, &params, SEED + i as u64, 0, None)
            })
            .collect()
    })
}

#[test]
fn served_search_matches_frozen_reference() {
    let require = artifacts_required();
    let named = std::env::var("LVV2_WEIGHTS").ok().filter(|s| !s.is_empty());
    assert!(
        !(require && named.is_none()),
        "LVV2_REQUIRE is set but LVV2_WEIGHTS names no export; the tracked default is never an armed subject"
    );
    // falling back to the tracked default means the net under test was never the subject
    let not_subject = named.is_none();
    let path = named.unwrap_or_else(|| LVV2_WEIGHTS_PATH.to_string());

    let pairs = read_search_states(STATES_PATH)
        .unwrap_or_else(|e| panic!("tracked search-state corpus {STATES_PATH}: {e}"));
    assert!(pairs.len() >= STATES, "corpus holds {} pairs, need {STATES}", pairs.len());
    let states = &pairs[..STATES];

    let scope = std::env::var(INT8_SCOPE_ENV).unwrap_or_default();
    let net = match LearnedValueV2::load_quantized(&path, &scope) {
        Ok(n) => n,
        Err(e) => {
            if require {
                panic!("LVV2_WEIGHTS resolved to {path}: {e}");
            }
            println!("SKIP search identity: states_compared=0 states_differing=0 iterations=0 int8={scope} weights={path}");
            return;
        }
    };

    // 2 workers, not the default 8: 8 workers over 8 states never revisit a worker, so a
    // stale per-thread scratch or cache could not fire
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let reference = net.reference_evaluator();
    let refs = run_all(&pool, states, &reference);
    let served = run_all(&pool, states, &net);

    let differing: Vec<usize> =
        (0..STATES).filter(|&i| !identical(&refs[i], &served[i])).collect();
    let tag = if not_subject { " NOT-SUBJECT" } else { "" };
    println!(
        "search identity: states_compared={} states_differing={} iterations={} int8={} weights={path}{tag}",
        STATES,
        differing.len(),
        refs[0].iterations,
        net.int8_scope(),
    );

    assert!(differing.is_empty(), "served search diverged from the reference at states {differing:?}");
    for (i, r) in refs.iter().enumerate() {
        assert_eq!(r.iterations, ITERS, "state {i} did not run the full iteration cap");
    }
}
