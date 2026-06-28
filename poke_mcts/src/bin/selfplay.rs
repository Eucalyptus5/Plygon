use poke_mcts::chance::OpenLoop;
use poke_mcts::eval::{winner_value, Handcrafted};
use poke_mcts::policies::{greedy_action, random_action};
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::search::{search_world, SearchParams};
use pkmn_engine::state::*;
use serde::Deserialize;

#[derive(Deserialize, Clone)]
struct MonJson {
    species_id: u16, ability_id: u16, item_id: u16,
    moves: Vec<u16>, ivs: Vec<u8>, evs: Vec<u8>,
    nature: u8, level: u8, tera_type: u8, is_female: bool,
}

#[derive(Deserialize)]
struct Fixture { teams: Vec<Vec<MonJson>> }

fn to_input(m: &MonJson) -> MonBuildInput {
    let arr4 = |v: &Vec<u16>| { let mut a = [0u16; 4]; for (i, &x) in v.iter().take(4).enumerate() { a[i] = x; } a };
    let arr6 = |v: &Vec<u8>| { let mut a = [0u8; 6]; for (i, &x) in v.iter().take(6).enumerate() { a[i] = x; } a };
    MonBuildInput {
        species_id: m.species_id, ability_id: m.ability_id, item_id: m.item_id,
        moves: arr4(&m.moves), ivs: arr6(&m.ivs), evs: arr6(&m.evs),
        nature: m.nature, level: m.level, tera_type: m.tera_type, is_female: m.is_female,
    }
}

fn build(team: &[MonJson]) -> ([MonSlot; 6], [MonBuildData; 6], [u8; 6]) {
    let mut inputs: [MonBuildInput; 6] = std::array::from_fn(|_| MonBuildInput {
        species_id: 0, ability_id: 0, item_id: 0, moves: [0; 4], ivs: [0; 6], evs: [0; 6],
        nature: 0, level: 100, tera_type: 0, is_female: false,
    });
    for (i, m) in team.iter().take(6).enumerate() { inputs[i] = to_input(m); }
    let (mut mons, bd, levels) = build_team(&inputs);
    // showdown_type_to_engine is 18-wide and clamps Stellar (18) to Normal; restore it post-build.
    for (i, inp) in inputs.iter().enumerate() {
        if inp.tera_type == 18 { mons[i].tera_type = 18; }
    }
    (mons, bd, levels)
}

#[derive(Clone, Copy, PartialEq)]
enum Kind { Random, Greedy, Mcts, Pimc }

fn parse_kind(s: &str) -> Kind {
    match s { "random" => Kind::Random, "greedy" => Kind::Greedy, "mcts" => Kind::Mcts, "pimc" => Kind::Pimc, _ => panic!("unknown policy {s}") }
}

fn mcts_choose(state: &BattleState, teams: &TeamData, side: usize, time_ms: u64, seed: u64) -> u8 {
    let legal = legal_actions(state, side);
    if legal.count == 0 { return ACTION_STRUGGLE; }
    if legal.count == 1 { return legal.actions[0]; }
    let params = SearchParams { time_ms, ..Default::default() };
    let r = search_world(state, teams, &Handcrafted, &OpenLoop, &params, seed);
    r.side(side).iter().max_by_key(|a| a.visits).map(|a| a.action).unwrap_or(legal.actions[0])
}

fn choose(kind: Kind, state: &BattleState, teams: &TeamData, side: usize, rng: &mut Lcg, time_ms: u64, seed: u64,
          beliefs: &[poke_mcts::belief::Belief; 2], worlds: usize, max_iters: u64) -> u8 {
    match kind {
        Kind::Random => random_action(state, side, rng),
        Kind::Greedy => greedy_action(state, side, rng),
        Kind::Mcts => mcts_choose(state, teams, side, time_ms, seed),
        Kind::Pimc => {
            let obs = poke_mcts::determinize::Observation { state, teams, our_side: side };
            let cfg = poke_mcts::driver::PimcConfig {
                num_worlds: worlds, time_ms_per_world: time_ms, max_iters_per_world: max_iters, seed,
            };
            poke_mcts::driver::choose_action(&obs, &beliefs[side], &poke_mcts::determinize::RandomBattle, &cfg)
        }
    }
}

/// Returns value for side 0: 1.0 win / 0.5 draw / 0.0 loss.
fn play(p1: Kind, p2: Kind, t1: &[MonJson], t2: &[MonJson], game_seed: u64, time_ms: u64, worlds: usize, max_iters: u64) -> f64 {
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
        let a1 = if legal_actions(&state, 0).count > 0 { choose(p1, &state, &teams, 0, &mut pol_rng, time_ms, s1, &beliefs, worlds, max_iters) } else { ACTION_STRUGGLE };
        let a2 = if legal_actions(&state, 1).count > 0 { choose(p2, &state, &teams, 1, &mut pol_rng, time_ms, s2, &beliefs, worlds, max_iters) } else { ACTION_STRUGGLE };
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
    let teams_path = get("--teams", "data/fixture_teams.json");

    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(&teams_path).unwrap()).unwrap();
    let nt = fixture.teams.len() as u64;
    let mut score = 0.0f64;
    let (mut w, mut d, mut l) = (0u64, 0u64, 0u64);
    for g in 0..games {
        // alternate seats so team/seat luck cancels
        let (ta, tb) = ((splitmix64(seed ^ g) % nt) as usize, (splitmix64(seed ^ g ^ 0xF00D) % nt) as usize);
        let v = if g % 2 == 0 {
            play(p1, p2, &fixture.teams[ta], &fixture.teams[tb], seed ^ g, time_ms, worlds, max_iters)
        } else {
            1.0 - play(p2, p1, &fixture.teams[ta], &fixture.teams[tb], seed ^ g, time_ms, worlds, max_iters)
        };
        score += v;
        if v > 0.6 { w += 1 } else if v < 0.4 { l += 1 } else { d += 1 }
        if (g + 1) % 20 == 0 { eprintln!("[{}/{}] p1 score {:.1}%", g + 1, games, 100.0 * score / (g + 1) as f64); }
    }
    println!("p1={:?} p2={:?} games={} -> p1 {:.1}% (W{} D{} L{})",
        get("--p1", "mcts"), get("--p2", "random"), games, 100.0 * score / games as f64, w, d, l);
}
