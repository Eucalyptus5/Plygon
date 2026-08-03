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

#[derive(serde::Deserialize)]
struct MonSpec { species: u16, level: u8, ability: u16, item: u16, moves: [u16;4],
    #[serde(default)] evs: [u8;6] }
#[derive(serde::Deserialize, Default)]
struct Overrides {
    #[serde(default)] side1_active_hp: Option<u16>,
    #[serde(default)] side0_active_hp: Option<u16>,
}
#[derive(serde::Deserialize)]
struct SuiteCase {
    name: String, source: String,
    #[allow(dead_code)] justification: String,
    side0: MonSpec, side1: MonSpec,
    #[serde(default)] overrides: Overrides,
    correct_action: u8,
}
#[derive(serde::Deserialize)]
struct Suite { cases: Vec<SuiteCase> }

fn spec_to_input(m: &MonSpec) -> MonBuildInput {
    MonBuildInput { species_id: m.species, ability_id: m.ability, item_id: m.item, moves: m.moves,
        ivs: [31;6], evs: m.evs, nature: 0, level: m.level, tera_type: 0, is_female: false }
}
fn empty_input() -> MonBuildInput {
    MonBuildInput { species_id:0, ability_id:0, item_id:0, moves:[0;4],
        ivs:[0;6], evs:[0;6], nature:0, level:100, tera_type:0, is_female:false }
}
fn build_case(c: &SuiteCase) -> (BattleState, TeamData) {
    let mut p1: [MonBuildInput;6] = std::array::from_fn(|_| empty_input());
    let mut p2: [MonBuildInput;6] = std::array::from_fn(|_| empty_input());
    p1[0] = spec_to_input(&c.side0);
    p2[0] = spec_to_input(&c.side1);
    let (t1, b1, l1) = build_team(&p1);
    let (t2, b2, l2) = build_team(&p2);
    let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
    let mut state = BattleState::default();
    state.sides[0].team = t1;
    state.sides[1].team = t2;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);
    if let Some(hp) = c.overrides.side1_active_hp {
        let ai = state.sides[1].active_index as usize;
        state.sides[1].team[ai].current_hp = hp; // active HP lives in team[active_index]; apply after switch_in
    }
    if let Some(hp) = c.overrides.side0_active_hp {
        let ai = state.sides[0].active_index as usize;
        state.sides[0].team[ai].current_hp = hp; // active HP lives in team[active_index]; apply after switch_in
    }
    (state, teams)
}

fn run_suite() {
    let suite: Suite = serde_json::from_str(&std::fs::read_to_string("data/scenario_suite.json").unwrap()).unwrap();
    let (mut open_ok, mut closed_ok) = (0u32, 0u32);
    for c in &suite.cases {
        let (state, teams) = build_case(c);
        let open_p = SearchParams { time_ms: 200, max_iters: u64::MAX, max_nodes: 2_000_000, explore_coeff: 2.0 };
        let closed_p = SearchParams { time_ms: 200, max_iters: u64::MAX, max_nodes: poke_mcts::search::closed_loop_max_nodes(1), explore_coeff: 2.0 };
        let o = search_world(&state, &teams, &Handcrafted, &OpenLoop, &open_p, 7, 0, None);
        let c2 = search_world_closed(&state, &teams, &Handcrafted, &closed_p, 7);
        let ob = best(&o.s1).action; let cb = best(&c2.s1).action;
        if ob == c.correct_action { open_ok += 1; }
        if cb == c.correct_action { closed_ok += 1; }
        println!("[{}] src={} correct={} open={} closed={}", c.name, c.source, c.correct_action, ob, cb);
    }
    let n = suite.cases.len() as u32;
    println!("oracle agreement: open {}/{}  closed {}/{}", open_ok, n, closed_ok, n);
    println!("flip_eligible_oracle: {}", closed_ok > open_ok); // strict > is the C2/C4 flip gate
    assert!(closed_ok >= open_ok, "closed-loop must not REGRESS oracle agreement (flip eligibility requires strict >, checked at the C2/C4 gates)");
}

fn main() {
    if std::env::args().any(|a| a == "--suite") { run_suite(); return; }
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
        let closed_params = SearchParams { time_ms: 100, max_iters: u64::MAX, max_nodes: poke_mcts::search::closed_loop_max_nodes(1), ..Default::default() };
        let seed = splitmix64(i as u64 ^ 0x5EED);
        let open = search_world(&state, &teams, &Handcrafted, &OpenLoop, &open_params, seed, 0, None);
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
