use pkmn_engine::state::team_builder::showdown_type_to_engine;
use pkmn_engine::state::{data_bridge, effective_moves, legal_actions, move_base_pp, BattleState, MonBuildData, MonSlot, TeamData, MON_FLAG_FEMALE, ACTION_SWITCH_0, ACTION_SWITCH_5, ACTION_TERA_0, ACTION_TERA_3, STATUS_BAD_POISON, STATUS_SLEEP, VOL_TRANSFORMED};
use poke_mcts::belief::{possible, species_sets_with_base_fallback, Belief, MonBelief};
use poke_mcts::chance::OpenLoop;
use poke_mcts::determinize::{Determinizer, Observation, RandomBattle};
use poke_mcts::driver::{aggregate, choose_action_traced, choose_action_traced_salted, pick_from, PickMode, PimcConfig, WorldTrace};
use poke_mcts::eval::{Evaluator, Handcrafted};
use poke_mcts::eval_learned::LearnedValueV2;
use poke_mcts::frontier::{gen_team, read_all, read_rows, Declairvoyant, NativeSnapshot, PlayoutEval, PlayoutPolicy, Row, PLAYOUT_STEP_CAP};
use poke_mcts::gen_sets::{SetEntry, GEN9_SET_POOL};
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::search::{search_world, ArmStat, ChanceMode, SearchParams, NO_ARM};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

const EXPLORE_COEFF: f64 = 0.49; // c = 0.7, and explore_coeff is c squared
const _: () = assert!(EXPLORE_COEFF == 0.49);
const TIME_MS: u64 = 60_000;
const PICK_FILTER: f64 = 0.75;
const ROW_SALT: u64 = 0x94D0_49BB_1331_11EB;
const RESEED_SALT: u64 = 0xD6E8_FEB8_6659_FD93;
const DEFAULT_ROWS: &str = "../.decompose/Learned-Eval/leaf-temp/corpus/rows.0.bin.gz,../.decompose/Learned-Eval/leaf-temp/corpus/rows.1.bin.gz";

fn arg(args: &[String], k: &str, d: &str) -> String {
    match args.iter().position(|a| a == k) {
        Some(i) => args.get(i + 1).cloned().unwrap_or_else(|| panic!("{k} needs a value")),
        None => d.to_string(),
    }
}

fn parse<T: std::str::FromStr>(args: &[String], k: &str, d: &str) -> T
where
    T::Err: std::fmt::Display,
{
    let v = arg(args, k, d);
    v.parse().unwrap_or_else(|e| panic!("{k} {v:?}: {e}"))
}

fn list<T: std::str::FromStr>(args: &[String], k: &str, d: &str) -> Vec<T>
where
    T::Err: std::fmt::Display,
{
    arg(args, k, d).split(',').map(|x| x.parse().unwrap_or_else(|e| panic!("{k} {x:?}: {e}"))).collect()
}

// keyed on the run seed and the row's index in the loaded vector (a --limit prefix keeps the
// full-corpus index), so the thread count cannot move a search seed
fn row_seed(run_seed: u64, row: usize) -> u64 {
    splitmix64(run_seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ ROW_SALT ^ row as u64)
}

fn full_pp(m: u16) -> u8 {
    (move_base_pp(m) as u16 * 8 / 5) as u8
}

// both tie-breaks keep the lowest slot index
fn highest_pp_slot(mon: &MonSlot) -> Option<usize> {
    (0..4).filter(|&i| mon.moves[i] != 0).max_by_key(|&i| (mon.pp[i], std::cmp::Reverse(i)))
}

fn lowest_pp_slot(mon: &MonSlot) -> Option<usize> {
    (0..4).filter(|&i| mon.moves[i] != 0).min_by_key(|&i| (mon.pp[i], i))
}

#[derive(Clone, Copy)]
enum Field {
    ToxicCounter,
    ToxicStatusCounter,
    HighPpToOne,
    LowPpToFull,
}

const FIELDS: [Field; 4] = [Field::ToxicCounter, Field::ToxicStatusCounter, Field::HighPpToOne, Field::LowPpToFull];
const N_FIELDS: usize = FIELDS.len();

impl Field {
    fn name(self) -> &'static str {
        match self {
            Field::ToxicCounter => "badpoison_toxic_counter",
            Field::ToxicStatusCounter => "badpoison_status_counter",
            Field::HighPpToOne => "highpp_to_one",
            Field::LowPpToFull => "lowpp_to_full",
        }
    }

    fn applies(self, state: &BattleState, side: usize) -> bool {
        let mon = state.active_mon(side);
        match self {
            Field::ToxicCounter => mon.status == STATUS_BAD_POISON && state.sides[side].active.toxic_counter <= 2,
            Field::ToxicStatusCounter => mon.status == STATUS_BAD_POISON && mon.status_counter <= 2,
            Field::HighPpToOne => highest_pp_slot(mon).is_some_and(|i| mon.pp[i] >= 8),
            Field::LowPpToFull => lowest_pp_slot(mon).is_some_and(|i| mon.pp[i] <= 4),
        }
    }

    fn perturb(self, state: &mut BattleState, side: usize) {
        match self {
            Field::ToxicCounter => state.sides[side].active.toxic_counter = 5,
            Field::ToxicStatusCounter => state.active_mon_mut(side).status_counter = 5,
            Field::HighPpToOne => {
                let mon = state.active_mon_mut(side);
                mon.pp[highest_pp_slot(mon).unwrap()] = 1;
            }
            Field::LowPpToFull => {
                let mon = state.active_mon_mut(side);
                let i = lowest_pp_slot(mon).unwrap();
                mon.pp[i] = full_pp(mon.moves[i]);
            }
        }
    }
}

fn load(args: &[String]) -> Vec<Row> {
    let paths: Vec<String> = list(args, "--rows", DEFAULT_ROWS);
    let mut rows = read_rows(&paths).unwrap_or_else(|e| panic!("read_rows {paths:?}: {e}"));
    let skip: usize = parse(args, "--skip", "0");
    rows.drain(..skip.min(rows.len()));
    let limit: usize = parse(args, "--limit", &rows.len().to_string());
    rows.truncate(limit);
    println!("rows: paths={paths:?} skip={skip} rows={}", rows.len());
    rows
}

fn count(args: &[String]) {
    let rows = load(args);
    let mut n = [0usize; N_FIELDS];
    let mut transformed = [0usize; N_FIELDS];
    let (mut bad, mut no_choice, mut zero_pp) = (0usize, 0usize, 0usize);
    let (mut toxic_hist, mut status_hist) = ([0usize; 16], [0usize; 16]);
    let (mut spikes, mut asleep, mut substitute, mut screen) = (0usize, 0usize, 0usize, 0usize);
    for r in &rows {
        let side = r.side as usize;
        let mon = r.state.active_mon(side);
        let sd = &r.state.sides[side];
        if legal_actions(&r.state, side).count < 2 {
            no_choice += 1;
        }
        if mon.status == STATUS_BAD_POISON {
            bad += 1;
            toxic_hist[(sd.active.toxic_counter as usize).min(15)] += 1;
            status_hist[(mon.status_counter as usize).min(15)] += 1;
        }
        if lowest_pp_slot(mon).is_some_and(|i| mon.pp[i] == 0) {
            zero_pp += 1;
        }
        for (k, f) in FIELDS.iter().enumerate() {
            if f.applies(&r.state, side) {
                n[k] += 1;
                if sd.active.has_volatile(VOL_TRANSFORMED) {
                    transformed[k] += 1;
                }
            }
        }
        let sc = &sd.side_conditions;
        spikes += (sc.spikes >= 1) as usize;
        asleep += (mon.status == STATUS_SLEEP) as usize;
        substitute += (sd.active.substitute_hp > 0) as usize;
        screen += (sc.reflect_turns > 0 || sc.light_screen_turns > 0 || sc.aurora_veil_turns > 0) as usize;
    }
    println!("count rows={} decider_without_choice={no_choice}", rows.len());
    println!("badpoison active={bad} toxic_counter_hist={toxic_hist:?} status_counter_hist={status_hist:?}");
    for (k, f) in FIELDS.iter().enumerate() {
        println!("field {} n={} transformed_active={}", f.name(), n[k], transformed[k]);
    }
    println!("lowpp zero_pp_rows={zero_pp}");
    println!("alt spikes_ge1={spikes} asleep={asleep} substitute={substitute} screen={screen}");
}

fn root_pick(state: &BattleState, teams: &TeamData, side: usize, params: &SearchParams, seed: u64) -> (u8, u64) {
    let r = search_world(state, teams, &Handcrafted, &OpenLoop, params, seed, side, None);
    let agg = aggregate(&[(r.side(side).to_vec(), 1.0)]);
    let pick = pick_from(&agg, &legal_actions(state, side), &mut Lcg::new(seed), PickMode::Argmax, PICK_FILTER);
    (pick, r.iterations)
}

struct Cell {
    budget: u64,
    seed: u64,
    base: u8,
    repeat: u8,
    reseed: u8,
    perturbed: [Option<u8>; N_FIELDS],
    min_iters: u64,
}

#[derive(Clone, Copy, Default)]
struct Tally {
    n: f64,
    flips: f64,
    reseed: f64,
    pair: f64,
    short: f64,
}

impl Tally {
    fn add(&mut self, c: &Cell, pick: Option<u8>) {
        self.n += 1.0;
        self.flips += (pick.is_some_and(|p| p != c.base)) as u64 as f64;
        self.reseed += (c.reseed != c.base) as u64 as f64;
        self.pair += (c.repeat != c.base) as u64 as f64;
        self.short += (c.min_iters < c.budget) as u64 as f64;
    }

    fn mean(tallies: &[Tally]) -> Tally {
        let k = tallies.len() as f64;
        let mut m = Tally::default();
        for t in tallies {
            m.n = t.n;
            m.flips += t.flips / k;
            m.reseed += t.reseed / k;
            m.pair += t.pair / k;
            m.short += t.short / k;
        }
        m
    }
}

fn num(x: f64) -> String {
    if x.fract() == 0.0 { format!("{x}") } else { format!("{x:.3}") }
}

fn rate(k: f64, n: f64) -> String {
    if n > 0.0 { format!("{:.6}\t{:.6}", k / n, (k / n * (1.0 - k / n) / n).sqrt()) } else { "-\t-".to_string() }
}

fn summary_line(field: &str, budget: u64, seed: &str, t: &Tally, with_flips: bool) -> String {
    let flips = if with_flips { format!("{}\t{}", num(t.flips), rate(t.flips, t.n)) } else { "-\t-\t-".to_string() };
    format!("{field}\t{budget}\t{seed}\t{}\t{flips}\t{}\t{}\t{}\t{}\n", num(t.n), num(t.reseed), rate(t.reseed, t.n), num(t.pair), num(t.short))
}

fn flip_fields(args: &[String]) {
    let rows = load(args);
    let skip: usize = parse(args, "--skip", "0");
    let budgets: Vec<u64> = list(args, "--budgets", "1024,32768");
    let seeds: Vec<u64> = list(args, "--seeds", "1,2,3");
    let threads: usize = parse(args, "--threads", &std::thread::available_parallelism().map_or(1, |n| n.get()).to_string());
    let out = arg(args, "--out", "results/0.6.rows.tsv");
    let summary = arg(args, "--summary", "results/0.6.flips.tsv");
    for path in [&out, &summary] {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        }
    }
    println!("flip-fields: budgets={budgets:?} seeds={seeds:?} threads={threads} evaluator=Handcrafted chance=OpenLoop explore_coeff={EXPLORE_COEFF} time_ms={TIME_MS} pick=Argmax filter={PICK_FILTER} decider=row.side");
    let params: Vec<SearchParams> = budgets.iter().map(|&b| SearchParams { max_iters: b, time_ms: TIME_MS, explore_coeff: EXPLORE_COEFF, ..Default::default() }).collect();

    let done = AtomicU64::new(0);
    let iters_total = AtomicU64::new(0);
    let wall = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let cells: Vec<Vec<Cell>> = pool.install(|| {
        rows.par_iter().enumerate().map(|(i, row)| {
            let side = row.side as usize;
            let perturbed_states: Vec<Option<BattleState>> = FIELDS.iter().map(|f| f.applies(&row.state, side).then(|| {
                let mut s = row.state;
                f.perturb(&mut s, side);
                s
            })).collect();
            let mut out = Vec::with_capacity(params.len() * seeds.len());
            for p in &params {
                for &s in &seeds {
                    let seed = row_seed(s, i + skip);
                    let (base, it0) = root_pick(&row.state, &row.teams, side, p, seed);
                    let (repeat, it1) = root_pick(&row.state, &row.teams, side, p, seed);
                    let (reseed, it2) = root_pick(&row.state, &row.teams, side, p, row_seed(s ^ RESEED_SALT, i + skip));
                    let mut min_iters = it0.min(it1).min(it2);
                    let mut total = it0 + it1 + it2;
                    let mut perturbed = [None; N_FIELDS];
                    for (k, st) in perturbed_states.iter().enumerate() {
                        if let Some(st) = st {
                            let (pick, it) = root_pick(st, &row.teams, side, p, seed);
                            perturbed[k] = Some(pick);
                            min_iters = min_iters.min(it);
                            total += it;
                        }
                    }
                    iters_total.fetch_add(total, Ordering::Relaxed);
                    out.push(Cell { budget: p.max_iters, seed: s, base, repeat, reseed, perturbed, min_iters });
                }
            }
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            if d % 500 == 0 {
                eprintln!("progress rows={d}/{} wall_s={:.0}", rows.len(), wall.elapsed().as_secs_f64());
            }
            out
        }).collect()
    });
    let wall_s = wall.elapsed().as_secs_f64();
    let iters = iters_total.load(Ordering::Relaxed);
    println!("flip-fields-done rows={} wall_s={wall_s:.1} iters={iters} iters_per_s={:.0} iters_per_s_per_thread={:.0}", rows.len(), iters as f64 / wall_s, iters as f64 / wall_s / threads as f64);

    let fmt = |o: Option<u8>| o.map_or("-".to_string(), |p| p.to_string());
    let mut s = String::from("row\tgame_index\tgame_tag\tturn\tside\tbudget\tseed\tbase\trepeat\treseed\t");
    s.push_str(&FIELDS.iter().map(|f| f.name()).collect::<Vec<_>>().join("\t"));
    s.push_str("\tmin_iters\tpre_toxic_counter\n");
    for (i, (row, cs)) in rows.iter().zip(&cells).enumerate() {
        for c in cs {
            s.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                i + skip, row.game_index, row.game_tag, row.turn, row.side, c.budget, c.seed, c.base, c.repeat, c.reseed,
                c.perturbed.iter().map(|&p| fmt(p)).collect::<Vec<_>>().join("\t"), c.min_iters,
                row.state.sides[row.side as usize].active.toxic_counter));
        }
    }
    std::fs::write(&out, s).unwrap_or_else(|e| panic!("write {out}: {e}"));

    // the pooled row is the per-seed mean of every count, over the same n rows
    let mut t = String::from("field\tbudget\tseed\tn\tflips\tflip_rate\tflip_se\treseed_flips\treseed_rate\treseed_se\tpair_flips\trows_with_a_short_search\n");
    for &budget in &budgets {
        for (k, f) in FIELDS.iter().enumerate() {
            let mut tallies = Vec::new();
            for &seed in &seeds {
                let mut tally = Tally::default();
                for c in cells.iter().flatten().filter(|c| c.budget == budget && c.seed == seed) {
                    if c.perturbed[k].is_some() {
                        tally.add(c, c.perturbed[k]);
                    }
                }
                t.push_str(&summary_line(f.name(), budget, &seed.to_string(), &tally, true));
                tallies.push(tally);
            }
            t.push_str(&summary_line(f.name(), budget, "pooled", &Tally::mean(&tallies), true));
        }
        for &seed in &seeds {
            let mut tally = Tally::default();
            for c in cells.iter().flatten().filter(|c| c.budget == budget && c.seed == seed) {
                tally.add(c, None);
            }
            t.push_str(&summary_line("all_rows", budget, &seed.to_string(), &tally, false));
        }
    }
    std::fs::write(&summary, t).unwrap_or_else(|e| panic!("write {summary}: {e}"));
    println!("flip-fields-out rows={out} summary={summary}");
}

fn loadavg() -> String {
    let out = std::process::Command::new("sysctl").arg("-n").arg("vm.loadavg").output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().trim_matches(|c| c == '{' || c == '}').trim().replace(' ', "/"),
        _ => "-".to_string(),
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

const POLICIES: [PlayoutPolicy; 2] = [PlayoutPolicy::Uniform, PlayoutPolicy::Greedy];

fn cost(args: &[String]) {
    let rows = load(args);
    let turn_lo: u16 = parse(args, "--turn-lo", "5");
    let turn_hi: u16 = parse(args, "--turn-hi", "25");
    let n: usize = parse(args, "--n", "2000");
    let repeats: usize = parse(args, "--repeats", "3");
    let run_seed: u64 = parse(args, "--seed", "1");
    let out = arg(args, "--out", "results/0.3.cost.tsv");
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    }
    let picked: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| (turn_lo..=turn_hi).contains(&r.turn)).map(|(i, _)| i).take(n).collect();
    println!("cost: rows={} turns={turn_lo}..={turn_hi} picked={} repeats={repeats} seed={run_seed} k=1 sequential=yes step_cap={PLAYOUT_STEP_CAP}", rows.len(), picked.len());
    let mut tsv = String::from("run\tpolicy\trows\tk\tus_mean\tus_median\tus_p90\tus_max\tsteps_mean\tsteps_median\tturns_mean\tturns_median\tturns_max\tcapped\tvalue_mean\tload_start\tload_end\twall_s\n");
    for run in 0..repeats {
        for policy in POLICIES {
            let load_start = loadavg();
            let wall = Instant::now();
            let mut us: Vec<f64> = Vec::with_capacity(picked.len());
            let mut steps: Vec<f64> = Vec::with_capacity(picked.len());
            let mut turns: Vec<f64> = Vec::with_capacity(picked.len());
            let (mut capped, mut value_sum) = (0u64, 0.0f64);
            for &i in &picked {
                let row = &rows[i];
                let pe = PlayoutEval::new(row.teams.clone(), 1, row_seed(run_seed, i), policy);
                let game_seed = PlayoutEval::game_seed(pe.seed.get(), 0);
                let t = Instant::now();
                let p = pe.playout(&row.state, game_seed);
                us.push(t.elapsed().as_secs_f64() * 1e6);
                steps.push(p.steps as f64);
                turns.push(p.turn.saturating_sub(row.state.field.turn) as f64);
                capped += p.capped as u64;
                value_sum += p.value;
            }
            let wall_s = wall.elapsed().as_secs_f64();
            let load_end = loadavg();
            let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
            let mut us_s = us.clone();
            us_s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut st_s = steps.clone();
            st_s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut tu_s = turns.clone();
            tu_s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let line = format!(
                "{run}\t{}\t{}\t1\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{}\t{:.3}\t{}\t{}\t{capped}\t{:.4}\t{load_start}\t{load_end}\t{wall_s:.3}\n",
                policy.as_str(), picked.len(), mean(&us), percentile(&us_s, 0.5), percentile(&us_s, 0.9), us_s[us_s.len() - 1],
                mean(&steps), percentile(&st_s, 0.5), mean(&turns), percentile(&tu_s, 0.5), tu_s[tu_s.len() - 1], value_sum / picked.len() as f64
            );
            print!("{line}");
            tsv.push_str(&line);
        }
    }
    std::fs::write(&out, tsv).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("cost-out {out}");
}

fn signal(args: &[String]) {
    let rows = load(args);
    let ks: Vec<u32> = list(args, "--k", "8,32");
    let run_seed: u64 = parse(args, "--seed", "1");
    let threads: usize = parse(args, "--threads", &std::thread::available_parallelism().map_or(1, |n| n.get()).to_string());
    let out = arg(args, "--out", "results/0.3.values.tsv");
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    }
    let net = LearnedValueV2::from_env();
    let games_per_row: u32 = ks.iter().sum::<u32>() * POLICIES.len() as u32;
    println!(
        "signal: rows={} k={ks:?} policies=uniform,greedy seed={run_seed} threads={threads} hand=Handcrafted net={} int8_scope={} games_per_row={games_per_row} step_cap={PLAYOUT_STEP_CAP}",
        rows.len(), std::env::var("BRIDGE_EVAL_WEIGHTS_V2").unwrap_or_default(), net.int8_scope()
    );
    let done = AtomicU64::new(0);
    let wall = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let values: Vec<Vec<f32>> = pool.install(|| {
        rows.par_iter().enumerate().map(|(i, row)| {
            let mut v = vec![Handcrafted.eval(&row.state), net.eval(&row.state)];
            let seed = row_seed(run_seed, i);
            for policy in POLICIES {
                for &k in &ks {
                    v.push(PlayoutEval::new(row.teams.clone(), k, seed, policy).eval(&row.state));
                }
            }
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            if d % 2000 == 0 {
                eprintln!("progress rows={d}/{} wall_s={:.0}", rows.len(), wall.elapsed().as_secs_f64());
            }
            v
        }).collect()
    });
    let wall_s = wall.elapsed().as_secs_f64();
    let games = rows.len() as f64 * games_per_row as f64;
    println!("signal-done rows={} wall_s={wall_s:.1} playouts={games} playouts_per_s={:.0} playouts_per_s_per_thread={:.0}", rows.len(), games / wall_s, games / wall_s / threads as f64);
    let mut s = String::from("row\tgame_tag\tgen_eval\tturn\tside\tz\thand\tnet");
    for policy in POLICIES {
        for &k in &ks {
            s.push_str(&format!("\tplayout_{}_k{k}", policy.as_str()));
        }
    }
    s.push('\n');
    for (i, (row, v)) in rows.iter().zip(&values).enumerate() {
        s.push_str(&format!("{i}\t{}\t{}\t{}\t{}\t{}", row.game_tag, row.gen_eval, row.turn, row.side, row.z));
        for x in v {
            s.push_str(&format!("\t{x}"));
        }
        s.push('\n');
    }
    std::fs::write(&out, s).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("signal-out {out}");
}

const SEARCH_SALT: u64 = 0xA076_1D64_78BD_642F;

fn top_arm(arms: &[ArmStat]) -> Option<u8> {
    arms.iter().max_by(|a, b| a.visits.cmp(&b.visits).then(b.action.cmp(&a.action))).map(|a| a.action)
}

fn l1(a: &[ArmStat], b: &[ArmStat]) -> f64 {
    let mut d = [0.0f64; 256];
    for (arms, sign) in [(a, 1.0), (b, -1.0)] {
        let total: f64 = arms.iter().map(|x| x.visits as f64).sum();
        if total > 0.0 {
            for x in arms {
                d[x.action as usize] += sign * x.visits as f64 / total;
            }
        }
    }
    d.iter().map(|x| x.abs()).sum()
}

fn is_tera(x: Option<u8>) -> bool {
    matches!(x, Some(ACTION_TERA_0..=ACTION_TERA_3))
}

fn is_switch(x: Option<u8>) -> bool {
    matches!(x, Some(ACTION_SWITCH_0..=ACTION_SWITCH_5))
}

#[derive(Clone, Copy)]
struct RootRef {
    position: usize,
    game: u64,
    turn: u16,
    side: u8,
}

struct RootSample {
    roots: Vec<RootRef>,
    sampled: usize,
    discarded_legal: usize,
    discarded_opp_slots: usize,
}

fn root_ref(snaps: &[NativeSnapshot], position: usize) -> RootRef {
    let s = &snaps[position];
    RootRef { position, game: s.game, turn: s.turn, side: s.side }
}

fn opp_slots_identical(snap: &NativeSnapshot, cfg_seed: u64, worlds: usize) -> bool {
    let side = snap.side as usize;
    let opp = 1 - side;
    let obs = Observation { state: &snap.state, teams: &snap.teams, our_side: side };
    let belief = &snap.beliefs[side];
    let a = RandomBattle.sample_worlds(&obs, belief, worlds, &mut Lcg::new(splitmix64(cfg_seed)));
    let denied = Declairvoyant { inner: RandomBattle, opp_view: snap.beliefs[opp] };
    let b = denied.sample_worlds(&obs, belief, worlds, &mut Lcg::new(splitmix64(cfg_seed)));
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| {
        x.state.sides[opp].team == y.state.sides[opp].team && x.teams.mons[opp] == y.teams.mons[opp] && x.teams.levels[opp] == y.teams.levels[opp]
    })
}

// keyed by file position because (game, turn, side) repeats in the snapshot file
fn sample_roots(snaps: &[NativeSnapshot], n: usize, turn_lo: u16, turn_hi: u16, sample_seed: u64, seeds: &[u64], worlds: usize) -> RootSample {
    let mut eligible: Vec<usize> = (0..snaps.len()).filter(|&i| (turn_lo..=turn_hi).contains(&snaps[i].turn)).collect();
    let mut rng = Lcg::new(sample_seed);
    let take = n.min(eligible.len());
    for i in 0..take {
        let j = i + rng.roll((eligible.len() - i) as u32) as usize;
        eligible.swap(i, j);
    }
    let mut drawn = eligible[..take].to_vec();
    drawn.sort_unstable();
    let (mut discarded_legal, mut discarded_opp_slots) = (0, 0);
    let mut roots = Vec::with_capacity(take);
    for position in drawn {
        let snap = &snaps[position];
        if (0..2).any(|side| legal_actions(&snap.state, side).count < 2) {
            discarded_legal += 1;
        } else if seeds.iter().any(|&s| !opp_slots_identical(snap, row_seed(s, position), worlds)) {
            discarded_opp_slots += 1;
        } else {
            roots.push(root_ref(snaps, position));
        }
    }
    RootSample { roots, sampled: take, discarded_legal, discarded_opp_slots }
}

fn write_roots(path: &str, roots: &[RootRef]) {
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    }
    let mut s = String::from("position\tgame\tturn\tside\n");
    for r in roots {
        s.push_str(&format!("{}\t{}\t{}\t{}\n", r.position, r.game, r.turn, r.side));
    }
    std::fs::write(path, s).unwrap_or_else(|e| panic!("write {path}: {e}"));
}

fn load_roots(path: &str, snaps: &[NativeSnapshot]) -> Vec<RootRef> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    text.lines().skip(1).filter(|l| !l.is_empty()).map(|line| {
        let f: Vec<u64> = line.split('\t').map(|x| x.parse().unwrap_or_else(|e| panic!("{path} {x:?}: {e}"))).collect();
        assert_eq!(f.len(), 4, "{path}: bad line {line:?}");
        let position = f[0] as usize;
        let s = snaps.get(position).unwrap_or_else(|| panic!("{path}: position {position} is beyond the {} snapshots", snaps.len()));
        assert!(
            (s.game, s.turn as u64, s.side as u64) == (f[1], f[2], f[3]),
            "{path}: position {position} is game={} turn={} side={} in the snapshot file, not game={} turn={} side={}",
            s.game, s.turn, s.side, f[1], f[2], f[3]
        );
        root_ref(snaps, position)
    }).collect()
}

struct WorldCell {
    opp_species: u16,
    top: [Option<u8>; 3],
    iters: [u64; 3],
    // per arm (shipped, Declairvoyant, salted): the principal line and what its opponent bytes name
    principal: [[(u8, u8); 3]; 3],
    opp_named: [[Named; 3]; 3],
}

#[derive(Default)]
struct Run {
    seed: u64,
    flips_ab: u32,
    flips_af: u32,
    tera_ab: u32,
    switch_ab: u32,
    ts_ab: u32,
    tera_af: u32,
    switch_af: u32,
    ts_af: u32,
    l1_ab: Vec<f64>,
    l1_af: Vec<f64>,
    pick: [u8; 3],
    pick_ab: bool,
    pick_af: bool,
    short: u32,
    arms_lt2: u32,
    min_iters: u64,
    worlds: Vec<WorldCell>,
}

fn opp_key(w: &WorldTrace) -> (u16, u16, u16, u16, u16, u8) {
    (w.opp_species, w.opp_item, w.opp_ability, w.opp_hp, w.opp_max_hp, w.opp_status)
}

fn run_root(snap: &NativeSnapshot, position: usize, seed: u64, cfg: &PimcConfig) -> Run {
    let side = snap.side as usize;
    let obs = Observation { state: &snap.state, teams: &snap.teams, our_side: side };
    let belief = &snap.beliefs[side];
    let denied = Declairvoyant { inner: RandomBattle, opp_view: snap.beliefs[1 - side] };
    let a = choose_action_traced_salted(&obs, belief, &RandomBattle, cfg, 0);
    let b = choose_action_traced_salted(&obs, belief, &denied, cfg, 0);
    let f = choose_action_traced_salted(&obs, belief, &RandomBattle, cfg, SEARCH_SALT);
    let worlds = cfg.num_worlds;
    for t in [&a, &b, &f] {
        assert_eq!(t.per_world.len(), worlds, "root {position}: world count");
    }
    let opp = 1 - side;
    // the shipped arms' draw replayed for the sampled sets the trace omits; Declairvoyant draws the opponent's side first from the same stream, so its worlds share them
    let sampled = RandomBattle.sample_worlds(&obs, belief, worlds, &mut Lcg::new(splitmix64(cfg.seed)));
    let mut run = Run { seed, pick: [a.picked, b.picked, f.picked], pick_ab: a.picked != b.picked, pick_af: a.picked != f.picked, min_iters: u64::MAX, ..Default::default() };
    for k in 0..worlds {
        let (wa, wb, wf) = (&a.per_world[k], &b.per_world[k], &f.per_world[k]);
        assert!(opp_key(wa) == opp_key(wb) && opp_key(wa) == opp_key(wf), "root {position} world {k}: opponent slot differs between runs");
        let ws = &sampled[k].state;
        let am = ws.active_mon(opp);
        assert!(
            (am.species_id, am.item_id, am.ability_id, am.current_hp, am.max_hp, am.status) == opp_key(wa),
            "root {position} world {k}: the replayed world is not the searched one"
        );
        let principal = [wa.principal, wb.principal, wf.principal];
        let opp_named = principal.map(|p| opp_meaning_path(ws, opp, &p));
        let top = [top_arm(&wa.s2), top_arm(&wb.s2), top_arm(&wf.s2)];
        let (flip_ab, flip_af) = (top[0] != top[1], top[0] != top[2]);
        let tera_ab = flip_ab && (is_tera(top[0]) || is_tera(top[1]));
        let switch_ab = flip_ab && (is_switch(top[0]) || is_switch(top[1]));
        let tera_af = flip_af && (is_tera(top[0]) || is_tera(top[2]));
        let switch_af = flip_af && (is_switch(top[0]) || is_switch(top[2]));
        run.flips_ab += flip_ab as u32;
        run.flips_af += flip_af as u32;
        run.tera_ab += tera_ab as u32;
        run.switch_ab += switch_ab as u32;
        run.ts_ab += (tera_ab || switch_ab) as u32;
        run.tera_af += tera_af as u32;
        run.switch_af += switch_af as u32;
        run.ts_af += (tera_af || switch_af) as u32;
        run.l1_ab.push(l1(&wa.s2, &wb.s2));
        run.l1_af.push(l1(&wa.s2, &wf.s2));
        run.arms_lt2 += (wa.s2.len() < 2) as u32;
        let iters = [wa.iterations, wb.iterations, wf.iterations];
        run.short += iters.iter().filter(|&&i| i < cfg.max_iters_per_world).count() as u32;
        run.min_iters = run.min_iters.min(*iters.iter().min().unwrap());
        run.worlds.push(WorldCell { opp_species: wa.opp_species, top, iters, principal, opp_named });
    }
    run
}

// The opponent's action at ply d of two principal lines: None where either line has no ply
// there (both bytes NO_ARM), else whether the named actions differ.
fn off_root_flip(x: &[(u8, u8); 3], nx: &[Named; 3], y: &[(u8, u8); 3], ny: &[Named; 3], d: usize) -> Option<bool> {
    if x[d] == (NO_ARM, NO_ARM) || y[d] == (NO_ARM, NO_ARM) { return None; }
    Some(nx[d] != ny[d])
}

#[derive(Default, Clone, Copy)]
struct PlyTally {
    n: u64,
    flips: u64,
    tera: u64,
    switch: u64,
    ts: u64,
    absent: u64,
    trunc: u64,
    same1_n: u64,
    same1_flips: u64,
}

#[derive(Default, Clone, Copy)]
struct OffRoot {
    ply: [PlyTally; 3],
    trunc_worlds: u64,
    offroot_flip: bool,
    pick_same: bool,
}

// arms x and y of a Run's cells (0 shipped, 1 Declairvoyant, 2 salted); off-root means plies 2 and 3,
// and same1 restricts a ply to worlds whose ply-1 joint action is the same in both arms
fn off_root(run: &Run, x: usize, y: usize) -> OffRoot {
    let mut o = OffRoot { pick_same: run.pick[x] == run.pick[y], ..Default::default() };
    for w in &run.worlds {
        let (px, py) = (&w.principal[x], &w.principal[y]);
        let (nx, ny) = (&w.opp_named[x], &w.opp_named[y]);
        let same1 = px[0].0 == py[0].0 && nx[0] == ny[0];
        let mut trunc = false;
        for d in 0..3 {
            let t = &mut o.ply[d];
            match off_root_flip(px, nx, py, ny, d) {
                None => {
                    t.trunc += 1;
                    trunc |= d > 0;
                }
                Some(flip) => {
                    t.n += 1;
                    t.flips += flip as u64;
                    if flip {
                        let tera = matches!(nx[d], Named::Move(_, true)) || matches!(ny[d], Named::Move(_, true));
                        let switch = matches!(nx[d], Named::Switch(_)) || matches!(ny[d], Named::Switch(_));
                        t.tera += tera as u64;
                        t.switch += switch as u64;
                        t.ts += (tera || switch) as u64;
                        t.absent += ((nx[d] == Named::Absent) != (ny[d] == Named::Absent)) as u64;
                        o.offroot_flip |= d > 0;
                    }
                    if same1 && d > 0 {
                        t.same1_n += 1;
                        t.same1_flips += flip as u64;
                    }
                }
            }
        }
        o.trunc_worlds += trunc as u64;
    }
    o
}

fn offroot_rows(label: &str, per_root: &[Vec<&Run>], worlds: usize) -> String {
    let per_cell: Vec<Vec<[OffRoot; 2]>> = per_root.iter().map(|rs| rs.iter().map(|r| [off_root(r, 0, 1), off_root(r, 0, 2)]).collect()).collect();
    let cells: usize = per_cell.iter().map(|c| c.len()).sum();
    let mut tot = [[PlyTally::default(); 3]; 2];
    let mut trunc_worlds = [0u64; 2];
    let mut flip_cells = [0u64; 2];
    let mut pick_same = [0u64; 2];
    let mut both = [0u64; 2];
    for o in per_cell.iter().flatten() {
        for c in 0..2 {
            for d in 0..3 {
                let (t, u) = (&mut tot[c][d], &o[c].ply[d]);
                t.n += u.n;
                t.flips += u.flips;
                t.tera += u.tera;
                t.switch += u.switch;
                t.ts += u.ts;
                t.absent += u.absent;
                t.trunc += u.trunc;
                t.same1_n += u.same1_n;
                t.same1_flips += u.same1_flips;
            }
            trunc_worlds[c] += o[c].trunc_worlds;
            flip_cells[c] += o[c].offroot_flip as u64;
            pick_same[c] += o[c].pick_same as u64;
            both[c] += (o[c].pick_same && o[c].offroot_flip) as u64;
        }
    }
    let share = |k: u64, n: u64| if n > 0 { format!("{:.6}", k as f64 / n as f64) } else { "-".to_string() };
    // ab minus af at ply d: pooled rates, and the standard error over roots of the per-root rate difference
    let diff = |d: usize| {
        let rate = |rs: &[[OffRoot; 2]], c: usize| {
            let (f, n) = rs.iter().fold((0u64, 0u64), |(f, n), o| (f + o[c].ply[d].flips, n + o[c].ply[d].n));
            (n > 0).then(|| f as f64 / n as f64)
        };
        let x: Vec<f64> = per_cell.iter().filter_map(|rs| Some(rate(rs, 0)? - rate(rs, 1)?)).collect();
        let k = x.len();
        let m = x.iter().sum::<f64>() / k.max(1) as f64;
        let se = if k < 2 { 0.0 } else { (x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (k - 1) as f64).sqrt() / (k as f64).sqrt() };
        let pooled = match (tot[0][d].n, tot[1][d].n) {
            (a, f) if a > 0 && f > 0 => format!("{:.4}", (tot[0][d].flips as f64 / a as f64 - tot[1][d].flips as f64 / f as f64) * 100.0),
            _ => "-".to_string(),
        };
        format!("{pooled}\t{:.4}\t{k}", se * 100.0)
    };
    let mut s = String::new();
    for (c, cmp) in ["ab", "af"].into_iter().enumerate() {
        for d in 0..3 {
            let t = &tot[c][d];
            s.push_str(&format!(
                "{label}\t{cmp}\t{}\t{}\t{cells}\t{worlds}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                d + 1, per_root.len(), t.n, t.flips, share(t.flips, t.n), t.trunc, t.tera, t.switch, share(t.ts, t.flips), t.absent,
                t.same1_n, t.same1_flips, share(t.same1_flips, t.same1_n),
                trunc_worlds[c], flip_cells[c], pick_same[c], both[c], share(both[c], cells as u64),
                if c == 0 { diff(d) } else { "-\t-\t-".to_string() }
            ));
        }
    }
    s
}

// per_root holds one Run per seed for a per-seed row and every seed's Run for the pooled row
fn summary_row(label: &str, per_root: &[Vec<&Run>], worlds: usize) -> String {
    let all: Vec<&Run> = per_root.iter().flatten().copied().collect();
    let roots = per_root.len();
    let n = (all.len() * worlds) as f64;
    let count = |f: fn(&Run) -> u32| all.iter().map(|r| f(r) as u64).sum::<u64>();
    let (ab, af) = (count(|r| r.flips_ab), count(|r| r.flips_af));
    let root_diff = |rs: &[&Run]| rs.iter().map(|r| (r.flips_ab as f64 - r.flips_af as f64) / worlds as f64).sum::<f64>() / rs.len() as f64;
    let d: Vec<f64> = per_root.iter().map(|rs| root_diff(rs)).collect();
    let se_pp = if roots < 2 {
        0.0
    } else {
        let m = d.iter().sum::<f64>() / roots as f64;
        (d.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (roots - 1) as f64).sqrt() / (roots as f64).sqrt() * 100.0
    };
    let share = |k: u64, total: u64| if total > 0 { format!("{:.6}", k as f64 / total as f64) } else { "-".to_string() };
    let stats = |mut v: Vec<f64>| {
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        format!("{mean:.6}\t{:.6}", percentile(&v, 0.5))
    };
    let (tera_ab, switch_ab, ts_ab, ts_af) = (count(|r| r.tera_ab), count(|r| r.switch_ab), count(|r| r.ts_ab), count(|r| r.ts_af));
    let (pick_ab, pick_af) = (count(|r| r.pick_ab as u32), count(|r| r.pick_af as u32));
    let l1_ab = stats(all.iter().flat_map(|r| r.l1_ab.iter().copied()).collect());
    let l1_af = stats(all.iter().flat_map(|r| r.l1_af.iter().copied()).collect());
    format!(
        "{label}\t{roots}\t{worlds}\t{ab}\t{:.6}\t{af}\t{:.6}\t{:.4}\t{:.4}\t{tera_ab}\t{switch_ab}\t{ts_ab}\t{}\t{}\t{}\t{ts_af}\t{}\t{l1_ab}\t{l1_af}\t{pick_ab}\t{:.6}\t{pick_af}\t{:.6}\t{}\t{}\n",
        ab as f64 / n, af as f64 / n, (ab as f64 - af as f64) / n * 100.0, se_pp,
        share(tera_ab, ab), share(switch_ab, ab), share(ts_ab, ab), share(ts_af, af),
        pick_ab as f64 / all.len() as f64, pick_af as f64 / all.len() as f64, count(|r| r.short), count(|r| r.arms_lt2)
    )
}

fn declairvoyance(args: &[String]) {
    let path = arg(args, "--snapshots", "../.decompose/frontier/results/0.1.snapshots.bin");
    let snaps = read_all(&path).unwrap_or_else(|e| panic!("read_all {path}: {e}"));
    println!("snapshots: path={path} count={}", snaps.len());
    let seeds: Vec<u64> = list(args, "--seeds", "1,2,3");
    let worlds: usize = parse(args, "--worlds", "8");
    let iters: u64 = parse(args, "--iters", "35199");
    let time_ms: u64 = parse(args, "--time-ms", "600000");
    let threads: usize = parse(args, "--threads", &std::thread::available_parallelism().map_or(1, |n| n.get()).to_string());
    let out = arg(args, "--out", "results/0.7.diff.tsv");
    let dump = arg(args, "--dump", "results/0.7.worlds.tsv");
    let offroot = arg(args, "--offroot", "results/0.7.offroot.tsv");
    let roots_file: String;
    let roots = if args.iter().any(|a| a == "--roots") {
        roots_file = arg(args, "--roots", "");
        let roots = load_roots(&roots_file, &snaps);
        println!("roots: loaded={} file={roots_file}", roots.len());
        roots
    } else {
        let n: usize = parse(args, "--n", "3000");
        let turn_lo: u16 = parse(args, "--turn-lo", "2");
        let turn_hi: u16 = parse(args, "--turn-hi", "20");
        let sample_seed: u64 = parse(args, "--sample-seed", "11");
        roots_file = arg(args, "--roots-out", "results/0.7.roots.tsv");
        let s = sample_roots(&snaps, n, turn_lo, turn_hi, sample_seed, &seeds, worlds);
        write_roots(&roots_file, &s.roots);
        println!(
            "roots: sampled={} discarded_legal={} discarded_opp_slots={} kept={} sample_seed={sample_seed} turns={turn_lo}..={turn_hi} file={roots_file}",
            s.sampled, s.discarded_legal, s.discarded_opp_slots, s.roots.len()
        );
        s.roots
    };
    assert!(!roots.is_empty(), "no roots kept");
    for p in [&out, &dump, &offroot] {
        if let Some(dir) = std::path::Path::new(p).parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        }
    }
    println!(
        "declairvoyance: roots={} seeds={seeds:?} worlds={worlds} iters={iters} time_ms={time_ms} evaluator=Handcrafted determinizer_a=RandomBattle determinizer_b=Declairvoyant(RandomBattle) chance=OpenLoop pick=Argmax filter={PICK_FILTER} raw_root=false explore_coeff={EXPLORE_COEFF} value_temp=1.0 search_salt=0x{SEARCH_SALT:x} top_arm=max_visits_lowest_byte_on_ties principal=max_visit_joint_path_3_plies threads={threads}",
        roots.len()
    );
    let cfg = |seed: u64| PimcConfig {
        num_worlds: worlds,
        time_ms_per_world: time_ms,
        max_iters_per_world: iters,
        seed,
        chance_mode: ChanceMode::OpenLoop,
        pick_mode: PickMode::Argmax,
        filter_threshold: PICK_FILTER,
        raw_root: false,
        explore_coeff: EXPLORE_COEFF,
        value_temp: 1.0,
    };

    let done = AtomicU64::new(0);
    let iters_total = AtomicU64::new(0);
    let first = AtomicBool::new(false);
    let wall = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let runs: Vec<Vec<Run>> = pool.install(|| {
        roots.par_iter().map(|root| {
            let snap = &snaps[root.position];
            let runs: Vec<Run> = seeds.iter().map(|&s| run_root(snap, root.position, s, &cfg(row_seed(s, root.position)))).collect();
            let total: u64 = runs.iter().flat_map(|r| &r.worlds).map(|w| w.iters.iter().sum::<u64>()).sum();
            iters_total.fetch_add(total, Ordering::Relaxed);
            if !first.swap(true, Ordering::Relaxed) {
                println!("first root={} game={} turn={} side={} iters_min={}", root.position, root.game, root.turn, root.side, runs.iter().map(|r| r.min_iters).min().unwrap());
            }
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            if d % 100 == 0 {
                eprintln!("progress roots={d}/{} wall_s={:.0}", roots.len(), wall.elapsed().as_secs_f64());
            }
            runs
        }).collect()
    });
    let wall_s = wall.elapsed().as_secs_f64();
    let total = iters_total.load(Ordering::Relaxed);
    let short: u64 = runs.iter().flatten().map(|r| r.short as u64).sum();
    let lt2: u64 = runs.iter().flatten().map(|r| r.arms_lt2 as u64).sum();
    println!(
        "declairvoyance-done roots={} seeds={} wall_s={wall_s:.1} iters={total} iters_per_s={:.0} iters_per_s_per_thread={:.0} short_searches={short} opp_arms_lt2={lt2}",
        roots.len(), seeds.len(), total as f64 / wall_s, total as f64 / wall_s / threads as f64
    );

    let fmt = |o: Option<u8>| o.map_or("-".to_string(), |p| p.to_string());
    let byte = |b: u8| if b == NO_ARM { "-".to_string() } else { b.to_string() };
    let off = |o: Option<bool>| o.map_or("-".to_string(), |f| (f as u8).to_string());
    let mut s = String::from("position\tgame\tturn\tside\tseed\tworld\topp_species\ttop_a\ttop_b\ttop_f\tflip_ab\tflip_af\tl1_ab\tl1_af\titers_a\titers_b\titers_f\tpick_a\tpick_b\tpick_f\ta_p1_ours\ta_p1_opp\ta_p2_ours\ta_p2_opp\ta_p3_ours\ta_p3_opp\tb_p1_ours\tb_p1_opp\tb_p2_ours\tb_p2_opp\tb_p3_ours\tb_p3_opp\tf_p1_ours\tf_p1_opp\tf_p2_ours\tf_p2_opp\tf_p3_ours\tf_p3_opp\ta_p1_opp_named\ta_p2_opp_named\ta_p3_opp_named\tb_p1_opp_named\tb_p2_opp_named\tb_p3_opp_named\tf_p1_opp_named\tf_p2_opp_named\tf_p3_opp_named\toff_ab_p2\toff_af_p2\toff_ab_p3\toff_af_p3\n");
    for (root, rs) in roots.iter().zip(&runs) {
        for r in rs {
            for (k, w) in r.worlds.iter().enumerate() {
                s.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{k}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{}\t{}\t{}\t{}\t{}\t{}",
                    root.position, root.game, root.turn, root.side, r.seed, w.opp_species, fmt(w.top[0]), fmt(w.top[1]), fmt(w.top[2]),
                    (w.top[0] != w.top[1]) as u8, (w.top[0] != w.top[2]) as u8, r.l1_ab[k], r.l1_af[k], w.iters[0], w.iters[1], w.iters[2], r.pick[0], r.pick[1], r.pick[2]
                ));
                for p in &w.principal {
                    for &(x, y) in p {
                        s.push_str(&format!("\t{}\t{}", byte(x), byte(y)));
                    }
                }
                for n in &w.opp_named {
                    for m in n {
                        s.push('\t');
                        s.push_str(&m.label());
                    }
                }
                for d in 1..3 {
                    for y in 1..3 {
                        s.push('\t');
                        s.push_str(&off(off_root_flip(&w.principal[0], &w.opp_named[0], &w.principal[y], &w.opp_named[y], d)));
                    }
                }
                s.push('\n');
            }
        }
    }
    std::fs::write(&dump, s).unwrap_or_else(|e| panic!("write {dump}: {e}"));

    let mut o = String::from("seed\tcmp\tply\troots\tcells\tworlds\tn_plies\tflips\trate\ttrunc_plies\ttera_flips\tswitch_flips\tts_share\tabsent_flips\tsame1_n\tsame1_flips\tsame1_rate\ttrunc_worlds\toffroot_flip_cells\tpick_same_cells\tpick_same_offroot_flip_cells\tpick_same_offroot_flip_frac\tdiff_pp\tdiff_se_pp\tdiff_roots\n");
    for (i, seed) in seeds.iter().enumerate() {
        let per_root: Vec<Vec<&Run>> = runs.iter().map(|rs| vec![&rs[i]]).collect();
        o.push_str(&offroot_rows(&seed.to_string(), &per_root, worlds));
    }
    let per_root: Vec<Vec<&Run>> = runs.iter().map(|rs| rs.iter().collect()).collect();
    o.push_str(&offroot_rows("pooled", &per_root, worlds));
    print!("{o}");
    std::fs::write(&offroot, o).unwrap_or_else(|e| panic!("write {offroot}: {e}"));

    let mut t = String::from("seed\troots\tworlds\tab_flips\tab_rate\taf_flips\taf_rate\tdiff_pp\tdiff_se_pp\tab_flips_tera\tab_flips_switch\tab_flips_tera_or_switch\ttera_share\tswitch_share\ttera_or_switch_share\taf_flips_tera_or_switch\taf_tera_or_switch_share\tl1_ab_mean\tl1_ab_median\tl1_af_mean\tl1_af_median\tpick_ab_flips\tpick_ab_rate\tpick_af_flips\tpick_af_rate\tshort_searches\topp_arms_lt2\n");
    for (i, seed) in seeds.iter().enumerate() {
        let per_root: Vec<Vec<&Run>> = runs.iter().map(|rs| vec![&rs[i]]).collect();
        t.push_str(&summary_row(&seed.to_string(), &per_root, worlds));
    }
    let per_root: Vec<Vec<&Run>> = runs.iter().map(|rs| rs.iter().collect()).collect();
    t.push_str(&summary_row("pooled", &per_root, worlds));
    print!("{t}");
    std::fs::write(&out, t).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("declairvoyance-out summary={out} dump={dump} offroot={offroot} roots={roots_file}");
}

// What one side's action byte names in one world: a move by id with the Tera flag carried, or a
// switch by the target's species. Our bytes mean the same thing in every world; the opponent's
// index that world's own sampled set, so only this form compares across worlds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Named {
    Absent,
    Move(u16, bool),
    Switch(u16),
}

impl Named {
    fn label(self) -> String {
        match self {
            Named::Absent => "-".to_string(),
            Named::Move(id, tera) => format!("{}{id}", if tera { "t" } else { "m" }),
            Named::Switch(sp) => format!("s{sp}"),
        }
    }
}

fn named(state: &BattleState, side: usize, active: usize, byte: u8) -> Named {
    let moves = if active == state.sides[side].active_index as usize {
        effective_moves(state, side)
    } else {
        state.sides[side].team[active].moves
    };
    match byte {
        0..=3 => Named::Move(moves[byte as usize], false),
        ACTION_SWITCH_0..=ACTION_SWITCH_5 => {
            Named::Switch(state.sides[side].team[(byte - ACTION_SWITCH_0) as usize].species_id)
        }
        ACTION_TERA_0..=ACTION_TERA_3 => Named::Move(moves[(byte - ACTION_TERA_0) as usize], true),
        _ => Named::Absent,
    }
}

// The opponent's active slot at ply d is the last switch byte the path took on that side (a
// forced replacement is itself a switch byte), so the path decodes without the intermediate states.
fn opp_meaning_path(state: &BattleState, opp: usize, path: &[(u8, u8); 3]) -> [Named; 3] {
    let mut active = state.sides[opp].active_index as usize;
    let mut out = [Named::Absent; 3];
    for (d, &(_, byte)) in path.iter().enumerate() {
        out[d] = named(state, opp, active, byte);
        if let Named::Switch(_) = out[d] {
            active = (byte - ACTION_SWITCH_0) as usize;
        }
    }
    out
}

// the three highest-visit arms, ties to the lowest byte
fn top3(arms: &[ArmStat]) -> Vec<u8> {
    let mut v: Vec<&ArmStat> = arms.iter().collect();
    v.sort_by(|a, b| b.visits.cmp(&a.visits).then(a.action.cmp(&b.action)));
    v.iter().take(3).map(|a| a.action).collect()
}

fn jaccard<T: PartialEq>(a: &[T], b: &[T]) -> f64 {
    let inter = a.iter().filter(|x| b.contains(x)).count();
    let union = a.len() + b.len() - inter;
    if union == 0 { 1.0 } else { inter as f64 / union as f64 }
}

fn mean_pairwise_jaccard<T: PartialEq>(sets: &[Vec<T>]) -> f64 {
    let mut sum = 0.0;
    let mut pairs = 0u32;
    for i in 0..sets.len() {
        for j in i + 1..sets.len() {
            sum += jaccard(&sets[i], &sets[j]);
            pairs += 1;
        }
    }
    if pairs == 0 { 0.0 } else { sum / pairs as f64 }
}

// how many worlds fall into the largest group agreeing on the prefix through ply d, and how
// many distinct prefixes there are; sharing at depth d is "largest group >= 2"
fn prefix_groups<T: PartialEq>(prefixes: &[Vec<T>], d: usize) -> (usize, usize) {
    let mut reps: Vec<(&[T], usize)> = Vec::with_capacity(prefixes.len());
    for p in prefixes {
        let key = &p[..d];
        match reps.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => reps.push((key, 1)),
        }
    }
    (reps.len(), reps.iter().map(|(_, n)| *n).max().unwrap_or(0))
}

struct ShareCell {
    opp_species: u16,
    principal: [(u8, u8); 3],
    opp_named: [Named; 3],
    top3_ours: Vec<u8>,
    iters: u64,
}

#[derive(Default)]
struct ShareRun {
    seed: u64,
    share_raw: [bool; 3],
    share_mng: [bool; 3],
    distinct_raw: [usize; 3],
    distinct_mng: [usize; 3],
    maxgroup_raw: [usize; 3],
    maxgroup_mng: [usize; 3],
    jac_ours: f64,
    jac_opp: f64,
    short: u32,
    truncated: u32,
    one_sided: u32,
    min_iters: u64,
    worlds: Vec<ShareCell>,
}

fn share_root(snap: &NativeSnapshot, position: usize, seed: u64, cfg: &PimcConfig) -> ShareRun {
    let side = snap.side as usize;
    let opp = 1 - side;
    let obs = Observation { state: &snap.state, teams: &snap.teams, our_side: side };
    let belief = &snap.beliefs[side];
    let trace = choose_action_traced(&obs, belief, &RandomBattle, cfg);
    // the same draw `choose_action_traced` made, replayed for the sampled sets the trace omits
    let worlds = RandomBattle.sample_worlds(&obs, belief, cfg.num_worlds, &mut Lcg::new(splitmix64(cfg.seed)));
    assert_eq!(trace.per_world.len(), cfg.num_worlds, "root {position}: world count");
    assert_eq!(worlds.len(), cfg.num_worlds, "root {position}: replayed world count");
    let mut run = ShareRun { seed, min_iters: u64::MAX, ..Default::default() };
    let mut raw: Vec<Vec<(u8, u8)>> = Vec::with_capacity(cfg.num_worlds);
    let mut mng: Vec<Vec<(u8, Named)>> = Vec::with_capacity(cfg.num_worlds);
    let mut ours: Vec<Vec<u8>> = Vec::with_capacity(cfg.num_worlds);
    let mut opps: Vec<Vec<Named>> = Vec::with_capacity(cfg.num_worlds);
    for (k, w) in trace.per_world.iter().enumerate() {
        let ws = &worlds[k].state;
        let am = ws.active_mon(opp);
        assert!(
            (am.species_id, am.item_id, am.ability_id, am.current_hp, am.max_hp, am.status)
                == (w.opp_species, w.opp_item, w.opp_ability, w.opp_hp, w.opp_max_hp, w.opp_status),
            "root {position} world {k}: the replayed world is not the searched one"
        );
        let opp_named = opp_meaning_path(ws, opp, &w.principal);
        run.short += (w.iterations < cfg.max_iters_per_world) as u32;
        run.min_iters = run.min_iters.min(w.iterations);
        run.truncated += w.principal.iter().any(|&(a, b)| a == NO_ARM && b == NO_ARM) as u32;
        run.one_sided += w
            .principal
            .iter()
            .filter(|&&(a, b)| (a == NO_ARM) != (b == NO_ARM))
            .count() as u32;
        raw.push(w.principal.to_vec());
        mng.push(w.principal.iter().zip(opp_named).map(|(&(a, _), n)| (a, n)).collect());
        let t3 = top3(&w.arms);
        opps.push(top3(&w.s2).iter().map(|&b| named(ws, opp, ws.sides[opp].active_index as usize, b)).collect());
        ours.push(t3.clone());
        run.worlds.push(ShareCell {
            opp_species: w.opp_species,
            principal: w.principal,
            opp_named,
            top3_ours: t3,
            iters: w.iterations,
        });
    }
    for d in 0..3 {
        let (dr, gr) = prefix_groups(&raw, d + 1);
        let (dm, gm) = prefix_groups(&mng, d + 1);
        run.distinct_raw[d] = dr;
        run.maxgroup_raw[d] = gr;
        run.share_raw[d] = gr >= 2;
        run.distinct_mng[d] = dm;
        run.maxgroup_mng[d] = gm;
        run.share_mng[d] = gm >= 2;
    }
    run.jac_ours = mean_pairwise_jaccard(&ours);
    run.jac_opp = mean_pairwise_jaccard(&opps);
    run
}

// mean over roots of the per-root value (itself averaged over the seeds in the row), with the
// standard error of that per-root mean at n = roots
fn root_mean_se(per_root: &[Vec<&ShareRun>], f: impl Fn(&ShareRun) -> f64) -> (f64, f64) {
    let x: Vec<f64> = per_root
        .iter()
        .map(|rs| rs.iter().map(|r| f(r)).sum::<f64>() / rs.len() as f64)
        .collect();
    let n = x.len();
    let m = x.iter().sum::<f64>() / n as f64;
    if n < 2 { return (m, 0.0); }
    let sd = (x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt();
    (m, sd / (n as f64).sqrt())
}

fn sharing_row(label: &str, per_root: &[Vec<&ShareRun>], worlds: usize) -> String {
    let all: Vec<&ShareRun> = per_root.iter().flatten().copied().collect();
    let mut s = format!("{label}\t{}\t{worlds}", per_root.len());
    for d in 0..3 {
        let (m, se) = root_mean_se(per_root, |r| r.share_raw[d] as u8 as f64);
        s.push_str(&format!("\t{m:.6}\t{se:.6}"));
    }
    for d in 0..3 {
        let (m, se) = root_mean_se(per_root, |r| r.share_mng[d] as u8 as f64);
        s.push_str(&format!("\t{m:.6}\t{se:.6}"));
    }
    for d in 0..3 {
        let (m, _) = root_mean_se(per_root, |r| r.distinct_raw[d] as f64);
        let (g, _) = root_mean_se(per_root, |r| r.maxgroup_raw[d] as f64);
        s.push_str(&format!("\t{m:.4}\t{g:.4}"));
    }
    for d in 0..3 {
        let (m, _) = root_mean_se(per_root, |r| r.distinct_mng[d] as f64);
        let (g, _) = root_mean_se(per_root, |r| r.maxgroup_mng[d] as f64);
        s.push_str(&format!("\t{m:.4}\t{g:.4}"));
    }
    let (jo, jo_se) = root_mean_se(per_root, |r| r.jac_ours);
    let (jp, jp_se) = root_mean_se(per_root, |r| r.jac_opp);
    let sum = |f: fn(&ShareRun) -> u32| all.iter().map(|r| f(r) as u64).sum::<u64>();
    s.push_str(&format!(
        "\t{jo:.6}\t{jo_se:.6}\t{jp:.6}\t{jp_se:.6}\t{}\t{}\t{}\t{}\n",
        sum(|r| r.truncated),
        sum(|r| (r.truncated > 0) as u32),
        sum(|r| r.one_sided),
        sum(|r| r.short)
    ));
    s
}

fn support_sharing(args: &[String]) {
    let path = arg(args, "--snapshots", "../.decompose/frontier/results/0.1.snapshots.bin");
    let snaps = read_all(&path).unwrap_or_else(|e| panic!("read_all {path}: {e}"));
    println!("snapshots: path={path} count={}", snaps.len());
    let seeds: Vec<u64> = list(args, "--seeds", "1,2,3");
    let worlds: usize = parse(args, "--worlds", "8");
    let iters: u64 = parse(args, "--iters", "35199");
    let time_ms: u64 = parse(args, "--time-ms", "600000");
    let threads: usize = parse(args, "--threads", &std::thread::available_parallelism().map_or(1, |n| n.get()).to_string());
    let out = arg(args, "--out", "results/0.11.sharing.tsv");
    let dump = arg(args, "--dump", "results/0.11.paths.tsv");
    let roots_file = arg(args, "--roots", "results/0.7.roots.tsv");
    let roots = load_roots(&roots_file, &snaps);
    println!("roots: loaded={} file={roots_file}", roots.len());
    assert!(!roots.is_empty(), "no roots kept");
    for p in [&out, &dump] {
        if let Some(dir) = std::path::Path::new(p).parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        }
    }
    println!(
        "support-sharing: roots={} seeds={seeds:?} worlds={worlds} iters={iters} time_ms={time_ms} evaluator=Handcrafted determinizer=RandomBattle chance=OpenLoop pick=Argmax filter={PICK_FILTER} raw_root=false explore_coeff={EXPLORE_COEFF} value_temp=1.0 top_arm=max_visits_lowest_byte_on_ties threads={threads}",
        roots.len()
    );
    let cfg = |seed: u64| PimcConfig {
        num_worlds: worlds,
        time_ms_per_world: time_ms,
        max_iters_per_world: iters,
        seed,
        chance_mode: ChanceMode::OpenLoop,
        pick_mode: PickMode::Argmax,
        filter_threshold: PICK_FILTER,
        raw_root: false,
        explore_coeff: EXPLORE_COEFF,
        value_temp: 1.0,
    };

    let done = AtomicU64::new(0);
    let iters_total = AtomicU64::new(0);
    let first = AtomicBool::new(false);
    let wall = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let runs: Vec<Vec<ShareRun>> = pool.install(|| {
        roots.par_iter().map(|root| {
            let snap = &snaps[root.position];
            let runs: Vec<ShareRun> = seeds.iter().map(|&s| share_root(snap, root.position, s, &cfg(row_seed(s, root.position)))).collect();
            let total: u64 = runs.iter().flat_map(|r| &r.worlds).map(|w| w.iters).sum();
            iters_total.fetch_add(total, Ordering::Relaxed);
            if !first.swap(true, Ordering::Relaxed) {
                println!("first root={} game={} turn={} side={} iters_min={}", root.position, root.game, root.turn, root.side, runs.iter().map(|r| r.min_iters).min().unwrap());
            }
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            if d % 100 == 0 {
                eprintln!("progress roots={d}/{} wall_s={:.0}", roots.len(), wall.elapsed().as_secs_f64());
            }
            runs
        }).collect()
    });
    let wall_s = wall.elapsed().as_secs_f64();
    let total = iters_total.load(Ordering::Relaxed);
    let short: u64 = runs.iter().flatten().map(|r| r.short as u64).sum();
    let truncated: u64 = runs.iter().flatten().map(|r| r.truncated as u64).sum();
    let one_sided: u64 = runs.iter().flatten().map(|r| r.one_sided as u64).sum();
    let short_principals: u64 = runs.iter().flatten().filter(|r| r.truncated > 0).count() as u64;
    println!(
        "support-sharing-done roots={} seeds={} wall_s={wall_s:.1} iters={total} iters_per_s={:.0} iters_per_s_per_thread={:.0} short_searches={short} short_principal_worlds={truncated} short_principal_roots={short_principals} one_sided_plies={one_sided}",
        roots.len(), seeds.len(), total as f64 / wall_s, total as f64 / wall_s / threads as f64
    );

    let byte = |b: u8| if b == NO_ARM { "-".to_string() } else { b.to_string() };
    let mut s = String::from("position\tgame\tturn\tside\tseed\tworld\topp_species\tp1_ours\tp1_opp\tp2_ours\tp2_opp\tp3_ours\tp3_opp\tp1_opp_named\tp2_opp_named\tp3_opp_named\ttop3_ours\titers\tshare_d1_raw\tshare_d2_raw\tshare_d3_raw\tshare_d1_meaning\tshare_d2_meaning\tshare_d3_meaning\tjaccard_ours\tjaccard_opp_meaning\n");
    for (root, rs) in roots.iter().zip(&runs) {
        for r in rs {
            for (k, w) in r.worlds.iter().enumerate() {
                s.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{k}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.6}\n",
                    root.position, root.game, root.turn, root.side, r.seed, w.opp_species,
                    byte(w.principal[0].0), byte(w.principal[0].1),
                    byte(w.principal[1].0), byte(w.principal[1].1),
                    byte(w.principal[2].0), byte(w.principal[2].1),
                    w.opp_named[0].label(), w.opp_named[1].label(), w.opp_named[2].label(),
                    w.top3_ours.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(","),
                    w.iters,
                    r.share_raw[0] as u8, r.share_raw[1] as u8, r.share_raw[2] as u8,
                    r.share_mng[0] as u8, r.share_mng[1] as u8, r.share_mng[2] as u8,
                    r.jac_ours, r.jac_opp
                ));
            }
        }
    }
    std::fs::write(&dump, s).unwrap_or_else(|e| panic!("write {dump}: {e}"));

    let mut t = String::from("seed\troots\tworlds\tshare_d1_raw\tshare_d1_raw_se\tshare_d2_raw\tshare_d2_raw_se\tshare_d3_raw\tshare_d3_raw_se\tshare_d1_meaning\tshare_d1_meaning_se\tshare_d2_meaning\tshare_d2_meaning_se\tshare_d3_meaning\tshare_d3_meaning_se\tdistinct_d1_raw\tmaxgroup_d1_raw\tdistinct_d2_raw\tmaxgroup_d2_raw\tdistinct_d3_raw\tmaxgroup_d3_raw\tdistinct_d1_meaning\tmaxgroup_d1_meaning\tdistinct_d2_meaning\tmaxgroup_d2_meaning\tdistinct_d3_meaning\tmaxgroup_d3_meaning\tjaccard_ours\tjaccard_ours_se\tjaccard_opp_meaning\tjaccard_opp_meaning_se\tshort_principal_worlds\tshort_principal_roots\tone_sided_plies\tshort_searches\n");
    for (i, seed) in seeds.iter().enumerate() {
        let per_root: Vec<Vec<&ShareRun>> = runs.iter().map(|rs| vec![&rs[i]]).collect();
        t.push_str(&sharing_row(&seed.to_string(), &per_root, worlds));
    }
    let per_root: Vec<Vec<&ShareRun>> = runs.iter().map(|rs| rs.iter().collect()).collect();
    t.push_str(&sharing_row("pooled", &per_root, worlds));
    print!("{t}");
    std::fs::write(&out, t).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("support-sharing-out summary={out} dump={dump} roots={roots_file}");
}

const N_ARMS: usize = 5;
const ARM_A: usize = 0;
const ARM_C: usize = 2;
const ARM_RESEED: usize = 3;

struct ArmSpec {
    label: String,
    worlds: usize,
    iters: u64,
    reseed: bool,
    salt: u64,
}

// "8x8192" -> (8, 8192)
fn worlds_iters(args: &[String], k: &str, d: &str) -> (usize, u64) {
    let v = arg(args, k, d);
    let (w, i) = v.split_once('x').unwrap_or_else(|| panic!("{k} {v:?}: want WORLDSxITERS"));
    (w.parse().unwrap_or_else(|e| panic!("{k} {v:?}: {e}")), i.parse().unwrap_or_else(|e| panic!("{k} {v:?}: {e}")))
}

#[derive(Default)]
struct FlipRun {
    seed: u64,
    picks: [u8; N_ARMS],
    iters: [u64; N_ARMS],
    depth: [u64; N_ARMS],
    short: [u32; N_ARMS],
    prefix_shared: bool,
}

// (move-move, move-switch, switch-switch, tera-non-tera); the first three tile every flip, the
// fourth is an orthogonal cut that overlaps them.
fn flip_split(a: u8, b: u8) -> [bool; 4] {
    let (sa, sb) = (is_switch(Some(a)), is_switch(Some(b)));
    let (ta, tb) = (is_tera(Some(a)), is_tera(Some(b)));
    [!sa && !sb, sa != sb, sa && sb, ta != tb]
}

// The world draw is `Lcg::new(splitmix64(cfg.seed))` consumed once per world in order, so a
// larger draw should reproduce a smaller one's worlds as its prefix. Only `World::weight` differs.
fn world_prefix_shared(snap: &NativeSnapshot, cfg_seed: u64, small: usize, large: usize) -> bool {
    let side = snap.side as usize;
    let obs = Observation { state: &snap.state, teams: &snap.teams, our_side: side };
    let belief = &snap.beliefs[side];
    let a = RandomBattle.sample_worlds(&obs, belief, small, &mut Lcg::new(splitmix64(cfg_seed)));
    let b = RandomBattle.sample_worlds(&obs, belief, large, &mut Lcg::new(splitmix64(cfg_seed)));
    large >= small && a.len() == small && a.iter().zip(&b).all(|(x, y)| x.state == y.state && x.teams == y.teams)
}

fn flip_root(snap: &NativeSnapshot, position: usize, run_seed: u64, specs: &[ArmSpec; N_ARMS], time_ms: u64) -> FlipRun {
    let side = snap.side as usize;
    let obs = Observation { state: &snap.state, teams: &snap.teams, our_side: side };
    let belief = &snap.beliefs[side];
    let base_seed = row_seed(run_seed, position);
    let reseed_seed = row_seed(run_seed ^ RESEED_SALT, position);
    let mut run = FlipRun { seed: run_seed, ..Default::default() };
    for (k, sp) in specs.iter().enumerate() {
        let cfg = PimcConfig {
            num_worlds: sp.worlds,
            time_ms_per_world: time_ms,
            max_iters_per_world: sp.iters,
            seed: if sp.reseed { reseed_seed } else { base_seed },
            chance_mode: ChanceMode::OpenLoop,
            pick_mode: PickMode::Argmax,
            filter_threshold: PICK_FILTER,
            raw_root: false,
            explore_coeff: EXPLORE_COEFF,
            value_temp: 1.0,
        };
        let t = choose_action_traced_salted(&obs, belief, &RandomBattle, &cfg, sp.salt);
        assert_eq!(t.per_world.len(), sp.worlds, "root {position} arm {}: world count", sp.label);
        run.picks[k] = t.picked;
        run.iters[k] = t.per_world.iter().map(|w| w.iterations).sum();
        run.depth[k] = t.per_world.iter().map(|w| w.depth_sum).sum();
        run.short[k] = t.per_world.iter().filter(|w| w.iterations < sp.iters).count() as u32;
    }
    run.prefix_shared = world_prefix_shared(snap, base_seed, specs[ARM_A].worlds, specs[ARM_C].worlds);
    run
}

fn mean_se(v: &[f64]) -> (f64, f64) {
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    if v.len() < 2 {
        return (m, 0.0);
    }
    let var = v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0);
    (m, (var / n).sqrt())
}

// One row per contrast: rates are per-root means over the row's seeds, so every SE is at n = roots.
fn contrast_row(label: &str, seed: &str, per_root: &[Vec<&FlipRun>], arm: usize) -> String {
    let roots = per_root.len();
    let per_root_mean = |f: &dyn Fn(&FlipRun) -> f64| -> Vec<f64> {
        per_root.iter().map(|rs| rs.iter().map(|&r| f(r)).sum::<f64>() / rs.len() as f64).collect()
    };
    let flipped = |r: &FlipRun| (r.picks[ARM_A] != r.picks[arm]) as u8 as f64;
    let (rate, se) = mean_se(&per_root_mean(&flipped));
    let mut cells = String::new();
    let mut counts = String::new();
    for k in 0..4 {
        let f = move |r: &FlipRun| {
            (r.picks[ARM_A] != r.picks[arm] && flip_split(r.picks[ARM_A], r.picks[arm])[k]) as u8 as f64
        };
        let (m, s) = mean_se(&per_root_mean(&f));
        let n: f64 = per_root.iter().flatten().map(|&r| f(r)).sum();
        counts.push_str(&format!("\t{n:.0}"));
        cells.push_str(&format!("\t{m:.6}\t{s:.6}"));
    }
    let floor = |r: &FlipRun| (r.picks[ARM_A] != r.picks[ARM_RESEED]) as u8 as f64;
    let d: Vec<f64> = per_root
        .iter()
        .map(|rs| rs.iter().map(|&r| flipped(r) - floor(r)).sum::<f64>() / rs.len() as f64)
        .collect();
    let (dm, dse) = mean_se(&d);
    let flips: f64 = per_root.iter().flatten().map(|&r| flipped(r)).sum();
    format!(
        "contrast\t{label}\t{seed}\t{roots}\t{flips:.0}\t{rate:.6}\t{se:.6}{counts}{cells}\t{:.4}\t{:.4}\n",
        dm * 100.0,
        dse * 100.0
    )
}

fn arm_row(spec: &ArmSpec, seed: &str, runs: &[&FlipRun], arm: usize, roots: usize) -> String {
    let iters: u64 = runs.iter().map(|r| r.iters[arm]).sum();
    let depth: u64 = runs.iter().map(|r| r.depth[arm]).sum();
    let short: u64 = runs.iter().map(|r| r.short[arm] as u64).sum();
    format!(
        "arm\t{}\t{seed}\t{roots}\t{}\t{}\t{iters}\t{depth}\t{:.6}\t{short}\n",
        spec.label,
        spec.worlds,
        spec.iters,
        depth as f64 / iters as f64
    )
}

fn worlds_flip(args: &[String]) {
    let path = arg(args, "--snapshots", "../.decompose/frontier/results/0.1.snapshots.bin");
    let snaps = read_all(&path).unwrap_or_else(|e| panic!("read_all {path}: {e}"));
    println!("snapshots: path={path} count={}", snaps.len());
    let seeds: Vec<u64> = list(args, "--seeds", "1,2,3");
    let time_ms: u64 = parse(args, "--time-ms", "600000");
    let threads: usize = parse(args, "--threads", &std::thread::available_parallelism().map_or(1, |n| n.get()).to_string());
    let out = arg(args, "--out", "results/0.9.worlds.tsv");
    let dump = arg(args, "--dump", "");
    let roots_file = arg(args, "--roots", "results/0.7.roots.tsv");
    let (aw, ai) = worlds_iters(args, "--arm-a", "8x8192");
    let (bw, bi) = worlds_iters(args, "--arm-b", "64x1024");
    let (cw, ci) = worlds_iters(args, "--arm-c", "64x8192");
    let (fw, fi) = worlds_iters(args, "--floor", "8x8192");
    let name = |w: usize, i: u64| format!("w{w}_i{i}");
    let specs: [ArmSpec; N_ARMS] = [
        ArmSpec { label: name(aw, ai), worlds: aw, iters: ai, reseed: false, salt: 0 },
        ArmSpec { label: name(bw, bi), worlds: bw, iters: bi, reseed: false, salt: 0 },
        ArmSpec { label: name(cw, ci), worlds: cw, iters: ci, reseed: false, salt: 0 },
        ArmSpec { label: format!("{}_reseed", name(fw, fi)), worlds: fw, iters: fi, reseed: true, salt: 0 },
        ArmSpec { label: format!("{}_salt", name(fw, fi)), worlds: fw, iters: fi, reseed: false, salt: SEARCH_SALT },
    ];
    let roots = load_roots(&roots_file, &snaps);
    println!("roots: loaded={} file={roots_file}", roots.len());
    assert!(!roots.is_empty(), "no roots kept");
    for p in [&out, &dump] {
        if p.is_empty() {
            continue;
        }
        if let Some(dir) = std::path::Path::new(p).parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        }
    }
    println!(
        "worlds-flip: roots={} seeds={seeds:?} arms=[{}] time_ms={time_ms} evaluator=Handcrafted determinizer=RandomBattle chance=OpenLoop pick=Argmax filter={PICK_FILTER} raw_root=false explore_coeff={EXPLORE_COEFF} value_temp=1.0 prior=none reseed=row_seed(seed^0x{RESEED_SALT:x}) search_salt=0x{SEARCH_SALT:x} threads={threads}",
        roots.len(),
        specs.iter().map(|s| format!("{} {}x{}", s.label, s.worlds, s.iters)).collect::<Vec<_>>().join(", ")
    );

    let done = AtomicU64::new(0);
    let iters_total = AtomicU64::new(0);
    let first = AtomicBool::new(false);
    let wall = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let runs: Vec<Vec<FlipRun>> = pool.install(|| {
        roots
            .par_iter()
            .map(|root| {
                let snap = &snaps[root.position];
                let runs: Vec<FlipRun> = seeds.iter().map(|&s| flip_root(snap, root.position, s, &specs, time_ms)).collect();
                let total: u64 = runs.iter().map(|r| r.iters.iter().sum::<u64>()).sum();
                iters_total.fetch_add(total, Ordering::Relaxed);
                if !first.swap(true, Ordering::Relaxed) {
                    println!("first root={} game={} turn={} side={} picks={:?}", root.position, root.game, root.turn, root.side, runs[0].picks);
                }
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                if d % 100 == 0 {
                    eprintln!("progress roots={d}/{} wall_s={:.0}", roots.len(), wall.elapsed().as_secs_f64());
                }
                runs
            })
            .collect()
    });
    let wall_s = wall.elapsed().as_secs_f64();
    let total = iters_total.load(Ordering::Relaxed);
    let short: u64 = runs.iter().flatten().map(|r| r.short.iter().map(|&x| x as u64).sum::<u64>()).sum();
    let shared = runs.iter().flatten().filter(|r| r.prefix_shared).count();
    println!(
        "worlds-flip-done roots={} seeds={} wall_s={wall_s:.1} iters={total} iters_per_s={:.0} iters_per_s_per_thread={:.0} short_searches={short} c_prefix_equals_a={shared}/{}",
        roots.len(),
        seeds.len(),
        total as f64 / wall_s,
        total as f64 / wall_s / threads as f64,
        runs.iter().flatten().count()
    );

    if !dump.is_empty() {
        let mut s = String::from("position\tgame\tturn\tside\tseed");
        for sp in &specs {
            s.push_str(&format!("\tpick_{}", sp.label));
        }
        s.push_str("\tc_prefix_equals_a\n");
        for (root, rs) in roots.iter().zip(&runs) {
            for r in rs {
                s.push_str(&format!("{}\t{}\t{}\t{}\t{}", root.position, root.game, root.turn, root.side, r.seed));
                for p in r.picks {
                    s.push_str(&format!("\t{p}"));
                }
                s.push_str(&format!("\t{}\n", r.prefix_shared as u8));
            }
        }
        std::fs::write(&dump, s).unwrap_or_else(|e| panic!("write {dump}: {e}"));
    }

    let mut t = String::from("kind\tlabel\tseed\troots\tflips_or_worlds\trate_or_iters_cap\tse_or_iters\tmm\tms\tss\ttn\tmm_rate\tmm_se\tms_rate\tms_se\tss_rate\tss_se\ttn_rate\ttn_se\tminus_reseed_floor_pp\tminus_reseed_floor_se_pp\n");
    let mut a = String::from("kind\tlabel\tseed\troots\tworlds\titers_cap\titers_total\tdepth_total\tmean_depth_per_world\tshort_searches\n");
    for (i, seed) in seeds.iter().enumerate() {
        let per_root: Vec<Vec<&FlipRun>> = runs.iter().map(|rs| vec![&rs[i]]).collect();
        let flat: Vec<&FlipRun> = runs.iter().map(|rs| &rs[i]).collect();
        for k in 1..N_ARMS {
            t.push_str(&contrast_row(&format!("{}_vs_{}", specs[ARM_A].label, specs[k].label), &seed.to_string(), &per_root, k));
        }
        for (k, sp) in specs.iter().enumerate() {
            a.push_str(&arm_row(sp, &seed.to_string(), &flat, k, roots.len()));
        }
    }
    let per_root: Vec<Vec<&FlipRun>> = runs.iter().map(|rs| rs.iter().collect()).collect();
    let flat: Vec<&FlipRun> = runs.iter().flatten().collect();
    for k in 1..N_ARMS {
        t.push_str(&contrast_row(&format!("{}_vs_{}", specs[ARM_A].label, specs[k].label), "pooled", &per_root, k));
    }
    for (k, sp) in specs.iter().enumerate() {
        a.push_str(&arm_row(sp, "pooled", &flat, k, roots.len()));
    }
    print!("{t}\n{a}");
    std::fs::write(&out, format!("{t}\n{a}")).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("worlds-flip-out summary={out} dump={dump:?} roots={roots_file}");
}

// gen_sets tera is Showdown-space; MonSlot's is engine-space. This is exactly the map
// `determinize::install` applies at build time, Stellar identity included.
fn set_tera_to_engine(t: u8) -> u8 {
    if t == 18 {
        18
    } else {
        showdown_type_to_engine(t)
    }
}

fn sorted_moves(m: [u16; 4]) -> [u16; 4] {
    let mut m = m;
    m.sort_unstable();
    m
}

// The true set is the pool entry the mon was built from: every field `build_mon` installs, taken
// from the snapshot's true state (moves, item, ability, tera, level, gender) and its true TeamData
// (EVs, IVs). Species is fixed by the pool lookup, which follows the base-species fallback.
// `item` is skipped only under the relaxed rule, and only for a mon whose item is already gone.
fn set_is_true_mon(set: &SetEntry, mon: &MonSlot, bd: &MonBuildData, relax_item: bool) -> bool {
    let item_ok = set.item_id == mon.item_id || (relax_item && mon.item_id == 0);
    item_ok
        && sorted_moves(set.moves) == sorted_moves(mon.moves)
        && set.ability_id == mon.ability_id
        && set_tera_to_engine(set.tera_type) == mon.tera_type
        && set.level == mon.level
        && set.is_female == (mon.flags & MON_FLAG_FEMALE != 0)
        && set.evs == bd.evs
        && set.ivs == bd.ivs
}

fn known_mon_belief(belief: &Belief, species_id: u16) -> Option<&MonBelief> {
    let base = data_bridge::base_species(species_id);
    belief.mons.iter().find(|mb| mb.species_id != 0 && data_bridge::base_species(mb.species_id) == base)
}

#[derive(Default)]
struct NllRow {
    position: usize,
    game: u64,
    turn: u16,
    side: u8,
    species: u16,
    pool: usize,
    support: usize,
    matches: usize,
    matches_pool: usize,
    nll: f64,
    h: f64,
    nll_relaxed: f64,
    item_zero: bool,
    scored: bool,
    relaxed_scored: bool,
    excl_no_slot: bool,
    excl_no_pool: bool,
    excl_no_match_anywhere: bool,
    excl_match_pool_only: bool,
}

fn nll_root(snap: &NativeSnapshot, position: usize) -> NllRow {
    let side = snap.side as usize;
    let opp = 1 - side;
    let mon = snap.state.active_mon(opp);
    let bd = &snap.teams.mons[opp][snap.state.sides[opp].active_index as usize];
    let mut row = NllRow {
        position,
        game: snap.game,
        turn: snap.turn,
        side: snap.side,
        species: mon.species_id,
        item_zero: mon.item_id == 0,
        ..Default::default()
    };
    let Some(mb) = known_mon_belief(&snap.beliefs[side], mon.species_id) else {
        row.excl_no_slot = true;
        return row;
    };
    let Some(ss) = species_sets_with_base_fallback(mon.species_id) else {
        row.excl_no_pool = true;
        return row;
    };
    row.pool = ss.sets.len();
    row.matches_pool = ss.sets.iter().filter(|s| set_is_true_mon(s, mon, bd, false)).count();
    let live: Vec<&SetEntry> = ss.sets.iter().enumerate().filter(|(i, s)| possible(*i, s, mb)).map(|(_, s)| s).collect();
    row.support = live.len();
    let total: f64 = live.iter().map(|s| s.count as f64).sum();
    if total <= 0.0 {
        row.excl_no_match_anywhere = true;
        return row;
    }
    row.h = -live.iter().map(|s| s.count as f64 / total).map(|p| if p > 0.0 { p * p.ln() } else { 0.0 }).sum::<f64>();
    let mass = |relax: bool| -> f64 { live.iter().filter(|s| set_is_true_mon(s, mon, bd, relax)).map(|s| s.count as f64).sum() };
    row.matches = live.iter().filter(|s| set_is_true_mon(s, mon, bd, false)).count();
    // an item consumed in battle zeroes MonSlot.item_id, so the strict rule misses that mon; the
    // relaxed rule drops item for exactly those and is reported as a sensitivity, never as headline
    let relaxed_mass = mass(true);
    if relaxed_mass > 0.0 {
        row.relaxed_scored = true;
        row.nll_relaxed = -(relaxed_mass / total).ln();
    }
    if row.matches == 0 {
        if row.matches_pool == 0 {
            row.excl_no_match_anywhere = true;
        } else {
            row.excl_match_pool_only = true;
        }
        return row;
    }
    row.nll = -(mass(false) / total).ln();
    row.scored = true;
    row
}

const NLL_BUCKETS: [(u16, u16); 4] = [(2, 4), (5, 8), (9, 14), (15, 20)];

fn nll_bucket_row(label: &str, rows: &[&NllRow]) -> String {
    let scored: Vec<&NllRow> = rows.iter().copied().filter(|r| r.scored).collect();
    let mean = |f: &dyn Fn(&NllRow) -> f64| -> f64 {
        if scored.is_empty() {
            0.0
        } else {
            scored.iter().map(|&r| f(r)).sum::<f64>() / scored.len() as f64
        }
    };
    let d: Vec<f64> = scored.iter().map(|r| r.nll - r.h).collect();
    let (dm, dse) = if d.is_empty() { (0.0, 0.0) } else { mean_se(&d) };
    let rd: Vec<f64> = rows.iter().filter(|r| r.relaxed_scored).map(|r| r.nll_relaxed - r.h).collect();
    let (rm, rse) = if rd.is_empty() { (0.0, 0.0) } else { mean_se(&rd) };
    let count = |f: &dyn Fn(&NllRow) -> bool| rows.iter().filter(|&&r| f(r)).count();
    format!(
        "{label}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.4}\t{:.4}\t{}\t{}\t{}\t{:.6}\t{:.6}\n",
        rows.len(),
        scored.len(),
        rows.len() - scored.len(),
        count(&|r| r.excl_no_slot),
        count(&|r| r.excl_no_pool),
        count(&|r| r.excl_no_match_anywhere),
        count(&|r| r.excl_match_pool_only),
        mean(&|r| r.nll),
        mean(&|r| r.h),
        dm,
        dse,
        mean(&|r| r.support as f64),
        mean(&|r| r.pool as f64),
        count(&|r| r.item_zero),
        count(&|r| r.matches > 1),
        count(&|r| r.relaxed_scored),
        rm,
        rse
    )
}

fn prior_nll(args: &[String]) {
    let path = arg(args, "--snapshots", "../.decompose/frontier/results/0.1.snapshots.bin");
    let snaps = read_all(&path).unwrap_or_else(|e| panic!("read_all {path}: {e}"));
    println!("snapshots: path={path} count={}", snaps.len());
    let out = arg(args, "--out", "results/0.9.nll.tsv");
    let dump = arg(args, "--dump", "");
    let roots_file = arg(args, "--roots", "results/0.7.roots.tsv");
    let roots = load_roots(&roots_file, &snaps);
    println!("roots: loaded={} file={roots_file}", roots.len());
    assert!(!roots.is_empty(), "no roots kept");
    for p in [&out, &dump] {
        if p.is_empty() {
            continue;
        }
        if let Some(dir) = std::path::Path::new(p).parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        }
    }
    println!("prior-nll: roots={} prior=count_weighted_over_gen_sets restricted_by=belief::possible belief=beliefs[decider] buckets={NLL_BUCKETS:?}", roots.len());

    // what the native beliefs actually carry, since `possible` reads exactly these
    let (mut pool_active, mut with_item, mut with_ability, mut with_scarf, mut with_excl, mut with_tera, mut with_moves) = (0, 0, 0, 0, 0, 0, 0);
    for root in &roots {
        let snap = &snaps[root.position];
        let mon = snap.state.active_mon(1 - snap.side as usize);
        if let Some(mb) = known_mon_belief(&snap.beliefs[snap.side as usize], mon.species_id) {
            pool_active += mb.pool_active as usize;
            with_item += (mb.item_id != 0) as usize;
            with_ability += (mb.ability_id != 0) as usize;
            with_scarf += (mb.scarf_item_id != 0) as usize;
            with_excl += (mb.excluded_bits != 0) as usize;
            with_tera += mb.tera_revealed as usize;
            with_moves += (mb.n_moves > 0) as usize;
        }
    }
    println!(
        "belief-content roots={} pool_active={pool_active} item_known={with_item} ability_known={with_ability} scarf_pinned={with_scarf} excluded_bits={with_excl} tera_revealed={with_tera} any_move_revealed={with_moves}",
        roots.len()
    );

    let rows: Vec<NllRow> = roots.iter().map(|r| nll_root(&snaps[r.position], r.position)).collect();
    let ambiguous = rows.iter().filter(|r| r.matches > 1).count();
    println!(
        "prior-nll-done roots={} scored={} excluded={} multi_match={ambiguous}",
        rows.len(),
        rows.iter().filter(|r| r.scored).count(),
        rows.iter().filter(|r| !r.scored).count()
    );

    if !dump.is_empty() {
        let mut s = String::from("position\tgame\tturn\tside\topp_species\tpool\tsupport\tmatches\tmatches_pool\tnll\th\tnll_minus_h\tscored\titem_zero\trelaxed_scored\tnll_relaxed\n");
        for r in &rows {
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{:.6}\t{}\t{}\t{}\t{:.6}\n",
                r.position, r.game, r.turn, r.side, r.species, r.pool, r.support, r.matches, r.matches_pool, r.nll, r.h, r.nll - r.h, r.scored as u8, r.item_zero as u8, r.relaxed_scored as u8, r.nll_relaxed
            ));
        }
        std::fs::write(&dump, s).unwrap_or_else(|e| panic!("write {dump}: {e}"));
    }

    let mut t = String::from("turn_bucket\troots\tscored\texcluded\texcl_no_belief_slot\texcl_no_set_pool\texcl_no_match_anywhere\texcl_match_in_pool_only\tmean_nll_nats\tmean_entropy_nats\tmean_nll_minus_h_nats\tse_nll_minus_h_nats\tmean_restricted_support\tmean_pool_size\titem_zero\tmulti_match\trelaxed_scored\trelaxed_nll_minus_h_nats\tse_relaxed_nll_minus_h_nats\n");
    for (lo, hi) in NLL_BUCKETS {
        let sel: Vec<&NllRow> = rows.iter().filter(|r| (lo..=hi).contains(&r.turn)).collect();
        t.push_str(&nll_bucket_row(&format!("{lo}-{hi}"), &sel));
    }
    let outside: Vec<&NllRow> = rows.iter().filter(|r| !NLL_BUCKETS.iter().any(|&(lo, hi)| (lo..=hi).contains(&r.turn))).collect();
    t.push_str(&nll_bucket_row("outside", &outside));
    t.push_str(&nll_bucket_row("pooled", &rows.iter().collect::<Vec<_>>()));
    print!("{t}");
    std::fs::write(&out, t).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("prior-nll-out summary={out} dump={dump:?} roots={roots_file}");
}

#[cfg(feature = "train_value")]
const LEAF_BINS: usize = 20;

// midpoint of bin b of the 20 equal bins over [0, 1] that `SearchResult.leaf_hist` counts
#[cfg(feature = "train_value")]
fn leaf_bin_mid(b: usize) -> f64 {
    (b as f64 + 0.5) / LEAF_BINS as f64
}

// SD is of the binned distribution, at bin midpoints -- the histogram is all the search keeps.
// "outside" is bin 0 plus bin 19, i.e. a leaf value under 0.05 or 0.95 and over.
#[cfg(feature = "train_value")]
fn leaf_stats(hist: &[u32; LEAF_BINS]) -> (u64, f64, f64) {
    let n: u64 = hist.iter().map(|&c| c as u64).sum();
    if n == 0 {
        return (0, 0.0, 0.0);
    }
    let mut mean = 0.0;
    for (b, &c) in hist.iter().enumerate() {
        mean += leaf_bin_mid(b) * c as f64;
    }
    mean /= n as f64;
    let mut var = 0.0;
    for (b, &c) in hist.iter().enumerate() {
        let d = leaf_bin_mid(b) - mean;
        var += d * d * c as f64;
    }
    (n, (var / n as f64).sqrt(), (hist[0] as f64 + hist[LEAF_BINS - 1] as f64) / n as f64)
}

#[cfg(feature = "train_value")]
struct LeafCell {
    iterations: u64,
    leaves: u64,
    sd: f64,
    outside: f64,
    hist: [u32; LEAF_BINS],
}

#[cfg(feature = "train_value")]
fn leaf_cell(row: &Row, evaluator: &impl Evaluator, params: &SearchParams, seed: u64) -> LeafCell {
    let r = search_world(&row.state, &row.teams, evaluator, &OpenLoop, params, seed, row.side as usize, None);
    let (leaves, sd, outside) = leaf_stats(&r.leaf_hist);
    LeafCell { iterations: r.iterations, leaves, sd, outside, hist: r.leaf_hist }
}

#[cfg(feature = "train_value")]
const LEAF_EVALUATORS: [&str; 2] = ["learned_v2", "handcrafted"];
#[cfg(feature = "train_value")]
const OVER_SHARE: f64 = 1.0 / 3.0;

#[cfg(feature = "train_value")]
fn leaf_summary_row(evaluator: &str, stratum: &str, cells: &[&LeafCell], iqr_ref: Option<f64>) -> (String, f64) {
    if cells.is_empty() {
        return (format!("{evaluator}\t{stratum}\t0\t-\t-\t-\t-\t-\t-\t-\t-\t-\n"), 0.0);
    }
    let n = cells.len() as f64;
    let mut sds: Vec<f64> = cells.iter().map(|c| c.sd).collect();
    sds.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (q1, med, q3) = (percentile(&sds, 0.25), percentile(&sds, 0.5), percentile(&sds, 0.75));
    let iqr = q3 - q1;
    let leaves: u64 = cells.iter().map(|c| c.leaves).sum();
    let ratio = match iqr_ref {
        Some(r) if r > 0.0 => format!("{:.4}", iqr / r),
        _ => "-".to_string(),
    };
    let line = format!(
        "{evaluator}\t{stratum}\t{}\t{:.1}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{ratio}\n",
        cells.len(),
        leaves as f64 / n,
        cells.iter().map(|c| c.sd).sum::<f64>() / n,
        q1, med, q3, iqr,
        cells.iter().map(|c| c.outside).sum::<f64>() / n,
        cells.iter().filter(|c| c.outside > OVER_SHARE).count() as f64 / n,
    );
    (line, iqr)
}

#[cfg(not(feature = "train_value"))]
fn leaf_dispersion(_args: &[String]) {
    eprintln!("--leaf-dispersion: built without train_value; rebuild with `cargo build --release --features train_value`");
    std::process::exit(2);
}

#[cfg(feature = "train_value")]
fn leaf_dispersion(args: &[String]) {
    let rows = load(args);
    let run_seed: u64 = parse(args, "--seed", "1");
    let iters: u64 = parse(args, "--iters", "16384");
    let threads: usize = parse(args, "--threads", &std::thread::available_parallelism().map_or(1, |n| n.get()).to_string());
    let out = arg(args, "--out", "results/0.14.dispersion.tsv");
    let dump = arg(args, "--dump", "results/0.14.leaves.tsv");
    for path in [&out, &dump] {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        }
    }
    let net = LearnedValueV2::from_env();
    let params = SearchParams { max_iters: iters, time_ms: TIME_MS, explore_coeff: EXPLORE_COEFF, ..Default::default() };
    println!(
        "leaf-dispersion: rows={} iters={iters} seed={run_seed} threads={threads} evaluators={LEAF_EVALUATORS:?} chance=OpenLoop explore_coeff={EXPLORE_COEFF} time_ms={TIME_MS} value_temp={} decider=row.side bins={LEAF_BINS} net={} int8_scope={} load_start={}",
        rows.len(), params.value_temp, std::env::var("BRIDGE_EVAL_WEIGHTS_V2").unwrap_or_default(), net.int8_scope(), loadavg()
    );
    let done = AtomicU64::new(0);
    let wall = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let cells: Vec<[LeafCell; 2]> = pool.install(|| {
        rows.par_iter().enumerate().map(|(i, row)| {
            let seed = row_seed(run_seed, i);
            let cell = [leaf_cell(row, &net, &params, seed), leaf_cell(row, &Handcrafted, &params, seed)];
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            if d % 500 == 0 {
                eprintln!("progress rows={d}/{} wall_s={:.0}", rows.len(), wall.elapsed().as_secs_f64());
            }
            cell
        }).collect()
    });
    let wall_s = wall.elapsed().as_secs_f64();
    let total_iters: u64 = cells.iter().flatten().map(|c| c.iterations).sum();
    let short: usize = cells.iter().flatten().filter(|c| c.iterations < iters).count();
    println!(
        "leaf-dispersion-done rows={} searches={} wall_s={wall_s:.1} iters={total_iters} iters_per_s={:.0} iters_per_s_per_thread={:.0} short_searches={short} load_end={}",
        rows.len(), cells.len() * LEAF_EVALUATORS.len(), total_iters as f64 / wall_s, total_iters as f64 / wall_s / threads as f64, loadavg()
    );

    let mut d = String::from("row\tgame_index\tgame_tag\tgen_eval\tturn\tside\tz\tevaluator\titerations\tleaves\tsd\toutside");
    for b in 0..LEAF_BINS {
        d.push_str(&format!("\tb{b}"));
    }
    d.push('\n');
    for (i, (row, cs)) in rows.iter().zip(&cells).enumerate() {
        for (e, c) in LEAF_EVALUATORS.iter().zip(cs) {
            d.push_str(&format!(
                "{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{e}\t{}\t{}\t{:.6}\t{:.6}\t{}\n",
                row.game_index, row.game_tag, row.gen_eval, row.turn, row.side, row.z, c.iterations, c.leaves, c.sd, c.outside,
                c.hist.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("\t")
            ));
        }
    }
    std::fs::write(&dump, d).unwrap_or_else(|e| panic!("write {dump}: {e}"));

    let strata: [(&str, Option<u8>); 3] = [("all", None), ("gen_eval=0", Some(0)), ("gen_eval=1", Some(1))];
    let mut t = String::from("evaluator\tstratum\troots\tleaves_mean\tsd_mean\tsd_q1\tsd_median\tsd_q3\tsd_iqr\toutside_mean\troots_over_third\tiqr_ratio_vs_hand\n");
    for (stratum, gen) in strata {
        let pick = |e: usize| -> Vec<&LeafCell> {
            rows.iter().zip(&cells).filter(|(r, _)| gen.is_none_or(|g| r.gen_eval == g)).map(|(_, c)| &c[e]).collect()
        };
        let (hand_line, hand_iqr) = leaf_summary_row(LEAF_EVALUATORS[1], stratum, &pick(1), None);
        let (net_line, _) = leaf_summary_row(LEAF_EVALUATORS[0], stratum, &pick(0), Some(hand_iqr));
        t.push_str(&net_line);
        t.push_str(&hand_line);
    }
    t.push_str("\nevaluator\tstratum\tbin\tlo\thi\tcount\tshare\n");
    for (stratum, gen) in strata {
        for (e, name) in LEAF_EVALUATORS.iter().enumerate() {
            let mut pooled = [0u64; LEAF_BINS];
            for (_, c) in rows.iter().zip(&cells).filter(|(r, _)| gen.is_none_or(|g| r.gen_eval == g)) {
                for (b, &x) in c[e].hist.iter().enumerate() {
                    pooled[b] += x as u64;
                }
            }
            let total: u64 = pooled.iter().sum();
            for (b, &x) in pooled.iter().enumerate() {
                let share = if total > 0 { format!("{:.8}", x as f64 / total as f64) } else { "-".to_string() };
                t.push_str(&format!(
                    "{name}\t{stratum}\t{b}\t{:.2}\t{:.2}\t{x}\t{share}\n",
                    b as f64 / LEAF_BINS as f64, (b + 1) as f64 / LEAF_BINS as f64
                ));
            }
        }
    }
    std::fs::write(&out, t).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("leaf-dispersion-out {out} dump={dump}");
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(h: &mut u64, v: u16) {
    for b in v.to_le_bytes() {
        *h ^= b as u64;
        *h = h.wrapping_mul(FNV_PRIME);
    }
}

const DUP_KINDS: [&str; 4] = ["rev-rev", "rev-unrev", "unrev-unrev", "true-true"];

#[derive(Default)]
struct DupTally {
    roots: usize,
    worlds: usize,
    dup_worlds: usize,
    dup_worlds_opp: usize,
    dup_worlds_us: usize,
    dup_pairs: usize,
    kinds: [usize; 4],
    offenders: std::collections::BTreeMap<u16, usize>,
}

impl DupTally {
    fn merge(&mut self, o: &DupTally) {
        self.roots += o.roots;
        self.worlds += o.worlds;
        self.dup_worlds += o.dup_worlds;
        self.dup_worlds_opp += o.dup_worlds_opp;
        self.dup_worlds_us += o.dup_worlds_us;
        self.dup_pairs += o.dup_pairs;
        for k in 0..4 {
            self.kinds[k] += o.kinds[k];
        }
        for (b, c) in &o.offenders {
            *self.offenders.entry(*b).or_default() += c;
        }
    }

    fn row(&self, det: &str, label: &str) -> String {
        format!(
            "{det}\t{label}\t{}\t{}\t{}\t{:.6}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            self.roots, self.worlds, self.dup_worlds, self.dup_worlds as f64 / self.worlds.max(1) as f64,
            self.dup_worlds_opp, self.dup_worlds_us, self.dup_pairs, self.kinds[0], self.kinds[1], self.kinds[2], self.kinds[3]
        )
    }
}

fn revealed_by(belief: &Belief, true_species: u16) -> bool {
    let base = data_bridge::base_species(true_species);
    belief.mons.iter().any(|mb| mb.species_id != 0 && data_bridge::base_species(mb.species_id) == base)
}

struct DupPair {
    side: usize,
    base: u16,
    slot_a: usize,
    sp_a: u16,
    slot_b: usize,
    sp_b: u16,
    kind: usize,
}

// revealed[side][slot]: Some(true) a revealed slot, Some(false) a drawn slot, None the true team
fn world_dup_pairs(state: &BattleState, revealed: &[[Option<bool>; 6]; 2]) -> Vec<DupPair> {
    let mut out = Vec::new();
    for side in 0..2 {
        let team = &state.sides[side].team;
        for a in 0..6 {
            if team[a].species_id == 0 {
                continue;
            }
            let base = data_bridge::base_species(team[a].species_id);
            for b in a + 1..6 {
                if team[b].species_id == 0 || data_bridge::base_species(team[b].species_id) != base {
                    continue;
                }
                let kind = match (revealed[side][a], revealed[side][b]) {
                    (None, _) | (_, None) => 3,
                    (Some(true), Some(true)) => 0,
                    (Some(false), Some(false)) => 2,
                    _ => 1,
                };
                out.push(DupPair { side, base, slot_a: a, sp_a: team[a].species_id, slot_b: b, sp_b: team[b].species_id, kind });
            }
        }
    }
    out
}

fn world_dups(args: &[String]) {
    let path = arg(args, "--snapshots", "../.decompose/frontier/results/0.1.snapshots.bin");
    let snaps = read_all(&path).unwrap_or_else(|e| panic!("read_all {path}: {e}"));
    println!("snapshots: path={path} count={}", snaps.len());
    let roots_file = arg(args, "--roots", "results/0.7.roots.tsv");
    let n: usize = parse(args, "--n", "2000");
    let seeds: Vec<u64> = list(args, "--seeds", "1,2,3");
    let worlds: usize = parse(args, "--worlds", "8");
    let team_seeds: u64 = parse(args, "--team-seeds", "2000");
    let out = arg(args, "--out", "results/F3.dups.tsv");
    let dump = arg(args, "--dump", "results/F3.worlds.tsv");
    let offenders_out = arg(args, "--offenders", "results/F3.offenders.tsv");
    for p in [&out, &dump, &offenders_out] {
        if let Some(dir) = std::path::Path::new(p).parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        }
    }
    let mut roots = load_roots(&roots_file, &snaps);
    let loaded = roots.len();
    roots.truncate(n);
    println!("roots: loaded={loaded} used={} file={roots_file} order=first_rows_in_file", roots.len());
    assert!(!roots.is_empty(), "no roots");

    let mut by_base: std::collections::BTreeMap<u16, Vec<u16>> = std::collections::BTreeMap::new();
    for sp in GEN9_SET_POOL {
        by_base.entry(data_bridge::base_species(sp.species_id)).or_default().push(sp.species_id);
    }
    let multi: Vec<(&u16, &Vec<u16>)> = by_base.iter().filter(|(_, v)| v.len() >= 2).collect();
    println!("pool: ids={} bases={} bases_with_ge2_ids={}", GEN9_SET_POOL.len(), by_base.len(), multi.len());
    for (b, ids) in &multi {
        println!("pool-multi base={b} ids={ids:?}");
    }

    let mut h = FNV_OFFSET;
    for s in 1..=team_seeds {
        for m in &gen_team(&mut Lcg::new(s)) {
            fnv1a(&mut h, m.species_id);
        }
    }
    println!("gen-team-digest seeds=1..={team_seeds} fnv1a64=0x{h:016x}");
    println!("world-dups: roots={} seeds={seeds:?} worlds={worlds} draw=Lcg::new(splitmix64(row_seed(seed, position))) rule=base_species_equal_either_side", roots.len());

    let mut tsv = String::from("determinizer\tseed\troots\tworlds\tdup_worlds\tdup_rate\tdup_worlds_opp\tdup_worlds_us\tdup_pairs\trev_rev\trev_unrev\tunrev_unrev\ttrue_true\n");
    let mut dump_s = String::from("determinizer\tseed\tposition\tgame\tturn\tside\tworld\tdup_side\tbase\tslot_a\tsp_a\tslot_b\tsp_b\tkind\n");
    let mut offenders_s = String::from("determinizer\tbase\tdup_pairs\n");
    let wall = Instant::now();
    for det in ["RandomBattle", "Declairvoyant"] {
        let mut pooled = DupTally::default();
        let mut digest = FNV_OFFSET;
        for &seed in &seeds {
            let mut t = DupTally::default();
            for r in &roots {
                let snap = &snaps[r.position];
                let side = snap.side as usize;
                let opp = 1 - side;
                let obs = Observation { state: &snap.state, teams: &snap.teams, our_side: side };
                let belief = &snap.beliefs[side];
                let mut rng = Lcg::new(splitmix64(row_seed(seed, r.position)));
                let ws = if det == "RandomBattle" {
                    RandomBattle.sample_worlds(&obs, belief, worlds, &mut rng)
                } else {
                    Declairvoyant { inner: RandomBattle, opp_view: snap.beliefs[opp] }.sample_worlds(&obs, belief, worlds, &mut rng)
                };
                let mut revealed = [[None; 6]; 2];
                let active = snap.state.sides[side].active_index as usize;
                for slot in 0..6 {
                    let sid = snap.state.sides[opp].team[slot].species_id;
                    revealed[opp][slot] = (sid != 0).then(|| revealed_by(belief, sid));
                    if det == "Declairvoyant" {
                        let sid = snap.state.sides[side].team[slot].species_id;
                        revealed[side][slot] = (sid != 0).then(|| slot == active || revealed_by(&snap.beliefs[opp], sid));
                    }
                }
                t.roots += 1;
                for (k, w) in ws.iter().enumerate() {
                    t.worlds += 1;
                    for s in 0..2 {
                        for slot in 0..6 {
                            fnv1a(&mut digest, w.state.sides[s].team[slot].species_id);
                        }
                    }
                    let pairs = world_dup_pairs(&w.state, &revealed);
                    if pairs.is_empty() {
                        continue;
                    }
                    t.dup_worlds += 1;
                    t.dup_worlds_opp += pairs.iter().any(|p| p.side == opp) as usize;
                    t.dup_worlds_us += pairs.iter().any(|p| p.side == side) as usize;
                    for p in &pairs {
                        t.dup_pairs += 1;
                        t.kinds[p.kind] += 1;
                        *t.offenders.entry(p.base).or_default() += 1;
                        dump_s.push_str(&format!(
                            "{det}\t{seed}\t{}\t{}\t{}\t{}\t{k}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                            r.position, r.game, r.turn, r.side, p.side, p.base, p.slot_a, p.sp_a, p.slot_b, p.sp_b, DUP_KINDS[p.kind]
                        ));
                    }
                }
            }
            let line = t.row(det, &seed.to_string());
            print!("{line}");
            tsv.push_str(&line);
            pooled.merge(&t);
        }
        let line = pooled.row(det, "pooled");
        print!("{line}");
        tsv.push_str(&line);
        let mut off: Vec<(u16, usize)> = pooled.offenders.iter().map(|(b, c)| (*b, *c)).collect();
        off.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (b, c) in &off {
            offenders_s.push_str(&format!("{det}\t{b}\t{c}\n"));
        }
        println!("world-digest det={det} fnv1a64=0x{digest:016x} top_offenders_base_pairs={:?}", off.iter().take(10).collect::<Vec<_>>());
    }
    println!("world-dups-done wall_s={:.1}", wall.elapsed().as_secs_f64());
    std::fs::write(&out, tsv).unwrap_or_else(|e| panic!("write {out}: {e}"));
    std::fs::write(&dump, dump_s).unwrap_or_else(|e| panic!("write {dump}: {e}"));
    std::fs::write(&offenders_out, offenders_s).unwrap_or_else(|e| panic!("write {offenders_out}: {e}"));
    println!("world-dups-out {out} dump={dump} offenders={offenders_out}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--count") {
        count(&args);
    } else if args.iter().any(|a| a == "--flip-fields") {
        flip_fields(&args);
    } else if args.iter().any(|a| a == "--cost") {
        cost(&args);
    } else if args.iter().any(|a| a == "--signal") {
        signal(&args);
    } else if args.iter().any(|a| a == "--declairvoyance") {
        declairvoyance(&args);
    } else if args.iter().any(|a| a == "--support-sharing") {
        support_sharing(&args);
    } else if args.iter().any(|a| a == "--worlds-flip") {
        worlds_flip(&args);
    } else if args.iter().any(|a| a == "--prior-nll") {
        prior_nll(&args);
    } else if args.iter().any(|a| a == "--leaf-dispersion") {
        leaf_dispersion(&args);
    } else if args.iter().any(|a| a == "--world-dups") {
        world_dups(&args);
    } else {
        eprintln!("usage: frontier_probe (--count | --flip-fields | --cost | --signal | --declairvoyance | --support-sharing | --worlds-flip | --prior-nll | --leaf-dispersion | --world-dups) [--rows A,B] [--limit N] [--budgets 1024,32768] [--seeds 1,2,3] [--threads T] [--out ROWS.tsv] [--summary FLIPS.tsv]");
        eprintln!("  --cost [--turn-lo 5] [--turn-hi 25] [--n 2000] [--repeats 3] [--seed 1] [--out results/0.3.cost.tsv]");
        eprintln!("  --signal [--k 8,32] [--seed 1] [--threads T] [--out results/0.3.values.tsv]  (needs BRIDGE_EVAL_WEIGHTS_V2)");
        eprintln!("  --declairvoyance [--snapshots PATH] [--roots FILE] [--roots-out FILE] [--n 3000] [--turn-lo 2] [--turn-hi 20] [--sample-seed 11] [--seeds 1,2,3] [--worlds 8] [--iters 35199] [--time-ms 600000] [--threads T] [--out results/0.7.diff.tsv] [--dump results/0.7.worlds.tsv] [--offroot results/0.7.offroot.tsv]");
        eprintln!("  --support-sharing [--snapshots PATH] [--roots results/0.7.roots.tsv] [--seeds 1,2,3] [--worlds 8] [--iters 35199] [--time-ms 600000] [--threads T] [--out results/0.11.sharing.tsv] [--dump results/0.11.paths.tsv]");
        eprintln!("  --worlds-flip [--snapshots PATH] [--roots results/0.7.roots.tsv] [--seeds 1,2,3] [--arm-a 8x8192] [--arm-b 64x1024] [--arm-c 64x8192] [--floor 8x8192] [--time-ms 600000] [--threads T] [--out results/0.9.worlds.tsv] [--dump PICKS.tsv]");
        eprintln!("  --prior-nll [--snapshots PATH] [--roots results/0.7.roots.tsv] [--out results/0.9.nll.tsv] [--dump ROWS.tsv]");
        eprintln!("  --leaf-dispersion [--rows A,B] [--limit N] [--seed 1] [--iters 16384] [--threads T] [--out results/0.14.dispersion.tsv] [--dump results/0.14.leaves.tsv]  (needs train_value and BRIDGE_EVAL_WEIGHTS_V2)");
        eprintln!("  --world-dups [--snapshots PATH] [--roots results/0.7.roots.tsv] [--n 2000] [--seeds 1,2,3] [--worlds 8] [--team-seeds 2000] [--out results/F3.dups.tsv] [--dump results/F3.worlds.tsv] [--offenders results/F3.offenders.tsv]");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkmn_engine::state::PHASE_SWITCH_P2;
    use poke_mcts::belief::Belief;
    use poke_mcts::frontier::{gen_team, initial_state};

    fn arm(action: u8, visits: u32) -> ArmStat {
        ArmStat { action, visits, ..Default::default() }
    }

    #[test]
    fn top_arm_prefers_the_lowest_byte_on_ties() {
        assert_eq!(top_arm(&[arm(4, 10), arm(0, 10), arm(1, 9)]), Some(0));
        assert_eq!(top_arm(&[]), None);
    }

    #[test]
    fn l1_is_zero_on_identical_and_two_on_disjoint_supports() {
        let a = [arm(0, 3), arm(1, 1)];
        assert_eq!(l1(&a, &a), 0.0);
        assert_eq!(l1(&a, &[arm(4, 2), arm(10, 2)]), 2.0);
        assert_eq!(l1(&a, &[arm(0, 1), arm(1, 3)]), 1.0);
        assert_eq!(l1(&a, &[]), 1.0);
    }

    #[test]
    fn flip_kind_flags() {
        assert!(!is_tera(Some(0)) && !is_switch(Some(0)));
        assert!(!is_tera(Some(4)) && is_switch(Some(4)));
        assert!(!is_tera(Some(9)) && is_switch(Some(9)));
        assert!(is_tera(Some(10)) && !is_switch(Some(10)));
        assert!(is_tera(Some(13)) && !is_switch(Some(13)));
        assert!(!is_tera(None) && !is_switch(None));
    }

    #[test]
    fn off_root_flips_compare_names_and_skip_truncated_plies() {
        let full = [(0u8, 1u8), (2, 3), (4, 5)];
        let short = [(0u8, 1u8), (2, 3), (NO_ARM, NO_ARM)];
        let one_sided = [(0u8, 1u8), (2, NO_ARM), (4, 5)];
        let nx = [Named::Move(10, false), Named::Move(11, false), Named::Switch(7)];
        let ny = [Named::Move(10, false), Named::Move(11, true), Named::Switch(7)];
        let nz = [Named::Move(10, false), Named::Absent, Named::Switch(7)];
        assert_eq!(off_root_flip(&full, &nx, &full, &nx, 1), Some(false));
        assert_eq!(off_root_flip(&full, &nx, &full, &ny, 1), Some(true));
        assert_eq!(off_root_flip(&full, &nx, &short, &nx, 2), None);
        assert_eq!(off_root_flip(&short, &nx, &full, &nx, 2), None);
        assert_eq!(off_root_flip(&full, &nx, &one_sided, &nz, 1), Some(true));
        let cell = |principal, opp_named| WorldCell { opp_species: 0, top: [None; 3], iters: [0; 3], principal, opp_named };
        let run = Run {
            pick: [0, 0, 1],
            worlds: vec![cell([full, full, full], [nx, ny, nx]), cell([full, short, one_sided], [nx, nx, nz])],
            ..Default::default()
        };
        let ab = off_root(&run, 0, 1);
        assert_eq!((ab.ply[1].n, ab.ply[1].flips, ab.ply[1].tera, ab.ply[1].ts, ab.ply[1].trunc), (2, 1, 1, 1, 0));
        assert_eq!((ab.ply[2].n, ab.ply[2].flips, ab.ply[2].trunc), (1, 0, 1));
        assert_eq!((ab.ply[1].same1_n, ab.ply[1].same1_flips), (2, 1));
        assert!(ab.pick_same && ab.offroot_flip && ab.trunc_worlds == 1);
        let af = off_root(&run, 0, 2);
        assert_eq!((af.ply[1].n, af.ply[1].flips, af.ply[1].absent), (2, 1, 1));
        assert!(!af.pick_same && af.offroot_flip && af.trunc_worlds == 0);
        let rows = offroot_rows("1", &[vec![&run]], 2);
        assert_eq!(rows.lines().count(), 6);
        let ab_p2: Vec<&str> = rows.lines().nth(1).unwrap().split('\t').collect();
        assert_eq!(&ab_p2[..9], &["1", "ab", "2", "1", "1", "2", "2", "1", "0.500000"]);
        assert_eq!(ab_p2[22], "0.0000");
    }

    #[test]
    fn top3_is_the_three_busiest_arms_lowest_byte_on_ties() {
        assert_eq!(top3(&[arm(4, 10), arm(0, 10), arm(1, 9), arm(7, 11)]), vec![7, 0, 4]);
        assert_eq!(top3(&[arm(2, 1)]), vec![2]);
        assert!(top3(&[]).is_empty());
    }

    #[test]
    fn jaccard_is_intersection_over_union() {
        assert_eq!(jaccard(&[0u8, 1, 2], &[0, 1, 2]), 1.0);
        assert_eq!(jaccard(&[0u8, 1, 2], &[3, 4, 5]), 0.0);
        assert_eq!(jaccard(&[0u8, 1, 2], &[1, 2, 3]), 0.5);
        assert_eq!(jaccard::<u8>(&[], &[]), 1.0);
        assert_eq!(mean_pairwise_jaccard(&[vec![0u8, 1], vec![0, 1], vec![2, 3]]), 1.0 / 3.0);
    }

    #[test]
    fn prefix_groups_counts_the_largest_agreeing_set() {
        let p = vec![vec![1u8, 2, 3], vec![1, 2, 9], vec![1, 7, 7], vec![1, 2, 3]];
        assert_eq!(prefix_groups(&p, 1), (1, 4));
        assert_eq!(prefix_groups(&p, 2), (2, 3));
        assert_eq!(prefix_groups(&p, 3), (3, 2));
        assert_eq!(prefix_groups(&[vec![0u8], vec![1], vec![2]], 1), (3, 1));
    }

    #[test]
    fn action_bytes_decode_to_what_they_name() {
        let mut rng = Lcg::new(4);
        let (state, _teams) = initial_state(&gen_team(&mut rng), &gen_team(&mut rng));
        let side = 0usize;
        let active = state.sides[side].active_index as usize;
        let moves = effective_moves(&state, side);
        assert_eq!(named(&state, side, active, 0), Named::Move(moves[0], false));
        assert_eq!(named(&state, side, active, ACTION_TERA_0 + 2), Named::Move(moves[2], true));
        assert_ne!(named(&state, side, active, 0), named(&state, side, active, ACTION_TERA_0));
        let bench = (0..6).find(|&j| j != active).unwrap();
        assert_eq!(
            named(&state, side, active, ACTION_SWITCH_0 + bench as u8),
            Named::Switch(state.sides[side].team[bench].species_id)
        );
        assert_eq!(named(&state, side, active, NO_ARM), Named::Absent);
        // a switch byte moves the slot the later plies read their moves from
        let path = [(0u8, ACTION_SWITCH_0 + bench as u8), (0, 1), (0, NO_ARM)];
        let got = opp_meaning_path(&state, side, &path);
        assert_eq!(got[0], Named::Switch(state.sides[side].team[bench].species_id));
        assert_eq!(got[1], Named::Move(state.sides[side].team[bench].moves[1], false));
        assert_eq!(got[2], Named::Absent);
    }

    fn snapshot(seed: u64, game: u64, turn: u16, side: u8) -> NativeSnapshot {
        let mut rng = Lcg::new(seed);
        let a = gen_team(&mut rng);
        let b = gen_team(&mut rng);
        let (state, teams) = initial_state(&a, &b);
        let mut beliefs = [Belief::default(); 2];
        for s in 0..2 {
            let om = state.active_mon(1 - s);
            beliefs[s].note_species(om.species_id, om.level);
        }
        NativeSnapshot { game, turn, side, state, teams, beliefs, pick: 0, seed, builds: None }
    }

    fn snapshots() -> Vec<NativeSnapshot> {
        let turns = [1u16, 2, 5, 20, 21, 7, 7];
        let mut snaps: Vec<NativeSnapshot> = turns.iter().enumerate().map(|(i, &t)| snapshot(100 + i as u64, i as u64 / 2, t, (i % 2) as u8)).collect();
        snaps[2].state.phase = PHASE_SWITCH_P2;
        snaps
    }

    fn positions(roots: &[RootRef]) -> Vec<usize> {
        roots.iter().map(|r| r.position).collect()
    }

    #[test]
    fn flip_split_puts_every_pair_in_exactly_one_structural_class() {
        for a in [0u8, 3, ACTION_TERA_0, ACTION_TERA_3, ACTION_SWITCH_0, ACTION_SWITCH_5] {
            for b in [0u8, 3, ACTION_TERA_0, ACTION_TERA_3, ACTION_SWITCH_0, ACTION_SWITCH_5] {
                let k = flip_split(a, b);
                assert_eq!(k[..3].iter().filter(|&&x| x).count(), 1, "{a} vs {b}");
            }
        }
        assert_eq!(flip_split(0, 1), [true, false, false, false]);
        assert_eq!(flip_split(0, ACTION_SWITCH_0), [false, true, false, false]);
        assert_eq!(flip_split(ACTION_SWITCH_0, ACTION_SWITCH_0 + 1), [false, false, true, false]);
        assert_eq!(flip_split(0, ACTION_TERA_0), [true, false, false, true]);
        assert_eq!(flip_split(ACTION_TERA_0, ACTION_TERA_0 + 1), [true, false, false, false]);
        assert_eq!(flip_split(ACTION_TERA_0, ACTION_SWITCH_0), [false, true, false, true]);
    }

    #[test]
    fn a_large_world_draw_starts_with_the_small_draw_at_the_same_seed() {
        let snap = snapshot(41, 0, 6, 0);
        assert!(world_prefix_shared(&snap, 7, 8, 64));
        assert!(world_prefix_shared(&snap, 7, 8, 8));
        let side = snap.side as usize;
        let obs = Observation { state: &snap.state, teams: &snap.teams, our_side: side };
        let a = RandomBattle.sample_worlds(&obs, &snap.beliefs[side], 8, &mut Lcg::new(splitmix64(7)));
        let b = RandomBattle.sample_worlds(&obs, &snap.beliefs[side], 8, &mut Lcg::new(splitmix64(8)));
        assert!(a.iter().zip(&b).any(|(x, y)| x.state != y.state), "a different seed must move the worlds");
        assert_eq!(a[0].weight, 1.0 / 8.0);
    }

    #[test]
    fn the_true_built_mon_matches_a_pool_entry() {
        let snap = snapshot(23, 0, 4, 0);
        for side in 0..2 {
            for slot in 0..6 {
                let mon = &snap.state.sides[side].team[slot];
                let ss = species_sets_with_base_fallback(mon.species_id).expect("pooled species");
                let bd = &snap.teams.mons[side][slot];
                assert!(ss.sets.iter().any(|e| set_is_true_mon(e, mon, bd, false)), "side {side} slot {slot} species {}", mon.species_id);
            }
        }
    }

    #[test]
    fn nll_of_the_true_set_sits_under_a_species_only_belief() {
        let snap = snapshot(23, 0, 4, 0);
        let row = nll_root(&snap, 0);
        assert!(row.scored, "excluded: {} {} {}", row.excl_no_slot, row.excl_no_match_anywhere, row.excl_match_pool_only);
        let mon = snap.state.active_mon(1);
        let bd = &snap.teams.mons[1][snap.state.sides[1].active_index as usize];
        let ss = species_sets_with_base_fallback(mon.species_id).unwrap();
        assert_eq!(row.support, ss.sets.len(), "a species-only belief prunes nothing");
        assert_eq!(row.pool, ss.sets.len());
        assert!(row.matches >= 1);
        let total: f64 = ss.sets.iter().map(|e| e.count as f64).sum();
        let mass: f64 = ss.sets.iter().filter(|e| set_is_true_mon(e, mon, bd, false)).map(|e| e.count as f64).sum();
        assert!((row.nll - -(mass / total).ln()).abs() < 1e-12);
        assert!(row.h >= 0.0 && row.nll >= 0.0);
        assert!(row.h <= (ss.sets.len() as f64).ln() + 1e-12);
    }

    #[test]
    fn mean_se_on_indicators_is_the_binomial_standard_error() {
        let v: Vec<f64> = (0..100).map(|i| (i < 25) as u8 as f64).collect();
        let (m, se) = mean_se(&v);
        assert!((m - 0.25).abs() < 1e-12);
        let binom = (0.25f64 * 0.75 / 100.0).sqrt();
        assert!((se / binom - 1.0).abs() < 0.01, "{se} vs {binom}");
        assert_eq!(mean_se(&[1.0]), (1.0, 0.0));
    }

    #[test]
    fn sample_roots_is_deterministic_and_applies_both_rules() {
        let snaps = snapshots();
        assert_eq!(legal_actions(&snaps[2].state, 0).count, 0);
        let full = sample_roots(&snaps, 10, 2, 20, 11, &[1], 2);
        assert_eq!(full.sampled, 5);
        assert_eq!(full.discarded_legal, 1);
        assert_eq!(full.discarded_opp_slots, 0);
        assert_eq!(positions(&full.roots), vec![1, 3, 5, 6]);
        for r in &full.roots {
            let s = &snaps[r.position];
            assert_eq!((r.game, r.turn, r.side), (s.game, s.turn, s.side));
        }
        assert_eq!(positions(&sample_roots(&snaps, 10, 2, 20, 11, &[1], 2).roots), positions(&full.roots));
        let two = sample_roots(&snaps, 2, 2, 20, 11, &[1], 2);
        assert_eq!(two.sampled, 2);
        assert!(positions(&two.roots).windows(2).all(|w| w[0] < w[1]));
        assert!(two.roots.iter().all(|r| (2..=20).contains(&snaps[r.position].turn)));
        assert_eq!(positions(&sample_roots(&snaps, 2, 2, 20, 11, &[1], 2).roots), positions(&two.roots));
        assert_ne!(positions(&sample_roots(&snaps, 2, 2, 20, 12, &[1], 2).roots), positions(&two.roots));
    }

    #[test]
    fn roots_round_trip() {
        let snaps = snapshots();
        let roots = sample_roots(&snaps, 10, 2, 20, 11, &[1], 2).roots;
        let path = std::env::temp_dir().join(format!("frontier_probe_roots_{}.tsv", std::process::id()));
        let path = path.to_string_lossy().into_owned();
        write_roots(&path, &roots);
        let back = load_roots(&path, &snaps);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(back.len(), roots.len());
        for (x, y) in roots.iter().zip(&back) {
            assert_eq!((x.position, x.game, x.turn, x.side), (y.position, y.game, y.turn, y.side));
        }
    }

    #[test]
    #[should_panic(expected = "not game=")]
    fn load_roots_panics_on_a_mismatched_snapshot_file() {
        let snaps = snapshots();
        let path = std::env::temp_dir().join(format!("frontier_probe_roots_bad_{}.tsv", std::process::id()));
        let path = path.to_string_lossy().into_owned();
        std::fs::write(&path, format!("position\tgame\tturn\tside\n1\t{}\t{}\t{}\n", snaps[1].game + 1, snaps[1].turn, snaps[1].side)).unwrap();
        let r = std::panic::catch_unwind(|| load_roots(&path, &snaps));
        std::fs::remove_file(&path).unwrap();
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn opp_slots_identical_holds_on_a_fixture() {
        for side in 0..2u8 {
            let snap = snapshot(7, 0, 3, side);
            for s in 1..=3 {
                assert!(opp_slots_identical(&snap, row_seed(s, 0), 8), "side {side} seed {s}");
            }
        }
    }
}
