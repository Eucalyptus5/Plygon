use pkmn_engine::state::BattleState;
use pkmn_engine::state::zobrist::*;
use pkmn_engine::state::mutations::*;
use pkmn_engine::state::switch::*;
use pkmn_engine::state::end_of_turn::*;
use pkmn_engine::state::legal_moves::*;
use pkmn_engine::state::calc::*;
use pkmn_engine::state::forme::*;
use pkmn_engine::state::structs::*;
use pkmn_engine::data::types::Type;

fn setup() -> (BattleState, ZobristKeys) {
    let mut state = BattleState::default();
    let keys = ZobristKeys::new(42);
    
    for i in 0..6 {
        state.sides[0].team[i].species_id = 1;
        state.sides[0].team[i].max_hp = 300;
        state.sides[0].team[i].current_hp = 300;
        state.sides[0].team[i].stats = [300, 100, 100, 100, 100];
        
        state.sides[1].team[i].species_id = 4;
        state.sides[1].team[i].max_hp = 300;
        state.sides[1].team[i].current_hp = 300;
        state.sides[1].team[i].stats = [300, 100, 100, 100, 100];
    }
    
    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

#[test]
fn test_copy_independence() {
    let (mut state, keys) = setup();
    let mut state_copy = state.clone();
    
    deal_damage(&mut state_copy, &keys, 0, 0, 50);
    
    assert_eq!(state.sides[0].team[0].current_hp, 300);
    assert_eq!(state_copy.sides[0].team[0].current_hp, 250);
}

#[test]
fn test_copy_independence_1000() {
    let (state, keys) = setup();
    let mut copies = vec![];
    for _ in 0..1000 {
        copies.push(state.clone());
    }
    
    for (i, copy) in copies.iter_mut().enumerate() {
        deal_damage(copy, &keys, 0, 0, (i % 100) as u16);
    }
    
    // Original unchanged
    assert_eq!(state.sides[0].team[0].current_hp, 300);
    // All copies divergent
    assert_eq!(copies[50].sides[0].team[0].current_hp, 250);
    assert_eq!(copies[99].sides[0].team[0].current_hp, 201);
}

#[test]
fn test_mcts_branch_pattern() {
    let (state, keys) = setup();
    
    let mut branch1 = state.clone();
    deal_damage(&mut branch1, &keys, 0, 0, 50);
    
    let mut branch2 = state.clone();
    // Apply boost instead of heal, since heal on full HP does nothing
    apply_boost(&mut branch2, &keys, 0, 1, 1);
    
    let mut branch3 = state.clone();
    set_status(&mut branch3, &keys, 0, 0, STATUS_BURN, 0);
    
    assert_ne!(state.zobrist, branch1.zobrist);
    assert_ne!(state.zobrist, branch2.zobrist);
    assert_ne!(state.zobrist, branch3.zobrist);
    
    assert_ne!(branch1.zobrist, branch2.zobrist);
    assert_ne!(branch2.zobrist, branch3.zobrist);
}

#[test]
fn test_zobrist_50_mutations() {
    let (mut state, keys) = setup();
    
    for i in 0..50 {
        match i % 5 {
            0 => deal_damage(&mut state, &keys, 0, 0, 5),
            1 => heal(&mut state, &keys, 1, 0, 2),
            2 => { set_status(&mut state, &keys, 0, 0, STATUS_POISON, 0); },
            3 => { apply_boost(&mut state, &keys, 0, 1, 1); },
            4 => { set_weather(&mut state, &keys, WEATHER_SAND, 5); },
            _ => (),
        }
        assert!(validate_hash(&state, &keys), "Failed at step {}", i);
    }
}

#[test]
fn test_full_turn_simulation() {
    let (mut state, keys) = setup();
    
    // Check legal actions
    let a1 = legal_actions(&state, 0);
    assert!(a1.count > 0);
    
    // Calc damage
    let res = calc_damage(&state, 0, 1, &mut |_| 1); // Pound
    
    // Apply damage
    deal_damage(&mut state, &keys, 1, 0, res.damage);
    
    // End of turn
    end_of_turn(&mut state, &keys);
    
    assert!(state.sides[1].team[0].current_hp < 300);
    assert_eq!(state.field.turn, 1);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_switch_hazard_eot_chain() {
    let (mut state, keys) = setup();
    
    state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
    state.sides[0].side_conditions.spikes = 1;
    state.field.weather = WEATHER_SAND;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);
    
    // Switch P1
    switch_out(&mut state, &keys, 0);
    assert!(validate_hash(&state, &keys));
    
    switch_in(&mut state, &keys, 0, 1);
    assert!(validate_hash(&state, &keys));
    
    // P1 took SR (37) and Spikes (37) -> 74 damage. HP = 226
    
    // End of turn
    end_of_turn(&mut state, &keys);
    assert!(validate_hash(&state, &keys));
    
    // Sand damage -> 18. HP = 208
    assert!(state.sides[0].team[1].current_hp < 226);
}

#[test]
fn test_faint_game_over_detection() {
    let (mut state, keys) = setup();
    
    deal_damage(&mut state, &keys, 0, 0, 999); // Lethal
    assert_eq!(state.sides[0].team[0].current_hp, 0);
    assert!(state.sides[0].team[0].is_fainted());
    
    for i in 1..6 {
        deal_damage(&mut state, &keys, 0, i, 999);
    }
    
    // Check available switches
    // We can't query `available_switches` directly unless it's public. Let's just check `legal_actions` in switch phase.
    state.phase = PHASE_SWITCH_P1;
    let a = legal_actions(&state, 0);
    assert_eq!(a.count, 0); // No switches available
}

#[test]
fn test_transform_roundtrip() {
    let (mut state, keys) = setup();
    state.sides[1].team[0].stats = [300, 200, 200, 200, 200];
    state.sides[1].team[0].moves = [1, 2, 0, 0];
    
    // Transform P1 -> P2
    apply_transform(&mut state, &keys, 0, 1);
    
    assert!(state.sides[0].active.has_volatile(VOL_TRANSFORMED));
    assert_eq!(state.sides[0].active.override_stats[1], 200);
    assert_eq!(state.sides[0].active.override_moves[0], 1);
    
    // Switch out
    switch_out(&mut state, &keys, 0);
    
    assert!(!state.sides[0].active.has_volatile(VOL_TRANSFORMED));
    // It's zeroed by switch_out
    assert_eq!(state.sides[0].active.override_stats[1], 0);
    assert_eq!(state.sides[0].active.override_moves[0], 0);
}

#[test]
fn test_weather_lifecycle() {
    let (mut state, keys) = setup();
    set_weather(&mut state, &keys, WEATHER_RAIN, 3);
    
    end_of_turn(&mut state, &keys);
    assert_eq!(state.field.weather_turns, 2);
    
    end_of_turn(&mut state, &keys);
    assert_eq!(state.field.weather_turns, 1);
    
    end_of_turn(&mut state, &keys);
    assert_eq!(state.field.weather, 0);
    assert_eq!(state.field.weather_turns, 0);
}

#[test]
fn test_perish_song_lifecycle() {
    let (mut state, keys) = setup();
    state.sides[0].active.set_volatile(VOL_PERISH_SONG);
    state.sides[0].active.perish_count = 3;
    state.zobrist = compute_full_hash(&state, &keys);
    
    end_of_turn(&mut state, &keys);
    assert_eq!(state.sides[0].active.perish_count, 2);
    assert_eq!(state.sides[0].team[0].current_hp, 300);
    
    end_of_turn(&mut state, &keys);
    assert_eq!(state.sides[0].active.perish_count, 1);
    
    end_of_turn(&mut state, &keys);
    assert_eq!(state.sides[0].active.perish_count, 0);
    assert_eq!(state.sides[0].team[0].current_hp, 300); // Not fainted yet!
    
    end_of_turn(&mut state, &keys);
    assert_eq!(state.sides[0].team[0].current_hp, 0); // Fainted
}

#[test]
fn test_status_persistence() {
    let (mut state, keys) = setup();
    set_status(&mut state, &keys, 0, 0, STATUS_BURN, 0);
    
    switch_out(&mut state, &keys, 0);
    switch_in(&mut state, &keys, 0, 1);
    switch_out(&mut state, &keys, 0); // Active is 1
    switch_in(&mut state, &keys, 0, 0); // Back to 0
    
    assert_eq!(state.sides[0].team[0].status, STATUS_BURN);
}

#[test]
fn test_volatile_cleared_on_switch() {
    let (mut state, keys) = setup();
    apply_boost(&mut state, &keys, 0, 1, 6);
    assert_eq!(state.sides[0].active.boosts[1], 6);
    
    switch_out(&mut state, &keys, 0);
    switch_in(&mut state, &keys, 0, 1);
    
    assert_eq!(state.sides[0].active.boosts[1], 0); // Boosts cleared for new mon
}
