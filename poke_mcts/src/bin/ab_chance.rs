// For each fixture battle: play T=4 greedy-vs-greedy turns to reach a mid-game
// position, then search it with both chance models at equal budget and compare
// the chosen action byte and the root best-arm avg_score.
use poke_mcts::chance::OpenLoop;
use poke_mcts::chance_closed::search_world_closed;
use poke_mcts::eval::Handcrafted;
use poke_mcts::fixtures::{build, Fixture};
use poke_mcts::policies::greedy_action;
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::search::{search_world, ArmStat, SearchParams};
use pkmn_engine::state::*;

fn best(arms: &[ArmStat]) -> &ArmStat {
    arms.iter().max_by_key(|a| a.visits).unwrap()
}

fn main() {
    let fixture: Fixture =
        serde_json::from_str(&std::fs::read_to_string("data/fixture_teams.json").unwrap()).unwrap();
    let nt = fixture.teams.len();
    let mut positions = 0u32;
    let mut agree = 0u32;
    let mut d_sum = 0.0f64;
    for i in 0..50usize {
        let (ta, tb) = (i % nt, (i * 7 + 1) % nt);
        let (team1, b1, l1) = build(&fixture.teams[ta]);
        let (team2, b2, l2) = build(&fixture.teams[tb]);
        let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
        let mut state = BattleState::default();
        state.sides[0].team = team1;
        state.sides[1].team = team2;
        state.phase = PHASE_ACTIONS;
        switch::switch_in(&mut state, &teams, 0, 0);
        switch::switch_in(&mut state, &teams, 1, 0);

        let mut battle_rng = Lcg::new(splitmix64(i as u64));
        let mut pol_rng = Lcg::new(splitmix64(i as u64 ^ 0xA5A5));
        for _ in 0..4 {
            if state.is_game_over() { break; }
            let a1 = if legal_actions(&state, 0).count > 0 { greedy_action(&state, 0, &mut pol_rng) } else { ACTION_STRUGGLE };
            let a2 = if legal_actions(&state, 1).count > 0 { greedy_action(&state, 1, &mut pol_rng) } else { ACTION_STRUGGLE };
            match state.phase {
                PHASE_ACTIONS => execute_turn(&mut state, &teams, a1, a2, &mut |m| battle_rng.roll(m)),
                PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH =>
                    execute_switch_turn(&mut state, &teams, a1, a2, &mut |m| battle_rng.roll(m)),
                _ => break,
            }
        }
        if state.is_game_over() || legal_actions(&state, 0).count == 0 {
            println!("pos {i}: skipped");
            continue;
        }

        let open_params = SearchParams { time_ms: 100, max_iters: u64::MAX, ..Default::default() };
        let closed_params = SearchParams { time_ms: 100, max_iters: u64::MAX, max_nodes: poke_mcts::search::closed_loop_max_nodes(1) };
        let seed = splitmix64(i as u64 ^ 0x5EED);
        let open = search_world(&state, &teams, &Handcrafted, &OpenLoop, &open_params, seed);
        let closed = search_world_closed(&state, &teams, &Handcrafted, &closed_params, seed);
        let (ob, cb) = (best(&open.s1), best(&closed.s1));
        let m = ob.action == cb.action;
        let dv = (ob.avg_score - cb.avg_score).abs();
        println!("pos {i}: open={} closed={} match={} d_value={:.4}", ob.action, cb.action, m, dv);
        positions += 1;
        if m { agree += 1; }
        d_sum += dv;
    }
    println!(
        "summary: positions={} agreement={:.1}% mean|d_value|={:.4}",
        positions,
        100.0 * agree as f64 / positions.max(1) as f64,
        d_sum / positions.max(1) as f64
    );
}
