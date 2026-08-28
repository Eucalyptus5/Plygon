use pkmn_engine::state::{
    effective_moves, legal_actions, BattleState, MonSlot, TeamData, ACTION_MOVE_0, ACTION_MOVE_3, ACTION_STRUGGLE,
    ACTION_SWITCH_0, ACTION_SWITCH_5, ACTION_TERA_0, ACTION_TERA_3,
};
use poke_mcts::belief::Belief;
use poke_mcts::determinize::{Observation, RandomBattle};
use poke_mcts::driver::{choose_action_eval_iters, EvalKind, PickMode, PimcConfig};
use poke_mcts::features::{self, NUM_SEGMENTS};
use poke_mcts::frontier::{self, bincode_err, Kind, NativeSnapshot};
use poke_mcts::policies::random_action;
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::search::ChanceMode;
use poke_mcts::selfplay::{play_to_terminal_timed, LoopTimes};
use poke_mcts::train_dump::{read_records, TrainRecord};
use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TEAM_SALT: u64 = 0x7EA3_5EED;

fn decision_tokens(state: &BattleState, side: usize, belief: &Belief, buf: &mut Vec<u32>) -> [u16; NUM_SEGMENTS] {
    let skel = frontier::mask_to_skeleton(state, side, belief);
    buf.clear();
    features::extract_segmented(&skel, buf)
}

fn bench(games: u64, seed: u64, extract: bool, timers: bool) {
    let mut engine = Duration::ZERO;
    let mut belief = Duration::ZERO;
    let mut mask_extract = Duration::ZERO;
    let mut setup = Duration::ZERO;
    let mut steps = 0u64;
    let mut turns = 0u64;
    let mut decisions = 0u64;
    let mut tally = [0u64; 3];
    let mut buf: Vec<u32> = Vec::new();
    let wall = Instant::now();
    for g in 0..games {
        let game_seed = splitmix64(seed ^ g.wrapping_mul(0x9E3779B97F4A7C15));
        let mut team_rng = Lcg::new(game_seed ^ TEAM_SALT);
        let t = Instant::now();
        let a = frontier::gen_team(&mut team_rng);
        let b = frontier::gen_team(&mut team_rng);
        let (state, teams) = frontier::initial_state(&a, &b);
        setup += t.elapsed();
        let mut lt = LoopTimes::default();
        let choose = |side: usize, st: &BattleState, _tm: &TeamData, bel: &[Belief; 2], _seed: u64, rng: &mut Lcg| {
            decisions += 1;
            if extract {
                let t = timers.then(Instant::now);
                let lens = decision_tokens(st, side, &bel[side], &mut buf);
                std::hint::black_box(&buf);
                std::hint::black_box(lens);
                if let Some(t) = t {
                    mask_extract += t.elapsed();
                }
            }
            random_action(st, side, rng)
        };
        let times = if timers { Some(&mut lt) } else { None };
        let v = play_to_terminal_timed(state, &teams, [Belief::default(); 2], game_seed, choose, times);
        engine += lt.engine;
        belief += lt.belief;
        steps += lt.steps as u64;
        turns += lt.turns as u64;
        tally[if v == 1.0 { 0 } else if v == 0.0 { 2 } else { 1 }] += 1;
    }
    let wall = wall.elapsed();
    let n = games as f64;
    let us = |d: Duration| d.as_secs_f64() * 1e6 / n;
    let total = us(wall);
    let other = total - us(engine) - us(belief) - us(mask_extract);
    println!(
        "seed\tgames\textract\ttimers\tus_per_game_total\tus_engine\tus_belief\tus_mask_extract\tus_other\tus_setup\tmean_turns\tmean_steps\tmean_decisions\tgames_per_s_core"
    );
    println!(
        "{}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.1}",
        seed,
        games,
        if extract { "yes" } else { "no" },
        if timers { "yes" } else { "no" },
        total,
        us(engine),
        us(belief),
        us(mask_extract),
        other,
        us(setup),
        turns as f64 / n,
        steps as f64 / n,
        decisions as f64 / n,
        n / wall.as_secs_f64()
    );
    eprintln!(
        "bench seed={} games={} extract={} timers={} total={:.2} us/game engine={:.2} belief={:.2} mask+extract={:.2} other={:.2} (setup {:.2}) turns={:.2} steps={:.2} decisions={:.2} wins/draws/losses={}/{}/{}",
        seed,
        games,
        if extract { "yes" } else { "no" },
        if timers { "yes" } else { "no" },
        total,
        us(engine),
        us(belief),
        us(mask_extract),
        other,
        us(setup),
        turns as f64 / n,
        steps as f64 / n,
        decisions as f64 / n,
        tally[0],
        tally[1],
        tally[2]
    );
}

const GEN_NUM_WORLDS: usize = 4;
const GEN_ITERS_PER_WORLD: u64 = 2048;
const GEN_TIME_MS_PER_WORLD: u64 = 60_000;
const GEN_FILTER_THRESHOLD: f64 = 0.75;
const GEN_EXPLORE_COEFF: f64 = 0.49;
const GEN_VALUE_TEMP: f32 = 1.0;
const GEN_CHUNK: u64 = 256;
const SNAPSHOT_TURNS: std::ops::RangeInclusive<u16> = 2..=20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    Search,
    Random,
}

struct GenOpts {
    games: u64,
    seed: u64,
    out: PathBuf,
    snapshots: Option<PathBuf>,
    opp_random: bool,
    threads: usize,
    policy: Policy,
}

#[derive(Serialize)]
struct GenConfig {
    games: u64,
    seed: u64,
    worlds: usize,
    iters_per_world: u64,
    time_ms_per_world: u64,
    evaluator: &'static str,
    determinizer: &'static str,
    chance: &'static str,
    pick: &'static str,
    filter: f64,
    explore_coeff: f64,
    value_temp: f32,
    opp_random: bool,
    threads: usize,
    out: String,
    snapshots: Option<String>,
}

impl GenConfig {
    fn new(o: &GenOpts) -> Self {
        GenConfig {
            games: o.games,
            seed: o.seed,
            worlds: GEN_NUM_WORLDS,
            iters_per_world: GEN_ITERS_PER_WORLD,
            time_ms_per_world: GEN_TIME_MS_PER_WORLD,
            evaluator: "Handcrafted",
            determinizer: "RandomBattle",
            chance: "OpenLoop",
            pick: "Argmax",
            filter: GEN_FILTER_THRESHOLD,
            explore_coeff: GEN_EXPLORE_COEFF,
            value_temp: GEN_VALUE_TEMP,
            opp_random: o.opp_random,
            threads: o.threads,
            out: o.out.display().to_string(),
            snapshots: o.snapshots.as_ref().map(|p| p.display().to_string()),
        }
    }

    fn line(&self) -> String {
        format!(
            "gen: games={} seed={} worlds={} iters_per_world={} time_ms_per_world={} evaluator={} determinizer={} chance={} pick={} filter={} explore_coeff={} value_temp={:.1} opp_random={} threads={} out={} snapshots={}",
            self.games,
            self.seed,
            self.worlds,
            self.iters_per_world,
            self.time_ms_per_world,
            self.evaluator,
            self.determinizer,
            self.chance,
            self.pick,
            self.filter,
            self.explore_coeff,
            self.value_temp,
            if self.opp_random { "yes" } else { "no" },
            self.threads,
            self.out,
            self.snapshots.as_deref().unwrap_or("none")
        )
    }
}

#[derive(Default, Clone, Copy)]
struct Counters {
    decisions_seen: u64,
    records_written: u64,
    skipped_no_legal: u64,
    skipped_struggle: u64,
    records_by_decider: [u64; 2],
    labels_in_l: u64,
    l_move_part_sum: u64,
    l_size_sum: u64,
}

impl Counters {
    fn add(&mut self, o: &Counters) {
        self.decisions_seen += o.decisions_seen;
        self.records_written += o.records_written;
        self.skipped_no_legal += o.skipped_no_legal;
        self.skipped_struggle += o.skipped_struggle;
        self.records_by_decider[0] += o.records_by_decider[0];
        self.records_by_decider[1] += o.records_by_decider[1];
        self.labels_in_l += o.labels_in_l;
        self.l_move_part_sum += o.l_move_part_sum;
        self.l_size_sum += o.l_size_sum;
    }
}

#[derive(Serialize)]
struct Summary {
    games: u64,
    decisions_seen: u64,
    records_written: u64,
    skipped_no_legal: u64,
    skipped_struggle: u64,
    skipped: u64,
    records_by_decider: [u64; 2],
    snapshots: u64,
    snapshot_turn_min: u16,
    snapshot_turn_max: u16,
    #[serde(rename = "mean_L_move_part")]
    mean_l_move_part: f64,
    #[serde(rename = "mean_L_size")]
    mean_l_size: f64,
    #[serde(rename = "labels_in_L")]
    labels_in_l: u64,
    wins: u64,
    draws: u64,
    losses: u64,
    mean_turns: f64,
    wall_seconds: f64,
    first_search_iters: Option<u64>,
    config: GenConfig,
}

impl Summary {
    fn line(&self) -> String {
        format!(
            "gen summary: games={} decisions_seen={} records_written={} skipped_no_legal={} skipped_struggle={} skipped={} records_by_decider=[{},{}] snapshots={} snapshot_turn_min={} snapshot_turn_max={} mean_L_move_part={:.3} mean_L_size={:.3} labels_in_L={} ({:.4}) wins/draws/losses={}/{}/{} mean_turns={:.2} wall_seconds={:.1}",
            self.games,
            self.decisions_seen,
            self.records_written,
            self.skipped_no_legal,
            self.skipped_struggle,
            self.skipped,
            self.records_by_decider[0],
            self.records_by_decider[1],
            self.snapshots,
            self.snapshot_turn_min,
            self.snapshot_turn_max,
            self.mean_l_move_part,
            self.mean_l_size,
            self.labels_in_l,
            if self.records_written == 0 { 0.0 } else { self.labels_in_l as f64 / self.records_written as f64 },
            self.wins,
            self.draws,
            self.losses,
            self.mean_turns,
            self.wall_seconds
        )
    }
}

struct GameOutput {
    records: Vec<TrainRecord>,
    sidecar: Vec<String>,
    snaps: Vec<NativeSnapshot>,
    outcome: f64,
    turns: u16,
    counters: Counters,
    first_search: Option<(u8, u16, u64)>,
}

struct PendingRecord {
    turn: u16,
    side: u8,
    skeleton: BattleState,
    labels: Vec<(Kind, u16)>,
    prior: Vec<f64>,
}

struct Pending {
    action: u8,
    rec: Option<PendingRecord>,
}

#[derive(Serialize)]
struct SidecarLine {
    key: String,
    kind: &'static str,
    id: u16,
    side_of_decider: u8,
    turn: u16,
    opp_legal: Vec<(&'static str, u16)>,
    prior: Vec<(&'static str, u16, f64)>,
}

#[derive(Serialize)]
struct OutcomeLine {
    game_tag: u64,
    winner_side0: f64,
}

fn search_action(state: &BattleState, teams: &TeamData, side: usize, belief: &Belief, seed: u64) -> (u8, u64) {
    let cfg = PimcConfig {
        num_worlds: GEN_NUM_WORLDS,
        time_ms_per_world: GEN_TIME_MS_PER_WORLD,
        max_iters_per_world: GEN_ITERS_PER_WORLD,
        seed,
        chance_mode: ChanceMode::OpenLoop,
        pick_mode: PickMode::Argmax,
        filter_threshold: GEN_FILTER_THRESHOLD,
        raw_root: false,
        explore_coeff: GEN_EXPLORE_COEFF,
        value_temp: GEN_VALUE_TEMP,
    };
    choose_action_eval_iters(&Observation { state, teams, our_side: side }, belief, &RandomBattle, &cfg, EvalKind::Handcrafted, None)
}

fn opp_label(state: &BattleState, opp: usize, action: u8) -> (Kind, u16) {
    match action {
        ACTION_MOVE_0..=ACTION_MOVE_3 => (Kind::Move, effective_moves(state, opp)[action as usize]),
        ACTION_SWITCH_0..=ACTION_SWITCH_5 => (Kind::Switch, state.sides[opp].team[(action - ACTION_SWITCH_0) as usize].species_id),
        ACTION_TERA_0..=ACTION_TERA_3 => (Kind::Tera, effective_moves(state, opp)[(action - ACTION_TERA_0) as usize]),
        _ => unreachable!("opponent action byte {action}"),
    }
}

fn finish(out: &mut GameOutput, game: u64, rec: PendingRecord, state: &BattleState, opp: usize, opp_action: u8) {
    if opp_action == ACTION_STRUGGLE {
        out.counters.skipped_struggle += 1;
        return;
    }
    let (kind, id) = opp_label(state, opp, opp_action);
    let in_l = rec.labels.contains(&(kind, id)) || (kind == Kind::Switch && rec.labels.contains(&(Kind::SwitchUnseen, 0)));
    let line = SidecarLine {
        key: format!("{game}.records.bin:{}", out.records.len()),
        kind: kind.as_str(),
        id,
        side_of_decider: rec.side,
        turn: rec.turn,
        opp_legal: rec.labels.iter().map(|&(k, i)| (k.as_str(), i)).collect(),
        prior: rec.labels.iter().zip(&rec.prior).map(|(&(k, i), &p)| (k.as_str(), i, p)).collect(),
    };
    out.sidecar.push(serde_json::to_string(&line).unwrap());
    out.records.push(TrainRecord {
        #[cfg(feature = "train_value")]
        record_version: 2,
        game_tag: game,
        turn: rec.turn,
        side_of_decider: rec.side,
        world_idx: 0,
        #[cfg(feature = "train_value")]
        root_value: 0.0,
        state: rec.skeleton,
    });
    let c = &mut out.counters;
    c.records_written += 1;
    c.records_by_decider[rec.side as usize] += 1;
    c.labels_in_l += in_l as u64;
    c.l_move_part_sum += rec.labels.iter().filter(|(k, _)| *k == Kind::Move).count() as u64;
    c.l_size_sum += rec.labels.len() as u64;
}

fn run_game(g: u64, seed: u64, opp_random: bool, policy: Policy) -> GameOutput {
    let game_seed = splitmix64(seed ^ g.wrapping_mul(0x9E3779B97F4A7C15));
    let mut team_rng = Lcg::new(game_seed ^ TEAM_SALT);
    let a = frontier::gen_team(&mut team_rng);
    let b = frontier::gen_team(&mut team_rng);
    let (state, teams) = frontier::initial_state(&a, &b);
    let random_seat = opp_random.then(|| (g & 1) as usize);
    let mut out = GameOutput { records: Vec::new(), sidecar: Vec::new(), snaps: Vec::new(), outcome: 0.5, turns: 0, counters: Counters::default(), first_search: None };
    let mut pending: Option<Pending> = None;
    let mut lt = LoopTimes::default();
    let choose = |side: usize, st: &BattleState, tm: &TeamData, bel: &[Belief; 2], seed: u64, rng: &mut Lcg| -> u8 {
        let opp = 1 - side;
        out.counters.decisions_seen += 1;
        let opp_legal = legal_actions(st, opp).count;
        let a = if random_seat == Some(side) || policy == Policy::Random {
            random_action(st, side, rng)
        } else {
            let (a, iters) = search_action(st, tm, side, &bel[side], seed);
            if g == 0 && out.first_search.is_none() && legal_actions(st, side).count >= 2 {
                out.first_search = Some((side as u8, st.field.turn, iters));
            }
            a
        };
        let turn = st.field.turn;
        if SNAPSHOT_TURNS.contains(&turn) {
            out.snaps.push(NativeSnapshot { game: g, turn, side: side as u8, state: *st, teams: tm.clone(), beliefs: *bel, pick: a, seed: game_seed });
        }
        let rec = (random_seat != Some(side)).then(|| {
            let (labels, prior) = frontier::label_space(st, side, &bel[side]);
            PendingRecord { turn, side: side as u8, skeleton: frontier::mask_to_skeleton(st, side, &bel[side]), labels, prior }
        });
        if side == 0 {
            assert!(pending.is_none(), "a side-0 decision outlived its step");
            if opp_legal == 0 {
                out.counters.skipped_no_legal += rec.is_some() as u64;
            } else {
                pending = Some(Pending { action: a, rec });
            }
        } else {
            assert_eq!(pending.is_some(), opp_legal > 0, "side-1 call does not match side 0's legality");
            match pending.take() {
                Some(p) => {
                    if let Some(r0) = p.rec {
                        finish(&mut out, g, r0, st, 1, a);
                    }
                    if let Some(r1) = rec {
                        finish(&mut out, g, r1, st, 0, p.action);
                    }
                }
                None => out.counters.skipped_no_legal += rec.is_some() as u64,
            }
        }
        a
    };
    let v = play_to_terminal_timed(state, &teams, [Belief::default(); 2], game_seed, choose, Some(&mut lt));
    assert!(pending.is_none());
    out.outcome = v;
    out.turns = lt.turns;
    out
}

fn generate(opts: &GenOpts) -> io::Result<Summary> {
    let refuse = |p: &Path| io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists; refusing to overwrite", p.display()));
    let sidecar_path = opts.out.join("opp_action.jsonl");
    if sidecar_path.exists() {
        return Err(refuse(&sidecar_path));
    }
    if let Some(p) = &opts.snapshots {
        if p.exists() {
            return Err(refuse(p));
        }
    }
    std::fs::create_dir_all(&opts.out)?;
    let mut sidecar = BufWriter::new(File::create(&sidecar_path)?);
    let mut outcomes = BufWriter::new(File::create(opts.out.join("outcomes.jsonl"))?);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.threads)
        .build()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let mut snaps: Vec<NativeSnapshot> = Vec::new();
    let mut counters = Counters::default();
    let mut tally = [0u64; 3];
    let mut turns = 0u64;
    let mut n_snaps = 0u64;
    let mut turn_min = u16::MAX;
    let mut turn_max = 0u16;
    let mut first_search_iters: Option<u64> = None;
    let wall = Instant::now();
    let mut start = 0u64;
    while start < opts.games {
        let end = (start + GEN_CHUNK).min(opts.games);
        let outs: Vec<GameOutput> =
            pool.install(|| (start..end).into_par_iter().map(|g| run_game(g, opts.seed, opts.opp_random, opts.policy)).collect());
        for (g, o) in (start..end).zip(outs) {
            if let (0, Some((s, t, n))) = (g, o.first_search) {
                eprintln!("thought game=0 side={s} turn={t} iters={n} worlds={GEN_NUM_WORLDS}");
                first_search_iters = Some(n);
            }
            if !o.records.is_empty() {
                let mut f = BufWriter::new(File::create(opts.out.join(format!("{g}.records.bin")))?);
                for r in &o.records {
                    bincode::serialize_into(&mut f, r).map_err(bincode_err)?;
                }
                f.flush()?;
            }
            for line in &o.sidecar {
                writeln!(sidecar, "{line}")?;
            }
            writeln!(outcomes, "{}", serde_json::to_string(&OutcomeLine { game_tag: g, winner_side0: o.outcome })?)?;
            tally[if o.outcome == 1.0 { 0 } else if o.outcome == 0.0 { 2 } else { 1 }] += 1;
            turns += o.turns as u64;
            counters.add(&o.counters);
            n_snaps += o.snaps.len() as u64;
            for s in &o.snaps {
                turn_min = turn_min.min(s.turn);
                turn_max = turn_max.max(s.turn);
            }
            if opts.snapshots.is_some() {
                snaps.extend(o.snaps);
            }
        }
        eprintln!("gen: {end}/{} games, {} records, {:.1}s", opts.games, counters.records_written, wall.elapsed().as_secs_f64());
        start = end;
    }
    sidecar.flush()?;
    outcomes.flush()?;
    if let Some(p) = &opts.snapshots {
        frontier::write_all(p.to_str().unwrap(), &snaps)?;
    }
    let per_record = |x: u64| if counters.records_written == 0 { 0.0 } else { x as f64 / counters.records_written as f64 };
    let summary = Summary {
        games: opts.games,
        decisions_seen: counters.decisions_seen,
        records_written: counters.records_written,
        skipped_no_legal: counters.skipped_no_legal,
        skipped_struggle: counters.skipped_struggle,
        skipped: counters.skipped_no_legal + counters.skipped_struggle,
        records_by_decider: counters.records_by_decider,
        snapshots: n_snaps,
        snapshot_turn_min: if n_snaps == 0 { 0 } else { turn_min },
        snapshot_turn_max: turn_max,
        mean_l_move_part: per_record(counters.l_move_part_sum),
        mean_l_size: per_record(counters.l_size_sum),
        labels_in_l: counters.labels_in_l,
        wins: tally[0],
        draws: tally[1],
        losses: tally[2],
        mean_turns: if opts.games == 0 { 0.0 } else { turns as f64 / opts.games as f64 },
        wall_seconds: wall.elapsed().as_secs_f64(),
        first_search_iters,
        config: GenConfig::new(opts),
    };
    std::fs::write(opts.out.join("gen.json"), serde_json::to_string_pretty(&summary)?)?;
    Ok(summary)
}

fn shard_files(dir: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut shards = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if let Some(tag) = name.strip_suffix(".records.bin").and_then(|s| s.parse::<u64>().ok()) {
            shards.push((tag, path));
        }
    }
    shards.sort();
    Ok(shards)
}

fn zero_opp(src: &Path, dst: &Path) -> io::Result<u64> {
    if dst.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists; refusing to overwrite", dst.display())));
    }
    std::fs::create_dir_all(dst)?;
    let mut n = 0u64;
    for (tag, path) in shard_files(src)? {
        let mut f = BufWriter::new(File::create(dst.join(format!("{tag}.records.bin")))?);
        for mut rec in read_records(path.to_str().unwrap())? {
            let opp = 1 - rec.side_of_decider as usize;
            for k in 0..6 {
                rec.state.sides[opp].team[k] = MonSlot::default();
            }
            bincode::serialize_into(&mut f, &rec).map_err(bincode_err)?;
            n += 1;
        }
        f.flush()?;
    }
    for name in ["outcomes.jsonl", "opp_action.jsonl"] {
        std::fs::copy(src.join(name), dst.join(name))?;
    }
    if src.join("gen.json").exists() {
        std::fs::copy(src.join("gen.json"), dst.join("gen.json"))?;
    }
    Ok(n)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |flag: &str, default: &str| -> String {
        args.iter().position(|a| a == flag).map(|i| args[i + 1].clone()).unwrap_or_else(|| default.to_string())
    };
    if let Some(i) = args.iter().position(|a| a == "--zero-opp") {
        let (Some(src), Some(dst)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: frontier_native --zero-opp SRC DST");
            std::process::exit(2);
        };
        match zero_opp(Path::new(src), Path::new(dst)) {
            Ok(n) => println!("zero-opp: {n} records -> {dst}"),
            Err(e) => {
                eprintln!("frontier_native --zero-opp: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.iter().any(|a| a == "--gen") {
        let out = get("--out", "");
        if out.is_empty() {
            eprintln!("usage: frontier_native --gen --out DIR [--games N] [--seed S] [--snapshots PATH] [--opp-random] [--threads T]");
            std::process::exit(2);
        }
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let snapshots = get("--snapshots", "");
        let opts = GenOpts {
            games: get("--games", "3000").parse().unwrap(),
            seed: get("--seed", "7").parse().unwrap(),
            out: PathBuf::from(out),
            snapshots: (!snapshots.is_empty()).then(|| PathBuf::from(snapshots)),
            opp_random: args.iter().any(|a| a == "--opp-random"),
            threads: get("--threads", &threads.to_string()).parse().unwrap(),
            policy: Policy::Search,
        };
        let line = GenConfig::new(&opts).line();
        println!("{line}");
        eprintln!("{line}");
        match generate(&opts) {
            Ok(s) => println!("{}", s.line()),
            Err(e) => {
                eprintln!("frontier_native --gen: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let games: u64 = get("--games", "20000").parse().unwrap();
    let seed: u64 = get("--seed", "1").parse().unwrap();
    let extract = !args.iter().any(|a| a == "--no-extract");
    let timers = !args.iter().any(|a| a == "--no-timers");
    if !args.iter().any(|a| a == "--bench") {
        eprintln!("usage: frontier_native --bench [--games N] [--seed S] [--no-extract] [--no-timers] | --gen --out DIR [...] | --zero-opp SRC DST");
        std::process::exit(2);
    }
    bench(games, seed, extract, timers);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("frontier_native_{}_{}", tag, std::process::id()))
    }

    fn gen_with(tag: &str, games: u64, threads: usize, opp_random: bool, policy: Policy) -> (Summary, PathBuf, PathBuf) {
        let dir = tmp(tag);
        let snaps = tmp(&format!("{tag}.snaps"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&snaps);
        let opts = GenOpts { games, seed: 11, out: dir.clone(), snapshots: Some(snaps.clone()), opp_random, threads, policy };
        (generate(&opts).unwrap(), dir, snaps)
    }

    fn gen_random(tag: &str, threads: usize, opp_random: bool) -> (Summary, PathBuf, PathBuf) {
        gen_with(tag, 4, threads, opp_random, Policy::Random)
    }

    fn assert_same_bytes(d1: PathBuf, s1: PathBuf, d4: PathBuf, s4: PathBuf) {
        let names = |d: &Path| {
            let mut v: Vec<String> = std::fs::read_dir(d)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n != "gen.json")
                .collect();
            v.sort();
            v
        };
        let n1 = names(&d1);
        assert_eq!(n1, names(&d4));
        assert!(n1.iter().any(|n| n.ends_with(".records.bin")));
        for n in &n1 {
            assert_eq!(std::fs::read(d1.join(n)).unwrap(), std::fs::read(d4.join(n)).unwrap(), "{n} differs");
        }
        assert_eq!(std::fs::read(&s1).unwrap(), std::fs::read(&s4).unwrap());
        for d in [d1, d4] {
            std::fs::remove_dir_all(d).unwrap();
        }
        for s in [s1, s4] {
            std::fs::remove_file(s).unwrap();
        }
    }

    fn lines(p: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(p).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn shards(dir: &Path) -> HashMap<u64, Vec<TrainRecord>> {
        shard_files(dir).unwrap().into_iter().map(|(tag, p)| (tag, read_records(p.to_str().unwrap()).unwrap())).collect()
    }

    #[test]
    fn gen_corpus_is_internally_consistent() {
        let (s, dir, snaps) = gen_random("gen", 2, false);
        assert_eq!(s.decisions_seen, s.records_written + s.skipped_no_legal + s.skipped_struggle);
        assert!(s.records_written > 0);
        assert_eq!(s.records_by_decider[0] + s.records_by_decider[1], s.records_written);
        let by_tag = shards(&dir);
        assert_eq!(by_tag.values().map(|v| v.len() as u64).sum::<u64>(), s.records_written);
        let side = lines(&dir.join("opp_action.jsonl"));
        assert_eq!(side.len() as u64, s.records_written);
        let mut next_idx: HashMap<u64, usize> = HashMap::new();
        for row in &side {
            let key = row["key"].as_str().unwrap();
            let (stem, idx) = key.split_once(':').unwrap();
            let tag: u64 = stem.strip_suffix(".records.bin").unwrap().parse().unwrap();
            let idx: usize = idx.parse().unwrap();
            let slot = next_idx.entry(tag).or_insert(0);
            assert_eq!(idx, *slot, "sidecar rows follow shard order");
            *slot += 1;
            let rec = &by_tag[&tag][idx];
            assert_eq!(rec.game_tag, tag);
            assert_eq!(rec.turn as u64, row["turn"].as_u64().unwrap());
            assert_eq!(rec.side_of_decider as u64, row["side_of_decider"].as_u64().unwrap());
            assert_eq!(rec.world_idx, 0);
            let legal = row["opp_legal"].as_array().unwrap();
            let prior = row["prior"].as_array().unwrap();
            assert_eq!(legal.len(), prior.len());
            assert!(!legal.is_empty());
            let sum: f64 = prior.iter().map(|p| p[2].as_f64().unwrap()).sum();
            assert!((sum - 1.0).abs() < 1e-9, "{key}: prior sums to {sum}");
            for (l, p) in legal.iter().zip(prior) {
                assert_eq!(l[0], p[0]);
                assert_eq!(l[1], p[1]);
            }
            assert!(matches!(row["kind"].as_str().unwrap(), "move" | "tera" | "switch"));
        }
        for (tag, n) in &next_idx {
            assert_eq!(*n, by_tag[tag].len());
        }
        let outs = lines(&dir.join("outcomes.jsonl"));
        assert_eq!(outs.len(), 4);
        for (g, row) in outs.iter().enumerate() {
            assert_eq!(row["game_tag"].as_u64().unwrap(), g as u64);
            let z = row["winner_side0"].as_f64().unwrap();
            assert!(z == 0.0 || z == 0.5 || z == 1.0);
        }
        let back = frontier::read_all(snaps.to_str().unwrap()).unwrap();
        assert_eq!(back.len() as u64, s.snapshots);
        assert!(back.iter().all(|x| SNAPSHOT_TURNS.contains(&x.turn)));
        assert!(back.windows(2).all(|w| w[0].game <= w[1].game));
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_file(&snaps).unwrap();
    }

    #[test]
    fn gen_opp_random_records_only_the_searching_seat() {
        let (s, dir, snaps) = gen_random("oppr", 2, true);
        assert!(s.records_written > 0);
        for (tag, recs) in shards(&dir) {
            for r in recs {
                assert_eq!(r.side_of_decider as u64, 1 - (tag & 1));
            }
        }
        let back = frontier::read_all(snaps.to_str().unwrap()).unwrap();
        assert!(back.iter().any(|x| x.side == 0) && back.iter().any(|x| x.side == 1), "snapshots cover both seats");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_file(&snaps).unwrap();
    }

    #[test]
    fn gen_output_does_not_depend_on_the_thread_count() {
        let (_, d1, s1) = gen_random("t1", 1, false);
        let (_, d4, s4) = gen_random("t4", 4, false);
        assert_same_bytes(d1, s1, d4, s4);
    }

    #[test]
    #[ignore]
    fn gen_search_output_does_not_depend_on_the_thread_count() {
        let (s1, d1, p1) = gen_with("search_t1", 2, 1, false, Policy::Search);
        let (s4, d4, p4) = gen_with("search_t4", 2, 4, false, Policy::Search);
        assert!(s1.records_written > 0);
        assert_eq!(s1.first_search_iters, s4.first_search_iters);
        assert_same_bytes(d1, p1, d4, p4);
    }

    #[test]
    fn zero_opp_blanks_the_opponent_team_only() {
        let (s, dir, snaps) = gen_random("zero", 2, false);
        let dst = tmp("zero.dst");
        let _ = std::fs::remove_dir_all(&dst);
        assert_eq!(zero_opp(&dir, &dst).unwrap(), s.records_written);
        assert!(zero_opp(&dir, &dst).is_err());
        let src = shards(&dir);
        for (tag, recs) in shards(&dst) {
            let orig = &src[&tag];
            assert_eq!(recs.len(), orig.len());
            for (r, o) in recs.iter().zip(orig) {
                let opp = 1 - r.side_of_decider as usize;
                assert!(r.state.sides[opp].team.iter().all(|m| *m == MonSlot::default()));
                assert!(r.state.sides[1 - opp] == o.state.sides[1 - opp]);
                assert!(r.state.sides[opp].active == o.state.sides[opp].active);
                assert!(r.state.field == o.state.field);
                assert_eq!((r.game_tag, r.turn, r.side_of_decider), (o.game_tag, o.turn, o.side_of_decider));
            }
        }
        for name in ["outcomes.jsonl", "opp_action.jsonl"] {
            assert_eq!(std::fs::read(dir.join(name)).unwrap(), std::fs::read(dst.join(name)).unwrap());
        }
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
        std::fs::remove_file(&snaps).unwrap();
    }
}
