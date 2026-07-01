use crate::eval::{winner_value, Evaluator};
use crate::node::{child_key, CNode, NO_CHILD};
use crate::rng::Lcg;
use crate::search::{arm0, harvest_bandits, leaf, pick, SearchParams, SearchResult};
use pkmn_engine::state::*;
use std::collections::HashMap;
use std::time::Instant;

const K_SAMPLES: u32 = 12;

const HP_BANDS: u16 = 16; // band width ~6.25% max-HP; keeps within-band eval drift small (spec §6)

#[inline]
pub fn hp_band(cur: u16, max: u16) -> u8 {
    if max == 0 { return 0; }
    ((cur as u32 * HP_BANDS as u32) / max as u32).min(HP_BANDS as u32 - 1) as u8
}

// Strategically-meaningful categorical fingerprint of a state. Two samples with equal
// Signature merge into one outcome (their exact HP differences are collapsed to a band).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Signature {
    phase: u8,
    alive: [u8; 2],
    active_species: [u16; 2],
    active_hp_band: [u8; 2],
    status: [u8; 2],
    active_idx: [u8; 2],
}

pub fn signature(s: &BattleState) -> Signature {
    let mut sig = Signature {
        phase: s.phase,
        alive: [0; 2], active_species: [0; 2], active_hp_band: [0; 2],
        status: [0; 2], active_idx: [0; 2],
    };
    for side in 0..2 {
        let st = &s.sides[side];
        sig.alive[side] = (0..6).filter(|&i| st.team[i].species_id != 0 && st.team[i].current_hp > 0).count() as u8;
        let am = &st.team[st.active_index as usize];
        sig.active_species[side] = am.species_id;
        sig.active_hp_band[side] = hp_band(am.current_hp, am.max_hp);
        sig.status[side] = am.status;
        sig.active_idx[side] = st.active_index;
    }
    sig
}

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
    let mut tree: Vec<CNode> = vec![CNode::from_state(root_state)];
    let mut edges: HashMap<(u32, u16), Edge> = HashMap::new();
    if root_state.is_game_over() || (tree[0].s1.is_empty() && tree[0].s2.is_empty()) {
        return harvest_bandits(&tree[0].s1, &tree[0].s2, 0, 0, 0);
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
            if a1 == 255 && a2 == 255 { value = leaf(&tree[idx].state, evaluator, root_eval); break; }
            let key = (idx as u32, child_key(arm0(a1), arm0(a2)) as u16);
            path.push((idx, a1, a2));
            let edge = edges.entry(key).or_insert_with(|| {
                // first visit: empirically enumerate K sampled futures
                let mut outcomes: Vec<Outcome> = Vec::new();
                for _ in 0..K_SAMPLES {
                    let mut s = tree[idx].state;
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
                    tree.push(CNode::from_state(&outcome_state));
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
    harvest_bandits(&tree[0].s1, &tree[0].s2, iters, 0, 0)
}
