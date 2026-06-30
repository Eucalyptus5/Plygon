use crate::eval::{winner_value, Evaluator};
use crate::node::{child_key, Node, NO_CHILD};
use crate::rng::Lcg;
use crate::search::{arm0, harvest, leaf, pick, SearchParams, SearchResult};
use pkmn_engine::state::*;
use std::collections::HashMap;
use std::time::Instant;

const K_SAMPLES: u32 = 12;

struct Outcome { state: BattleState, count: u32, child: u32 }
struct Edge { outcomes: Vec<Outcome> }

pub fn search_world_closed(
    root_state: &BattleState,
    teams: &TeamData,
    evaluator: &impl Evaluator,
    params: &SearchParams,
    seed: u64,
) -> SearchResult {
    let mut rng = Lcg::new(seed);
    let mut tree: Vec<Node> = vec![Node::from_state(root_state)];
    let mut states: Vec<BattleState> = vec![*root_state]; // closed-loop nodes own a state
    let mut edges: HashMap<(u32, u16), Edge> = HashMap::new();
    if root_state.is_game_over() || (tree[0].s1.is_empty() && tree[0].s2.is_empty()) {
        return harvest(&tree[0], 0, 0, 0);
    }
    let root_eval = evaluator.eval(root_state);
    let start = Instant::now();
    let mut iters = 0u64;
    let mut path: Vec<(usize, u8, u8)> = Vec::with_capacity(64);

    while iters < params.max_iters {
        if iters % 256 == 0 && iters > 0 && start.elapsed().as_millis() as u64 >= params.time_ms { break; }
        path.clear();
        let mut idx = 0usize;
        let value: f64;
        loop {
            let node = &tree[idx];
            let (a1, b1) = pick(&node.s1, node.visits);
            let (a2, b2) = pick(&node.s2, node.visits);
            if a1 == 255 && a2 == 255 { value = leaf(&states[idx], evaluator, root_eval); break; }
            let key = (idx as u32, child_key(arm0(a1), arm0(a2)) as u16);
            path.push((idx, a1, a2));
            let edge = edges.entry(key).or_insert_with(|| {
                // first visit: empirically enumerate K sampled futures
                let mut outcomes: Vec<Outcome> = Vec::new();
                for _ in 0..K_SAMPLES {
                    let mut s = states[idx];
                    match s.phase {
                        PHASE_ACTIONS => execute_turn(&mut s, teams, b1, b2, &mut |m| rng.roll(m)),
                        PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH =>
                            execute_switch_turn(&mut s, teams, b1, b2, &mut |m| rng.roll(m)),
                        _ => {}
                    }
                    match outcomes.iter_mut().find(|o| o.state == s) {
                        Some(o) => o.count += 1,
                        None => outcomes.push(Outcome { state: s, count: 1, child: NO_CHILD }),
                    }
                }
                Edge { outcomes }
            });
            // weighted re-sample among persistent outcomes
            let total: u32 = edge.outcomes.iter().map(|o| o.count).sum();
            let mut r = rng.roll(total);
            let mut oi = edge.outcomes.len() - 1;
            for (i, o) in edge.outcomes.iter().enumerate() {
                if r < o.count { oi = i; break; }
                r -= o.count;
            }
            let outcome_state = edge.outcomes[oi].state;
            if edge.outcomes[oi].child == NO_CHILD {
                value = if outcome_state.is_game_over() { winner_value(&outcome_state) }
                        else { leaf(&outcome_state, evaluator, root_eval) };
                if (tree.len() as u32) < params.max_nodes && !outcome_state.is_game_over() {
                    tree.push(Node::from_state(&outcome_state));
                    states.push(outcome_state);
                    let id = (tree.len() - 1) as u32;
                    edges.get_mut(&key).unwrap().outcomes[oi].child = id;
                }
                break;
            }
            if outcome_state.is_game_over() { value = winner_value(&outcome_state); break; }
            idx = edge.outcomes[oi].child as usize;
        }
        for &(n, a1, a2) in &path {
            let node = &mut tree[n];
            node.visits += 1;
            if a1 != 255 { let a = &mut node.s1.arms[a1 as usize]; a.visits += 1; a.total_score += value; }
            if a2 != 255 { let a = &mut node.s2.arms[a2 as usize]; a.visits += 1; a.total_score += 1.0 - value; }
        }
        iters += 1;
    }
    harvest(&tree[0], iters, 0, 0)
}
