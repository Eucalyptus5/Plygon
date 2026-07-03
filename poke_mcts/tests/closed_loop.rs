use poke_mcts::belief::Belief;
use poke_mcts::chance::OpenLoop;
use poke_mcts::chance_closed::{last_max_outcomes_per_edge, merge_centroid_hp, search_world_closed, signature};
use poke_mcts::determinize::{Observation, RandomBattle};
use poke_mcts::driver::{choose_action, PimcConfig};
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

#[test]
fn driver_runs_both_modes_and_is_reproducible() {
    let (s, t) = build_state(
        vec![mon(25, 9, [85, 150, 0, 0]), mon(143, 47, [34, 0, 0, 0])],
        vec![mon(445, 24, [89, 14, 0, 0]), mon(130, 22, [57, 0, 0, 0])],
    );
    let mut belief = Belief::default();
    belief.note_species(445, s.sides[1].team[0].level);
    let obs = Observation { state: &s, our_side: 0, teams: &t };
    for mode in [ChanceMode::OpenLoop, ChanceMode::ClosedLoop] {
        let cfg = PimcConfig {
            num_worlds: 4, time_ms_per_world: 1000, max_iters_per_world: 1500,
            seed: 9, chance_mode: mode,
        };
        let a = choose_action(&obs, &belief, &RandomBattle, &cfg);
        let b = choose_action(&obs, &belief, &RandomBattle, &cfg);
        assert_eq!(a, b, "{mode:?} not reproducible under seed");
        assert!(legal_actions(&s, 0).as_slice().contains(&a));
    }
}

#[test]
fn signature_merges_damage_rolls_but_splits_ko() {
    // Two states identical except a small HP delta within one band => same signature.
    let (mut a, _t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
    let mut b = a;
    let max = a.sides[1].team[0].max_hp;
    a.sides[1].team[0].current_hp = max;            // 100%
    b.sides[1].team[0].current_hp = max - max / 50; // ~98%, same band
    assert_eq!(signature(&a), signature(&b), "small HP delta must merge");

    // A KO (alive-count change) must NOT merge with a survivor.
    let mut k = a;
    k.sides[1].team[0].current_hp = 0;
    assert_ne!(signature(&a), signature(&k), "KO must split");
}

#[test]
fn centroid_is_count_weighted_mean() {
    // existing outcome: HP=80 over count=3; new sample HP=100 => mean = (80*3+100)/4 = 85
    assert_eq!(merge_centroid_hp(80, 3, 100), 85);
    assert_eq!(merge_centroid_hp(50, 1, 50), 50);
}

#[test]
fn closed_loop_outcomes_per_edge_are_bounded() {
    // A damaging-move position with crit + damage-roll spread: K_SAMPLES=12 exact draws produce
    // many distinct full-state HP outcomes; signature/band dedup must collapse same-band rolls.
    let (s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 58, 0, 0]));
    let p = SearchParams { time_ms: 600_000, max_iters: 3000,
        max_nodes: closed_loop_max_nodes(1), ..Default::default() };
    let _ = search_world_closed(&s, &t, &Handcrafted, &p, 7);
    let n = last_max_outcomes_per_edge();
    assert!(n >= 1, "no outcomes recorded");
    // Exact dedup over 12 draws on this damaging edge gives >6 distinct HP states; band-centroid
    // signature dedup must collapse them to a handful of strategically-distinct buckets.
    assert!(n <= 6, "signature dedup failed to collapse damage rolls: {} (exact would give up to {})", n, 12);
}
