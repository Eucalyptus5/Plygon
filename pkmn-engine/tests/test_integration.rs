use pkmn_engine::state::BattleState;
use pkmn_engine::state::zobrist::*;
use pkmn_engine::state::mutations::*;
use pkmn_engine::state::switch::*;
use pkmn_engine::state::end_of_turn::*;
use pkmn_engine::state::legal_moves::*;
use pkmn_engine::state::calc::*;
use pkmn_engine::state::forme::*;
use pkmn_engine::state::structs::*;
use pkmn_engine::state::move_exec::execute_move;
use pkmn_engine::state::turn::{execute_turn, execute_switch_turn};
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
    
    assert_eq!(state.sides[0].team[0].current_hp, 300);
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
    
    let a1 = legal_actions(&state, 0);
    assert!(a1.count > 0);

    let res = calc_damage(&state, 0, 1, &mut |_| 1); // Pound
    deal_damage(&mut state, &keys, 1, 0, res.damage);
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
    
    switch_out(&mut state, &keys, 0);
    assert!(validate_hash(&state, &keys));
    
    switch_in(&mut state, &keys, 0, 1);
    assert!(validate_hash(&state, &keys));
    
    // SR (37) + Spikes (37) = 74 damage, then sand (18) at EOT
    end_of_turn(&mut state, &keys);
    assert!(validate_hash(&state, &keys));
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
    
    state.phase = PHASE_SWITCH_P1;
    let a = legal_actions(&state, 0);
    assert_eq!(a.count, 0); // No switches available
}

#[test]
fn test_transform_roundtrip() {
    let (mut state, keys) = setup();
    state.sides[1].team[0].stats = [300, 200, 200, 200, 200];
    state.sides[1].team[0].moves = [1, 2, 0, 0];
    
    apply_transform(&mut state, &keys, 0, 1);
    
    assert!(state.sides[0].active.has_volatile(VOL_TRANSFORMED));
    assert_eq!(state.sides[0].active.override_stats[1], 200);
    assert_eq!(state.sides[0].active.override_moves[0], 1);
    
    switch_out(&mut state, &keys, 0);

    assert!(!state.sides[0].active.has_volatile(VOL_TRANSFORMED));
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

// Phase 4: Charge moves, locked moves, pivot resolution

fn fixed_rng(val: u32) -> impl FnMut(u32) -> u32 { move |max| val % max }

/// Setup with moves assigned for charge/pivot testing.
fn setup_with_moves() -> (BattleState, ZobristKeys) {
    let mut state = BattleState::default();
    let keys = ZobristKeys::new(42);

    for i in 0..6 {
        state.sides[0].team[i].species_id = 25; // Pikachu (Electric)
        state.sides[0].team[i].max_hp = 300;
        state.sides[0].team[i].current_hp = 300;
        state.sides[0].team[i].stats = [150, 100, 150, 100, 100];

        state.sides[1].team[i].species_id = 25;
        state.sides[1].team[i].max_hp = 300;
        state.sides[1].team[i].current_hp = 300;
        state.sides[1].team[i].stats = [150, 100, 150, 100, 100];
    }

    // P1 moves: Fly(19), Outrage(200), U-turn(369), Baton Pass(226)
    state.sides[0].team[0].moves = [19, 200, 369, 226];
    state.sides[0].team[0].pp = [15, 10, 20, 40];

    // P2 moves: Dig(91), Pound(1), Earthquake(89), Parting Shot(575)
    state.sides[1].team[0].moves = [91, 1, 89, 575];
    state.sides[1].team[0].pp = [10, 35, 10, 20];

    state.phase = PHASE_ACTIONS;
    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

#[test]
fn test_fly_semi_invuln_turn1_attacks_turn2() {
    let (mut state, keys) = setup_with_moves();
    let fly_id = 19u16;

    execute_move(&mut state, &keys, 0, fly_id, 0, &mut fixed_rng(0));
    assert!(state.sides[0].active.has_volatile(VOL_CHARGING));
    assert!(state.sides[0].active.has_volatile(VOL_SEMI_INVULNERABLE));
    assert_eq!(state.sides[0].active._padding[1], 1); // air
    assert_eq!(state.sides[1].team[0].current_hp, 300);

    execute_move(&mut state, &keys, 0, 0, 0, &mut fixed_rng(0));
    assert!(!state.sides[0].active.has_volatile(VOL_CHARGING));
    assert!(!state.sides[0].active.has_volatile(VOL_SEMI_INVULNERABLE));
    assert!(state.sides[1].team[0].current_hp < 300);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_earthquake_hits_dig() {
    let (mut state, keys) = setup_with_moves();

    set_volatile(&mut state, &keys, 1, VOL_CHARGING);
    set_volatile(&mut state, &keys, 1, VOL_SEMI_INVULNERABLE);
    state.sides[1].active._padding[1] = 2; // underground
    state.sides[1].active.last_move = 91; // Dig

    execute_move(&mut state, &keys, 0, 89, 2, &mut fixed_rng(0)); // Earthquake
    assert!(state.sides[1].team[0].current_hp < 300);
}

#[test]
fn test_power_herb_skips_charge_consumed() {
    let (mut state, keys) = setup_with_moves();
    let fly_id = 19u16;

    state.sides[0].team[0].item_id = 358; // Power Herb
    state.zobrist = compute_full_hash(&state, &keys);

    // Power Herb skips charge turn, deals damage immediately, then is consumed
    execute_move(&mut state, &keys, 0, fly_id, 0, &mut fixed_rng(0));

    assert!(!state.sides[0].active.has_volatile(VOL_CHARGING));
    assert!(state.sides[1].team[0].current_hp < 300);
    assert_eq!(state.sides[0].team[0].item_id, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_solar_beam_skips_charge_in_sun() {
    let (mut state, keys) = setup_with_moves();
    let solarbeam_id = 76u16;

    state.sides[0].team[0].moves[0] = solarbeam_id;
    state.field.weather = WEATHER_SUN;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 0, solarbeam_id, 0, &mut fixed_rng(0));

    // Sun skips Solar Beam charge (no item consumed, unlike Power Herb)
    assert!(!state.sides[0].active.has_volatile(VOL_CHARGING));
    assert!(state.sides[1].team[0].current_hp < 300);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_outrage_locks_then_confuses() {
    let (mut state, keys) = setup_with_moves();
    let outrage_id = 200u16;

    // Outrage locks for 2-3 turns; rng(2)=0 means 1 extra turn after this
    execute_move(&mut state, &keys, 0, outrage_id, 1, &mut fixed_rng(0));
    assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
    assert!(state.sides[1].team[0].current_hp < 300);
    let hp_after_t1 = state.sides[1].team[0].current_hp;

    execute_move(&mut state, &keys, 0, outrage_id, 1, &mut fixed_rng(0));
    assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
    // Outrage inflicts confusion when lock expires
    assert!(state.sides[0].active.confusion_turns > 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_legal_moves_during_outrage_lock() {
    let (mut state, keys) = setup_with_moves();

    state.sides[0].active.volatile_flags |= VOL_MOVE_LOCKED;
    state.sides[0].active.last_move = 200; // Outrage
    state.sides[0].active._padding[2] = 1;

    let actions = legal_actions(&state, 0);
    assert_eq!(actions.count, 1);
    assert_eq!(actions.actions[0], 1); // only the locked move
    assert!(!actions.as_slice().iter().any(|&a| a >= ACTION_SWITCH_0));
}

#[test]
fn test_uturn_damage_then_switch() {
    let (mut state, keys) = setup_with_moves();
    let uturn_id = 369u16;

    state.sides[0].team[0].moves[2] = uturn_id;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 0, uturn_id, 2, &mut fixed_rng(0));

    assert!(state.sides[1].team[0].current_hp < 300);
    assert!(state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_baton_pass_preserves_boosts() {
    let (mut state, keys) = setup_with_moves();

    apply_boost(&mut state, &keys, 0, ATK, 2);
    assert_eq!(state.sides[0].active.boosts[ATK], 2);

    let bpass_id = 226u16;
    execute_move(&mut state, &keys, 0, bpass_id, 3, &mut fixed_rng(0));

    assert!(state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    assert_eq!(state.sides[0].active._padding[0], 1); // baton pass flag

    clear_volatile(&mut state, &keys, 0, VOL_MUST_SWITCH);
    perform_switch(&mut state, &keys, 0, 1);

    assert_eq!(state.sides[0].active_index, 1);
    // Baton Pass preserves boosts to the incoming mon
    assert_eq!(state.sides[0].active.boosts[ATK], 2);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_parting_shot_debuffs_then_switch() {
    let (mut state, keys) = setup_with_moves();
    let parting_shot_id = 575u16;

    execute_move(&mut state, &keys, 1, parting_shot_id, 3, &mut fixed_rng(0));

    // Parting Shot: -1 Atk/-1 SpA to target, then self-switch
    assert_eq!(state.sides[0].active.boosts[ATK], -1);
    assert_eq!(state.sides[0].active.boosts[SPA], -1);
    assert!(state.sides[1].active.has_volatile(VOL_MUST_SWITCH));
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_legal_moves_switch_phase_only_switches() {
    let (mut state, _keys) = setup_with_moves();

    state.phase = PHASE_SWITCH_P1;
    let actions = legal_actions(&state, 0);

    assert_eq!(actions.count, 5);
    assert!(actions.as_slice().iter().all(|&a| a >= ACTION_SWITCH_0));
}

#[test]
fn test_must_switch_triggers_switch_phase_via_turn() {
    let (mut state, keys) = setup_with_moves();
    let uturn_id = 369u16;
    state.sides[0].team[0].moves[2] = uturn_id;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_turn(&mut state, &keys, 2, 1, &mut fixed_rng(0));
    assert!(matches!(state.phase, PHASE_SWITCH_P1 | PHASE_SWITCH_BOTH));
}

#[test]
fn test_full_pivot_uturn_then_switch() {
    let (mut state, keys) = setup_with_moves();
    let uturn_id = 369u16;
    state.sides[0].team[0].moves[2] = uturn_id;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_turn(&mut state, &keys, 2, 1, &mut fixed_rng(0));

    let phase = state.phase;
    assert!(phase == PHASE_SWITCH_P1 || phase == PHASE_SWITCH_BOTH);

    assert!(state.sides[1].team[0].current_hp < 300);

    execute_switch_turn(&mut state, &keys, ACTION_SWITCH_0 + 1, 0, &mut fixed_rng(0));
    assert_eq!(state.sides[0].active_index, 1);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_legal_moves_during_charging() {
    let (mut state, keys) = setup_with_moves();
    let fly_id = 19u16;

    execute_move(&mut state, &keys, 0, fly_id, 0, &mut fixed_rng(0));
    assert!(state.sides[0].active.has_volatile(VOL_CHARGING));

    let actions = legal_actions(&state, 0);
    assert_eq!(actions.count, 1);
    assert_eq!(actions.actions[0], 0);
    assert!(!actions.as_slice().iter().any(|&a| a >= ACTION_SWITCH_0));
}

#[test]
fn test_random_battles_no_panics() {
    // Move pools: mix of Physical, Special, Status, Healing, multi-turn, pivot
    const MOVE_POOLS: [[u16; 4]; 6] = [
        [85, 89, 14, 105],    // Thunderbolt, Earthquake, Swords Dance, Recover
        [53, 58, 182, 261],   // Flamethrower, Ice Beam, Protect, Will-O-Wisp
        [57, 200, 92, 19],    // Surf, Outrage, Toxic, Fly
        [63, 369, 85, 53],    // Hyper Beam, U-Turn, Thunderbolt, Flamethrower
        [89, 105, 58, 182],   // Earthquake, Recover, Ice Beam, Protect
        [57, 14, 261, 200],   // Surf, Swords Dance, Will-O-Wisp, Outrage
    ];

    for battle_idx in 0..200u64 {
        let mut state = BattleState::default();
        let keys = ZobristKeys::new(battle_idx);
        let mut seed = battle_idx.wrapping_mul(6364136223846793005).wrapping_add(1);

        for side in 0..2 {
            for i in 0..6 {
                let pool_idx = ((side * 3 + i) + battle_idx as usize) % 6;
                state.sides[side].team[i] = MonSlot {
                    species_id: (i as u16 + 1) + (side as u16 * 10),
                    current_hp: 300,
                    max_hp: 300,
                    stats: [120, 100, 100, 100, 80 + (i as u16 * 5)],
                    moves: MOVE_POOLS[pool_idx],
                    pp: [24, 24, 24, 24],
                    tera_type: if i == 0 { Type::Fire as u8 } else { 0 },
                    ..Default::default()
                };
                // Give some mons Assault Vest to exercise that path
                if i == 2 && side == 0 {
                    state.sides[side].team[i].item_id = 581;
                }
            }
        }

        state.phase = PHASE_ACTIONS;
        state.zobrist = compute_full_hash(&state, &keys);

        let mut rng = move |max: u32| -> u32 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as u32) % max.max(1)
        };

        for _turn in 0..500 {
            if state.is_game_over() { break; }

            let a1 = legal_actions(&state, 0);
            let a2 = legal_actions(&state, 1);

            if state.phase == PHASE_ACTIONS {
                assert!(!a1.is_empty(), "battle {battle_idx}: p1 has no actions in PHASE_ACTIONS");
                assert!(!a2.is_empty(), "battle {battle_idx}: p2 has no actions in PHASE_ACTIONS");
                let act1 = a1.actions[rng(a1.count as u32) as usize];
                let act2 = a2.actions[rng(a2.count as u32) as usize];
                execute_turn(&mut state, &keys, act1, act2, &mut rng);
            } else if state.phase == PHASE_SWITCH_P1 {
                assert!(!a1.is_empty(), "battle {battle_idx}: p1 has no switch targets");
                let act1 = a1.actions[rng(a1.count as u32) as usize];
                execute_switch_turn(&mut state, &keys, act1, 0, &mut rng);
            } else if state.phase == PHASE_SWITCH_P2 {
                assert!(!a2.is_empty(), "battle {battle_idx}: p2 has no switch targets");
                let act2 = a2.actions[rng(a2.count as u32) as usize];
                execute_switch_turn(&mut state, &keys, 0, act2, &mut rng);
            } else if state.phase == PHASE_SWITCH_BOTH {
                assert!(!a1.is_empty(), "battle {battle_idx}: p1 has no switch targets (both)");
                assert!(!a2.is_empty(), "battle {battle_idx}: p2 has no switch targets (both)");
                let act1 = a1.actions[rng(a1.count as u32) as usize];
                let act2 = a2.actions[rng(a2.count as u32) as usize];
                execute_switch_turn(&mut state, &keys, act1, act2, &mut rng);
            }
        }
    }
}
