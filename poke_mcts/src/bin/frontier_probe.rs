use pkmn_engine::state::{legal_actions, move_base_pp, BattleState, MonSlot, TeamData, STATUS_BAD_POISON, STATUS_SLEEP, VOL_TRANSFORMED};
use poke_mcts::chance::OpenLoop;
use poke_mcts::driver::{aggregate, pick_from, PickMode};
use poke_mcts::eval::Handcrafted;
use poke_mcts::frontier::{read_rows, Row};
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::search::{search_world, SearchParams};
use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--count") {
        count(&args);
    } else if args.iter().any(|a| a == "--flip-fields") {
        flip_fields(&args);
    } else {
        eprintln!("usage: frontier_probe (--count | --flip-fields) [--rows A,B] [--limit N] [--budgets 1024,32768] [--seeds 1,2,3] [--threads T] [--out ROWS.tsv] [--summary FLIPS.tsv]");
        std::process::exit(2);
    }
}
