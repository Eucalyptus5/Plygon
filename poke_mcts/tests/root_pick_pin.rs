use pkmn_engine::state::BattleState;
use poke_mcts::audit_snapshot::read_search_states;
use poke_mcts::belief::Belief;
use poke_mcts::determinize::{Observation, RandomBattle};
use poke_mcts::driver::{choose_action, choose_action_traced, PickMode, PimcConfig};
use poke_mcts::search::ChanceMode;

const STATES_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/search_states.bin");
const STATES: usize = 16;
const ITERS: u64 = 2048;

// what the opponent has shown of itself: the active mon's species and its move set
fn belief_of(state: &BattleState, opp: usize) -> Belief {
    let mut b = Belief::default();
    let mon = state.active_mon(opp);
    let slot = b.note_species(mon.species_id, mon.level);
    for &m in mon.moves.iter().filter(|&&m| m != 0) {
        b.note_move(slot, m);
    }
    b
}

fn cfg(seed: u64) -> PimcConfig {
    PimcConfig {
        num_worlds: 8,
        time_ms_per_world: 600_000,
        max_iters_per_world: ITERS,
        seed,
        chance_mode: ChanceMode::OpenLoop,
        pick_mode: PickMode::Argmax,
        filter_threshold: 0.75,
        raw_root: false,
        explore_coeff: 0.49,
        value_temp: 1.0,
        blind_opponent: false,
    }
}

fn digest(bytes: impl Iterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn picks_and_visit_digests() -> (Vec<u8>, Vec<u64>) {
    let states = read_search_states(STATES_PATH).expect("search_states.bin");
    assert!(states.len() >= STATES, "fixture holds {} states", states.len());
    let mut picks = Vec::with_capacity(STATES);
    let mut visits = Vec::with_capacity(STATES);
    for (i, (s, t)) in states.iter().take(STATES).enumerate() {
        let obs = Observation { state: s, teams: t, our_side: 0 };
        let belief = belief_of(s, 1);
        let c = cfg(0x5EA4_C11D + i as u64);
        picks.push(choose_action(&obs, &belief, &RandomBattle, &c));
        let tr = choose_action_traced(&obs, &belief, &RandomBattle, &c);
        assert_eq!(tr.picked, picks[i], "state {i}: the traced mirror must reproduce the pick");
        visits.push(digest(tr.per_world.iter().flat_map(|w| {
            w.arms.iter().chain(w.s2.iter()).flat_map(|a| {
                std::iter::once(a.action).chain(a.visits.to_le_bytes())
            })
        })));
    }
    println!("picks: {picks:?}");
    println!("visits: {}", visits.iter().map(|v| format!("0x{v:016x}")).collect::<Vec<_>>().join(", "));
    (picks, visits)
}

#[test]
fn root_pick_and_visit_counts_are_frozen() {
    let (picks, visits) = picks_and_visit_digests();
    assert_eq!(picks, EXPECTED_PICKS, "the root pick moved");
    assert_eq!(visits, EXPECTED_VISITS, "a root bandit's visit counts moved");
}

// The pins below were captured with no histogram in the search, so reproducing them in a build
// that fills one on every iteration is the histogram-off-versus-on comparison.
#[cfg(feature = "train_value")]
#[test]
fn the_leaf_histogram_fills_without_moving_the_pick() {
    use poke_mcts::chance::OpenLoop;
    use poke_mcts::eval::Handcrafted;
    use poke_mcts::search::{search_world, SearchParams};

    let states = read_search_states(STATES_PATH).expect("search_states.bin");
    let params = SearchParams { max_iters: ITERS, time_ms: 600_000, explore_coeff: 0.49, ..Default::default() };
    for (i, (s, t)) in states.iter().take(STATES).enumerate() {
        let r = search_world(s, t, &Handcrafted, &OpenLoop, &params, 0x5EA4_C11D + i as u64, 0, None);
        let leaves: u64 = r.leaf_hist.iter().map(|&c| c as u64).sum();
        assert_eq!(r.iterations, ITERS, "state {i}: the search stopped short");
        assert_eq!(leaves, r.iterations, "state {i}: {leaves} binned leaves over {} iterations", r.iterations);
    }
    let (picks, visits) = picks_and_visit_digests();
    assert_eq!(picks, EXPECTED_PICKS, "the histogram moved the root pick");
    assert_eq!(visits, EXPECTED_VISITS, "the histogram moved a root bandit's visit counts");
}

const EXPECTED_PICKS: [u8; STATES] = [10, 3, 3, 1, 3, 3, 0, 13, 1, 8, 9, 3, 1, 3, 10, 6];
const EXPECTED_VISITS: [u64; STATES] = [
    0x7245cc27e7ccb8ad, 0x9aafeadbc43939e5, 0xad002f548edfac11, 0xa9a5643c0ab9d9ec,
    0xc1d09f3393a4724c, 0x64c5cc91ae2f1ab5, 0x9a2381d4ac85270d, 0x8be2f2d8992a529f,
    0x224a127ba0da64e8, 0x62968e30b4ed28ae, 0x72eb3746e359f22f, 0x407fb40e15f93f38,
    0xf46246c38847719e, 0x55eb429e52b15bff, 0x85c446aba544c8e1, 0x24a890d0d22f70c4,
];
