use poke_mcts::chance::OpenLoop;
use poke_mcts::chance_analytic::AnalyticRoot;
use poke_mcts::eval::{Evaluator, Handcrafted};
use poke_mcts::eval_learned::{LearnedEval, LearnedValueV2};
use poke_mcts::fixtures::{build, Fixture, MonJson};
use poke_mcts::policies::{greedy_action, random_action};
use poke_mcts::rng::{splitmix64, Lcg};
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

fn sweep_single(fixture: &Fixture, eval: &impl Evaluator, label: &str, start_n: u64, target_ms: u64) {
    let (state, teams) = sweep_root(fixture);
    let seen_iters = std::cell::Cell::new(0u64);
    let (n, us, sweeps, unsettled) = sweep_fixed_point(
        start_n,
        target_ms,
        |n| {
            let params = SearchParams { time_ms: u64::MAX, max_iters: n, ..Default::default() };
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
    println!("settled {label}: N={n} us_per_iter={us:.3} sweeps={sweeps} target_ms={target_ms} unsettled={unsettled}");
}

struct SweepSnapshot { n: u64, iterations: u64, per_world_us: Vec<f64>, mean_us: f64 }

#[allow(clippy::too_many_arguments)]
fn sweep_pooled(fixture: &Fixture, eval: &(impl Evaluator + Sync), label: &str, start_n: u64,
                target_ms: u64, num_worlds: usize, threads: usize, world_seed: u64) {
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
            let params = SearchParams { time_ms: u64::MAX, max_iters: n, ..Default::default() };
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
    println!("settled8 {label}: N_8w={n} mean_us_per_iter={mean_us:.3} worlds={num_worlds} threads={threads} target_ms={target_ms} seed={world_seed} sweeps={sweeps} unsettled={unsettled}");
}

#[allow(clippy::too_many_arguments)]
fn bench_sweep(fixture: &Fixture, eval: &(impl Evaluator + Sync), label: &str, start_n: u64,
               target_ms: u64, num_worlds: usize, threads: usize, world_seed: u64) {
    if num_worlds > 1 {
        sweep_pooled(fixture, eval, label, start_n, target_ms, num_worlds, threads, world_seed);
    } else {
        sweep_single(fixture, eval, label, start_n, target_ms);
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
        match get("--eval", "handcrafted").as_str() {
            "handcrafted" => bench_sweep(&fixture, &Handcrafted, "handcrafted", max_iters, target_ms, sweep_worlds, sweep_threads, sweep_seed),
            "learned" => {
                let le = LearnedEval::from_env();
                bench_sweep(&fixture, &le, "learned", max_iters, target_ms, sweep_worlds, sweep_threads, sweep_seed);
            }
            "learned-v2" => {
                let lv = LearnedValueV2::from_env();
                bench_sweep(&fixture, &lv, "learned_v2", max_iters, target_ms, sweep_worlds, sweep_threads, sweep_seed);
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
