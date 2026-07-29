use pkmn_engine::data::moves::MoveCategory;
use pkmn_engine::data::{GEN_ITEMS, GEN_MOVES, MOVE_BELCH, TOTAL_SPECIES};
use pkmn_engine::state::data_bridge::{
    ItemFlag, ABILITY_ARENA_TRAP, ABILITY_MAGNET_PULL, ABILITY_SHADOW_TAG,
};
use pkmn_engine::state::{
    item, legal_actions, move_base_pp, move_hot, BattleState, MonSlot, SideState, PHASE_ACTIONS,
    PHASE_SWITCH_BOTH,
};
use poke_mcts::eval_learned::NUM_ACTIONS;
use poke_mcts::rng::Lcg;
use poke_mcts::train_dump::TrainRecord;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const PPM: u32 = 1_000_000;
const SWITCH_PHASE_PPM: u32 = 132_900;
const FOUR_MOVES_PPM: u32 = 859_000;
const TERA_PPM: u32 = 616_500;
const BENCH_ALIVE_PPM: u32 = 632_340;
const SWITCH_BASE: usize = 4;
const TERA_BASE: usize = 10;
const BAND: f64 = 0.05;
const MAX_ATTEMPTS: u32 = 64;
const DEFAULT_SEED: u64 = 0x5EED_5EED;
// codegen sizes its per-ability bit table as [u64; 5], bounding ability ids at 320
const ABILITY_SPACE: u32 = 320;

#[derive(Clone, Copy, PartialEq)]
enum Mix {
    Corpus,
    AllDmg,
}

impl Mix {
    fn name(&self) -> &'static str {
        match self {
            Mix::Corpus => "corpus",
            Mix::AllDmg => "alldmg",
        }
    }
}

struct MintCfg {
    out: PathBuf,
    mix: Mix,
    shards: u64,
    records_per_shard: u64,
    seed: u64,
}

#[derive(Default)]
struct MintStats {
    shards: u64,
    records: u64,
    rows: u64,
    redraws: u64,
    move_bytes: u64,
    switch_bytes: u64,
    tera_bytes: u64,
    cat: [u64; 3],
}

struct Profile {
    ids: f64,
    total: f64,
    mv: f64,
    sw: f64,
    tera: f64,
}

struct Pools {
    by_cat: [Vec<u16>; 3],
    all: Vec<u16>,
    items: Vec<u16>,
}

struct SidePlan {
    usable: u8,
    tera: bool,
    alive_bench: u8,
}

fn bern(rng: &mut Lcg, ppm: u32) -> bool {
    rng.roll(PPM) < ppm
}

fn pick(rng: &mut Lcg, pool: &[u16]) -> u16 {
    pool[rng.roll(pool.len() as u32) as usize]
}

fn sample_category(rng: &mut Lcg, mix: Mix) -> MoveCategory {
    let (status, physical) = match mix {
        Mix::Corpus => (316_000u32, 402_200u32),
        Mix::AllDmg => (0u32, 588_000u32),
    };
    let r = rng.roll(PPM);
    if r < status {
        MoveCategory::Status
    } else if r < status + physical {
        MoveCategory::Physical
    } else {
        MoveCategory::Special
    }
}

fn sample_usable_moves(rng: &mut Lcg) -> u8 {
    if bern(rng, FOUR_MOVES_PPM) {
        4
    } else {
        3
    }
}

fn build_pools() -> Pools {
    let mut by_cat: [Vec<u16>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut all = Vec::new();
    for id in 1..GEN_MOVES.len() as u16 {
        // pp 0 marks a hole in the generated table; Belch is legality-gated on an eaten berry
        if id as usize == MOVE_BELCH || move_base_pp(id) == 0 {
            continue;
        }
        by_cat[move_hot(id).category as usize].push(id);
        all.push(id);
    }
    let mut items = Vec::new();
    for id in 1..GEN_ITEMS.len() as u16 {
        let d = item(id);
        // both flags gate move legality in generate_legal_moves
        if d.has(ItemFlag::IS_CHOICE) || d.has(ItemFlag::ASSAULT_VEST) {
            continue;
        }
        items.push(id);
    }
    Pools { by_cat, all, items }
}

fn draw_ability(rng: &mut Lcg) -> u16 {
    loop {
        let a = 1 + rng.roll(ABILITY_SPACE - 1) as u16;
        // a trapping ability on either active zeroes the other side's switch bytes
        if a != ABILITY_MAGNET_PULL && a != ABILITY_ARENA_TRAP && a != ABILITY_SHADOW_TAG {
            return a;
        }
    }
}

fn draw_mon(rng: &mut Lcg, pools: &Pools, moves: [u16; 4]) -> MonSlot {
    let max_hp = 200 + rng.roll(200) as u16;
    MonSlot {
        species_id: 1 + rng.roll(TOTAL_SPECIES as u32 - 1) as u16,
        ability_id: draw_ability(rng),
        item_id: pick(rng, &pools.items),
        current_hp: 1 + rng.roll(max_hp as u32) as u16,
        max_hp,
        stats: [100 + rng.roll(150) as u16; 5],
        moves,
        pp: [16; 4],
        level: 100,
        ..Default::default()
    }
}

fn build_side(rng: &mut Lcg, pools: &Pools, mix: Mix, is_switch: bool) -> (SideState, SidePlan) {
    let mut moves = [0u16; 4];
    for m in moves.iter_mut() {
        let c = sample_category(rng, mix) as usize;
        *m = pick(rng, &pools.by_cat[c]);
    }
    let mut active = draw_mon(rng, pools, moves);
    let mut plan = SidePlan { usable: 4, tera: false, alive_bench: 0 };
    if !is_switch {
        plan.usable = sample_usable_moves(rng);
        let mut order = [0usize, 1, 2, 3];
        for i in (1..4).rev() {
            let j = rng.roll(i as u32 + 1) as usize;
            order.swap(i, j);
        }
        for &k in order.iter().take(4 - plan.usable as usize) {
            active.pp[k] = 0;
        }
        plan.tera = bern(rng, TERA_PPM);
        if plan.tera {
            active.tera_type = 1 + rng.roll(18) as u8;
        }
    }
    let mut side = SideState::default();
    side.team[0] = active;
    for j in 1..6 {
        let mut bench_moves = [0u16; 4];
        for m in bench_moves.iter_mut() {
            *m = pick(rng, &pools.all);
        }
        let mut mon = draw_mon(rng, pools, bench_moves);
        if bern(rng, BENCH_ALIVE_PPM) {
            plan.alive_bench += 1;
        } else {
            mon.current_hp = 0;
        }
        side.team[j] = mon;
    }
    (side, plan)
}

fn achieved(state: &BattleState, side: usize) -> [u32; 3] {
    let mut c = [0u32; 3];
    for &a in legal_actions(state, side).as_slice() {
        let b = a as usize;
        if b < SWITCH_BASE {
            c[0] += 1;
        } else if b < TERA_BASE {
            c[1] += 1;
        } else if b < NUM_ACTIONS {
            c[2] += 1;
        }
    }
    c
}

fn intended(plan: &SidePlan, is_switch: bool) -> [u32; 3] {
    if is_switch {
        return [0, plan.alive_bench as u32, 0];
    }
    let mv = plan.usable as u32;
    [mv, plan.alive_bench as u32, if plan.tera { mv } else { 0 }]
}

fn mint_state(rng: &mut Lcg, pools: &Pools, mix: Mix, redraws: &mut u64) -> Result<BattleState, String> {
    let names = ["move", "switch", "tera"];
    let mut last = String::from("no attempt made");
    // drawn once: a retry must resample only the sides, never the phase
    let is_switch = bern(rng, SWITCH_PHASE_PPM);
    for _ in 0..MAX_ATTEMPTS {
        let (s0, p0) = build_side(rng, pools, mix, is_switch);
        let (s1, p1) = build_side(rng, pools, mix, is_switch);
        let mut state = BattleState::default();
        state.phase = if is_switch { PHASE_SWITCH_BOTH } else { PHASE_ACTIONS };
        state.sides[0] = s0;
        state.sides[1] = s1;
        let mut bad = None;
        for (side, plan) in [(0usize, &p0), (1usize, &p1)] {
            let want = intended(plan, is_switch);
            let got = achieved(&state, side);
            if got != want {
                let parts: Vec<String> = (0..3)
                    .filter(|&k| got[k] != want[k])
                    .map(|k| {
                        format!(
                            "{} got {} want {} (delta {:+})",
                            names[k],
                            got[k],
                            want[k],
                            got[k] as i64 - want[k] as i64
                        )
                    })
                    .collect();
                bad = Some(format!("side {side}: {}", parts.join(", ")));
                break;
            }
        }
        match bad {
            None => return Ok(state),
            Some(m) => {
                *redraws += 1;
                last = m;
            }
        }
    }
    Err(last)
}

fn mdist_json(state: &BattleState, decider: u8) -> String {
    let legal: Vec<u8> = legal_actions(state, decider as usize)
        .as_slice()
        .iter()
        .copied()
        .filter(|&a| (a as usize) < NUM_ACTIONS)
        .collect();
    if legal.is_empty() {
        return String::from("{}");
    }
    let share = 1.0f64 / legal.len() as f64;
    let body: Vec<String> = legal.iter().map(|b| format!("\"{b}\":{share}")).collect();
    format!("{{{}}}", body.join(","))
}

fn mint(cfg: &MintCfg) -> io::Result<MintStats> {
    let pools = build_pools();
    let mut rng = Lcg::new(cfg.seed);
    match std::fs::remove_dir_all(&cfg.out) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    std::fs::create_dir_all(&cfg.out)?;
    let mut outcomes = BufWriter::new(File::create(cfg.out.join("outcomes.jsonl"))?);
    let mut labels = BufWriter::new(File::create(cfg.out.join("labels.jsonl"))?);
    let mut stats = MintStats::default();
    for tag in 0..cfg.shards {
        let mut shard = BufWriter::new(File::create(cfg.out.join(format!("{tag}.records.bin")))?);
        for idx in 0..cfg.records_per_shard {
            let state = match mint_state(&mut rng, &pools, cfg.mix, &mut stats.redraws) {
                Ok(s) => s,
                Err(m) => {
                    eprintln!(
                        "gen_synth_shards: {tag}.records.bin:{idx} legality diverged on all {MAX_ATTEMPTS} attempts: {m}"
                    );
                    std::process::exit(1);
                }
            };
            let decider = rng.roll(2) as u8;
            let _root_value = rng.roll(PPM + 1) as f32 / PPM as f32;
            let rec = TrainRecord {
                #[cfg(feature = "train_value")]
                record_version: 2,
                game_tag: tag,
                turn: (idx / 2 + 1) as u16,
                side_of_decider: decider,
                world_idx: (idx % 2) as u8,
                #[cfg(feature = "train_value")]
                root_value: _root_value,
                state,
            };
            let bytes = bincode::serialize(&rec)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            shard.write_all(&bytes)?;
            writeln!(
                labels,
                "{{\"key\":\"{tag}.records.bin:{idx}\",\"legal_match\":true,\"mdist\":{}}}",
                mdist_json(&rec.state, decider)
            )?;
            for side in 0..2 {
                let c = achieved(&rec.state, side);
                stats.move_bytes += c[0] as u64;
                stats.switch_bytes += c[1] as u64;
                stats.tera_bytes += c[2] as u64;
                let mv = rec.state.sides[side].team[0].moves;
                for &a in legal_actions(&rec.state, side).as_slice() {
                    if (a as usize) < SWITCH_BASE {
                        stats.cat[move_hot(mv[a as usize]).category as usize] += 1;
                    }
                }
            }
            stats.rows += 2;
            stats.records += 1;
        }
        shard.flush()?;
        let z = if bern(&mut rng, PPM / 2) { 1.0 } else { 0.0 };
        writeln!(outcomes, "{{\"game_tag\":{tag},\"winner_side0\":{z:.1}}}")?;
        stats.shards += 1;
    }
    outcomes.flush()?;
    labels.flush()?;
    Ok(stats)
}

fn components(p: &Profile) -> [(&'static str, f64, f64); 5] {
    [
        ("ids per row", p.ids, 99.1961),
        ("total legal bytes per row", p.total, 8.5713),
        ("move bytes per row", p.mv, 3.3464),
        ("switch bytes per row", p.sw, 3.1617),
        ("tera bytes per row", p.tera, 2.0632),
    ]
}

fn band_ok(measured: f64, target: f64) -> bool {
    ((measured - target) / target).abs() <= BAND
}

fn verify_exit_code(p: &Profile, games_dropped: u64) -> i32 {
    if games_dropped == 0 && components(p).iter().all(|&(_, m, t)| band_ok(m, t)) {
        0
    } else {
        1
    }
}

fn verify_profile(dir: &str) -> i32 {
    let meta_path = Path::new(dir).join("meta.json");
    let txt = match std::fs::read_to_string(&meta_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("gen_synth_shards: {}: {e}", meta_path.display());
            return 2;
        }
    };
    let meta: serde_json::Value = match serde_json::from_str(&txt) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("gen_synth_shards: {}: {e}", meta_path.display());
            return 2;
        }
    };
    let (Some(records), Some(features)) = (meta["records"].as_u64(), meta["features"].as_u64())
    else {
        eprintln!("gen_synth_shards: {}: missing records/features", meta_path.display());
        return 2;
    };
    if records == 0 {
        eprintln!("gen_synth_shards: {}: records 0", meta_path.display());
        return 2;
    }
    let (Some(games_joined), Some(games_dropped)) =
        (meta["games_joined"].as_u64(), meta["games_dropped"].as_u64())
    else {
        eprintln!(
            "gen_synth_shards: {}: missing games_joined/games_dropped",
            meta_path.display()
        );
        return 2;
    };
    let lm_path = Path::new(dir).join("legal_mask.bin");
    let want_len = records * NUM_ACTIONS as u64;
    match std::fs::metadata(&lm_path) {
        Ok(m) if m.len() == want_len => {}
        Ok(m) => {
            eprintln!(
                "gen_synth_shards: {}: {} bytes, want {want_len}",
                lm_path.display(),
                m.len()
            );
            return 2;
        }
        Err(e) => {
            eprintln!("gen_synth_shards: {}: {e}", lm_path.display());
            return 2;
        }
    }
    let mut f = match File::open(&lm_path) {
        Ok(f) => BufReader::new(f),
        Err(e) => {
            eprintln!("gen_synth_shards: {}: {e}", lm_path.display());
            return 2;
        }
    };
    const CHUNK: usize = 65536;
    let mut buf = vec![0u8; CHUNK * NUM_ACTIONS];
    let (mut mv, mut sw, mut tera) = (0u64, 0u64, 0u64);
    let mut done = 0u64;
    while done < records {
        let n = (CHUNK as u64).min(records - done) as usize;
        if let Err(e) = f.read_exact(&mut buf[..n * NUM_ACTIONS]) {
            eprintln!("gen_synth_shards: {}: {e}", lm_path.display());
            return 2;
        }
        for row in 0..n {
            let legal = &buf[row * NUM_ACTIONS..(row + 1) * NUM_ACTIONS];
            mv += legal[..SWITCH_BASE].iter().filter(|&&v| v != 0).count() as u64;
            sw += legal[SWITCH_BASE..TERA_BASE].iter().filter(|&&v| v != 0).count() as u64;
            tera += legal[TERA_BASE..].iter().filter(|&&v| v != 0).count() as u64;
        }
        done += n as u64;
    }
    let rows = records as f64;
    let p = Profile {
        ids: features as f64 / rows,
        total: (mv + sw + tera) as f64 / rows,
        mv: mv as f64 / rows,
        sw: sw as f64 / rows,
        tera: tera as f64 / rows,
    };
    println!("converted dir: {dir}");
    println!("games joined {games_joined}  games dropped {games_dropped}  records {records}  features {features}");
    println!(
        "{:<26} {games_dropped:>9}  target {:>8}  {:14}{}",
        "games dropped",
        0,
        "",
        if games_dropped == 0 { "OK" } else { "OUT" }
    );
    for (name, got, want) in components(&p) {
        println!(
            "{name:<26} {got:>9.4}  target {want:>8.4}  dev {:>+7.2}%  {}",
            (got - want) / want * 100.0,
            if band_ok(got, want) { "OK" } else { "OUT" }
        );
    }
    let code = verify_exit_code(&p, games_dropped);
    println!("verify-profile {}", if code == 0 { "PASS" } else { "FAIL" });
    code
}

fn usage() -> ! {
    eprintln!(
        "usage: gen_synth_shards --out <dir> --mix corpus|alldmg [--shards N] [--records-per-shard N] [--seed U64]\n       gen_synth_shards --verify-profile <converted dir>"
    );
    std::process::exit(2)
}

fn parse_u64(v: Option<String>) -> u64 {
    match v.and_then(|s| s.parse::<u64>().ok()) {
        Some(n) => n,
        None => usage(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out: Option<String> = None;
    let mut mix: Option<Mix> = None;
    let mut verify: Option<String> = None;
    let mut shards = 256u64;
    let mut records_per_shard = 8u64;
    let mut seed = DEFAULT_SEED;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next(),
            "--verify-profile" => verify = it.next(),
            "--mix" => {
                mix = match it.next().as_deref() {
                    Some("corpus") => Some(Mix::Corpus),
                    Some("alldmg") => Some(Mix::AllDmg),
                    _ => usage(),
                }
            }
            "--shards" => shards = parse_u64(it.next()),
            "--records-per-shard" => records_per_shard = parse_u64(it.next()),
            "--seed" => seed = parse_u64(it.next()),
            _ => {
                eprintln!("unexpected argument: {a}");
                std::process::exit(2);
            }
        }
    }
    if let Some(dir) = verify {
        std::process::exit(verify_profile(&dir));
    }
    let (Some(out), Some(mix)) = (out, mix) else { usage() };
    if shards == 0 || records_per_shard == 0 {
        usage();
    }
    let cfg = MintCfg { out: PathBuf::from(out), mix, shards, records_per_shard, seed };
    let stats = match mint(&cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("gen_synth_shards: {e}");
            std::process::exit(1);
        }
    };
    let rows = stats.rows as f64;
    let total = (stats.move_bytes + stats.switch_bytes + stats.tera_bytes) as f64;
    let cats = stats.cat.iter().sum::<u64>().max(1) as f64;
    println!("minted shards {}", stats.shards);
    println!("minted records {}", stats.records);
    println!(
        "minted profile rows {}  total {:.4}  move {:.4}  switch {:.4}  tera {:.4}  mix {}  seed {}",
        stats.rows,
        total / rows,
        stats.move_bytes as f64 / rows,
        stats.switch_bytes as f64 / rows,
        stats.tera_bytes as f64 / rows,
        cfg.mix.name(),
        cfg.seed
    );
    println!(
        "minted category mix  Status {:.2}%  Physical {:.2}%  Special {:.2}%",
        stats.cat[MoveCategory::Status as usize] as f64 / cats * 100.0,
        stats.cat[MoveCategory::Physical as usize] as f64 / cats * 100.0,
        stats.cat[MoveCategory::Special as usize] as f64 / cats * 100.0
    );
    println!("minted redraws {}", stats.redraws);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_sampler_matches_the_requested_mix() {
        const N: usize = 200_000;
        for (mix, want) in [
            (Mix::Corpus, [0.4022f64, 0.2818, 0.3160]),
            (Mix::AllDmg, [0.5880f64, 0.4120, 0.0]),
        ] {
            let mut rng = Lcg::new(7);
            let mut cat = [0u64; 3];
            for _ in 0..N {
                cat[sample_category(&mut rng, mix) as usize] += 1;
            }
            for k in 0..3 {
                let got = cat[k] as f64 / N as f64;
                assert!(
                    (got - want[k]).abs() < 0.005,
                    "category {k}: got {got:.4} want {:.4}",
                    want[k]
                );
            }
        }
    }

    #[test]
    fn usable_move_slot_sampler_is_two_point_with_mean_3859() {
        const N: usize = 200_000;
        let mut rng = Lcg::new(11);
        let mut sum = 0u64;
        for _ in 0..N {
            let c = sample_usable_moves(&mut rng);
            assert!(c == 3 || c == 4, "usable count {c} outside the two-point support");
            sum += c as u64;
        }
        let mean = sum as f64 / N as f64;
        assert!((mean - 3.859).abs() < 0.01, "mean {mean:.4} want 3.859");
    }

    #[test]
    fn verify_profile_bands_each_component_at_five_percent() {
        let at_target = Profile { ids: 99.1961, total: 8.5713, mv: 3.3464, sw: 3.1617, tera: 2.0632 };
        assert_eq!(verify_exit_code(&at_target, 0), 0, "on-target profile must pass");

        fn field(p: &mut Profile, k: usize) -> &mut f64 {
            match k {
                0 => &mut p.ids,
                1 => &mut p.total,
                2 => &mut p.mv,
                3 => &mut p.sw,
                _ => &mut p.tera,
            }
        }
        let targets: Vec<f64> = components(&at_target).iter().map(|c| c.2).collect();
        for k in 0..5 {
            for (scale, want) in [(1.049, 0), (0.951, 0), (1.051, 1), (0.949, 1)] {
                let mut p =
                    Profile { ids: 99.1961, total: 8.5713, mv: 3.3464, sw: 3.1617, tera: 2.0632 };
                *field(&mut p, k) = targets[k] * scale;
                assert_eq!(
                    verify_exit_code(&p, 0),
                    want,
                    "component {} at x{scale} must exit {want}",
                    components(&at_target)[k].0
                );
            }
        }
    }

    #[test]
    fn nonzero_games_dropped_fails_the_profile_gate() {
        let at_target = Profile { ids: 99.1961, total: 8.5713, mv: 3.3464, sw: 3.1617, tera: 2.0632 };
        assert_eq!(verify_exit_code(&at_target, 0), 0, "clean join must pass");
        assert_eq!(
            verify_exit_code(&at_target, 1),
            1,
            "one dropped game must fail an otherwise on-target profile"
        );
    }

    #[test]
    fn same_seed_mints_identical_bytes() {
        let base = std::env::temp_dir().join(format!("gen_synth_det_{}", std::process::id()));
        let dirs: Vec<PathBuf> = (0..3).map(|k| base.join(k.to_string())).collect();
        let seeds = [4242u64, 4242, 99];
        for (d, s) in dirs.iter().zip(seeds) {
            let cfg = MintCfg {
                out: d.clone(),
                mix: Mix::Corpus,
                shards: 4,
                records_per_shard: 3,
                seed: s,
            };
            let stats = mint(&cfg).expect("mint must succeed");
            assert_eq!(stats.records, 12, "4 shards x 3 records");
        }
        let read_all = |d: &PathBuf| -> Vec<(String, Vec<u8>)> {
            let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(d)
                .expect("mint dir must exist")
                .map(|e| {
                    let p = e.unwrap().path();
                    (
                        p.file_name().unwrap().to_string_lossy().into_owned(),
                        std::fs::read(&p).unwrap(),
                    )
                })
                .collect();
            v.sort();
            v
        };
        let a = read_all(&dirs[0]);
        assert!(!a.is_empty(), "mint must write files");
        assert_eq!(a, read_all(&dirs[1]), "same seed must mint identical bytes");
        assert_ne!(a, read_all(&dirs[2]), "a different seed must mint different bytes");
        std::fs::remove_dir_all(&base).ok();
    }
}
