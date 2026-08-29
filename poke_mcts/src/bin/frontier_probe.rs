use pkmn_engine::state::{legal_actions, move_base_pp, BattleState, MonSlot, TeamData, ACTION_SWITCH_0, ACTION_SWITCH_5, ACTION_TERA_0, ACTION_TERA_3, STATUS_BAD_POISON, STATUS_SLEEP, VOL_TRANSFORMED};
use poke_mcts::chance::OpenLoop;
use poke_mcts::determinize::{Determinizer, Observation, RandomBattle};
use poke_mcts::driver::{aggregate, choose_action_traced_salted, pick_from, PickMode, PimcConfig, WorldTrace};
use poke_mcts::eval::{Evaluator, Handcrafted};
use poke_mcts::eval_learned::LearnedValueV2;
use poke_mcts::frontier::{read_all, read_rows, Declairvoyant, NativeSnapshot, PlayoutEval, PlayoutPolicy, Row, PLAYOUT_STEP_CAP};
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::search::{search_world, ArmStat, ChanceMode, SearchParams};
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
    let limit: usize = parse(args, "--limit", &rows.len().to_string());
    rows.truncate(limit);
    println!("rows: paths={paths:?} rows={}", rows.len());
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
                    let seed = row_seed(s, i);
                    let (base, it0) = root_pick(&row.state, &row.teams, side, p, seed);
                    let (repeat, it1) = root_pick(&row.state, &row.teams, side, p, seed);
                    let (reseed, it2) = root_pick(&row.state, &row.teams, side, p, row_seed(s ^ RESEED_SALT, i));
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
    s.push_str("\tmin_iters\n");
    for (i, (row, cs)) in rows.iter().zip(&cells).enumerate() {
        for c in cs {
            s.push_str(&format!("{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                row.game_index, row.game_tag, row.turn, row.side, c.budget, c.seed, c.base, c.repeat, c.reseed,
                c.perturbed.iter().map(|&p| fmt(p)).collect::<Vec<_>>().join("\t"), c.min_iters));
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
    let mut run = Run { seed, pick: [a.picked, b.picked, f.picked], pick_ab: a.picked != b.picked, pick_af: a.picked != f.picked, min_iters: u64::MAX, ..Default::default() };
    for k in 0..worlds {
        let (wa, wb, wf) = (&a.per_world[k], &b.per_world[k], &f.per_world[k]);
        assert!(opp_key(wa) == opp_key(wb) && opp_key(wa) == opp_key(wf), "root {position} world {k}: opponent slot differs between runs");
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
        run.worlds.push(WorldCell { opp_species: wa.opp_species, top, iters });
    }
    run
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
    for p in [&out, &dump] {
        if let Some(dir) = std::path::Path::new(p).parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        }
    }
    println!(
        "declairvoyance: roots={} seeds={seeds:?} worlds={worlds} iters={iters} time_ms={time_ms} evaluator=Handcrafted determinizer_a=RandomBattle determinizer_b=Declairvoyant(RandomBattle) chance=OpenLoop pick=Argmax filter={PICK_FILTER} raw_root=false explore_coeff={EXPLORE_COEFF} value_temp=1.0 search_salt=0x{SEARCH_SALT:x} top_arm=max_visits_lowest_byte_on_ties threads={threads}",
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
    let mut s = String::from("position\tgame\tturn\tside\tseed\tworld\topp_species\ttop_a\ttop_b\ttop_f\tflip_ab\tflip_af\tl1_ab\tl1_af\titers_a\titers_b\titers_f\tpick_a\tpick_b\tpick_f\n");
    for (root, rs) in roots.iter().zip(&runs) {
        for r in rs {
            for (k, w) in r.worlds.iter().enumerate() {
                s.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{k}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    root.position, root.game, root.turn, root.side, r.seed, w.opp_species, fmt(w.top[0]), fmt(w.top[1]), fmt(w.top[2]),
                    (w.top[0] != w.top[1]) as u8, (w.top[0] != w.top[2]) as u8, r.l1_ab[k], r.l1_af[k], w.iters[0], w.iters[1], w.iters[2], r.pick[0], r.pick[1], r.pick[2]
                ));
            }
        }
    }
    std::fs::write(&dump, s).unwrap_or_else(|e| panic!("write {dump}: {e}"));

    let mut t = String::from("seed\troots\tworlds\tab_flips\tab_rate\taf_flips\taf_rate\tdiff_pp\tdiff_se_pp\tab_flips_tera\tab_flips_switch\tab_flips_tera_or_switch\ttera_share\tswitch_share\ttera_or_switch_share\taf_flips_tera_or_switch\taf_tera_or_switch_share\tl1_ab_mean\tl1_ab_median\tl1_af_mean\tl1_af_median\tpick_ab_flips\tpick_ab_rate\tpick_af_flips\tpick_af_rate\tshort_searches\topp_arms_lt2\n");
    for (i, seed) in seeds.iter().enumerate() {
        let per_root: Vec<Vec<&Run>> = runs.iter().map(|rs| vec![&rs[i]]).collect();
        t.push_str(&summary_row(&seed.to_string(), &per_root, worlds));
    }
    let per_root: Vec<Vec<&Run>> = runs.iter().map(|rs| rs.iter().collect()).collect();
    t.push_str(&summary_row("pooled", &per_root, worlds));
    print!("{t}");
    std::fs::write(&out, t).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("declairvoyance-out summary={out} dump={dump} roots={roots_file}");
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
    } else {
        eprintln!("usage: frontier_probe (--count | --flip-fields | --cost | --signal | --declairvoyance) [--rows A,B] [--limit N] [--budgets 1024,32768] [--seeds 1,2,3] [--threads T] [--out ROWS.tsv] [--summary FLIPS.tsv]");
        eprintln!("  --cost [--turn-lo 5] [--turn-hi 25] [--n 2000] [--repeats 3] [--seed 1] [--out results/0.3.cost.tsv]");
        eprintln!("  --signal [--k 8,32] [--seed 1] [--threads T] [--out results/0.3.values.tsv]  (needs BRIDGE_EVAL_WEIGHTS_V2)");
        eprintln!("  --declairvoyance [--snapshots PATH] [--roots FILE] [--roots-out FILE] [--n 3000] [--turn-lo 2] [--turn-hi 20] [--sample-seed 11] [--seeds 1,2,3] [--worlds 8] [--iters 35199] [--time-ms 600000] [--threads T] [--out results/0.7.diff.tsv] [--dump results/0.7.worlds.tsv]");
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
        NativeSnapshot { game, turn, side, state, teams, beliefs, pick: 0, seed }
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
