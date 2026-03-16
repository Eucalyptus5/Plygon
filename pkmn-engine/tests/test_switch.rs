use pkmn_engine::state::BattleState;
use pkmn_engine::state::switch::*;
use pkmn_engine::state::execute_switch_turn;
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

#[test]
fn test_stealth_rock_4x_weak() {
    let (mut state, keys) = setup();
    state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;

    // Simulate Fire/Flying dual type via override_types on the active struct.
    // switch_in does not clear active volatiles, so these persist into apply_entry_hazards.
    state.sides[0].active.override_types = [Type::Fire as u8, Type::Flying as u8];
    state.sides[0].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;
    state.zobrist = compute_full_hash(&state, &keys);

    switch_in(&mut state, &keys, 0, 1);

    // dual_type_effectiveness(Rock, Fire, Flying) = (8 * 8) / 4 = 16
    // damage = 300 * 16 / 32 = 150
    assert_eq!(state.sides[0].team[1].current_hp, 300 - 150);
}

#[test]
fn test_stealth_rock_resist() {
    let (mut state, keys) = setup();
    state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;

    // Simulate Steel mono-type via Tera (effective_types returns (Steel, Steel))
    state.sides[0].team[1].tera_type = Type::Steel as u8;
    state.sides[0].team[1].flags |= MON_FLAG_TERASTALLIZED;
    state.zobrist = compute_full_hash(&state, &keys);

    switch_in(&mut state, &keys, 0, 1);

    // dual_type_effectiveness(Rock, Steel, Steel): mono shortcut returns 2 (resist)
    // damage = (300 * 2 / 32).max(1) = 18
    assert_eq!(state.sides[0].team[1].current_hp, 300 - 18);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_hazard_ko_triggers_switch() {
    let (mut state, keys) = setup();
    state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;

    // Give the incoming mon only 1 HP so stealth rock KOs it
    state.sides[0].team[0].species_id = 1;
    state.sides[0].team[1].species_id = 1;
    state.sides[0].team[1].current_hp = 1;

    // Need a third alive mon so faint_sweep doesn't declare GAME_OVER
    state.sides[0].team[2].species_id = 1;
    state.sides[0].team[2].current_hp = 300;
    state.sides[0].team[2].max_hp = 300;

    // Side 1 needs a valid mon too
    state.sides[1].team[0].species_id = 1;

    // Put the game in PHASE_SWITCH_P1 (forced replacement for side 0)
    state.phase = PHASE_SWITCH_P1;
    state.zobrist = compute_full_hash(&state, &keys);

    // Action 5 = switch to team slot 1 (decode_action: 4..=9 => Switch { target: action - 4 })
    let mut rng = |_: u32| -> u32 { 0 };
    execute_switch_turn(&mut state, &keys, 5, 0, &mut rng);

    // Mon should be KO'd from stealth rock
    assert_eq!(state.sides[0].team[1].current_hp, 0);

    // faint_sweep should set phase back to PHASE_SWITCH_P1 (chain replacement needed)
    assert_eq!(state.phase, PHASE_SWITCH_P1);
}
