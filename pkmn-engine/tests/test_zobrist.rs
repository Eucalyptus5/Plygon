use pkmn_engine::state::BattleState;
use pkmn_engine::state::zobrist::{ZobristKeys, compute_full_hash, validate_hash, hp_bucket};
use pkmn_engine::state::mutations::*;
use pkmn_engine::state::structs::VOL_TYPES_OVERRIDDEN;

fn setup() -> (BattleState, ZobristKeys) {
    let mut state = BattleState::default();
    let keys = ZobristKeys::new(42);
    
    state.sides[0].team[0].species_id = 1;
    state.sides[0].team[0].current_hp = 300;
    state.sides[0].team[0].max_hp = 300;
    state.sides[0].team[0].item_id = 242;
    state.sides[0].team[0].status = 0;
    state.sides[0].team[0].pp = [10, 10, 10, 10];
    
    state.zobrist = compute_full_hash(&state, &keys);
    
    (state, keys)
}

#[test]
fn test_deterministic() {
    let (state1, keys1) = setup();
    let mut state2 = state1.clone();
    let keys2 = ZobristKeys::new(42); // Same seed
    
    state2.zobrist = compute_full_hash(&state2, &keys2);
    assert_eq!(state1.zobrist, state2.zobrist);
}

#[test]
fn test_different_states_different_hashes() {
    let (state1, keys) = setup();
    
    let mut state2 = state1.clone();
    state2.sides[0].team[0].current_hp = 10;
    state2.zobrist = compute_full_hash(&state2, &keys);
    
    assert_ne!(state1.zobrist, state2.zobrist);
}

#[test]
fn test_incremental_matches_full() {
    let (mut state, keys) = setup();
    
    // Damage
    deal_damage(&mut state, &keys, 0, 0, 150);
    assert!(validate_hash(&state, &keys));
    
    // Heal
    heal(&mut state, &keys, 0, 0, 50);
    assert!(validate_hash(&state, &keys));
    
    // Status
    set_status(&mut state, &keys, 0, 0, 1, 0);
    assert!(validate_hash(&state, &keys));
    
    // Boost
    apply_boost(&mut state, &keys, 0, 1, 2);
    assert!(validate_hash(&state, &keys));
    
    // Volatile
    set_volatile(&mut state, &keys, 0, VOL_TYPES_OVERRIDDEN);
    assert!(validate_hash(&state, &keys));
    
    // Weather
    set_weather(&mut state, &keys, 1, 5);
    assert!(validate_hash(&state, &keys));
    
    // Terrain
    set_terrain(&mut state, &keys, 1, 5);
    assert!(validate_hash(&state, &keys));
    
    // Item
    consume_item(&mut state, &keys, 0, 0);
    assert!(validate_hash(&state, &keys));
    
    // Phase
    set_phase(&mut state, &keys, 2);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_100_step_mutation_chain() {
    let (mut state, keys) = setup();
    
    for i in 0..100 {
        match i % 5 {
            0 => deal_damage(&mut state, &keys, 0, 0, 10),
            1 => heal(&mut state, &keys, 0, 0, 5),
            2 => { set_weather(&mut state, &keys, (i % 4) as u8, 5); },
            3 => { apply_boost(&mut state, &keys, 0, (i % 5) as usize, 1); },
            4 => { set_phase(&mut state, &keys, (i % 4) as u8); },
            _ => unreachable!(),
        }
        assert!(validate_hash(&state, &keys), "Failed at step {}", i);
    }
}

#[test]
fn test_hp_buckets() {
    let max = 100;
    // Formula: ((hp * 8) / 101).min(7)
    // 0% -> 0/101 = 0
    // 12% -> 96/101 = 0
    // 13% -> 104/101 = 1
    // 50% -> 400/101 = 3
    // 87% -> 696/101 = 6
    // 88% -> 704/101 = 6
    // 89% -> 712/101 = 7
    // 100% -> 800/101 = 7
    
    assert_eq!(hp_bucket(0, max), 0);
    assert_eq!(hp_bucket(12, max), 0);
    assert_eq!(hp_bucket(13, max), 1);
    assert_eq!(hp_bucket(50, max), 3);
    assert_eq!(hp_bucket(87, max), 6);
    assert_eq!(hp_bucket(88, max), 6);
    assert_eq!(hp_bucket(89, max), 7);
    assert_eq!(hp_bucket(100, max), 7);
}

#[test]
fn test_xor_reversibility() {
    let (mut state, keys) = setup();
    let original_hash = state.zobrist;
    
    // Apply volatile
    set_volatile(&mut state, &keys, 0, VOL_TYPES_OVERRIDDEN);
    assert_ne!(state.zobrist, original_hash);
    
    // Unapply volatile
    clear_volatile(&mut state, &keys, 0, VOL_TYPES_OVERRIDDEN);
    assert_eq!(state.zobrist, original_hash);
}

#[test]
fn test_identical_states_same_hash() {
    let (mut state1, keys) = setup();
    
    // Mutate state1
    deal_damage(&mut state1, &keys, 0, 0, 100);
    set_weather(&mut state1, &keys, 1, 5);
    
    // Construct state2 from scratch with same values
    let mut state2 = BattleState::default();
    state2.sides[0].team[0].species_id = 1;
    state2.sides[0].team[0].current_hp = 200;
    state2.sides[0].team[0].max_hp = 300;
    state2.sides[0].team[0].item_id = 242;
    state2.sides[0].team[0].pp = [10, 10, 10, 10];
    state2.field.weather = 1;
    state2.field.weather_turns = 5;
    
    state2.zobrist = compute_full_hash(&state2, &keys);
    
    assert_eq!(state1.zobrist, state2.zobrist);
}
