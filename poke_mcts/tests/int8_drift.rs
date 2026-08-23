use pkmn_engine::state::legal_actions;
use poke_mcts::audit_snapshot::{read_search_states, SearchStatePair};
use poke_mcts::chance::OpenLoop;
use poke_mcts::driver::{aggregate, pick_from, PickMode};
use poke_mcts::eval::Evaluator;
use poke_mcts::eval_learned::LearnedValueV2;
use poke_mcts::features::{DENSE_DIM, NUM_SEGMENTS};
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::search::{search_world, SearchParams};
use rayon::prelude::*;

const STATES_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/search_states.bin");
const N: usize = 256;
const ROWS: usize = 64;
const ITERS: u64 = 4096;
const SERVED_EXPLORE_COEFF: f64 = 0.49;
const FILTER: f64 = 0.75;
const DRIFT_BAR: f64 = 0.02;
const Z95: f64 = 1.645;
const DEFAULT_SCOPES: &str = "emb;web;fc;web,fc;emb,web,fc;emb,web,fc,act";
const DEFAULT_FLOOR_SALTS: &str = "11,12;11,13";

struct Row {
    ids: Vec<u32>,
    seg: [u16; NUM_SEGMENTS],
    dense: [f32; DENSE_DIM],
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_string())
}

// a fallback to the tracked default would score a net that was never the subject
fn required_env(key: &str) -> String {
    std::env::var(key).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| {
        panic!("{key} must name the armed export; this instrument never falls back to a default")
    })
}

fn scopes() -> Vec<String> {
    env_or("INT8_SCOPES", DEFAULT_SCOPES)
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn floor_salts(raw: &str) -> Vec<(u64, u64)> {
    let parse = |v: &str| -> u64 {
        v.trim()
            .parse()
            .unwrap_or_else(|e| panic!("INT8_FLOOR_SALTS salt {v:?} must read a u64: {e}"))
    };
    raw.split(';')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (a, b) = p
                .split_once(',')
                .unwrap_or_else(|| panic!("INT8_FLOOR_SALTS pair {p:?} must read as a,b"));
            (parse(a), parse(b))
        })
        .collect()
}

// the served constant, overridable only so the floor can be read on the tree the
// bench modes explore by default
fn explore_coeff() -> f64 {
    let raw = env_or("INT8_EXPLORE_COEFF", "");
    if raw.is_empty() {
        return SERVED_EXPLORE_COEFF;
    }
    raw.parse().unwrap_or_else(|e| panic!("INT8_EXPLORE_COEFF {raw:?} must read a float: {e}"))
}

fn threads() -> usize {
    let raw = env_or("INT8_THREADS", "3");
    raw.parse().unwrap_or_else(|e| panic!("INT8_THREADS {raw:?} must read a thread count: {e}"))
}

fn quantized(path: &str, scope: &str) -> LearnedValueV2 {
    LearnedValueV2::load_quantized(path, scope)
        .unwrap_or_else(|e| panic!("load_quantized({path}, {scope:?}): {e}"))
}

fn load_rows(path: &str) -> Vec<Row> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("LVV2_FIXTURES resolved to {path}: {e}"));
    let fx: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("LVV2_FIXTURES {path} must parse: {e}"));
    let rows: Vec<Row> = fx["fixtures"]
        .as_array()
        .unwrap_or_else(|| panic!("LVV2_FIXTURES {path} carries no fixtures array"))
        .iter()
        .enumerate()
        .map(|(k, f)| {
            let ids: Vec<u32> = f["ids"]
                .as_array()
                .unwrap_or_else(|| panic!("{path} fixture {k} carries no ids array"))
                .iter()
                .map(|v| {
                    v.as_u64().unwrap_or_else(|| panic!("{path} fixture {k} ids must be integers"))
                        as u32
                })
                .collect();
            let mut seg = [0u16; NUM_SEGMENTS];
            for (i, v) in f["seg_lens"]
                .as_array()
                .unwrap_or_else(|| panic!("{path} fixture {k} carries no seg_lens array"))
                .iter()
                .enumerate()
            {
                seg[i] = v
                    .as_u64()
                    .unwrap_or_else(|| panic!("{path} fixture {k} seg_lens must be integers"))
                    as u16;
            }
            let mut dense = [0f32; DENSE_DIM];
            for (i, v) in f["dense"]
                .as_array()
                .unwrap_or_else(|| panic!("{path} fixture {k} carries no dense array"))
                .iter()
                .enumerate()
            {
                dense[i] = v
                    .as_f64()
                    .unwrap_or_else(|| panic!("{path} fixture {k} dense must be numbers"))
                    as f32;
            }
            Row { ids, seg, dense }
        })
        .collect();
    assert!(rows.len() >= ROWS, "LVV2_FIXTURES {path} holds {} rows, need {ROWS}", rows.len());
    rows
}

fn corpus() -> Vec<SearchStatePair> {
    let pairs = read_search_states(STATES_PATH)
        .unwrap_or_else(|e| panic!("tracked search-state corpus {STATES_PATH}: {e}"));
    assert!(pairs.len() >= N, "corpus holds {} pairs, need {N}", pairs.len());
    pairs
}

fn decisions<E: Evaluator + Sync>(
    pool: &rayon::ThreadPool,
    states: &[SearchStatePair],
    ev: &E,
    salt: u64,
) -> Vec<u8> {
    // fixed iteration cap: the deadline is polled every 1024 iterations, so a wall budget would let evaluator cost move the compute
    let params = SearchParams {
        time_ms: u64::MAX,
        max_iters: ITERS,
        explore_coeff: explore_coeff(),
        ..Default::default()
    };
    assert!(
        std::env::var("INT8_EXPLORE_COEFF").is_ok() || params.explore_coeff == 0.49,
        "explore_coeff must echo c^2 = 0.49 unless it is deliberately overridden"
    );
    pool.install(|| {
        states
            .par_iter()
            .enumerate()
            .map(|(i, (state, teams))| {
                let seed = splitmix64(salt ^ i as u64);
                let r = search_world(state, teams, ev, &OpenLoop, &params, seed, 0, None);
                let per_world = vec![(r.side(0).to_vec(), 1.0f64)];
                let agg = aggregate(&per_world);
                pick_from(
                    &agg,
                    &legal_actions(state, 0),
                    &mut Lcg::new(seed),
                    PickMode::Argmax,
                    FILTER,
                )
            })
            .collect()
    })
}

fn agreement(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x == y).count()
}

fn pct(k: usize) -> f64 {
    100.0 * k as f64 / N as f64
}

// two-proportion one-sided 95% band at n = 256; the pre-registered figure is -4.4pp
fn margin_pp(floor: f64, cand: f64) -> f64 {
    let n = N as f64;
    let (p1, p2) = (floor / 100.0, cand / 100.0);
    100.0 * Z95 * ((p1 * (1.0 - p1) / n) + (p2 * (1.0 - p2) / n)).sqrt()
}

#[test]
#[ignore]
fn bar_i_logit_drift() {
    let w_path = required_env("LVV2_WEIGHTS");
    let f_path = required_env("LVV2_FIXTURES");
    let scopes = scopes();
    let rows = load_rows(&f_path);
    println!(
        "bar-i header: weights={w_path} fixtures={f_path} rows={} bar={DRIFT_BAR} scopes={}",
        rows.len(),
        scopes.join(";")
    );

    for scope in &scopes {
        let net = quantized(&w_path, scope);
        let mut max_abs = 0f64;
        let mut sum_abs = 0f64;
        let mut over = 0usize;
        for r in &rows {
            let served = net.natural_logit(&r.ids, &r.seg, &r.dense) as f64;
            let reference = net.natural_logit_reference(&r.ids, &r.seg, &r.dense) as f64;
            let d = (served - reference).abs();
            max_abs = max_abs.max(d);
            sum_abs += d;
            over += usize::from(d > DRIFT_BAR);
        }
        println!(
            "bar-i scope={} rows={} max_abs_dlogit={:.6e} rows_over_0.02={over} mean_abs_dlogit={:.6e} weights={w_path}",
            net.int8_scope(),
            rows.len(),
            max_abs,
            sum_abs / rows.len() as f64,
        );
    }
}

#[test]
#[ignore]
fn bar_ii_decision_agreement() {
    let w_path = required_env("LVV2_WEIGHTS");
    let scopes = scopes();
    let raw_salts = env_or("INT8_FLOOR_SALTS", DEFAULT_FLOOR_SALTS);
    let salt_pairs = floor_salts(&raw_salts);
    assert!(!salt_pairs.is_empty(), "INT8_FLOOR_SALTS must name at least one a,b pair");
    let s1 = salt_pairs[0].0;
    let nthreads = threads();

    let pairs = corpus();
    let states = &pairs[..N];
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(nthreads)
        .build()
        .unwrap_or_else(|e| panic!("rayon pool of {nthreads} threads: {e}"));

    println!(
        "bar-ii header: weights={w_path} corpus={STATES_PATH} corpus_pairs={} n={N} max_iters={ITERS} explore_coeff={} threads={nthreads} floor_salts={raw_salts} scope_salt={s1} scopes={}",
        pairs.len(),
        explore_coeff(),
        scopes.join(";")
    );

    let float = LearnedValueV2::load(&w_path)
        .unwrap_or_else(|e| panic!("LVV2_WEIGHTS resolved to {w_path}: {e}"));

    let mut primary_floor = None;
    for (a, b) in &salt_pairs {
        let da = decisions(&pool, states, &float, *a);
        let db = decisions(&pool, states, &float, *b);
        let k = agreement(&da, &db);
        println!("bar-ii-floor salts={a}v{b} n={N} agree={k} pct={:.2}", pct(k));
        primary_floor.get_or_insert(pct(k));
    }
    let floor_pct = primary_floor.expect("a floor pair was measured");

    let float_served = decisions(&pool, states, &float, s1);
    let float_ref = decisions(&pool, states, &float.reference_evaluator(), s1);
    let k_ctrl = agreement(&float_served, &float_ref);
    let ctrl_tag = if k_ctrl == N { "" } else { " CONTROL-BROKEN" };
    println!("bar-ii-refself n={N} agree={k_ctrl} pct={:.2}{ctrl_tag}", pct(k_ctrl));

    for scope in &scopes {
        let net = quantized(&w_path, scope);
        // the reference is re-read off the quantized net so an in-place quantize cannot pose as the float arm
        let arm_ref = decisions(&pool, states, &net.reference_evaluator(), s1);
        let arm_cand = decisions(&pool, states, &net, s1);
        let k = agreement(&arm_ref, &arm_cand);
        let p = pct(k);
        let margin = margin_pp(floor_pct, p);
        let verdict = if p >= floor_pct - margin { "PASS" } else { "FAIL" };
        println!(
            "bar-ii scope={} n={N} agree={k} pct={p:.2} vs_floor_pp={:.2} margin_pp={margin:.2} verdict={verdict}",
            net.int8_scope(),
            p - floor_pct,
        );
    }
}
