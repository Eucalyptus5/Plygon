use poke_mcts::chance::OpenLoop;
use poke_mcts::eval::Handcrafted;
use poke_mcts::search::{search_world, SearchParams};
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
