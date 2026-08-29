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

#[test]
fn root_pick_and_visit_counts_are_frozen() {
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
    assert_eq!(picks, EXPECTED_PICKS, "the root pick moved");
    assert_eq!(visits, EXPECTED_VISITS, "a root bandit's visit counts moved");
}

const EXPECTED_PICKS: [u8; STATES] = [0, 3, 1, 1, 2, 3, 6, 7, 7, 8, 9, 3, 3, 1, 0, 7];
const EXPECTED_VISITS: [u64; STATES] = [
    0x1c99506c4457111e, 0xebcaac6642828af5, 0x4217cd3a2af2acc7, 0x5675006fdd3052a6,
    0xb7b0b954a56bbb9f, 0xc07717fcbbb29941, 0xca03f4fa30d66ae0, 0x22a8adc23e664111,
    0xb0c27d1bbc8cd773, 0xbb2f2011625d8270, 0xa7c584d96766d58f, 0x40b7cd442235543a,
    0xea64212433642713, 0xb0b26fb333378595, 0xceaf87f6e4084f96, 0xb56191aa23e11ab9,
];
