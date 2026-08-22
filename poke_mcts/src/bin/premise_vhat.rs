#[cfg(not(feature = "train_value"))]
compile_error!("premise_vhat needs --features train_value: v-hat reads SearchResult::value_sum");

use poke_mcts::belief::Belief;
use poke_mcts::determinize::{Observation, RandomBattle};
use poke_mcts::driver::{choose_action_eval_iters, EvalKind, PickMode, PimcConfig};
use poke_mcts::eval::{Evaluator, Handcrafted};
use poke_mcts::eval_learned::LearnedValueV2;
use poke_mcts::fixtures::{build, Fixture};
use poke_mcts::rng::splitmix64;
use poke_mcts::search::{search_world, ChanceMode, SearchParams};
use poke_mcts::selfplay::{determinize_ground_truth, play_to_terminal};
use pkmn_engine::state::*;
use rayon::prelude::*;

const GEN_WORLDS: usize = 4;
const GEN_TIME_MS: u64 = 50;
const GEN_FILTER: f64 = 0.75;
const EXPLORE_COEFF: f64 = 0.49; // c = 0.7, and explore_coeff is c SQUARED
const SCORE_TIME_MS: u64 = 50;
const WORLD_SALT: u64 = 0x9E37_79B9_7F4A_7C15;
const TAG_SALT: u64 = 0xBF58_476D_1CE4_E5B9;
const ROW_SALT: u64 = 0x94D0_49BB_1331_11EB;
const HOLDOUT_MOD: u64 = 10;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Row {
    game_index: u32,
    game_tag: u64,
    gen_eval: u8, // 0 = handcrafted-driven game, 1 = net-driven game
    turn: u16,
    side: u8,
    z: f32,
    state: BattleState,
    teams: TeamData,
}

fn arg(args: &[String], k: &str, d: &str) -> String {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| d.to_string())
}

// search_world returns before searching unless both seats have a real choice
fn is_decision_point(state: &BattleState) -> bool {
    !state.is_game_over()
        && state.phase == PHASE_ACTIONS
        && legal_actions(state, 0).count >= 2
        && legal_actions(state, 1).count >= 2
}

fn root_from_teams(f: &Fixture, a: usize, b: usize) -> (BattleState, TeamData) {
    let (t1, b1, l1) = build(&f.teams[a]);
    let (t2, b2, l2) = build(&f.teams[b]);
    let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
    let mut state = BattleState::default();
    state.sides[0].team = t1;
    state.sides[1].team = t2;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);
    (state, teams)
}

fn gen(args: &[String]) {
    let teams_path = arg(args, "--teams", "data/fixture_teams.json");
    let out = arg(args, "--out", "premise_rows.bin");
    let seed: u64 = arg(args, "--seed", "1").parse().unwrap();
    let games: u64 = arg(args, "--games", "10").parse().unwrap();
    let start: u64 = arg(args, "--start", "0").parse().unwrap();
    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(&teams_path).unwrap()).unwrap();
    let nt = fixture.teams.len() as u64;
    let net = LearnedValueV2::from_env();
    println!("gen: teams={teams_path} n_teams={nt} seed={seed} start={start} games={games} worlds={GEN_WORLDS} time_ms={GEN_TIME_MS} explore_coeff={EXPLORE_COEFF} pick=argmax chance=open");

    let mut all: Vec<Row> = Vec::new();
    let mut kept = 0u64;
    let mut skipped_holdout = 0u64;
    let mut skipped_draw = 0u64;
    for gi in start..start + games {
        let game_tag = splitmix64(seed ^ TAG_SALT ^ gi.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        if (game_tag >> 1) % HOLDOUT_MOD == 0 {
            skipped_holdout += 1;
            continue;
        }
        let gen_eval = (gi % 2) as u8;
        let eval = if gen_eval == 0 { EvalKind::Handcrafted } else { EvalKind::LearnedV2(&net) };
        let gs = splitmix64(seed ^ gi);
        let (ta, tb) = ((splitmix64(gs) % nt) as usize, (splitmix64(gs ^ 0xF00D) % nt) as usize);
        let (state, teams) = root_from_teams(&fixture, ta, tb);
        let mut rows: Vec<Row> = Vec::new();
        let mut ord: u64 = 0;
        let t0 = std::time::Instant::now();
        let z = play_to_terminal(state, &teams, [Belief::default(); 2], gs,
            |side, st, tm, bel: &[Belief; 2], dseed, _rng| {
                if is_decision_point(st) {
                    ord += 1;
                    let wseed = splitmix64(gs ^ WORLD_SALT ^ (ord << 3) ^ side as u64);
                    let (ws, wt) = determinize_ground_truth(st, tm, &bel[side], side, wseed);
                    rows.push(Row {
                        game_index: gi as u32,
                        game_tag,
                        gen_eval,
                        turn: st.field.turn,
                        side: side as u8,
                        z: 0.0,
                        state: ws,
                        teams: wt,
                    });
                }
                let obs = Observation { state: st, teams: tm, our_side: side };
                let cfg = PimcConfig {
                    num_worlds: GEN_WORLDS,
                    time_ms_per_world: GEN_TIME_MS,
                    max_iters_per_world: u64::MAX,
                    seed: dseed,
                    chance_mode: ChanceMode::OpenLoop,
                    pick_mode: PickMode::Argmax,
                    filter_threshold: GEN_FILTER,
                    raw_root: false,
                    explore_coeff: EXPLORE_COEFF,
                };
                choose_action_eval_iters(&obs, &bel[side], &RandomBattle, &cfg, eval, None).0
            });
        if z == 0.5 {
            skipped_draw += 1;
            continue;
        }
        for r in rows.iter_mut() {
            r.z = z as f32;
        }
        kept += 1;
        println!("game gi={gi} tag={game_tag} gen_eval={gen_eval} z={z} rows={} wall_s={:.1}", rows.len(), t0.elapsed().as_secs_f64());
        all.extend(rows);
    }
    let bytes = bincode::serialize(&all).unwrap();
    std::fs::write(&out, bytes).unwrap();
    println!("gen-done out={out} games_kept={kept} skipped_holdout={skipped_holdout} skipped_draw={skipped_draw} rows={}", all.len());
}

fn vhat(r: &poke_mcts::search::SearchResult) -> Option<f64> {
    if r.value_count == 0 { None } else { Some(r.value_sum / r.value_count as f64) }
}

fn score(args: &[String]) {
    let corpus = arg(args, "--corpus", "premise_rows.bin");
    let out = arg(args, "--out", "premise_scores.tsv");
    let threads: usize = arg(args, "--threads", "6").parse().unwrap();
    let bytes = std::fs::read(&corpus).unwrap();
    let rows: Vec<Row> = bincode::deserialize(&bytes).unwrap();
    let net = LearnedValueV2::from_env();
    let wpath = std::env::var("BRIDGE_EVAL_WEIGHTS_V2").unwrap();

    // binding probe: the two evaluators must disagree on real rows, or the "net" arm is the hand arm
    for k in [0usize, rows.len() / 2, rows.len() - 1] {
        let (h, n) = (Handcrafted.eval(&rows[k].state), net.eval(&rows[k].state));
        assert!((h - n).abs() > 1e-6, "binding probe row {k}: handcrafted {h} == net {n}");
        println!("probe row={k} hand={h:.6} net={n:.6}");
    }
    println!("score: corpus={corpus} rows={} weights={wpath} threads={threads} time_ms={SCORE_TIME_MS} explore_coeff={EXPLORE_COEFF} chance=open", rows.len());
    let base = SearchParams { time_ms: SCORE_TIME_MS, max_iters: u64::MAX, explore_coeff: EXPLORE_COEFF, ..Default::default() };
    assert_eq!(base.explore_coeff, 0.49, "explore_coeff must echo c^2 = 0.49");

    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let lines: Vec<String> = pool.install(|| {
        rows.par_iter().enumerate().map(|(i, row)| {
            let seed = splitmix64(ROW_SALT ^ i as u64);
            let side = row.side as usize;

            let t1 = std::time::Instant::now();
            let r1 = search_world(&row.state, &row.teams, &Handcrafted, &poke_mcts::chance::OpenLoop, &base, seed, side, None);
            let ms1 = t1.elapsed().as_secs_f64() * 1e3;

            let t2 = std::time::Instant::now();
            let r2 = search_world(&row.state, &row.teams, &net, &poke_mcts::chance::OpenLoop, &base, seed, side, None);
            let ms2 = t2.elapsed().as_secs_f64() * 1e3;

            let p3 = SearchParams { time_ms: u64::MAX, max_iters: r1.iterations, ..base };
            let t3 = std::time::Instant::now();
            let r3 = search_world(&row.state, &row.teams, &net, &poke_mcts::chance::OpenLoop, &p3, seed, side, None);
            let ms3 = t3.elapsed().as_secs_f64() * 1e3;

            // wall-parity sensitivity arm: the clock check fires every 1024 iterations, so a
            // net search overshoots a 50 ms budget by a whole block; this rescales its
            // iteration cap to the wall the hand arm actually spent.
            let cap = ((r2.iterations as f64) * (ms1 / ms2.max(1e-9))).floor().max(1.0) as u64;
            let p2w = SearchParams { time_ms: u64::MAX, max_iters: cap, ..base };
            let t2w = std::time::Instant::now();
            let r2w = search_world(&row.state, &row.teams, &net, &poke_mcts::chance::OpenLoop, &p2w, seed, side, None);
            let ms2w = t2w.elapsed().as_secs_f64() * 1e3;

            let f = |o: Option<f64>| o.map_or("nan".to_string(), |v| format!("{v:.9}"));
            format!("{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.3}\t{}\t{}\t{:.3}\t{}\t{}\t{:.3}\t{}\t{}\t{:.3}\t{}",
                row.game_index, row.game_tag, row.gen_eval, row.turn, row.side, row.z,
                r1.iterations, ms1, f(vhat(&r1)),
                r2.iterations, ms2, f(vhat(&r2)),
                r3.iterations, ms3, f(vhat(&r3)),
                r2w.iterations, ms2w, f(vhat(&r2w)))
        }).collect()
    });
    let mut s = String::from("row\tgame_index\tgame_tag\tgen_eval\tturn\tside\tz\titers1\tms1\tvhat1\titers2\tms2\tvhat2\titers3\tms3\tvhat3\titers2w\tms2w\tvhat2w\n");
    for l in lines { s.push_str(&l); s.push('\n'); }
    std::fs::write(&out, s).unwrap();
    println!("score-done out={out}");
}

fn pick(args: &[String]) {
    let corpus = arg(args, "--corpus", "premise_rows.bin");
    let out = arg(args, "--out", "premise_picks.tsv");
    let threads: usize = arg(args, "--threads", "6").parse().unwrap();
    let iters: u64 = arg(args, "--iters", "4096").parse().unwrap();
    let seed_salt: u64 = arg(args, "--seed-salt", &ROW_SALT.to_string()).parse().unwrap();
    let temp: f32 = arg(args, "--temp", "1").parse().unwrap();
    assert!(temp > 0.0, "--temp must be > 0, got {temp}");
    let bytes = std::fs::read(&corpus).unwrap();
    let rows: Vec<Row> = bincode::deserialize(&bytes).unwrap();
    let net = LearnedValueV2::from_env();
    let wpath = std::env::var("BRIDGE_EVAL_WEIGHTS_V2").unwrap();

    // binding probe: the two evaluators must disagree on real rows, or the "net" arm is the hand arm
    for k in [0usize, rows.len() / 2, rows.len() - 1] {
        let (h, n) = (Handcrafted.eval(&rows[k].state), net.eval(&rows[k].state));
        assert!((h - n).abs() > 1e-6, "binding probe row {k}: handcrafted {h} == net {n}");
        println!("probe row={k} hand={h:.6} net={n:.6}");
    }
    println!("pick: corpus={corpus} rows={} weights={wpath} threads={threads} iters={iters} seed_salt={seed_salt} temp={temp} explore_coeff={EXPLORE_COEFF} chance=open", rows.len());
    // fixed iteration cap: the deadline check is batched every 1024 iterations, so a time budget would let evaluator cost move the compute
    let params = SearchParams { time_ms: u64::MAX, max_iters: iters, explore_coeff: EXPLORE_COEFF, value_temp: temp, ..Default::default() };
    assert_eq!(params.explore_coeff, 0.49, "explore_coeff must echo c^2 = 0.49");

    let fmt_arms = |st: &[poke_mcts::search::ArmStat]| st.iter()
        .map(|a| format!("{}:{}:{:016x}:{:016x}", a.action, a.visits, a.avg_score.to_bits(), a.win_chance.to_bits()))
        .collect::<Vec<String>>()
        .join(";");

    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let lines: Vec<String> = pool.install(|| {
        rows.par_iter().enumerate().map(|(i, row)| {
            let seed = splitmix64(seed_salt ^ i as u64);
            let side = row.side as usize;
            let r = search_world(&row.state, &row.teams, &net, &poke_mcts::chance::OpenLoop, &params, seed, side, None);
            let per_world = vec![(r.side(side).to_vec(), 1.0f64)];
            let agg = poke_mcts::driver::aggregate(&per_world);
            let pick = poke_mcts::driver::pick_from(&agg, &legal_actions(&row.state, side), &mut poke_mcts::rng::Lcg::new(seed), PickMode::Argmax, GEN_FILTER);
            format!("{i}\t{}\t{}\t{}\t{}\t{}\t{pick}\t{}\t{}\t{}\t{}\t{}",
                row.game_index, row.game_tag, row.gen_eval, row.turn, row.side,
                r.iterations, r.guard_hits, r.depth_sum, fmt_arms(&r.s1), fmt_arms(&r.s2))
        }).collect()
    });
    let mut s = String::from("row\tgame_index\tgame_tag\tgen_eval\tturn\tside\tpick\titerations\tguard_hits\tdepth_sum\tarms1\tarms2\n");
    for l in lines { s.push_str(&l); s.push('\n'); }
    std::fs::write(&out, s).unwrap();
    println!("pick-done out={out}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--gen") {
        gen(&args);
    } else if args.iter().any(|a| a == "--score") {
        score(&args);
    } else if args.iter().any(|a| a == "--pick") {
        pick(&args);
    } else {
        panic!("premise_vhat needs --gen, --score or --pick");
    }
}
