use poke_mcts::chance::OpenLoop;
use poke_mcts::eval::{winner_value, Handcrafted};
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
    let r = search_world(state, teams, &Handcrafted, &OpenLoop, &params, seed);
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

    let mut beliefs = [poke_mcts::belief::Belief::default(); 2]; // beliefs[s] = what side s knows about its opponent
    let note_active = |beliefs: &mut [poke_mcts::belief::Belief; 2], state: &BattleState| {
        for s in 0..2 {
            let om = state.active_mon(1 - s);
            if om.species_id != 0 && om.current_hp > 0 {
                beliefs[s].note_species(om.species_id, om.level);
            }
        }
    };
    note_active(&mut beliefs, &state);

    let mut battle_rng = Lcg::new(splitmix64(game_seed));
    let mut pol_rng = Lcg::new(splitmix64(game_seed ^ 0xA5A5));
    for turn in 0..500u64 {
        if state.is_game_over() { break; }
        let s1 = splitmix64(game_seed ^ (turn << 1));
        let s2 = splitmix64(game_seed ^ (turn << 1) ^ 1);
        let a1 = if legal_actions(&state, 0).count > 0 { choose(p1, &state, &teams, 0, &mut pol_rng, s1, &beliefs) } else { ACTION_STRUGGLE };
        let a2 = if legal_actions(&state, 1).count > 0 { choose(p2, &state, &teams, 1, &mut pol_rng, s2, &beliefs) } else { ACTION_STRUGGLE };
        for (s, a) in [(0usize, a1), (1usize, a2)] {
            if state.phase != PHASE_ACTIONS { continue; }
            let viewer = 1 - s;
            let slot = beliefs[viewer].note_species(state.active_mon(s).species_id, state.active_mon(s).level);
            match a {
                0..=3 => {
                    let mv = effective_moves(&state, s)[a as usize];
                    beliefs[viewer].note_move(slot, mv);
                }
                ACTION_TERA => {
                    let mv = effective_moves(&state, s)[0];
                    beliefs[viewer].note_move(slot, mv);
                    // MonBelief stores Showdown indices; inverse map handles the Stellar (18) identity.
                    let sd_tera = poke_mcts::belief::engine_type_to_showdown(state.active_mon(s).tera_type);
                    beliefs[viewer].note_tera(slot, sd_tera);
                }
                _ => {}
            }
        }
        match state.phase {
            PHASE_ACTIONS => execute_turn(&mut state, &teams, a1, a2, &mut |m| battle_rng.roll(m)),
            PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH =>
                execute_switch_turn(&mut state, &teams, a1, a2, &mut |m| battle_rng.roll(m)),
            _ => break,
        }
        note_active(&mut beliefs, &state);
    }
    winner_value(&state)
}

fn stats(mut v: Vec<f64>) -> (f64, f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    let med = if n % 2 == 0 { (v[n / 2 - 1] + v[n / 2]) / 2.0 } else { v[n / 2] };
    (v[0], med, v[n - 1])
}

fn bench(fixture: &Fixture) {
    let (team1, b1, l1) = build(&fixture.teams[0]);
    let (team2, b2, l2) = build(&fixture.teams[1]);
    let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
    let mut state = BattleState::default();
    state.sides[0].team = team1;
    state.sides[1].team = team2;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);

    let params = SearchParams { time_ms: 100, ..Default::default() };
    let mut iters: Vec<f64> = Vec::new();
    let (mut guards, mut depths) = (0u64, 0u64);
    for seed in 0..10u64 {
        let r = search_world(&state, &teams, &Handcrafted, &OpenLoop, &params, seed);
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
        let r = poke_mcts::chance_closed::search_world_closed(&state, &teams, &Handcrafted, &closed_p, seed);
        citers.push(r.iterations as f64);
    }
    let (cmin, cmed, cmax) = stats(citers);
    println!("search_world_closed iters/100ms: min {:.0} / median {:.0} / max {:.0}", cmin, cmed, cmax);
    println!("closed/open iters ratio (median): {:.2}", cmed / med);

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
        };
        let t0 = std::time::Instant::now();
        let _ = poke_mcts::driver::choose_action(&obs, &belief, &poke_mcts::determinize::RandomBattle, &cfg);
        wall.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let (wmin, wmed, wmax) = stats(wall);
    println!("choose_action 16w@100ms wall-ms/decision: min {:.1} / median {:.1} / max {:.1}", wmin, wmed, wmax);
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
        other => panic!("unknown --pick-mode {other}"),
    };
    let filter_threshold = if get("--filter-relax", "off") == "on" { 0.0 } else { 0.75 };
    let raw_root = args.iter().any(|a| a == "--raw-root");
    let teams_path = get("--teams", "data/fixture_teams.json");

    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(&teams_path).unwrap()).unwrap();
    if args.iter().any(|a| a == "--bench-search") {
        bench(&fixture);
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
    let e1 = Entrant { kind: p1, time_ms, worlds, max_iters, adaptive, chance_mode, pick_mode: poke_mcts::driver::PickMode::Weighted, filter_threshold: 0.75, raw_root: false };
    let e2 = Entrant { kind: p2, time_ms, worlds, max_iters, adaptive, chance_mode, pick_mode: poke_mcts::driver::PickMode::Weighted, filter_threshold: 0.75, raw_root: false };
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
