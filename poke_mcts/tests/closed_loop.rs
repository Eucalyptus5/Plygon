use poke_mcts::chance::OpenLoop;
use poke_mcts::eval::Handcrafted;
use poke_mcts::node::CNode;
use poke_mcts::search::{closed_loop_max_nodes, search_world, ChanceMode, SearchParams};
use poke_mcts::testutil::*;
use pkmn_engine::state::*;

// Pins the exact open-loop result for a fixed scenario+seed+iter cap.
// If this changes, the C0 refactor broke open-loop.
#[test]
fn open_loop_parity_snapshot() {
    let (s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 58, 0, 0]));
    let p = SearchParams { time_ms: 600_000, max_iters: 5000, ..Default::default() };
    let r = search_world(&s, &t, &Handcrafted, &OpenLoop, &p, 7);
    let got: Vec<(u8, u32, f64)> = r.s1.iter().map(|a| (a.action, a.visits, a.avg_score)).collect();
    let expected: Vec<(u8, u32, f64)> = vec![
        (0, 2500, 0.00035909845679492325),
        (1, 2500, 0.00035874445799163873),
    ];
    assert_eq!(got, expected, "open-loop result drifted");
    assert_eq!(r.iterations, 5000);
}

#[test]
fn chance_mode_default_is_open_loop() {
    assert_eq!(ChanceMode::default(), ChanceMode::OpenLoop);
}

#[test]
fn closed_loop_cap_shrinks_with_more_worlds() {
    // RAM-derived: more concurrent worlds => fewer nodes per world.
    let c8 = closed_loop_max_nodes(8);
    let c16 = closed_loop_max_nodes(16);
    assert!(c8 > c16, "{c8} !> {c16}");
    assert!(c16 >= 50_000, "cap too small to search usefully: {c16}");
    // 8 worlds must fit the ceiling: 8 * cap * ~1.1KB <= 2GB
    assert!(8 * c8 as u64 * 1100 <= 2_000_000_000);
}

#[test]
fn cnode_owns_state_and_builds_bandits() {
    let (s, _t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
    let n = CNode::from_state(&s);
    assert_eq!(n.state.phase, PHASE_ACTIONS);
    assert!(!n.s1.is_empty() && !n.s2.is_empty());
    assert_eq!(n.visits, 0);
}
