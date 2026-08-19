use poke_mcts::audit_snapshot::{read_search_states, SearchStatePair};
use poke_mcts::chance::OpenLoop;
use poke_mcts::eval::{sigmoid, Evaluator};
use poke_mcts::eval_learned::{
    artifacts_required, LearnedValueV2, LVV2_FIXTURES_PATH, LVV2_WEIGHTS_PATH,
};
use poke_mcts::features::{extract_dense, extract_segmented, DENSE_DIM, NUM_SEGMENTS};
use poke_mcts::search::{search_world, ArmStat, SearchParams, SearchResult};
use rayon::prelude::*;

const STATES_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/search_states.bin");
const STATES: usize = 8;
const ITERS: u64 = 4096;
const SEED: u64 = 0x5EA4_C11D;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn bits32(v: f32) -> String {
    format!("{:08x}", v.to_bits())
}

fn bits64(v: f64) -> String {
    format!("{:016x}", v.to_bits())
}

struct Body {
    lines: Vec<String>,
}

impl Body {
    fn push(&mut self, line: String) {
        println!("{line}");
        self.lines.push(line);
    }

    fn hash(&self) -> u64 {
        fnv1a(FNV_OFFSET, self.lines.join("\n").as_bytes())
    }
}

struct Fixture {
    ids: Vec<u32>,
    seg: [u16; NUM_SEGMENTS],
    dense: [f32; DENSE_DIM],
    logit: f32,
}

fn load_fixtures(fx: &serde_json::Value) -> Vec<Fixture> {
    fx["fixtures"]
        .as_array()
        .expect("fixtures array")
        .iter()
        .map(|f| {
            let ids: Vec<u32> =
                f["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
            let mut seg = [0u16; NUM_SEGMENTS];
            for (i, v) in f["seg_lens"].as_array().unwrap().iter().enumerate() {
                seg[i] = v.as_u64().unwrap() as u16;
            }
            let mut dense = [0f32; DENSE_DIM];
            for (i, v) in f["dense"].as_array().unwrap().iter().enumerate() {
                dense[i] = v.as_f64().unwrap() as f32;
            }
            Fixture { ids, seg, dense, logit: f["logit"].as_f64().unwrap() as f32 }
        })
        .collect()
}

fn run_all<E: Evaluator + Sync>(
    pool: &rayon::ThreadPool,
    states: &[SearchStatePair],
    ev: &E,
) -> Vec<SearchResult> {
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

fn arm_hash(r: &SearchResult) -> u64 {
    let mut h = FNV_OFFSET;
    for a in r.s1.iter().chain(r.s2.iter()) {
        h = fnv1a(h, &a.action.to_le_bytes());
        h = fnv1a(h, &a.visits.to_le_bytes());
        h = fnv1a(h, &a.avg_score.to_bits().to_le_bytes());
    }
    h
}

fn result_line(tag: &str, i: usize, r: &SearchResult) -> String {
    format!(
        "s{i} {tag} iters={} guard={} depth={} arms={}/{} hash={:016x}",
        r.iterations,
        r.guard_hits,
        r.depth_sum,
        r.s1.len(),
        r.s2.len(),
        arm_hash(r)
    )
}

fn arm_lines(body: &mut Body, i: usize, tag: &str, arms: &[ArmStat]) {
    for (j, a) in arms.iter().enumerate() {
        body.push(format!(
            "s{i} {tag} {j} act={} vis={} avg={}",
            a.action,
            a.visits,
            bits64(a.avg_score)
        ));
    }
}

#[test]
fn cross_arch_dump() {
    let require = artifacts_required();
    let named_w = std::env::var("LVV2_WEIGHTS").ok().filter(|s| !s.is_empty());
    let named_f = std::env::var("LVV2_FIXTURES").ok().filter(|s| !s.is_empty());
    if require && named_w.is_none() {
        panic!("LVV2_REQUIRE is set but LVV2_WEIGHTS names no export; the tracked default is never an armed subject");
    }
    let w_path = named_w.unwrap_or_else(|| LVV2_WEIGHTS_PATH.to_string());
    let f_path = named_f.unwrap_or_else(|| LVV2_FIXTURES_PATH.to_string());

    let pairs = read_search_states(STATES_PATH)
        .unwrap_or_else(|e| panic!("tracked search-state corpus {STATES_PATH}: {e}"));
    assert!(pairs.len() >= STATES, "corpus holds {} pairs, need {STATES}", pairs.len());
    let states = &pairs[..STATES];

    let net = match LearnedValueV2::load(&w_path) {
        Ok(n) => n,
        Err(e) => {
            if require {
                panic!("LVV2_WEIGHTS resolved to {w_path}: {e}");
            }
            println!("SKIP cross-arch dump: weights={w_path}");
            return;
        }
    };
    let parsed = std::fs::read_to_string(&f_path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()));
    let fx = match parsed {
        Ok(v) => v,
        Err(e) => {
            if require {
                panic!("LVV2_FIXTURES resolved to {f_path}: {e}");
            }
            println!("SKIP cross-arch dump: weights={w_path}");
            return;
        }
    };
    let rows = load_fixtures(&fx);
    let weights_len = std::fs::metadata(&w_path).map(|m| m.len()).unwrap_or(0);

    println!("=== CROSS-ARCH DUMP v2 ===");
    println!("arch={} os={}", std::env::consts::ARCH, std::env::consts::OS);
    println!("weights={w_path}");
    println!("fixtures={f_path}");
    println!("weights_sha256_len={weights_len}");
    println!("multiplier_bits={}", bits32(net.export_multiplier()));

    let mut body = Body { lines: Vec::new() };

    body.push("--- PROBE 1: EXTRACT ---".to_string());
    let mut ids: Vec<u32> = Vec::new();
    for (i, (state, _)) in states.iter().enumerate() {
        ids.clear();
        let seg_lens = extract_segmented(state, &mut ids);
        let dense = extract_dense(state);
        let seg = seg_lens.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",");
        let den = dense.iter().map(|&d| bits32(d)).collect::<Vec<_>>().join(",");
        body.push(format!("x{i} nids={} seg={seg} dense={den}", ids.len()));
        let mut h = FNV_OFFSET;
        for id in &ids {
            h = fnv1a(h, &id.to_le_bytes());
        }
        body.push(format!("x{i} idhash={h:016x}"));
    }

    body.push("--- PROBE 2: FORWARD ---".to_string());
    let mut served_hash = FNV_OFFSET;
    let mut ref_hash = FNV_OFFSET;
    for (r, row) in rows.iter().enumerate() {
        let served = net.natural_logit(&row.ids, &row.seg, &row.dense);
        let refv = net.natural_logit_reference(&row.ids, &row.seg, &row.dense);
        served_hash = fnv1a(served_hash, &served.to_bits().to_le_bytes());
        ref_hash = fnv1a(ref_hash, &refv.to_bits().to_le_bytes());
        body.push(format!(
            "f{r} served={} ref={} fixture_logit={}",
            bits32(served),
            bits32(refv),
            bits32(row.logit)
        ));
    }
    body.push(format!("f_all served_hash={served_hash:016x} ref_hash={ref_hash:016x}"));

    body.push("--- PROBE 3: EVAL ---".to_string());
    let reference = net.reference_evaluator();
    for (i, (state, _)) in states.iter().enumerate() {
        let served = net.eval(state);
        let refv = reference.eval(state);
        body.push(format!("e{i} served={} ref={}", bits32(served), bits32(refv)));
    }

    body.push("--- PROBE 4: SEARCH ---".to_string());
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let served_runs = run_all(&pool, states, &net);
    let ref_runs = run_all(&pool, states, &reference);
    for i in 0..STATES {
        body.push(result_line("served", i, &served_runs[i]));
        body.push(result_line("ref   ", i, &ref_runs[i]));
        arm_lines(&mut body, i, "a1", &served_runs[i].s1);
        arm_lines(&mut body, i, "a2", &served_runs[i].s2);
    }

    body.push("--- PROBE 5: LIBM ---".to_string());
    let mut h = FNV_OFFSET;
    let mut edges: Vec<String> = Vec::new();
    for i in 0..=2000u32 {
        let x = -10.0f64 + (i as f64) * 0.01;
        let y = x.exp();
        h = fnv1a(h, &y.to_bits().to_le_bytes());
        if i < 5 || i > 1995 {
            edges.push(format!("l_exp64 {i} x={:016x} y={:016x}", x.to_bits(), y.to_bits()));
        }
    }
    body.push(format!("l_exp64 hash={h:016x}"));
    for e in edges {
        body.push(e);
    }

    let mut h = FNV_OFFSET;
    let mut edges: Vec<String> = Vec::new();
    for n in 1u64..=4096 {
        let y = (n as f64).ln();
        h = fnv1a(h, &y.to_bits().to_le_bytes());
        if n <= 5 || n > 4091 {
            edges.push(format!("l_ln64 {n} y={:016x}", y.to_bits()));
        }
    }
    body.push(format!("l_ln64 hash={h:016x}"));
    for e in edges {
        body.push(e);
    }

    let mut h = FNV_OFFSET;
    let mut edges: Vec<String> = Vec::new();
    for i in 0..=2000u32 {
        let x = -1000.0f32 + (i as f32) * 1.0;
        let y = sigmoid(x);
        h = fnv1a(h, &y.to_bits().to_le_bytes());
        if i < 5 || i > 1995 {
            edges.push(format!("l_sigmoid {i} x={:08x} y={:016x}", x.to_bits(), y.to_bits()));
        }
    }
    body.push(format!("l_sigmoid hash={h:016x}"));
    for e in edges {
        body.push(e);
    }

    for i in 0..STATES {
        let root_eval = net.eval(&states[i].0);
        for j in 0..STATES {
            let arg = net.eval(&states[j].0) - root_eval;
            body.push(format!(
                "l_sig_real {i} {j} arg={:08x} y={:016x}",
                arg.to_bits(),
                sigmoid(arg).to_bits()
            ));
        }
    }

    println!("=== DUMP END total_hash={:016x} ===", body.hash());
}
