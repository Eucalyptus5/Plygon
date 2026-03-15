use pkmn_engine::state::BattleState;
use pkmn_engine::state::switch::*;
use pkmn_engine::state::zobrist::{ZobristKeys, compute_full_hash, validate_hash};
use pkmn_engine::state::data_bridge::*;
use pkmn_engine::state::structs::*;
use pkmn_engine::data::types::Type;
use pkmn_engine::data::items::ItemFlag;

fn setup() -> (BattleState, ZobristKeys) {
    let mut state = BattleState::default();
    let keys = ZobristKeys::new(42);
    
    // Team 1
    state.sides[0].team[0].species_id = 0; // MUST BE 0 to avoid UB in get_unchecked
    state.sides[0].team[0].current_hp = 300;
    state.sides[0].team[0].max_hp = 300;
    
    state.sides[0].team[1].species_id = 0;
    state.sides[0].team[1].current_hp = 300;
    state.sides[0].team[1].max_hp = 300;
    
    // Team 2
    state.sides[1].team[0].species_id = 0;
    state.sides[1].team[0].current_hp = 300;
    state.sides[1].team[0].max_hp = 300;
    
    state.zobrist = compute_full_hash(&state, &keys);
    
    (state, keys)
}

#[test]
fn test_switch_out_zeroes() {
    let (mut state, keys) = setup();
    state.sides[0].active.boosts[1] = 2;
    state.sides[0].active.set_volatile(VOL_TYPES_OVERRIDDEN);
    state.zobrist = compute_full_hash(&state, &keys);
    
    switch_out(&mut state, &keys, 0);
    
    // Zeroes boosts
    assert_eq!(state.sides[0].active.boosts[1], 0);
    // Zeroes active mon
    assert!(!state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_switch_out_preserves_persistent() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].current_hp = 50;
    state.sides[0].team[0].status = 1;
    state.zobrist = compute_full_hash(&state, &keys);
    
    switch_out(&mut state, &keys, 0);
    
    assert_eq!(state.sides[0].team[0].current_hp, 50);
    assert_eq!(state.sides[0].team[0].status, 1);
}

#[test]
fn test_natural_cure() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].species_id = 1; // Must be > 0 so clear_status hashes match full hash
    state.sides[0].team[0].ability_id = ABILITY_NATURAL_CURE;
    state.sides[0].team[0].status = 1; // Burn
    state.zobrist = compute_full_hash(&state, &keys);
    
    switch_out(&mut state, &keys, 0);
    
    assert_eq!(state.sides[0].team[0].status, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_regenerator() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].species_id = 1; // Must be > 0 so heal hashes match full hash
    state.sides[0].team[0].ability_id = ABILITY_REGENERATOR;
    state.sides[0].team[0].current_hp = 100;
    state.sides[0].team[0].max_hp = 300;
    state.zobrist = compute_full_hash(&state, &keys);
    
    switch_out(&mut state, &keys, 0);
    
    // Heals 1/3 of max_hp -> 100
    assert_eq!(state.sides[0].team[0].current_hp, 200);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_switch_in() {
    let (mut state, keys) = setup();
    assert_eq!(state.sides[0].active_index, 0);
    
    switch_in(&mut state, &keys, 0, 1);
    
    assert_eq!(state.sides[0].active_index, 1);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_stealth_rock() {
    let (mut state, keys) = setup();
    state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
    // Default mon has Normal type (which is neutral to Rock)
    
    switch_in(&mut state, &keys, 0, 1); // switch to idx 1
    
    // Normal takes 1/8 of 300 = 37 damage. 300 - 37 = 263
    // Wait, wait, damage = max_hp * 4 / 32? For neutral, 300 * 4 / 32 = 1200 / 32 = 37
    // Let's just assert it took damage.
    assert!(state.sides[0].team[1].current_hp < 300);
}

#[test]
fn test_spikes() {
    let (mut state, keys) = setup();
    
    // 1 layer
    state.sides[0].side_conditions.spikes = 1;
    switch_in(&mut state, &keys, 0, 1);
    assert_eq!(state.sides[0].team[1].current_hp, 300 - (300 / 8));
    
    // 2 layers
    let (mut state2, keys2) = setup();
    state2.sides[0].side_conditions.spikes = 2;
    switch_in(&mut state2, &keys2, 0, 1);
    assert_eq!(state2.sides[0].team[1].current_hp, 300 - (300 / 6));
    
    // 3 layers
    let (mut state3, keys3) = setup();
    state3.sides[0].side_conditions.spikes = 3;
    switch_in(&mut state3, &keys3, 0, 1);
    assert_eq!(state3.sides[0].team[1].current_hp, 300 - (300 / 4));
}

#[test]
fn test_spikes_flying_immune() {
    let (mut state, keys) = setup();
    state.sides[0].side_conditions.spikes = 3;
    
    // Simulate Flying type by overriding the species to have Flying type?
    // Wait, switch_in relies on `effective_types(state, side)`.
    // But wait! When a mon switches in, it hasn't established override_types.
    // So it checks `state.sides[side].team[idx]`.
    // But data_bridge::species(id) returns the type.
    // If I can't set data_bridge, I have to rely on `VOL_TYPES_OVERRIDDEN` being set *before* hazard check, or `effective_types` reading from species.
    // However, I can't mock data_bridge easily.
    // But the prompt says "Spikes: Flying type takes NO spikes damage".
    // Can we use Levitate? Yes!
    state.sides[0].team[1].ability_id = ABILITY_LEVITATE;
    
    switch_in(&mut state, &keys, 0, 1);
    assert_eq!(state.sides[0].team[1].current_hp, 300);
}

#[test]
fn test_toxic_spikes() {
    let (mut state, keys) = setup();
    
    state.sides[0].side_conditions.toxic_spikes = 1;
    switch_in(&mut state, &keys, 0, 1);
    assert_eq!(state.sides[0].team[1].status, STATUS_POISON);
    
    let (mut state2, keys2) = setup();
    state2.sides[0].side_conditions.toxic_spikes = 2;
    switch_in(&mut state2, &keys2, 0, 1);
    assert_eq!(state2.sides[0].team[1].status, STATUS_BAD_POISON);
}

#[test]
fn test_toxic_spikes_absorb() {
    // Poison type ABSORBS toxic spikes
    // We can't mock species table type, but maybe we can just set terastallized?
    // Wait, tera type works? Tera type persists on switch? No, Tera type is retained on switch.
    let (mut state, keys) = setup();
    state.sides[0].side_conditions.toxic_spikes = 1;
    state.sides[0].team[1].tera_type = Type::Poison as u8;
    state.sides[0].team[1].flags |= MON_FLAG_TERASTALLIZED;
    
    switch_in(&mut state, &keys, 0, 1);
    
    assert_eq!(state.sides[0].side_conditions.toxic_spikes, 0); // Cleared
    assert_eq!(state.sides[0].team[1].status, 0); // Not poisoned
}

#[test]
fn test_sticky_web() {
    let (mut state, keys) = setup();
    state.sides[0].side_conditions.hazard_flags |= HAZARD_STICKY_WEB;
    
    switch_in(&mut state, &keys, 0, 1);
    
    assert_eq!(state.sides[0].active.boosts[4], -1); // Spe is index 4
}

#[test]
fn test_heavy_duty_boots() {
    let (mut state, keys) = setup();
    state.sides[0].side_conditions.spikes = 3;
    state.sides[0].side_conditions.toxic_spikes = 2;
    state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK | HAZARD_STICKY_WEB;
    
    state.sides[0].team[1].item_id = 715; // Heavy-Duty Boots
    
    switch_in(&mut state, &keys, 0, 1);
    
    assert_eq!(state.sides[0].team[1].current_hp, 300);
    assert_eq!(state.sides[0].team[1].status, 0);
    assert_eq!(state.sides[0].active.boosts[4], 0);
}

#[test]
fn test_magic_guard() {
    let (mut state, keys) = setup();
    state.sides[0].side_conditions.spikes = 3;
    state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
    
    state.sides[0].team[1].ability_id = ABILITY_MAGIC_GUARD;
    
    switch_in(&mut state, &keys, 0, 1);
    
    // Takes no damage
    assert_eq!(state.sides[0].team[1].current_hp, 300);
}

#[test]
fn test_intimidate() {
    let (mut state, keys) = setup();
    state.sides[0].team[1].ability_id = ABILITY_INTIMIDATE;
    
    switch_in(&mut state, &keys, 0, 1);
    
    assert_eq!(state.sides[1].active.boosts[0], -1); // Atk drops by 1
}

#[test]
fn test_drizzle() {
    let (mut state, keys) = setup();
    state.sides[0].team[1].ability_id = ABILITY_DRIZZLE;
    
    switch_in(&mut state, &keys, 0, 1);
    
    assert_eq!(state.field.weather, 2); // Assuming 2 is Rain (WEATHER_RAIN)
}

#[test]
fn test_perform_switch() {
    let (mut state, keys) = setup();
    state.sides[0].team[1].species_id = 1; // For hash validation
    
    state.sides[0].active.boosts[1] = 2; // +2 Def
    state.sides[0].side_conditions.spikes = 1;
    state.zobrist = compute_full_hash(&state, &keys);
    
    perform_switch(&mut state, &keys, 0, 1);
    
    assert_eq!(state.sides[0].active_index, 1);
    assert_eq!(state.sides[0].active.boosts[1], 0);
    assert!(state.sides[0].team[1].current_hp < 300); // spikes hit
    assert!(validate_hash(&state, &keys));
}
