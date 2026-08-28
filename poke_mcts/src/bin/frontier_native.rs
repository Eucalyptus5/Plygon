use pkmn_engine::state::{BattleState, TeamData};
use poke_mcts::belief::Belief;
use poke_mcts::features::{self, NUM_SEGMENTS};
use poke_mcts::frontier;
use poke_mcts::policies::random_action;
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::selfplay::{play_to_terminal_timed, LoopTimes};
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |flag: &str, default: &str| -> String {
        args.iter().position(|a| a == flag).map(|i| args[i + 1].clone()).unwrap_or_else(|| default.to_string())
    };
    let games: u64 = get("--games", "20000").parse().unwrap();
    let seed: u64 = get("--seed", "1").parse().unwrap();
    let extract = !args.iter().any(|a| a == "--no-extract");
    let timers = !args.iter().any(|a| a == "--no-timers");
    if args.iter().any(|a| a == "--gen") {
        eprintln!("frontier_native --gen: not built");
        std::process::exit(2);
    }
    if !args.iter().any(|a| a == "--bench") {
        eprintln!("usage: frontier_native --bench|--gen [--games N] [--seed S] [--no-extract] [--no-timers]");
        std::process::exit(2);
    }
    bench(games, seed, extract, timers);
}
