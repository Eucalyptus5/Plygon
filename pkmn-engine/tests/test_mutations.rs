use pkmn_engine::state::BattleState;
use pkmn_engine::state::mutations::*;
use pkmn_engine::state::zobrist::{ZobristKeys, validate_hash};
use pkmn_engine::state::structs::{VOL_TRANSFORMED, VOL_TYPES_OVERRIDDEN};

fn setup() -> (BattleState, ZobristKeys) {
    let mut state = BattleState::default();
    let keys = ZobristKeys::new(42);
    // Initialize Zobrist hash
    state.zobrist = pkmn_engine::state::zobrist::compute_full_hash(&state, &keys);
    
    // Setup a basic mon
    state.sides[0].team[0].species_id = 1; // Must be > 0 to be included in full hash
    state.sides[0].team[0].current_hp = 300;
    state.sides[0].team[0].max_hp = 300;
    state.sides[0].team[0].item_id = 242; // Leftovers
    state.sides[0].team[0].status = 0;
    state.sides[0].team[0].pp = [10, 10, 10, 10];
    
    // Recompute since we modified directly without mutations
    state.zobrist = pkmn_engine::state::zobrist::compute_full_hash(&state, &keys);
    
    (state, keys)
}

#[test]
fn test_deal_damage() {
    let (mut state, keys) = setup();
    
    deal_damage(&mut state, &keys, 0, 0, 50);
    assert_eq!(state.sides[0].team[0].current_hp, 250);
    assert!(validate_hash(&state, &keys));
    
    // Floor at 0
    deal_damage(&mut state, &keys, 0, 0, 500);
    assert_eq!(state.sides[0].team[0].current_hp, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_heal() {
    let (mut state, keys) = setup();
    
    state.sides[0].team[0].current_hp = 100;
    state.zobrist = pkmn_engine::state::zobrist::compute_full_hash(&state, &keys);
    
    heal(&mut state, &keys, 0, 0, 50);
    assert_eq!(state.sides[0].team[0].current_hp, 150);
    assert!(validate_hash(&state, &keys));
    
    // Cap at max_hp
    heal(&mut state, &keys, 0, 0, 500);
    assert_eq!(state.sides[0].team[0].current_hp, 300);
    assert!(validate_hash(&state, &keys));
    
    // Heal from 0
    state.sides[0].team[0].current_hp = 0;
    state.zobrist = pkmn_engine::state::zobrist::compute_full_hash(&state, &keys);
    heal(&mut state, &keys, 0, 0, 50);
    assert_eq!(state.sides[0].team[0].current_hp, 50);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_deal_proportional_damage() {
    let (mut state, keys) = setup();
    
    deal_proportional_damage(&mut state, &keys, 0, 0, 1, 16);
    // 300 / 16 = 18
    assert_eq!(state.sides[0].team[0].current_hp, 282);
    assert!(validate_hash(&state, &keys));
    
    // Minimum 1 damage
    state.sides[0].team[0].max_hp = 10;
    state.sides[0].team[0].current_hp = 10;
    state.zobrist = pkmn_engine::state::zobrist::compute_full_hash(&state, &keys);
    
    deal_proportional_damage(&mut state, &keys, 0, 0, 1, 16);
    assert_eq!(state.sides[0].team[0].current_hp, 9);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_set_status() {
    let (mut state, keys) = setup();
    
    assert!(set_status(&mut state, &keys, 0, 0, 1, 0)); // Set Burn (or something)
    assert_eq!(state.sides[0].team[0].status, 1);
    assert!(validate_hash(&state, &keys));
    
    // Fails when already set
    assert!(!set_status(&mut state, &keys, 0, 0, 2, 0));
    assert_eq!(state.sides[0].team[0].status, 1); // Still 1
}

#[test]
fn test_clear_status() {
    let (mut state, keys) = setup();
    
    set_status(&mut state, &keys, 0, 0, 1, 5);
    clear_status(&mut state, &keys, 0, 0);
    assert_eq!(state.sides[0].team[0].status, 0);
    assert_eq!(state.sides[0].team[0].status_counter, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_apply_boost() {
    let (mut state, keys) = setup();
    
    // apply_boost(state, keys, side, stat_index, stages)
    let applied = apply_boost(&mut state, &keys, 0, 0, 4); // +4 Atk
    assert_eq!(applied, 4);
    assert_eq!(state.sides[0].active.boosts[0], 4);
    assert!(validate_hash(&state, &keys));
    
    let applied2 = apply_boost(&mut state, &keys, 0, 0, 4); // +4 more, but capped at +6
    assert_eq!(applied2, 2);
    assert_eq!(state.sides[0].active.boosts[0], 6);
    
    let applied3 = apply_boost(&mut state, &keys, 0, 0, -10); // +6 - 10 = -4
    assert_eq!(applied3, -10);
    assert_eq!(state.sides[0].active.boosts[0], -4);

    let applied4 = apply_boost(&mut state, &keys, 0, 0, -5); // -4 - 5 = -9, capped at -6
    assert_eq!(applied4, -2);
    assert_eq!(state.sides[0].active.boosts[0], -6);
}

#[test]
fn test_volatile() {
    let (mut state, keys) = setup();
    
    let flag = VOL_TYPES_OVERRIDDEN;
    set_volatile(&mut state, &keys, 0, flag);
    assert!(state.sides[0].active.has_volatile(flag));
    assert!(validate_hash(&state, &keys));
    
    let hash1 = state.zobrist;
    set_volatile(&mut state, &keys, 0, flag); // Already set, no change
    assert_eq!(state.zobrist, hash1);
    
    clear_volatile(&mut state, &keys, 0, flag);
    assert!(!state.sides[0].active.has_volatile(flag));
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_weather_terrain() {
    let (mut state, keys) = setup();
    
    set_weather(&mut state, &keys, 1, 5);
    assert_eq!(state.field.weather, 1);
    assert_eq!(state.field.weather_turns, 5);
    assert!(validate_hash(&state, &keys));
    
    clear_weather(&mut state, &keys);
    assert_eq!(state.field.weather, 0);
    assert_eq!(state.field.weather_turns, 0);
    assert!(validate_hash(&state, &keys));
    
    set_terrain(&mut state, &keys, 2, 5);
    assert_eq!(state.field.terrain, 2);
    assert_eq!(state.field.terrain_turns, 5);
    assert!(validate_hash(&state, &keys));
    
    clear_terrain(&mut state, &keys);
    assert_eq!(state.field.terrain, 0);
    assert_eq!(state.field.terrain_turns, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_trick_room_gravity() {
    let (mut state, keys) = setup();
    
    set_trick_room(&mut state, &keys, 5);
    assert_eq!(state.field.trick_room_turns, 5);
    assert!(validate_hash(&state, &keys));
    
    set_gravity(&mut state, &keys, 5);
    assert_eq!(state.field.gravity_turns, 5);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_consume_item() {
    let (mut state, keys) = setup();
    assert_eq!(state.sides[0].team[0].item_id, 242);
    
    consume_item(&mut state, &keys, 0, 0);
    assert_eq!(state.sides[0].team[0].item_id, 0);
    assert!(validate_hash(&state, &keys));
    
    let hash1 = state.zobrist;
    consume_item(&mut state, &keys, 0, 0); // already 0
    assert_eq!(state.zobrist, hash1);
}

#[test]
fn test_set_item() {
    let (mut state, keys) = setup();
    
    set_item(&mut state, &keys, 0, 0, 100);
    assert_eq!(state.sides[0].team[0].item_id, 100);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_deduct_pp() {
    let (mut state, _keys) = setup(); // deduct_pp doesn't mutate zobrist
    
    assert!(deduct_pp(&mut state, 0, 0, 1));
    assert_eq!(state.sides[0].team[0].pp[0], 9);
    
    // Try to deduct more than we have -> goes to 0, returns true
    assert!(deduct_pp(&mut state, 0, 0, 10));
    assert_eq!(state.sides[0].team[0].pp[0], 0);

    // Try to deduct when 0 -> returns false
    assert!(!deduct_pp(&mut state, 0, 0, 1));
    
    // VOL_TRANSFORMED -> deducts from override_pp
    state.sides[0].active.override_pp = [5, 5, 5, 5];
    state.sides[0].active.set_volatile(VOL_TRANSFORMED); // we don't care about Zobrist for deduct_pp here
    
    assert!(deduct_pp(&mut state, 0, 0, 1));
    assert_eq!(state.sides[0].active.override_pp[0], 4);
    assert_eq!(state.sides[0].team[0].pp[0], 0); // Original PP unchanged (was 0)
}
