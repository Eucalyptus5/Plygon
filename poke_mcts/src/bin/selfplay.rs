use poke_mcts::chance::OpenLoop;
use poke_mcts::chance_analytic::AnalyticRoot;
use poke_mcts::eval::{Evaluator, Handcrafted};
use poke_mcts::eval_learned::{value_v2_input_len, LearnedEval, LearnedValueV2};
use poke_mcts::fixtures::{build, Fixture, MonJson};
use poke_mcts::policies::{greedy_action, random_action};
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::features::{DENSE_DIM, NUM_SEGMENTS};
use poke_mcts::search::{search_world, SearchParams};
use pkmn_engine::state::*;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind { Random, Greedy, Mcts, Pimc }

fn parse_kind(s: &str) -> Kind {
    match s { "random" => Kind::Random, "greedy" => Kind::Greedy, "mcts" => Kind::Mcts, "pimc" => Kind::Pimc, _ => panic!("unknown policy {s}") }
}

#[derive(Clone, Copy)]
struct Entrant {
    kind: Kind,
    time_ms: u64,
    worlds: usize,
    max_iters: u64,
    adaptive: bool,
    chance_mode: poke_mcts::search::ChanceMode,
    pick_mode: poke_mcts::driver::PickMode,
    filter_threshold: f64,
    raw_root: bool,
}

fn mcts_choose(state: &BattleState, teams: &TeamData, side: usize, time_ms: u64, seed: u64) -> u8 {
    let legal = legal_actions(state, side);
    if legal.count == 0 { return ACTION_STRUGGLE; }
    if legal.count == 1 { return legal.actions[0]; }
    let params = SearchParams { time_ms, ..Default::default() };
    let r = search_world(state, teams, &Handcrafted, &OpenLoop, &params, seed, 0, None);
    r.side(side).iter().max_by_key(|a| a.visits).map(|a| a.action).unwrap_or(legal.actions[0])
}

fn choose(e: Entrant, state: &BattleState, teams: &TeamData, side: usize, rng: &mut Lcg, seed: u64,
          beliefs: &[poke_mcts::belief::Belief; 2]) -> u8 {
    match e.kind {
        Kind::Random => random_action(state, side, rng),
        Kind::Greedy => greedy_action(state, side, rng),
        Kind::Mcts => mcts_choose(state, teams, side, e.time_ms, seed),
        Kind::Pimc => {
            let obs = poke_mcts::determinize::Observation { state, teams, our_side: side };
            let (num_worlds, time_ms_per_world) = if e.adaptive {
                let opponent = 1 - side;
                let revealed = beliefs[side].revealed_count();
                let active_opp = state.active_mon(opponent);
                let base_opp = pkmn_engine::state::data_bridge::base_species(active_opp.species_id);
                let active_moves_revealed = beliefs[side].mons.iter()
                    .find(|m| m.species_id != 0
                        && pkmn_engine::state::data_bridge::base_species(m.species_id) == base_opp)
                    .map(|m| m.n_moves as usize)
                    .unwrap_or(0);
                let parallelism = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
                poke_mcts::driver::adaptive_budget(revealed, active_moves_revealed, parallelism, e.time_ms)
            } else {
                (e.worlds, e.time_ms)
            };
            let cfg = poke_mcts::driver::PimcConfig {
                num_worlds, time_ms_per_world, max_iters_per_world: e.max_iters, seed,
                chance_mode: e.chance_mode,
                pick_mode: e.pick_mode, filter_threshold: e.filter_threshold, raw_root: e.raw_root,
                explore_coeff: 2.0,
                value_temp: 1.0,
            };
            poke_mcts::driver::choose_action(&obs, &beliefs[side], &poke_mcts::determinize::RandomBattle, &cfg)
        }
    }
}

/// Returns value for side 0: 1.0 win / 0.5 draw / 0.0 loss.
fn play(p1: Entrant, p2: Entrant, t1: &[MonJson], t2: &[MonJson], game_seed: u64) -> f64 {
    let (team1, b1, l1) = build(t1);
    let (team2, b2, l2) = build(t2);
    let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
    let mut state = BattleState::default();
    state.sides[0].team = team1;
    state.sides[1].team = team2;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);
    let beliefs = [poke_mcts::belief::Belief::default(); 2];
    poke_mcts::selfplay::play_to_terminal(state, &teams, beliefs, game_seed,
        |side, st, tm, bel, seed, rng| choose(if side == 0 { p1 } else { p2 }, st, tm, side, rng, seed, bel))
}

fn stats(mut v: Vec<f64>) -> (f64, f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    let med = if n % 2 == 0 { (v[n / 2 - 1] + v[n / 2]) / 2.0 } else { v[n / 2] };
    (v[0], med, v[n - 1])
}

fn bench(fixture: &Fixture, eval: &impl Evaluator, kind: poke_mcts::driver::EvalKind<'_>, label: &str) {
    let (team1, b1, l1) = build(&fixture.teams[0]);
    let (team2, b2, l2) = build(&fixture.teams[1]);
    let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
    let mut state = BattleState::default();
    state.sides[0].team = team1;
    state.sides[1].team = team2;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);

    let mut sink = 0f32;
    for _ in 0..1_000 {
        sink += std::hint::black_box(eval.eval(std::hint::black_box(&state)));
    }
    let t0 = std::time::Instant::now();
    let mut n = 0u64;
    loop {
        for _ in 0..10_000 {
            sink += std::hint::black_box(eval.eval(std::hint::black_box(&state)));
        }
        n += 10_000;
        if t0.elapsed().as_secs_f64() >= 0.5 {
            break;
        }
    }
    std::hint::black_box(sink);
    println!("eval[{}] µs/eval: {:.3}", label, t0.elapsed().as_secs_f64() * 1e6 / n as f64);

    let params = SearchParams { time_ms: 100, ..Default::default() };
    let mut iters: Vec<f64> = Vec::new();
    let (mut guards, mut depths) = (0u64, 0u64);
    for seed in 0..10u64 {
        let r = search_world(&state, &teams, eval, &OpenLoop, &params, seed, 0, None);
        iters.push(r.iterations as f64);
        guards += r.guard_hits;
        depths += r.depth_sum;
    }
    let total: f64 = iters.iter().sum();
    let (min, med, max) = stats(iters);
    println!("search_world iters/100ms: min {:.0} / median {:.0} / max {:.0}", min, med, max);
    println!("mean depth: {:.2}", depths as f64 / total);
    println!("guard-fire %: {:.3}", 100.0 * guards as f64 / total);

    let closed_p = SearchParams { time_ms: 100, max_nodes: poke_mcts::search::closed_loop_max_nodes(1), ..Default::default() };
    let mut citers: Vec<f64> = Vec::new();
    for seed in 0..10u64 {
        let r = poke_mcts::chance_closed::search_world_closed(&state, &teams, eval, &closed_p, seed);
        citers.push(r.iterations as f64);
    }
    let (cmin, cmed, cmax) = stats(citers);
    println!("search_world_closed iters/100ms: min {:.0} / median {:.0} / max {:.0}", cmin, cmed, cmax);
    println!("closed/open iters ratio (median): {:.2}", cmed / med);

    // Design-1 analytic root (E2 G3' throughput twin): same search_world, AnalyticRoot model.
    let mut aiters: Vec<f64> = Vec::new();
    let (mut aguards, mut adepths) = (0u64, 0u64);
    for seed in 0..10u64 {
        let r = search_world(&state, &teams, eval, &AnalyticRoot, &params, seed, 0, None);
        aiters.push(r.iterations as f64);
        aguards += r.guard_hits;
        adepths += r.depth_sum;
    }
    let atotal: f64 = aiters.iter().sum();
    let (amin, amed, amax) = stats(aiters);
    println!("search_world_analytic iters/100ms: min {:.0} / median {:.0} / max {:.0}", amin, amed, amax);
    println!("analytic mean depth: {:.2}", adepths as f64 / atotal);
    println!("analytic guard-fire %: {:.3}", 100.0 * aguards as f64 / atotal);
    println!("analytic/open iters ratio (median): {:.2}", amed / med);

    let mut belief = poke_mcts::belief::Belief::default();
    let om = state.active_mon(1);
    belief.note_species(om.species_id, om.level);
    let obs = poke_mcts::determinize::Observation { state: &state, teams: &teams, our_side: 0 };
    let mut wall: Vec<f64> = Vec::new();
    for seed in 0..10u64 {
        let cfg = poke_mcts::driver::PimcConfig {
            num_worlds: 16, time_ms_per_world: 100, max_iters_per_world: u64::MAX, seed,
            chance_mode: poke_mcts::search::ChanceMode::OpenLoop,
            pick_mode: poke_mcts::driver::PickMode::Weighted, filter_threshold: 0.75, raw_root: false,
            explore_coeff: 2.0,
            value_temp: 1.0,
        };
        let t0 = std::time::Instant::now();
        let _ = poke_mcts::driver::choose_action_eval(&obs, &belief, &poke_mcts::determinize::RandomBattle, &cfg, kind, None);
        wall.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let (wmin, wmed, wmax) = stats(wall);
    println!("choose_action 16w@100ms wall-ms/decision: min {:.1} / median {:.1} / max {:.1}", wmin, wmed, wmax);
}

fn sweep_root(fixture: &Fixture) -> (BattleState, TeamData) {
    let (team1, b1, l1) = build(&fixture.teams[0]);
    let (team2, b2, l2) = build(&fixture.teams[1]);
    let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
    let mut state = BattleState::default();
    state.sides[0].team = team1;
    state.sides[1].team = team2;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);
    (state, teams)
}

const SWEEP_MAX: usize = 8;
const SWEEP_TOL: f64 = 0.02;
const SWEEP_REPS: usize = 5;

/// N -> median µs/iter under `measure`; returns (settled_N, µs/iter, sweeps, unsettled).
fn sweep_fixed_point(
    start_n: u64,
    target_ms: u64,
    mut measure: impl FnMut(u64) -> f64,
    mut report: impl FnMut(usize, u64, f64, u64, f64),
) -> (u64, f64, usize, bool) {
    let target_us = target_ms as f64 * 1000.0;
    let mut n = start_n.max(1);
    let mut consecutive = 0usize;
    let mut history: Vec<(u64, f64)> = Vec::with_capacity(SWEEP_MAX);
    for k in 0..SWEEP_MAX {
        let us_per_iter = measure(n);
        let next_n = (target_us / us_per_iter).round().max(1.0) as u64;
        let delta = (next_n as f64 - n as f64).abs() / n as f64;
        report(k, n, us_per_iter, next_n, 100.0 * delta);
        history.push((n, us_per_iter));
        consecutive = if delta < SWEEP_TOL { consecutive + 1 } else { 0 };
        if consecutive == 2 {
            return (n, us_per_iter, k + 1, false);
        }
        n = next_n;
    }
    let mut last3 = history[SWEEP_MAX - 3..].to_vec();
    last3.sort_by_key(|&(n, _)| n);
    (last3[1].0, last3[1].1, SWEEP_MAX, true)
}

fn sweep_single(fixture: &Fixture, eval: &impl Evaluator, label: &str, start_n: u64,
                target_ms: u64, explore_coeff: f64) {
    let (state, teams) = sweep_root(fixture);
    let seen_iters = std::cell::Cell::new(0u64);
    let (n, us, sweeps, unsettled) = sweep_fixed_point(
        start_n,
        target_ms,
        |n| {
            let params = SearchParams { time_ms: u64::MAX, max_iters: n, explore_coeff, ..Default::default() };
            let mut reps: Vec<f64> = Vec::with_capacity(SWEEP_REPS);
            for rep in 0..SWEEP_REPS as u64 {
                let t0 = std::time::Instant::now();
                let r = search_world(&state, &teams, eval, &OpenLoop, &params, rep, 0, None);
                reps.push(t0.elapsed().as_secs_f64() * 1e6 / r.iterations.max(1) as f64);
                if rep == 0 { seen_iters.set(r.iterations); }
            }
            stats(reps).1
        },
        |k, n, us, next_n, delta| {
            println!("sweep {label} {k}: max_iters={n} iterations={} us_per_iter={us:.3} next_N={next_n} delta={delta:.2}%",
                seen_iters.get());
        },
    );
    println!("settled {label}: N={n} us_per_iter={us:.3} sweeps={sweeps} target_ms={target_ms} unsettled={unsettled} explore_coeff={explore_coeff:.2}");
}

struct SweepSnapshot { n: u64, iterations: u64, per_world_us: Vec<f64>, mean_us: f64 }

#[allow(clippy::too_many_arguments)]
fn sweep_pooled(fixture: &Fixture, eval: &(impl Evaluator + Sync), label: &str, start_n: u64,
                target_ms: u64, num_worlds: usize, threads: usize, world_seed: u64,
                explore_coeff: f64) {
    use poke_mcts::determinize::Determinizer;
    use rayon::prelude::*;
    let (state, teams) = sweep_root(fixture);
    let mut belief = poke_mcts::belief::Belief::default();
    let om = state.active_mon(1);
    belief.note_species(om.species_id, om.level);
    let obs = poke_mcts::determinize::Observation { state: &state, teams: &teams, our_side: 0 };
    let mut rng = Lcg::new(splitmix64(world_seed));
    let worlds = poke_mcts::determinize::RandomBattle.sample_worlds(&obs, &belief, num_worlds, &mut rng);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let target_us = target_ms as f64 * 1000.0;
    let history: std::cell::RefCell<Vec<SweepSnapshot>> = std::cell::RefCell::new(Vec::new());
    let (n, _us, sweeps, unsettled) = sweep_fixed_point(
        start_n,
        target_ms,
        |n| {
            let params = SearchParams { time_ms: u64::MAX, max_iters: n, explore_coeff, ..Default::default() };
            let mut reps: Vec<(f64, f64, Vec<f64>)> = Vec::with_capacity(SWEEP_REPS);
            let mut iterations = 0u64;
            for rep in 0..SWEEP_REPS {
                let timed: Vec<(f64, u64)> = pool.install(|| {
                    worlds.par_iter().enumerate().map(|(k, w)| {
                        let seed = splitmix64(world_seed ^ (k as u64).wrapping_mul(0x9E3779B97F4A7C15));
                        let t0 = std::time::Instant::now();
                        let r = search_world(&w.state, &w.teams, eval, &OpenLoop, &params, seed, 0, None);
                        (t0.elapsed().as_secs_f64() * 1e6 / r.iterations.max(1) as f64, r.iterations)
                    }).collect()
                });
                if rep == 0 { iterations = timed[0].1; }
                let per_world: Vec<f64> = timed.iter().map(|&(us, _)| us).collect();
                let w = per_world.len() as f64;
                let mean_us = per_world.iter().sum::<f64>() / w;
                let mean_n = per_world.iter().map(|us| target_us / us).sum::<f64>() / w;
                reps.push((mean_us, mean_n, per_world));
            }
            let median_n = stats(reps.iter().map(|r| r.1).collect()).1;
            // one rep supplies both printed figures, so the list and its mean stay consistent
            let mut order: Vec<usize> = (0..reps.len()).collect();
            order.sort_by(|&a, &b| reps[a].0.partial_cmp(&reps[b].0).unwrap());
            let mid = order[reps.len() / 2];
            history.borrow_mut().push(SweepSnapshot {
                n, iterations, per_world_us: reps[mid].2.clone(), mean_us: reps[mid].0,
            });
            target_us / median_n
        },
        |k, n, _us, next_n, delta| {
            let h = history.borrow();
            let s = h.last().unwrap();
            let list: Vec<String> = s.per_world_us.iter().map(|us| format!("{us:.3}")).collect();
            // mean_us_per_iter and N_8w are separate readings; neither derives from the other
            println!("sweep8 {label} {k}: max_iters={n} iterations={} per_world_us_per_iter=[{}] mean_us_per_iter={:.3} N_8w={next_n} delta={delta:.2}%",
                s.iterations, list.join(","), s.mean_us);
        },
    );
    let mean_us = history.borrow().iter().rev().find(|s| s.n == n).map(|s| s.mean_us)
        .expect("the settled N must be one of the measured sweeps");
    println!("settled8 {label}: N_8w={n} mean_us_per_iter={mean_us:.3} worlds={num_worlds} threads={threads} target_ms={target_ms} seed={world_seed} sweeps={sweeps} unsettled={unsettled} explore_coeff={explore_coeff:.2}");
}

#[allow(clippy::too_many_arguments)]
fn bench_sweep(fixture: &Fixture, eval: &(impl Evaluator + Sync), label: &str, start_n: u64,
               target_ms: u64, num_worlds: usize, threads: usize, world_seed: u64,
               explore_coeff: f64) {
    if num_worlds > 1 {
        sweep_pooled(fixture, eval, label, start_n, target_ms, num_worlds, threads, world_seed,
            explore_coeff);
    } else {
        sweep_single(fixture, eval, label, start_n, target_ms, explore_coeff);
    }
}

const EVAL_REPS: usize = 5;

fn timed_us(mut body: impl FnMut()) -> f64 {
    for _ in 0..1_000 {
        body();
    }
    let t0 = std::time::Instant::now();
    let mut n = 0u64;
    loop {
        for _ in 0..10_000 {
            body();
        }
        n += 10_000;
        if t0.elapsed().as_secs_f64() >= 0.5 {
            break;
        }
    }
    t0.elapsed().as_secs_f64() * 1e6 / n as f64
}

fn fmt_us(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{x:.3}"),
        None => "n/a".to_string(),
    }
}

fn bench_eval(fixture: &Fixture, eval: &impl Evaluator, net: Option<&LearnedValueV2>, label: &str,
              weights: &str, seed: u64) {
    use poke_mcts::features;
    let (state, teams) = sweep_root(fixture);
    let (mut totals, mut extracts, mut forwards) = (Vec::new(), Vec::new(), Vec::new());
    for rep in 1..=EVAL_REPS {
        let total = timed_us(|| {
            std::hint::black_box(eval.eval(std::hint::black_box(&state)));
        });
        let split = net.map(|net| {
            let extract = timed_us(|| {
                let mut ids = Vec::with_capacity(192);
                let seg_lens = features::extract_segmented(std::hint::black_box(&state), &mut ids);
                let dense = features::extract_dense(std::hint::black_box(&state));
                std::hint::black_box((&ids, seg_lens, dense));
            });
            let mut ids = Vec::with_capacity(192);
            let seg_lens = features::extract_segmented(&state, &mut ids);
            let dense = features::extract_dense(&state);
            let forward = timed_us(|| {
                std::hint::black_box(net.natural_logit(std::hint::black_box(&ids),
                    std::hint::black_box(&seg_lens), std::hint::black_box(&dense)));
            });
            (extract, forward)
        });
        totals.push(total);
        if let Some((extract, forward)) = split {
            extracts.push(extract);
            forwards.push(forward);
        }
        println!("bench-eval {label} rep {rep}: total_us={total:.3} extract_us={} forward_us={}",
            fmt_us(split.map(|s| s.0)), fmt_us(split.map(|s| s.1)));
    }

    let counter = CountingEval::new(eval);
    let params = SearchParams { time_ms: u64::MAX, max_iters: 4096, ..Default::default() };
    let r = search_world(&state, &teams, &counter, &OpenLoop, &params, seed, 0, None);
    let evals_per_iter = counter.count() as f64 / r.iterations.max(1) as f64;

    let total = stats(totals).1;
    let extract = if extracts.is_empty() { None } else { Some(stats(extracts).1) };
    let forward = if forwards.is_empty() { None } else { Some(stats(forwards).1) };
    let pct = match extract {
        Some(e) => format!("{:.2}%", 100.0 * e / total),
        None => "n/a".to_string(),
    };
    // distinct_states=1 is literal: this instrument repeats one root, so it cannot see the leaf distribution
    println!("bench-eval {label}: total_us_per_eval={total:.3} extract_us_per_eval={} forward_us_per_eval={} extract_pct={pct} evals_per_iter={evals_per_iter:.3} distinct_states=1 reps={EVAL_REPS} weights={weights}",
        fmt_us(extract), fmt_us(forward));
}

struct CountingEval<'a> {
    inner: &'a dyn Evaluator,
    calls: std::sync::atomic::AtomicU64,
}

impl<'a> CountingEval<'a> {
    fn new(inner: &'a dyn Evaluator) -> Self {
        CountingEval { inner, calls: std::sync::atomic::AtomicU64::new(0) }
    }
    fn count(&self) -> u64 { self.calls.load(std::sync::atomic::Ordering::Relaxed) }
}

impl Evaluator for CountingEval<'_> {
    fn eval(&self, state: &BattleState) -> f32 {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.eval(state)
    }
}

#[derive(Default)]
struct EvalTrace {
    ids: Vec<u32>,
    spans: Vec<(u32, u32)>,
    seg_lens: Vec<[u16; NUM_SEGMENTS]>,
    dense: Vec<[f32; DENSE_DIM]>,
}

impl EvalTrace {
    fn push(&mut self, ids: &[u32], seg_lens: [u16; NUM_SEGMENTS], dense: [f32; DENSE_DIM]) {
        self.spans.push((self.ids.len() as u32, ids.len() as u32));
        self.ids.extend_from_slice(ids);
        self.seg_lens.push(seg_lens);
        self.dense.push(dense);
    }

    fn len(&self) -> usize { self.spans.len() }

    fn ids_at(&self, i: usize) -> &[u32] {
        let (off, len) = self.spans[i];
        &self.ids[off as usize..(off + len) as usize]
    }

    fn probe_input(&self) -> Vec<(Vec<u32>, [u16; NUM_SEGMENTS])> {
        (0..self.len()).map(|i| (self.ids_at(i).to_vec(), self.seg_lens[i])).collect()
    }
}

fn distinct_rows(trace: &EvalTrace) -> usize {
    let mut seen: std::collections::HashSet<(Vec<u32>, [u16; NUM_SEGMENTS], [u32; DENSE_DIM])> =
        std::collections::HashSet::with_capacity(trace.len());
    for i in 0..trace.len() {
        let mut bits = [0u32; DENSE_DIM];
        for (b, v) in bits.iter_mut().zip(trace.dense[i].iter()) {
            *b = v.to_bits();
        }
        seen.insert((trace.ids_at(i).to_vec(), trace.seg_lens[i], bits));
    }
    seen.len()
}

fn med_spread(v: Vec<f64>) -> (f64, f64, f64, f64) {
    let (min, med, max) = stats(v);
    let spread = if med != 0.0 { 100.0 * (max - min) / med } else { 0.0 };
    (med, min, max, spread)
}

struct NetGeom { aw: usize, r: usize, fc: Vec<(usize, usize)> }

struct Census {
    gather_adds: f64,
    tokfill_stores: f64,
    proj_macs: f64,
    acc_adds: f64,
    cell_ops: f64,
    xasm_ops: f64,
    fc_macs: f64,
    fc_relu_ops: f64,
    fit_stores: f64,
    fit_dead_stores: f64,
    proj_fill_stores: f64,
    total_ops: f64,
}

fn census(geom: &NetGeom, evals: u64, seg_misses: &[u64; NUM_SEGMENTS], missed_ids: u64) -> Census {
    let e = evals.max(1) as f64;
    let (aw, r) = (geom.aw as f64, geom.r as f64);
    let misses: u64 = seg_misses.iter().sum();
    // only the first twelve segments carry a web projection
    let misses_web: u64 = seg_misses[..12].iter().sum();
    let hmax = geom.fc.iter().map(|&(_, o)| o).max().unwrap() as f64;
    let gather_adds = missed_ids as f64 * aw / e;
    let tokfill_stores = misses as f64 * aw / e;
    let proj_macs = misses_web as f64 * aw * r / e;
    let acc_adds = 12.0 * aw + 2.0 * 2.0 * aw;
    let cell_ops = 36.0 * r * 3.0;
    let xasm_ops = 2.0 * aw + 2.0 * r + DENSE_DIM as f64;
    let fc_macs: f64 = geom.fc.iter().map(|&(i, o)| (i * o + o) as f64).sum();
    let fc_relu_ops: f64 = geom.fc[..geom.fc.len() - 1].iter().map(|&(_, o)| o as f64).sum();
    let fit_stores = 2.0 * aw + 3.0 * r + 2.0 * hmax + value_v2_input_len(aw as usize, r as usize) as f64;
    let fit_dead_stores = r + value_v2_input_len(aw as usize, r as usize) as f64 + 2.0 * hmax;
    Census {
        gather_adds, tokfill_stores, proj_macs, acc_adds, cell_ops, xasm_ops, fc_macs, fc_relu_ops,
        fit_stores, fit_dead_stores,
        proj_fill_stores: misses_web as f64 * r / e,
        total_ops: gather_adds + tokfill_stores + proj_macs + acc_adds + cell_ops + xasm_ops
            + fc_macs + fc_relu_ops + fit_stores + misses_web as f64 * r / e,
    }
}

fn net_geometry(weights: &str, net: &LearnedValueV2) -> NetGeom {
    use std::io::Read;
    let mut head = [0u8; 128];
    std::fs::File::open(weights)
        .expect("weights must open")
        .read_exact(&mut head)
        .expect("weights header must read");
    assert_eq!(&head[..4], b"LVV2", "{weights} is not an LVV2 weights file");
    let u = |i: usize| u32::from_le_bytes(head[4 + 4 * i..8 + 4 * i].try_into().unwrap()) as usize;
    let (aw, r, n_fc) = (u(2), u(4), u(12));
    assert!((2..=9).contains(&n_fc), "fc chain of {n_fc} layers is outside the header window");
    let fc: Vec<(usize, usize)> = (0..n_fc).map(|k| (u(13 + 2 * k), u(14 + 2 * k))).collect();
    assert_eq!(poke_mcts::eval_learned::value_v2_input_len(aw, r), net.fc1_input(),
        "header acc_width {aw} / web_rank {r} disagree with the loaded net");
    assert_eq!(fc[0].0, net.fc1_input(), "header fc1 input disagrees with the loaded net");
    NetGeom { aw, r, fc }
}

struct RecordingEval<'a> {
    net: &'a LearnedValueV2,
    scratch: std::cell::RefCell<Vec<u32>>,
    trace: std::cell::RefCell<EvalTrace>,
}

impl Evaluator for RecordingEval<'_> {
    fn eval(&self, state: &BattleState) -> f32 {
        use poke_mcts::features;
        let mut ids = self.scratch.borrow_mut();
        ids.clear();
        let seg_lens = features::extract_segmented(state, &mut ids);
        let dense = features::extract_dense(state);
        self.trace.borrow_mut().push(&ids, seg_lens, dense);
        self.net.natural_logit(&ids, &seg_lens, &dense) * self.net.export_multiplier()
    }
}

#[derive(Clone, Copy)]
enum StageKind { Loop, Staged(u8), Natural }

fn replay_pass(net: &LearnedValueV2, trace: &EvalTrace, kind: StageKind) -> f32 {
    let mut sink = 0f32;
    for i in 0..trace.len() {
        let ids = std::hint::black_box(trace.ids_at(i));
        let seg_lens = std::hint::black_box(&trace.seg_lens[i]);
        let dense = std::hint::black_box(&trace.dense[i]);
        sink += match kind {
            StageKind::Loop => {
                std::hint::black_box((ids, seg_lens, dense));
                0.0
            }
            StageKind::Staged(s) => net.staged_logit(ids, seg_lens, dense, s),
            StageKind::Natural => net.natural_logit(ids, seg_lens, dense),
        };
    }
    sink
}

#[allow(clippy::too_many_arguments)]
fn bench_eval_real(fixture: &Fixture, net: &LearnedValueV2, weights: &str, world_idx: usize,
                   iters: u64, reps: usize, num_worlds: usize, world_seed: u64,
                   explore_coeff: f64) {
    use poke_mcts::determinize::Determinizer;
    use poke_mcts::eval_learned::value_stage::*;
    let (state, teams) = sweep_root(fixture);
    let mut belief = poke_mcts::belief::Belief::default();
    let om = state.active_mon(1);
    belief.note_species(om.species_id, om.level);
    let obs = poke_mcts::determinize::Observation { state: &state, teams: &teams, our_side: 0 };
    let mut rng = Lcg::new(splitmix64(world_seed));
    let worlds = poke_mcts::determinize::RandomBattle.sample_worlds(&obs, &belief, num_worlds, &mut rng);
    assert!(world_idx < worlds.len(), "--trace-world {world_idx} is outside the {num_worlds}-world set");
    let w = &worlds[world_idx];
    let seed = splitmix64(world_seed ^ (world_idx as u64).wrapping_mul(0x9E3779B97F4A7C15));

    let rec = RecordingEval {
        net,
        scratch: std::cell::RefCell::new(Vec::with_capacity(192)),
        trace: std::cell::RefCell::new(EvalTrace::default()),
    };
    let params = SearchParams { time_ms: u64::MAX, max_iters: iters, explore_coeff, ..Default::default() };
    let r = search_world(&w.state, &w.teams, &rec, &OpenLoop, &params, seed, 0, None);
    let trace = rec.trace.into_inner();
    let evals = trace.len();
    assert!(evals > 0, "the traced search produced no evaluations");

    let (hits, misses, seg_misses, missed_ids) = net.value_cache_probe_detail(&trace.probe_input());
    let lens: Vec<f64> = (0..evals).map(|i| trace.ids_at(i).len() as f64).collect();
    let (id_med, id_min, id_max, _) = med_spread(lens);
    println!("trace: world={world_idx} iters={} evals={evals} distinct_rows={} ids_median={id_med:.1} ids_min={id_min:.0} ids_max={id_max:.0} seg_hits={hits} seg_misses={misses} seg_hit_pct={:.2}% explore_coeff={explore_coeff:.2}",
        r.iterations, distinct_rows(&trace), 100.0 * hits as f64 / (hits + misses).max(1) as f64);

    let geom = net_geometry(weights, net);
    let c = census(&geom, evals as u64, &seg_misses, missed_ids);
    println!("census: gather_adds={:.1} tokfill_stores={:.1} proj_macs={:.1} acc_adds={:.1} cell_ops={:.1} xasm_ops={:.1} fc_macs={:.1} fc_relu_ops={:.1} fit_stores={:.1} fit_dead_stores={:.1} proj_fill_stores={:.1} zero_stores_total={:.1} total_ops={:.1}",
        c.gather_adds, c.tokfill_stores, c.proj_macs, c.acc_adds, c.cell_ops, c.xasm_ops,
        c.fc_macs, c.fc_relu_ops, c.fit_stores, c.fit_dead_stores, c.proj_fill_stores,
        c.fit_stores + c.tokfill_stores + c.proj_fill_stores, c.total_ops);
    let fc: Vec<String> = geom.fc.iter().map(|&(i, o)| format!("{i}x{o}")).collect();
    println!("census-raw: evals={evals} seg_misses={misses} missed_ids={missed_ids} missed_seg_web={} aw={} r={} fc=[{}] weights={weights}",
        seg_misses[..12].iter().sum::<u64>(), geom.aw, geom.r, fc.join(","));

    let stages: [(&str, StageKind); 17] = [
        ("loop", StageKind::Loop),
        ("tls", StageKind::Staged(S_TLS)),
        ("zero", StageKind::Staged(S_ZERO)),
        ("key", StageKind::Staged(S_KEY)),
        ("tokfill", StageKind::Staged(S_TOKFILL)),
        ("gather0", StageKind::Staged(S_GATHER0)),
        ("gather", StageKind::Staged(S_GATHER)),
        ("acc", StageKind::Staged(S_ACC)),
        ("proj", StageKind::Staged(S_PROJ)),
        ("cell0", StageKind::Staged(S_CELL0)),
        ("cell", StageKind::Staged(S_CELL)),
        ("xasm", StageKind::Staged(S_XASM)),
        ("full", StageKind::Staged(S_FULL)),
        ("full_nd", StageKind::Staged(S_FULL_ND)),
        ("full_2fc", StageKind::Staged(S_FULL_2FC)),
        ("full_2proj", StageKind::Staged(S_FULL_2PROJ)),
        ("natural", StageKind::Natural),
    ];
    let mut per_stage: Vec<Vec<f64>> = vec![Vec::with_capacity(reps); stages.len()];
    let mut sums: Vec<Vec<u32>> = vec![Vec::with_capacity(reps); stages.len()];
    for rep in 1..=reps {
        for (si, &(label, kind)) in stages.iter().enumerate() {
            poke_mcts::eval_learned::reset_value_scratch();
            std::hint::black_box(replay_pass(net, &trace, kind));
            poke_mcts::eval_learned::reset_value_scratch();
            let t0 = std::time::Instant::now();
            let sum = replay_pass(net, &trace, kind);
            std::hint::black_box(sum);
            let us = t0.elapsed().as_secs_f64() * 1e6 / evals as f64;
            per_stage[si].push(us);
            sums[si].push(sum.to_bits());
            println!("stage-rep rep={rep} stage={label} us_per_eval={us:.3}");
        }
    }

    let at = |name: &str| stages.iter().position(|&(l, _)| l == name).expect("stage must be listed");
    let full_i = at("full");
    for name in ["natural", "full_2fc", "full_2proj"] {
        let si = at(name);
        for rep in 0..reps {
            assert_eq!(sums[full_i][rep], sums[si][rep],
                "rep {}: staged full logit sum {:#x} != {name} logit sum {:#x}; that stage is not the served path",
                rep + 1, sums[full_i][rep], sums[si][rep]);
        }
    }

    let mut med = vec![0.0f64; stages.len()];
    for (si, &(label, _)) in stages.iter().enumerate() {
        let (m, lo, hi, spread) = med_spread(per_stage[si].clone());
        med[si] = m;
        let delta = match label {
            "loop" => "n/a".to_string(),
            "full_nd" | "full_2fc" | "full_2proj" | "natural" => format!("{:.3}", m - med[full_i]),
            _ => format!("{:.3}", m - med[si - 1]),
        };
        println!("stage {label}: us_per_eval={m:.3} min={lo:.3} max={hi:.3} spread_pct={spread:.2} delta_vs_prev={delta}");
    }
    let parts_sum: f64 = (1..=full_i).map(|si| med[si] - med[si - 1]).sum();
    let span = med[full_i] - med[0];
    let residual = span - parts_sum;
    let residual_pct = if span != 0.0 { 100.0 * residual / span } else { 0.0 };
    println!("stage-sum: full={:.3} loop={:.3} parts_sum={parts_sum:.3} residual={residual:.3} residual_pct={residual_pct:.2} evals={evals} reps={reps}",
        med[full_i], med[0]);

    let paired = |a: usize, b: usize| {
        stats((0..reps).map(|r| per_stage[a][r] - per_stage[b][r]).collect()).1
    };
    let fc_prefix = paired(full_i, at("xasm"));
    let fc_dup = paired(at("full_2fc"), full_i);
    let proj_prefix = paired(at("proj"), at("acc"));
    let proj_dup_per = paired(at("full_2proj"), full_i) / 12.0;
    let misses_web_per_eval = seg_misses[..12].iter().sum::<u64>() as f64 / evals as f64;
    let proj_prefix_per = proj_prefix / misses_web_per_eval;
    println!("crosscheck: fc_prefix={fc_prefix:.3} fc_dup={fc_dup:.3} fc_ratio={:.3} proj_prefix={proj_prefix:.3} proj_dup_per_projection={proj_dup_per:.4} proj_prefix_per_projection={proj_prefix_per:.4} proj_ratio={:.3} misses_web_per_eval={misses_web_per_eval:.3}",
        fc_dup / fc_prefix, proj_dup_per / proj_prefix_per);
}

#[repr(align(128))]
struct Slot([std::sync::atomic::AtomicU64; 4]);

impl Slot {
    fn new() -> Self {
        use std::sync::atomic::AtomicU64;
        Slot([AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)])
    }
}

thread_local! {
    static SHIM_IDS: std::cell::RefCell<Vec<u32>> =
        std::cell::RefCell::new(Vec::with_capacity(192));
}

struct TimingShim<'a, E: Evaluator + Sync> {
    inner: &'a E,
    net: Option<&'a LearnedValueV2>,
    slots: Vec<Slot>,
}

impl<'a, E: Evaluator + Sync> TimingShim<'a, E> {
    fn new(inner: &'a E, net: Option<&'a LearnedValueV2>, slots: usize) -> Self {
        TimingShim { inner, net, slots: (0..slots).map(|_| Slot::new()).collect() }
    }

    fn reset(&self) {
        for s in &self.slots {
            for a in &s.0 {
                a.store(0, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    fn totals(&self) -> [u64; 4] {
        let mut t = [0u64; 4];
        for s in &self.slots {
            for (i, a) in s.0.iter().enumerate() {
                t[i] += a.load(std::sync::atomic::Ordering::Relaxed);
            }
        }
        t
    }
}

impl<E: Evaluator + Sync> Evaluator for TimingShim<'_, E> {
    fn eval(&self, state: &BattleState) -> f32 {
        use poke_mcts::features;
        use std::sync::atomic::Ordering::Relaxed;
        let slot = &self.slots[rayon::current_thread_index().unwrap_or(0).min(self.slots.len() - 1)];
        match self.net {
            Some(net) => SHIM_IDS.with(|c| {
                let mut ids = c.borrow_mut();
                let t0 = std::time::Instant::now();
                ids.clear();
                let seg_lens = features::extract_segmented(state, &mut ids);
                let dense = features::extract_dense(state);
                let t1 = std::time::Instant::now();
                let v = net.natural_logit(&ids, &seg_lens, &dense) * net.export_multiplier();
                let t2 = std::time::Instant::now();
                slot.0[0].fetch_add(1, Relaxed);
                slot.0[1].fetch_add(t1.duration_since(t0).as_nanos() as u64, Relaxed);
                slot.0[2].fetch_add(t2.duration_since(t1).as_nanos() as u64, Relaxed);
                slot.0[3].fetch_add(t2.duration_since(t0).as_nanos() as u64, Relaxed);
                v
            }),
            None => {
                let t0 = std::time::Instant::now();
                let v = self.inner.eval(state);
                let t1 = std::time::Instant::now();
                slot.0[0].fetch_add(1, Relaxed);
                slot.0[3].fetch_add(t1.duration_since(t0).as_nanos() as u64, Relaxed);
                v
            }
        }
    }
}

fn insitu_wall(pool: &rayon::ThreadPool, worlds: &[poke_mcts::determinize::World],
               eval: &(impl Evaluator + Sync), params: &SearchParams, world_seed: u64) -> (f64, u64) {
    use rayon::prelude::*;
    let t0 = std::time::Instant::now();
    let its: Vec<u64> = pool.install(|| {
        worlds.par_iter().enumerate().map(|(k, w)| {
            let seed = splitmix64(world_seed ^ (k as u64).wrapping_mul(0x9E3779B97F4A7C15));
            search_world(&w.state, &w.teams, eval, &OpenLoop, params, seed, 0, None).iterations
        }).collect()
    });
    (t0.elapsed().as_secs_f64() * 1e6, its.iter().sum())
}

fn clock_cost(reps: usize) -> (f64, f64) {
    let (mut pairs, mut triples) = (Vec::with_capacity(reps), Vec::with_capacity(reps));
    for _ in 0..reps {
        pairs.push(1000.0 * timed_us(|| {
            let a = std::time::Instant::now();
            let b = std::time::Instant::now();
            std::hint::black_box(b.duration_since(std::hint::black_box(a)));
        }));
        triples.push(1000.0 * timed_us(|| {
            let a = std::time::Instant::now();
            let b = std::time::Instant::now();
            let c = std::time::Instant::now();
            std::hint::black_box((b.duration_since(std::hint::black_box(a)),
                                  c.duration_since(std::hint::black_box(b))));
        }));
    }
    (stats(pairs).1, stats(triples).1)
}

#[allow(clippy::too_many_arguments)]
fn bench_eval_insitu<E: Evaluator + Sync>(fixture: &Fixture, inner: &E, net: Option<&LearnedValueV2>,
                                          label: &str, num_worlds: usize, threads: usize,
                                          iters: u64, reps: usize, world_seed: u64,
                                          explore_coeff: f64) {
    use poke_mcts::determinize::Determinizer;
    let (state, teams) = sweep_root(fixture);
    let mut belief = poke_mcts::belief::Belief::default();
    let om = state.active_mon(1);
    belief.note_species(om.species_id, om.level);
    let obs = poke_mcts::determinize::Observation { state: &state, teams: &teams, our_side: 0 };
    let mut rng = Lcg::new(splitmix64(world_seed));
    let worlds = poke_mcts::determinize::RandomBattle.sample_worlds(&obs, &belief, num_worlds, &mut rng);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let params = SearchParams { time_ms: u64::MAX, max_iters: iters, explore_coeff, ..Default::default() };

    let shim = TimingShim::new(inner, net, threads + 1);
    let (mut walls, mut evals_us, mut extract_us, mut forward_us) =
        (Vec::with_capacity(reps), Vec::with_capacity(reps), Vec::with_capacity(reps), Vec::with_capacity(reps));
    let mut per_iter_evals = Vec::with_capacity(reps);
    let mut bare = Vec::with_capacity(reps);
    // the shimmed and bare arms alternate so machine drift lands on both alike
    for rep in 1..=reps {
        shim.reset();
        let (wall_us, iters_total) = insitu_wall(&pool, &worlds, &shim, &params, world_seed);
        let t = shim.totals();
        let e = t[0].max(1) as f64;
        let wall_per_iter = wall_us * num_worlds as f64 / iters_total.max(1) as f64;
        let (tt, xx, ff) = (t[3] as f64 / e / 1000.0, t[1] as f64 / e / 1000.0, t[2] as f64 / e / 1000.0);
        let split = net.map(|_| (xx, ff));
        walls.push(wall_per_iter);
        evals_us.push(tt);
        if let Some((x, f)) = split {
            extract_us.push(x);
            forward_us.push(f);
        }
        per_iter_evals.push(t[0] as f64 / iters_total.max(1) as f64);
        let (bare_us, bare_iters) = insitu_wall(&pool, &worlds, inner, &params, world_seed);
        let bare_per_iter = bare_us * num_worlds as f64 / bare_iters.max(1) as f64;
        bare.push(bare_per_iter);
        println!("insitu-rep rep={rep} total_wall_us_per_iter={wall_per_iter:.3} noshim_wall_us_per_iter={bare_per_iter:.3} evals={} eval_us_per_eval={tt:.3} extract_us_per_eval={} forward_us_per_eval={}",
            t[0], fmt_us(split.map(|s| s.0)), fmt_us(split.map(|s| s.1)));
    }
    let extract = if extract_us.is_empty() { None } else { Some(stats(extract_us).1) };
    let forward = if forward_us.is_empty() { None } else { Some(stats(forward_us).1) };
    let int8 = net.map_or_else(|| "n/a".to_string(), |n| n.int8_scope());
    println!("insitu {label}: worlds={num_worlds} threads={threads} iters={iters} reps={reps} eval_us_per_eval={:.3} extract_us_per_eval={} forward_us_per_eval={} total_wall_us_per_iter={:.3} evals_per_iter={:.3} explore_coeff={explore_coeff:.2} int8={int8}",
        stats(evals_us).1, fmt_us(extract), fmt_us(forward), stats(walls).1, stats(per_iter_evals).1);
    println!("insitu-noshim {label}: total_wall_us_per_iter={:.3} iters={iters} reps={reps} explore_coeff={explore_coeff:.2}", stats(bare).1);

    let (pair_ns, triple_ns) = clock_cost(reps);
    println!("clock: pair_ns={pair_ns:.2} triple_ns={triple_ns:.2} reps={reps}");
}

const DUMP_PER_GAME: usize = 4;
const DUMP_STRIDE: usize = 6;
const DUMP_MAX_GAMES: u64 = 4_000;
const DUMP_POOL_FACTOR: usize = 12;

// search_world returns before searching unless both seats have a real choice
fn is_decision_point(state: &BattleState) -> bool {
    !state.is_game_over()
        && state.phase == PHASE_ACTIONS
        && legal_actions(state, 0).count >= 2
        && legal_actions(state, 1).count >= 2
}

fn root_from_teams(t1: &[MonJson], t2: &[MonJson]) -> (BattleState, TeamData) {
    let (team1, b1, l1) = build(t1);
    let (team2, b2, l2) = build(t2);
    let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
    let mut state = BattleState::default();
    state.sides[0].team = team1;
    state.sides[1].team = team2;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);
    (state, teams)
}

fn collect_decision_points(fixture: &Fixture, want: usize, seed: u64)
    -> (Vec<(BattleState, TeamData)>, u64) {
    use poke_mcts::features;
    let nt = fixture.teams.len() as u64;
    let pool_target = want.saturating_mul(DUMP_POOL_FACTOR);
    let mut out: Vec<(BattleState, TeamData)> = Vec::with_capacity(pool_target);
    let mut seen: Vec<Vec<u32>> = Vec::new();
    let mut game = 0u64;
    while out.len() < pool_target && game < DUMP_MAX_GAMES {
        let gs = splitmix64(seed ^ game);
        let (ta, tb) = ((splitmix64(gs) % nt) as usize, (splitmix64(gs ^ 0xF00D) % nt) as usize);
        let (state, teams) = root_from_teams(&fixture.teams[ta], &fixture.teams[tb]);
        let beliefs = [poke_mcts::belief::Belief::default(); 2];
        // the offset walks with the game index so captures land on many turn numbers
        let offset = (game as usize) % DUMP_STRIDE;
        let greedy_side = (game % 2) as usize;
        let (mut eligible, mut taken) = (0usize, 0usize);
        let mut captured: Vec<(BattleState, TeamData)> = Vec::new();
        poke_mcts::selfplay::play_to_terminal(state, &teams, beliefs, gs,
            |side, st, tm, _bel, _sd, rng| {
                if side == 0 && taken < DUMP_PER_GAME && is_decision_point(st) {
                    if eligible >= offset && (eligible - offset) % DUMP_STRIDE == 0 {
                        captured.push((*st, tm.clone()));
                        taken += 1;
                    }
                    eligible += 1;
                }
                if side == greedy_side { greedy_action(st, side, rng) } else { random_action(st, side, rng) }
            });
        for p in captured {
            let mut ids = Vec::with_capacity(192);
            features::extract_segmented(&p.0, &mut ids);
            if seen.contains(&ids) {
                continue;
            }
            seen.push(ids);
            out.push(p);
        }
        game += 1;
    }
    (select_covering(out, want), game)
}

// the corpus is a must-carry list, not a sample: rows carrying a segment length
// nothing else in the corpus has are admitted first, then the rest fill in order
fn select_covering(pool: Vec<(BattleState, TeamData)>, want: usize)
    -> Vec<(BattleState, TeamData)> {
    use poke_mcts::features;
    let lens: Vec<Vec<u16>> = pool
        .iter()
        .map(|(s, _)| {
            let mut ids = Vec::with_capacity(192);
            features::extract_segmented(s, &mut ids).to_vec()
        })
        .collect();
    let mut have: Vec<u16> = Vec::new();
    let mut picked = vec![false; pool.len()];
    let mut order: Vec<usize> = Vec::with_capacity(want);
    for (i, seg) in lens.iter().enumerate() {
        if order.len() >= want {
            break;
        }
        if seg.iter().any(|l| !have.contains(l)) {
            for &l in seg {
                if !have.contains(&l) {
                    have.push(l);
                }
            }
            picked[i] = true;
            order.push(i);
        }
    }
    for i in 0..pool.len() {
        if order.len() >= want {
            break;
        }
        if !picked[i] {
            order.push(i);
        }
    }
    order.sort_unstable();
    let mut keep: Vec<Option<(BattleState, TeamData)>> = pool.into_iter().map(Some).collect();
    order.into_iter().map(|i| keep[i].take().expect("each index taken once")).collect()
}

fn mint_fixture_rows(net: &LearnedValueV2, pairs: &[(BattleState, TeamData)]) -> serde_json::Value {
    use poke_mcts::features;
    let mut states: Vec<BattleState> = pairs.iter().map(|(s, _)| *s).collect();
    // every decision point has both seats occupied, so the one shape that reads the forward's
    // active-cell zero fill back out has to be built rather than collected
    let mut empty_seat = states[0];
    let slot = (empty_seat.sides[0].active_index as usize).min(5);
    empty_seat.sides[0].team[slot] = Default::default();
    states.push(empty_seat);
    let rows: Vec<serde_json::Value> = states
        .iter()
        .enumerate()
        .map(|(i, state)| {
            let mut ids = Vec::with_capacity(192);
            let seg_lens = features::extract_segmented(state, &mut ids);
            let dense = features::extract_dense(state);
            let logit = net.natural_logit(&ids, &seg_lens, &dense);
            serde_json::json!({
                "index": i,
                "ids": ids,
                "seg_lens": seg_lens.to_vec(),
                "dense": dense.iter().map(|v| f64::from(*v)).collect::<Vec<f64>>(),
                "logit": f64::from(logit),
            })
        })
        .collect();
    serde_json::json!({
        "feature_spec_version": poke_mcts::features::FEATURE_SPEC_VERSION,
        "vocab": poke_mcts::features::vocab_size(),
        "export_multiplier": f64::from(net.export_multiplier()),
        "fixtures": rows,
    })
}

fn dump_search_states(fixture: &Fixture, from_snapshots: Option<&str>, reuse_states: bool,
                      states_out: &str, fixtures_out: &str, want: usize, seed: u64) {
    use poke_mcts::audit_snapshot::{pack_search_states, read_search_states, write_search_states};
    let (memory, games) = match from_snapshots {
        Some(dir) => {
            let n = pack_search_states(dir, states_out, want).expect("snapshot pack must succeed");
            println!("dump-search-states source=snapshots dir={dir} packed={n}");
            (None, 0)
        }
        // re-minting the rows off the states already on disk leaves them untouched; rolling
        // fresh games does not reproduce them
        None if reuse_states => {
            println!("dump-search-states source=existing states={states_out}");
            (None, 0)
        }
        None => {
            let (pairs, games) = collect_decision_points(fixture, want, seed);
            write_search_states(states_out, &pairs).expect("states must write");
            (Some(pairs), games)
        }
    };
    // the rows are minted from the deserialized pairs, so both corpora describe the states on disk
    let pairs = read_search_states(states_out).expect("written states must read back");
    assert_eq!(pairs.len(), want, "wrote {} of {want} requested pairs", pairs.len());
    if let Some(m) = &memory {
        assert_eq!(m[0], pairs[0], "the first pair must survive the round trip");
    }

    let net = LearnedValueV2::from_env();
    let json = mint_fixture_rows(&net, &pairs);
    std::fs::write(fixtures_out, serde_json::to_string(&json).unwrap()).expect("fixtures must write");

    let mut turns: Vec<u32> = pairs.iter().map(|(s, _)| s.field.turn as u32).collect();
    turns.sort_unstable();
    turns.dedup();
    let mut lens: Vec<u16> = Vec::new();
    let (mut zero_len_rows, mut multi_id_rows) = (0usize, 0usize);
    for f in json["fixtures"].as_array().unwrap() {
        let seg: Vec<u16> =
            f["seg_lens"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u16).collect();
        zero_len_rows += usize::from(seg.iter().any(|&l| l == 0));
        multi_id_rows += usize::from(seg.iter().any(|&l| l >= 2));
        for l in seg {
            if !lens.contains(&l) {
                lens.push(l);
            }
        }
    }
    lens.sort_unstable();
    println!("dump-search-states: pairs_read_back={} games={games} seed={seed} distinct_turns={} turn_max={} zero_len_rows={zero_len_rows} multi_id_rows={multi_id_rows} seg_lens_present={lens:?} states={states_out} fixtures={fixtures_out}",
        pairs.len(), turns.len(), turns.last().copied().unwrap_or(0));
}

fn tournament(fixture: &Fixture, games: u64, time_ms: u64, max_iters: u64, seed: u64) {
    let base = Entrant { kind: Kind::Random, time_ms, worlds: 16, max_iters, adaptive: false, chance_mode: poke_mcts::search::ChanceMode::OpenLoop, pick_mode: poke_mcts::driver::PickMode::Weighted, filter_threshold: 0.75, raw_root: false };
    let entrants: [(&str, Entrant); 5] = [
        ("random", Entrant { kind: Kind::Random, ..base }),
        ("greedy", Entrant { kind: Kind::Greedy, ..base }),
        ("mcts", Entrant { kind: Kind::Mcts, ..base }),
        ("pimc", Entrant { kind: Kind::Pimc, ..base }),
        ("pimc-adaptive", Entrant { kind: Kind::Pimc, adaptive: true, ..base }),
    ];
    let n = entrants.len();
    let nt = fixture.teams.len() as u64;
    let mut matrix = vec![vec![0.0f64; n]; n];
    let mut totals = vec![0.0f64; n];
    let mut pair_idx = 0u64;
    for i in 0..n {
        for j in (i + 1)..n {
            let (mut score, mut w, mut d, mut l) = (0.0f64, 0u64, 0u64, 0u64);
            for g in 0..games {
                let gs = seed ^ splitmix64(pair_idx * 1000 + g);
                let (ta, tb) = ((splitmix64(gs) % nt) as usize, (splitmix64(gs ^ 0xF00D) % nt) as usize);
                let v = if g % 2 == 0 {
                    play(entrants[i].1, entrants[j].1, &fixture.teams[ta], &fixture.teams[tb], gs)
                } else {
                    1.0 - play(entrants[j].1, entrants[i].1, &fixture.teams[ta], &fixture.teams[tb], gs)
                };
                score += v;
                if v > 0.6 { w += 1 } else if v < 0.4 { l += 1 } else { d += 1 }
                if (g + 1) % 20 == 0 {
                    eprintln!("[{} vs {}] [{}/{}] score {:.1}%", entrants[i].0, entrants[j].0, g + 1, games, 100.0 * score / (g + 1) as f64);
                }
            }
            let pct = 100.0 * score / games as f64;
            println!("pair {} vs {}: {:.1}%  (W{} D{} L{})", entrants[i].0, entrants[j].0, pct, w, d, l);
            matrix[i][j] = pct;
            matrix[j][i] = 100.0 - pct;
            totals[i] += score;
            totals[j] += games as f64 - score;
            pair_idx += 1;
        }
    }
    println!();
    print!("{:>14}", "");
    for &(name, _) in &entrants { print!("{:>14}", name); }
    println!();
    for i in 0..n {
        print!("{:>14}", entrants[i].0);
        for j in 0..n {
            if i == j { print!("{:>14}", "-"); } else { print!("{:>13.1}%", matrix[i][j]); }
        }
        println!();
    }
    println!();
    let per_entrant_games = (games * (n as u64 - 1)) as f64;
    let mut avg: Vec<(f64, &str)> = (0..n).map(|i| (100.0 * totals[i] / per_entrant_games, entrants[i].0)).collect();
    avg.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("avg score per entrant:");
    for (pct, name) in avg { println!("  {:<14} {:.1}%", name, pct); }
}

// Paired CRN head-to-head: each (team-pair, game-seed) played TWICE, swapping which entrant
// sits side 0. A-A null swaps the side-0 TEAM on the second seating so seat/team-order bias surfaces.
fn head_to_head(fixture: &Fixture, a: Entrant, b: Entrant, games: u64, seed: u64) {
    let nt = fixture.teams.len() as u64;
    let (mut a_w, mut b_w, mut draws, mut n) = (0u64, 0u64, 0u64, 0u64);
    for g in 0..games {
        let gs = seed ^ splitmix64(g);
        let (ta, tb) = ((splitmix64(gs) % nt) as usize, (splitmix64(gs ^ 0xF00D) % nt) as usize);
        let v_a0 = play(a, b, &fixture.teams[ta], &fixture.teams[tb], gs);
        let (tb0, tb1) = if a.kind == b.kind && a.chance_mode == b.chance_mode { (tb, ta) } else { (ta, tb) };
        let v_b0 = play(b, a, &fixture.teams[tb0], &fixture.teams[tb1], gs);
        for a_val in [v_a0, 1.0 - v_b0] {
            if a_val > 0.6 { a_w += 1 } else if a_val < 0.4 { b_w += 1 } else { draws += 1 }
            n += 1;
        }
    }
    let decisive = (a_w + b_w) as f64;
    let phat = if decisive > 0.0 { a_w as f64 / decisive } else { 0.5 };
    let se = if decisive > 0.0 { (phat * (1.0 - phat) / decisive).sqrt() } else { 0.0 };
    let (lo, hi) = (phat - 1.96 * se, phat + 1.96 * se);
    let z = if se > 0.0 { (phat - 0.5) / se } else { 0.0 };
    println!("head-to-head A={:?}/{:?} B={:?}/{:?}: n={} A_w={} B_w={} draws={}",
        a.kind, a.chance_mode, b.kind, b.chance_mode, n, a_w, b_w, draws);
    println!("  A win-rate (decisive) = {:.3}  95% CI [{:.3}, {:.3}]  z = {:.2}  significant = {}",
        phat, lo, hi, z, lo > 0.5 || hi < 0.5);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |flag: &str, default: &str| -> String {
        args.iter().position(|a| a == flag).map(|i| args[i + 1].clone()).unwrap_or_else(|| default.to_string())
    };
    let p1 = parse_kind(&get("--p1", "mcts"));
    let p2 = parse_kind(&get("--p2", "random"));
    let games: u64 = get("--games", "200").parse().unwrap();
    let time_ms: u64 = get("--time-ms", "50").parse().unwrap();
    let seed: u64 = get("--seed", "1").parse().unwrap();
    let worlds: usize = get("--worlds", "8").parse().unwrap();
    let max_iters: u64 = get("--max-iters", &u64::MAX.to_string()).parse().unwrap();
    let adaptive: bool = args.iter().any(|a| a == "--adaptive");
    let chance_mode = match get("--chance-mode", "open").as_str() {
        "open" => poke_mcts::search::ChanceMode::OpenLoop,
        "closed" => poke_mcts::search::ChanceMode::ClosedLoop,
        other => panic!("unknown --chance-mode {other}"),
    };
    let pick_mode = match get("--pick-mode", "weighted").as_str() {
        "weighted" => poke_mcts::driver::PickMode::Weighted,
        "argmax" => poke_mcts::driver::PickMode::Argmax,
        "value" => poke_mcts::driver::PickMode::Value,
        other => panic!("unknown --pick-mode {other}"),
    };
    let p1_pick = match get("--p1-pick-mode", get("--pick-mode", "weighted").as_str()).as_str() {
        "weighted" => poke_mcts::driver::PickMode::Weighted,
        "argmax" => poke_mcts::driver::PickMode::Argmax,
        "value" => poke_mcts::driver::PickMode::Value,
        other => panic!("unknown --p1-pick-mode {other}"),
    };
    let p2_pick = match get("--p2-pick-mode", get("--pick-mode", "weighted").as_str()).as_str() {
        "weighted" => poke_mcts::driver::PickMode::Weighted,
        "argmax" => poke_mcts::driver::PickMode::Argmax,
        "value" => poke_mcts::driver::PickMode::Value,
        other => panic!("unknown --p2-pick-mode {other}"),
    };
    let filter_threshold = if get("--filter-relax", "off") == "on" { 0.0 } else { 0.75 };
    let raw_root = args.iter().any(|a| a == "--raw-root");
    let teams_path = get("--teams", "data/fixture_teams.json");

    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(&teams_path).unwrap()).unwrap();
    if args.iter().any(|a| a == "--bench-search") {
        match get("--eval", "handcrafted").as_str() {
            "handcrafted" => bench(&fixture, &Handcrafted, poke_mcts::driver::EvalKind::Handcrafted, "handcrafted"),
            "learned" => {
                let le = LearnedEval::from_env();
                bench(&fixture, &le, poke_mcts::driver::EvalKind::Learned(&le), "learned");
            }
            "learned-v2" => {
                let lv = LearnedValueV2::from_env();
                bench(&fixture, &lv, poke_mcts::driver::EvalKind::LearnedV2(&lv), "learned_v2");
            }
            other => panic!("unknown --eval {other}"),
        }
        return;
    }
    if args.iter().any(|a| a == "--bench-sweep") {
        let sweep_worlds: usize = get("--sweep-worlds", "1").parse::<usize>().unwrap().max(1);
        let sweep_threads: usize = get("--sweep-threads", "1").parse::<usize>().unwrap().max(1);
        let sweep_seed: u64 = get("--sweep-seed", "1").parse().unwrap();
        assert!(args.iter().any(|a| a == "--max-iters"),
            "--bench-sweep requires --max-iters: the sweep must start high and settle downward");
        assert!(sweep_worlds == 1 || args.iter().any(|a| a == "--sweep-target-ms"),
            "--sweep-worlds above 1 requires --sweep-target-ms: the pooled shape has no safe default target");
        let target_ms: u64 = get("--sweep-target-ms", "100").parse().unwrap();
        let explore_coeff: f64 = get("--explore-coeff", "2.0").parse().unwrap();
        match get("--eval", "handcrafted").as_str() {
            "handcrafted" => bench_sweep(&fixture, &Handcrafted, "handcrafted", max_iters, target_ms, sweep_worlds, sweep_threads, sweep_seed, explore_coeff),
            "learned" => {
                let le = LearnedEval::from_env();
                bench_sweep(&fixture, &le, "learned", max_iters, target_ms, sweep_worlds, sweep_threads, sweep_seed, explore_coeff);
            }
            "learned-v2" => {
                let lv = LearnedValueV2::from_env();
                bench_sweep(&fixture, &lv, "learned_v2", max_iters, target_ms, sweep_worlds, sweep_threads, sweep_seed, explore_coeff);
            }
            other => panic!("unknown --eval {other}"),
        }
        return;
    }
    if args.iter().any(|a| a == "--bench-eval") {
        match get("--eval", "learned-v2").as_str() {
            "handcrafted" => bench_eval(&fixture, &Handcrafted, None, "handcrafted", "n/a", seed),
            "learned-v2" => {
                let lv = LearnedValueV2::from_env();
                let weights = std::env::var("BRIDGE_EVAL_WEIGHTS_V2").unwrap();
                bench_eval(&fixture, &lv, Some(&lv), "learned_v2", &weights, seed);
            }
            other => panic!("--bench-eval supports --eval learned-v2 and --eval handcrafted, not {other}"),
        }
        return;
    }
    if args.iter().any(|a| a == "--bench-eval-real") {
        let kind = get("--eval", "learned-v2");
        assert_eq!(kind, "learned-v2", "--bench-eval-real requires --eval learned-v2");
        let lv = LearnedValueV2::from_env();
        let weights = std::env::var("BRIDGE_EVAL_WEIGHTS_V2").unwrap();
        let trace_world: usize = get("--trace-world", "0").parse().unwrap();
        let trace_iters: u64 = get("--trace-iters", "19669").parse().unwrap();
        let trace_reps: usize = get("--trace-reps", "5").parse::<usize>().unwrap().max(1);
        let trace_worlds: usize = get("--trace-worlds", "8").parse::<usize>().unwrap().max(1);
        let trace_seed: u64 = get("--trace-seed", "1").parse().unwrap();
        let explore_coeff: f64 = get("--explore-coeff", "0.49").parse().unwrap();
        bench_eval_real(&fixture, &lv, &weights, trace_world, trace_iters, trace_reps, trace_worlds,
            trace_seed, explore_coeff);
        return;
    }
    if args.iter().any(|a| a == "--bench-eval-insitu") {
        let insitu_worlds: usize = get("--insitu-worlds", "1").parse::<usize>().unwrap().max(1);
        let insitu_threads: usize = get("--insitu-threads", "1").parse::<usize>().unwrap().max(1);
        let insitu_iters: u64 = get("--insitu-iters", "19669").parse().unwrap();
        let insitu_reps: usize = get("--insitu-reps", "5").parse::<usize>().unwrap().max(1);
        let insitu_seed: u64 = get("--insitu-seed", "1").parse().unwrap();
        let explore_coeff: f64 = get("--explore-coeff", "0.49").parse().unwrap();
        match get("--eval", "learned-v2").as_str() {
            "handcrafted" => bench_eval_insitu(&fixture, &Handcrafted, None, "handcrafted",
                insitu_worlds, insitu_threads, insitu_iters, insitu_reps, insitu_seed,
                explore_coeff),
            "learned-v2" => {
                let lv = LearnedValueV2::from_env();
                bench_eval_insitu(&fixture, &lv, Some(&lv), "learned_v2", insitu_worlds,
                    insitu_threads, insitu_iters, insitu_reps, insitu_seed, explore_coeff);
            }
            other => panic!("--bench-eval-insitu supports --eval learned-v2 and --eval handcrafted, not {other}"),
        }
        return;
    }
    if args.iter().any(|a| a == "--dump-search-states") {
        let states_out = get("--states-out", "tests/data/search_states.bin");
        let fixtures_out = get("--fixtures-out", "tests/data/lvv2-ext.fixtures.json");
        let want: usize = get("--states-count", "256").parse().unwrap();
        let from = args
            .iter()
            .position(|a| a == "--from-snapshots")
            .map(|i| args[i + 1].clone());
        let reuse = args.iter().any(|a| a == "--from-states");
        dump_search_states(&fixture, from.as_deref(), reuse, &states_out, &fixtures_out, want, seed);
        return;
    }
    if args.iter().any(|a| a == "--tournament") {
        tournament(&fixture, games, time_ms, max_iters, seed);
        return;
    }
    if args.iter().any(|a| a == "--head-to-head") {
        let w = if raw_root { 1 } else { worlds };
        let mk = |cm| Entrant { kind: Kind::Pimc, time_ms, worlds: w, max_iters, adaptive: false, chance_mode: cm, pick_mode, filter_threshold, raw_root };
        let closed = mk(poke_mcts::search::ChanceMode::ClosedLoop);
        let open = mk(poke_mcts::search::ChanceMode::OpenLoop);
        println!("== closed vs open ==");
        head_to_head(&fixture, closed, open, games, seed);
        println!("== A-A null: open vs open =="); head_to_head(&fixture, open, open, games, seed);
        println!("== A-A null: closed vs closed =="); head_to_head(&fixture, closed, closed, games, seed);
        return;
    }
    let e1 = Entrant { kind: p1, time_ms, worlds, max_iters, adaptive, chance_mode, pick_mode: p1_pick, filter_threshold: 0.75, raw_root: false };
    let e2 = Entrant { kind: p2, time_ms, worlds, max_iters, adaptive, chance_mode, pick_mode: p2_pick, filter_threshold: 0.75, raw_root: false };
    let nt = fixture.teams.len() as u64;
    let mut score = 0.0f64;
    let (mut w, mut d, mut l) = (0u64, 0u64, 0u64);
    for g in 0..games {
        // alternate seats so team/seat luck cancels
        let (ta, tb) = ((splitmix64(seed ^ g) % nt) as usize, (splitmix64(seed ^ g ^ 0xF00D) % nt) as usize);
        let v = if g % 2 == 0 {
            play(e1, e2, &fixture.teams[ta], &fixture.teams[tb], seed ^ g)
        } else {
            1.0 - play(e2, e1, &fixture.teams[ta], &fixture.teams[tb], seed ^ g)
        };
        score += v;
        if v > 0.6 { w += 1 } else if v < 0.4 { l += 1 } else { d += 1 }
        if (g + 1) % 20 == 0 { eprintln!("[{}/{}] p1 score {:.1}%", g + 1, games, 100.0 * score / (g + 1) as f64); }
    }
    println!("p1={:?} p2={:?} games={} -> p1 {:.1}% (W{} D{} L{})",
        get("--p1", "mcts"), get("--p2", "random"), games, 100.0 * score / games as f64, w, d, l);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_report(_: usize, _: u64, _: f64, _: u64, _: f64) {}

    struct ConstEval(f32);
    impl Evaluator for ConstEval {
        fn eval(&self, _state: &BattleState) -> f32 { self.0 }
    }


    #[test]
    fn the_trace_container_round_trips_variable_length_id_lists() {
        let mut t = EvalTrace::default();
        t.push(&[1, 2, 3], [0; NUM_SEGMENTS], [0.0; DENSE_DIM]);
        t.push(&[9], [1; NUM_SEGMENTS], [1.0; DENSE_DIM]);
        t.push(&[4, 5], [2; NUM_SEGMENTS], [2.0; DENSE_DIM]);
        assert_eq!(t.len(), 3);
        assert_eq!(t.ids_at(0), &[1, 2, 3]);
        assert_eq!(t.ids_at(1), &[9]);
        assert_eq!(t.ids_at(2), &[4, 5]);
    }

    #[test]
    fn distinct_rows_counts_a_repeated_row_once() {
        let mut t = EvalTrace::default();
        t.push(&[7, 8], [1; NUM_SEGMENTS], [0.5; DENSE_DIM]);
        t.push(&[7, 9], [1; NUM_SEGMENTS], [0.5; DENSE_DIM]);
        t.push(&[7, 8], [1; NUM_SEGMENTS], [0.5; DENSE_DIM]);
        t.push(&[7, 8], [1; NUM_SEGMENTS], [0.25; DENSE_DIM]);
        assert_eq!(t.len(), 4);
        assert_eq!(distinct_rows(&t), 3);
    }

    #[test]
    fn the_spread_helper_matches_the_hand_computed_values() {
        let (med, min, max, spread) = med_spread(vec![4.0, 1.0, 2.0, 3.0, 10.0]);
        assert_eq!((min, med, max), (1.0, 3.0, 10.0));
        assert!((spread - 300.0).abs() < 1e-9, "spread was {spread}");
    }

    #[test]
    fn the_census_totals_the_hand_summed_op_count() {
        let geom = NetGeom { aw: 4, r: 2, fc: vec![(11, 3), (3, 1)] };
        let mut seg_misses = [0u64; NUM_SEGMENTS];
        seg_misses[0] = 2;
        seg_misses[12] = 4;
        let c = census(&geom, 2, &seg_misses, 6);
        // gather 6*4/2, tokfill 6*4/2, proj 2*4*2/2, acc 16*4, cell 36*2*3,
        // xasm 2*4+2*2+7, fc 11*3+3+3*1+1, fc relu 3, fit 2*4+3*2+2*3+19, proj fill 2*2/2
        assert_eq!(c.gather_adds, 12.0);
        assert_eq!(c.tokfill_stores, 12.0);
        assert_eq!(c.proj_macs, 8.0);
        assert_eq!(c.fit_stores, 39.0);
        assert_eq!(c.fit_dead_stores, 2.0 + 19.0 + 6.0);
        assert_eq!(c.proj_fill_stores, 2.0);
        assert_eq!(c.fc_relu_ops, 3.0);
        assert_eq!(c.total_ops, 12.0 + 12.0 + 8.0 + 64.0 + 216.0 + 19.0 + 40.0 + 3.0 + 39.0 + 2.0);
    }

    #[test]
    fn the_shim_counter_slot_is_padded_past_a_cache_line() {
        assert!(std::mem::size_of::<Slot>() >= 128, "slot is {} bytes", std::mem::size_of::<Slot>());
    }

    #[test]
    fn the_counting_evaluator_counts_every_call() {
        let inner = ConstEval(0.25);
        let counter = CountingEval::new(&inner);
        let state = BattleState::default();
        assert_eq!(counter.count(), 0);
        for _ in 0..7 {
            counter.eval(&state);
        }
        assert_eq!(counter.count(), 7);
    }

    #[test]
    fn the_counting_evaluator_returns_the_inner_value_unchanged() {
        let inner = ConstEval(-3.5);
        let counter = CountingEval::new(&inner);
        let state = BattleState::default();
        assert_eq!(counter.eval(&state), -3.5);
        assert_eq!(counter.count(), 1);
    }

    #[test]
    fn sweep_settles_when_cost_grows_with_the_iteration_count() {
        // us(N) = 2.0 + N*1e-5 has its 100 ms fixed point at N = 41421.
        let (n, us, sweeps, unsettled) =
            sweep_fixed_point(1_000, 100, |n| 2.0 + n as f64 * 1e-5, noop_report);
        assert_eq!(sweeps, 5, "converging model must settle on the fifth sweep");
        assert!(!unsettled, "a converging model must not be flagged unsettled");
        assert_eq!(n, 41_380);
        assert!((us - 2.4138).abs() < 1e-9, "us was {us}");
    }

    #[test]
    fn oscillating_cost_gives_up_at_eight_sweeps_with_the_median_of_the_last_three() {
        // 1000 -> 100000 -> 1000 -> ... ; last three Ns are [100000, 1000, 100000].
        let (n, _us, sweeps, unsettled) = sweep_fixed_point(
            1_000,
            100,
            |n| if n < 10_000 { 1.0 } else { 100.0 },
            noop_report,
        );
        assert_eq!(sweeps, SWEEP_MAX);
        assert!(unsettled, "an oscillating model must be flagged unsettled");
        assert_eq!(n, 100_000, "median N of the last three sweeps");
    }

    #[test]
    fn one_small_delta_alone_does_not_settle_the_sweep() {
        // sweep 0 is already at its fixed point, sweep 1 jumps away, then it re-converges.
        let mut call = 0usize;
        let (n, _us, sweeps, unsettled) = sweep_fixed_point(
            1_000,
            100,
            |_n| {
                call += 1;
                if call == 1 { 100.0 } else { 50.0 }
            },
            noop_report,
        );
        assert!(!unsettled);
        assert_eq!(n, 2_000, "must not stop at the lone sub-tolerance sweep 0 (N=1000)");
        assert_eq!(sweeps, 4);
    }

    #[test]
    fn every_sweep_is_reported_as_it_lands() {
        let mut seen: Vec<(usize, u64, u64)> = Vec::new();
        let (_n, _us, sweeps, _unsettled) = sweep_fixed_point(
            1_000,
            100,
            |n| 2.0 + n as f64 * 1e-5,
            |k, n, _us, next_n, _delta| seen.push((k, n, next_n)),
        );
        assert_eq!(seen.len(), sweeps, "one report per sweep");
        assert_eq!(seen[0].0, 0);
        assert_eq!(seen[0].1, 1_000, "first sweep runs at the starting N");
        for w in seen.windows(2) {
            assert_eq!(w[0].2, w[1].1, "each sweep runs at the previous sweep's next_N");
        }
    }
}
