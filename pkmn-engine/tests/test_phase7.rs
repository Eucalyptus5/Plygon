//! Phase 7 comprehensive tests: status effects, volatile management,
//! side/field conditions, forme changes, Tera, turn driver hardening.

use pkmn_engine::state::BattleState;
use pkmn_engine::state::structs::*;
use pkmn_engine::state::zobrist::{ZobristKeys, compute_full_hash, validate_hash};
use pkmn_engine::state::end_of_turn::end_of_turn;
use pkmn_engine::state::legal_moves::legal_actions;
use pkmn_engine::state::move_exec::execute_move;
use pkmn_engine::state::mutations::*;
use pkmn_engine::state::accessors::*;
use pkmn_engine::state::forme::{apply_battle_forme, revert_battle_forme};
use pkmn_engine::state::turn::{execute_turn, execute_switch_turn};
use pkmn_engine::state::calc_modifiers::stab_modifier;
use pkmn_engine::state::data_bridge;
use pkmn_engine::data::types::Type;
use pkmn_engine::data::moves::MoveCategory;

fn setup() -> (BattleState, ZobristKeys) {
    let keys = ZobristKeys::new(42);
    let mut state = BattleState::default();
    state.sides[0].team[0] = MonSlot {
        species_id: 25, current_hp: 300, max_hp: 300,
        stats: [150, 100, 150, 100, 100],
        moves: [1, 85, 269, 227], pp: [24, 24, 24, 24],
        // moves: Pound(1), Thunderbolt(85), Taunt(269), Encore(227)
        ..Default::default()
    };
    state.sides[0].team[1] = MonSlot {
        species_id: 6, current_hp: 200, max_hp: 200,
        stats: [100, 100, 100, 100, 80],
        moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
        ..Default::default()
    };
    state.sides[0].team[2] = MonSlot {
        species_id: 7, current_hp: 200, max_hp: 200,
        stats: [100, 100, 100, 100, 80],
        ..Default::default()
    };
    state.sides[1].team[0] = MonSlot {
        species_id: 50, current_hp: 300, max_hp: 300,
        stats: [100, 100, 100, 100, 80],
        moves: [1, 85, 3, 4], pp: [24, 24, 24, 24],
        ..Default::default()
    };
    state.sides[1].team[1] = MonSlot {
        species_id: 10, current_hp: 200, max_hp: 200,
        stats: [80, 80, 80, 80, 60],
        moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
        ..Default::default()
    };
    state.sides[1].team[2] = MonSlot {
        species_id: 11, current_hp: 200, max_hp: 200,
        stats: [80, 80, 80, 80, 60],
        ..Default::default()
    };
    state.phase = PHASE_ACTIONS;
    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

fn dummy_rng(_max: u32) -> u32 { 0 }

#[test]
fn test_belly_drum_boost_and_hp_cost() {
    let (mut state, keys) = setup();
    let max_hp = state.sides[0].team[0].max_hp;
    let hp_before = state.sides[0].team[0].current_hp;
    assert!(hp_before > max_hp / 2, "need >50% HP for BellyDrum");

    // Directly simulate BellyDrum effect (move 187 has MoveEffect::None in gen data,
    // so we test the mechanic via direct state manipulation matching move_exec logic)
    deal_damage(&mut state, &keys, 0, 0, max_hp / 2);
    let current_atk_boost = state.sides[0].active.boosts[ATK];
    apply_boost(&mut state, &keys, 0, ATK, 6 - current_atk_boost);

    assert_eq!(state.sides[0].team[0].current_hp, hp_before - max_hp / 2);
    assert_eq!(state.sides[0].active.boosts[ATK], 6);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_taunt_blocks_status_moves() {
    let (mut state, keys) = setup();
    // Side 1 uses Taunt (269) on side 0
    // Taunt effect is wired up in gen_moves, so execute_move should work
    execute_move(&mut state, &keys, 1, 269, 0, &mut dummy_rng);

    assert!(state.sides[0].active.taunt_turns > 0, "Taunt should set taunt_turns");

    // Now check legal_actions for side 0: Status moves should be blocked
    let actions = legal_actions(&state, 0);
    let moves = effective_moves(&state, 0);
    for &action in actions.as_slice() {
        if action <= 3 {
            let move_id = moves[action as usize];
            if move_id != 0 {
                let md = data_bridge::move_hot(move_id);
                assert_ne!(md.category, MoveCategory::Status,
                    "Taunt should block Status move {} in slot {}", move_id, action);
            }
        }
    }
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_encore_locks_to_last_move() {
    let (mut state, keys) = setup();
    // Side 0 uses Pound (slot 0, move_id 1) — set last_move
    state.sides[0].active.last_move = 1; // Pound

    // Side 1 uses Encore (227) on side 0
    execute_move(&mut state, &keys, 1, 227, 0, &mut dummy_rng);

    assert!(state.sides[0].active.encore_turns > 0);
    assert_eq!(state.sides[0].active.encore_move, 1);

    // Legal actions for side 0 should only allow the encored move (slot 0)
    let actions = legal_actions(&state, 0);
    let move_actions: Vec<u8> = actions.as_slice().iter()
        .copied().filter(|&a| a <= 3).collect();
    assert_eq!(move_actions.len(), 1, "Encore should restrict to 1 move");
    assert_eq!(move_actions[0], 0, "Encore should lock to slot 0 (Pound)");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_disable_prevents_specific_move() {
    let (mut state, keys) = setup();
    // Set last_move on side 0 to Thunderbolt (85, slot 1)
    state.sides[0].active.last_move = 85;
    // Apply Disable effect directly (move 50 has MoveEffect::None in gen data)
    state.sides[0].active.disabled_move = 85;
    state.sides[0].active.disable_turns = 4;

    let actions = legal_actions(&state, 0);
    let moves = effective_moves(&state, 0);
    for &action in actions.as_slice() {
        if action <= 3 {
            let move_id = moves[action as usize];
            assert_ne!(move_id, 85, "Disabled move (Thunderbolt) should not be in legal actions");
        }
    }
    // Other moves should still be available
    let move_count: usize = actions.as_slice().iter().filter(|&&a| a <= 3).count();
    assert!(move_count >= 2, "Other moves should remain legal");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_struggle_when_all_moves_restricted() {
    let (mut state, keys) = setup();
    // Set all PP to 0
    state.sides[0].team[0].pp = [0; 4];

    let actions = legal_actions(&state, 0);
    assert!(actions.as_slice().contains(&ACTION_STRUGGLE),
        "Should include Struggle when no PP");
    // No regular move actions
    assert!(!actions.as_slice().iter().any(|&a| a <= 3),
        "Should have no regular move actions");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_perish_song_kos_at_counter_0() {
    let (mut state, keys) = setup();
    // Set perish song on side 0 with count = 0
    set_volatile(&mut state, &keys, 0, VOL_PERISH_SONG);
    state.sides[0].active.perish_count = 0;

    // Also set side 1 with count = 2 (should decrement, not KO)
    set_volatile(&mut state, &keys, 1, VOL_PERISH_SONG);
    state.sides[1].active.perish_count = 2;

    end_of_turn(&mut state, &keys);

    // Side 0 should be KO'd
    assert_eq!(state.sides[0].team[0].current_hp, 0,
        "Perish Song at count 0 should KO");
    // Side 1 should have decremented
    assert_eq!(state.sides[1].active.perish_count, 1,
        "Perish count should decrement from 2 to 1");
    assert!(state.sides[1].team[0].current_hp > 0,
        "Side 1 should still be alive");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_destiny_bond_kos_attacker() {
    let (mut state, keys) = setup();
    // Set Destiny Bond on defender (side 1)
    set_volatile(&mut state, &keys, 1, VOL_DESTINY_BOND);
    // Set defender to 1 HP so any attack KOs
    state.sides[1].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    // Side 0 uses Pound (move_id=1, physical normal, 40bp)
    execute_move(&mut state, &keys, 0, 1, 0, &mut dummy_rng);

    // Defender should be fainted
    assert_eq!(state.sides[1].team[0].current_hp, 0,
        "Defender should be KO'd by Pound");
    // Attacker should also be KO'd by Destiny Bond
    assert_eq!(state.sides[0].team[0].current_hp, 0,
        "Destiny Bond should KO the attacker");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_trick_swaps_items() {
    let (mut state, keys) = setup();
    // Give items to both sides
    set_item(&mut state, &keys, 0, 0, 100); // Side 0 gets item 100
    set_item(&mut state, &keys, 1, 0, 200); // Side 1 gets item 200

    // Apply Trick effect directly
    let item_a = state.sides[0].team[0].item_id;
    let item_d = state.sides[1].team[0].item_id;
    set_item(&mut state, &keys, 0, 0, item_d);
    set_item(&mut state, &keys, 1, 0, item_a);

    assert_eq!(state.sides[0].team[0].item_id, 200, "Side 0 should have item 200");
    assert_eq!(state.sides[1].team[0].item_id, 100, "Side 1 should have item 100");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_tailwind_doubles_speed() {
    let (mut state, keys) = setup();
    // Side 0 speed = 100, Side 1 speed = 80
    // Without tailwind, side 0 is faster
    // Set tailwind on side 1
    state.sides[1].side_conditions.tailwind_turns = 4;

    // Use execute_turn: side 0 uses Pound, side 1 uses Pound
    // With tailwind, side 1 speed = 80*2 = 160 > side 0 speed = 100
    // Side 1 should go first
    // We test this by making side 1 KO side 0 (set low HP)
    state.sides[0].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_turn(&mut state, &keys, 0, 0, &mut dummy_rng);

    // Side 1 should have moved first due to tailwind → side 0 fainted before acting
    // If side 0 went first it would have hit side 1 with Pound first
    // With tailwind side 1 at 160 > side 0 at 100, side 1 goes first, KOs side 0
    assert_eq!(state.sides[0].team[0].current_hp, 0,
        "Side 0 should be fainted (side 1 faster with Tailwind)");
}

#[test]
fn test_gravity_blocks_fly() {
    let keys = ZobristKeys::new(42);
    let mut state = BattleState::default();
    state.sides[0].team[0] = MonSlot {
        species_id: 25, current_hp: 300, max_hp: 300,
        stats: [150, 100, 150, 100, 100],
        moves: [19, 1, 3, 4], pp: [24, 24, 24, 24], // Fly(19), Pound, etc.
        ..Default::default()
    };
    state.sides[0].team[1] = MonSlot {
        species_id: 6, current_hp: 200, max_hp: 200,
        stats: [100; 5], ..Default::default()
    };
    state.sides[1].team[0] = MonSlot {
        species_id: 50, current_hp: 300, max_hp: 300,
        stats: [100; 5], moves: [1, 2, 3, 4], pp: [24; 4],
        ..Default::default()
    };
    state.phase = PHASE_ACTIONS;
    state.zobrist = compute_full_hash(&state, &keys);

    // Without gravity, Fly (19) should be legal
    let actions_no_gravity = legal_actions(&state, 0);
    let has_fly_before = actions_no_gravity.as_slice().iter().any(|&a| {
        a <= 3 && effective_moves(&state, 0)[a as usize] == 19
    });
    assert!(has_fly_before, "Fly should be legal without Gravity");

    // Set gravity
    set_gravity(&mut state, &keys, 5);

    let actions_gravity = legal_actions(&state, 0);
    let has_fly_after = actions_gravity.as_slice().iter().any(|&a| {
        a <= 3 && effective_moves(&state, 0)[a as usize] == 19
    });
    assert!(!has_fly_after, "Fly should be blocked by Gravity");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_aegislash_forme_toggle() {
    let keys = ZobristKeys::new(42);
    let mut state = BattleState::default();
    // Aegislash Shield (681): atk:50, def:140, spa:50, spd:140, spe:60
    state.sides[0].team[0] = MonSlot {
        species_id: 681, current_hp: 300, max_hp: 300,
        ability_id: data_bridge::ABILITY_STANCE_CHANGE,
        stats: [100, 280, 100, 280, 120],
        moves: [1, 588, 3, 4], pp: [24, 24, 24, 24],
        // Pound(1), King's Shield(588)
        ..Default::default()
    };
    state.sides[1].team[0] = MonSlot {
        species_id: 25, current_hp: 300, max_hp: 300,
        stats: [100; 5], moves: [1, 2, 3, 4], pp: [24; 4],
        ..Default::default()
    };
    state.phase = PHASE_ACTIONS;
    state.zobrist = compute_full_hash(&state, &keys);

    // Shield → Blade on attacking move
    apply_battle_forme(&mut state, &keys, 0, 1103);
    assert_eq!(effective_species(&state, 0), 1103, "Should be Blade forme");
    // Stats should scale: atk 100*140/50=280, def 280*50/140=100
    assert_eq!(effective_stat(&state, 0, ATK), 280);
    assert_eq!(effective_stat(&state, 0, DEF), 100);
    assert!(validate_hash(&state, &keys));

    // Blade → Shield on King's Shield
    revert_battle_forme(&mut state, &keys, 0);
    assert_eq!(effective_species(&state, 0), 681, "Should revert to Shield forme");
    assert_eq!(effective_stat(&state, 0, ATK), 100);
    assert_eq!(effective_stat(&state, 0, DEF), 280);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_tera_stab_calculation() {
    let (mut state, keys) = setup();
    // Pikachu (25): Electric/Electric
    // Set tera_type to Fire
    state.sides[0].team[0].tera_type = Type::Fire as u8;
    state.zobrist = compute_full_hash(&state, &keys);

    // Before Tera: Electric move on Electric mon = STAB 1.5× → (6144, 4096)
    let (num, den) = stab_modifier(&state, 0, Type::Electric);
    assert_eq!((num, den), (6144, 4096), "Normal STAB should be 1.5×");

    // Apply Tera (Fire)
    state.sides[0].team[0].flags |= MON_FLAG_TERASTALLIZED;
    state.sides[0]._padding[0] |= 1;
    state.zobrist = compute_full_hash(&state, &keys);

    // Electric move on Tera-Fire Pikachu: matches original (Electric) but NOT tera (Fire)
    // → 1.5× = (6144, 4096)
    let (num, den) = stab_modifier(&state, 0, Type::Electric);
    assert_eq!((num, den), (6144, 4096), "Tera: original-only STAB should be 1.5×");

    // Fire move on Tera-Fire Pikachu: matches tera (Fire) but NOT original (Electric)
    // → 1.5× = (6144, 4096)
    let (num, den) = stab_modifier(&state, 0, Type::Fire);
    assert_eq!((num, den), (6144, 4096), "Tera: tera-only STAB should be 1.5×");

    // Now test tera type == original type: set tera_type to Electric
    state.sides[0].team[0].tera_type = Type::Electric as u8;
    // Electric move: matches both tera AND original → 2× = (8192, 4096)
    let (num, den) = stab_modifier(&state, 0, Type::Electric);
    assert_eq!((num, den), (8192, 4096), "Tera: both tera+original STAB should be 2×");

    // Non-STAB move (e.g. Grass): matches neither tera nor original → 1× = (4096, 4096)
    let (num, den) = stab_modifier(&state, 0, Type::Grass);
    assert_eq!((num, den), (4096, 4096), "Tera: no match should be 1×");
}

#[test]
fn test_tera_legal_action_once() {
    let (mut state, keys) = setup();
    // Set tera_type so tera is available
    state.sides[0].team[0].tera_type = Type::Fire as u8;
    state.zobrist = compute_full_hash(&state, &keys);

    let actions = legal_actions(&state, 0);
    let tera_count = actions.as_slice().iter().filter(|&&a| a == ACTION_TERA).count();
    assert_eq!(tera_count, 1, "ACTION_TERA should appear exactly once");

    // Use tera
    state.sides[0].team[0].flags |= MON_FLAG_TERASTALLIZED;
    state.sides[0]._padding[0] |= 1; // tera used
    state.zobrist = compute_full_hash(&state, &keys);

    let actions2 = legal_actions(&state, 0);
    let tera_count2 = actions2.as_slice().iter().filter(|&&a| a == ACTION_TERA).count();
    assert_eq!(tera_count2, 0, "ACTION_TERA should not appear after tera used");
}

#[test]
fn test_mid_turn_faint_triggers_switch_phase() {
    let (mut state, keys) = setup();
    // Set side 0 to low HP so any self-damage/recoil kills it
    state.sides[0].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    // Both sides use Pound (action 0)
    // Side 0 has higher speed (100 vs 80), goes first, attacks side 1
    // Side 1 attacks side 0, KOs it
    execute_turn(&mut state, &keys, 0, 0, &mut dummy_rng);

    // Side 0 should be fainted
    assert_eq!(state.sides[0].team[0].current_hp, 0);
    // Phase should be switch (side 0 needs replacement)
    assert!(
        state.phase == PHASE_SWITCH_P1 || state.phase == PHASE_SWITCH_BOTH || state.phase == PHASE_GAME_OVER,
        "Should transition to switch or game-over phase, got phase {}",
        state.phase
    );

    // If there are bench mons, phase should be SWITCH_P1
    if state.phase == PHASE_SWITCH_P1 {
        let actions = legal_actions(&state, 0);
        assert!(actions.as_slice().iter().all(|&a| a >= ACTION_SWITCH_0),
            "In switch phase, only switch actions should be legal");
        // Pick first available switch target
        let switch_action = actions.actions[0];
        execute_switch_turn(&mut state, &keys, switch_action, 0, &mut dummy_rng);
        // Active mon should now be alive
        assert!(state.active_mon(0).current_hp > 0,
            "New active mon should be alive after forced switch");
    }
}

#[test]
fn test_single_faint_triggers_switch_phase() {
    let (mut state, keys) = setup();
    // Give side 1's active 1 HP so side 0's Pound OHKOs it
    state.sides[1].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    // Side 0 speed=100 > Side 1 speed=80, so side 0 goes first
    // Side 0's Pound (action 0) should KO side 1
    execute_turn(&mut state, &keys, 0, 0, &mut dummy_rng);

    assert_eq!(state.sides[1].team[0].current_hp, 0, "Side 1 should be fainted");
    assert_eq!(state.phase, PHASE_SWITCH_P2,
        "Side 1 fainted, should be PHASE_SWITCH_P2, got {}", state.phase);
    assert_eq!(state.turn_subphase(), SUBPHASE_AFTER_MOVE1,
        "Should be in SUBPHASE_AFTER_MOVE1");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_both_faint_triggers_switch_both() {
    let (mut state, keys) = setup();
    // Simulate a double KO via direct state manipulation:
    // Set both actives to 0 HP, set phase and subphase, recompute hash.
    state.sides[0].team[0].current_hp = 0;
    state.sides[1].team[0].current_hp = 0;
    state.set_turn_resume(SUBPHASE_AFTER_MOVE1, 1, 0);
    state.phase = PHASE_SWITCH_BOTH;
    state.zobrist = compute_full_hash(&state, &keys);

    assert_eq!(state.phase, PHASE_SWITCH_BOTH);
    assert_eq!(state.turn_subphase(), SUBPHASE_AFTER_MOVE1);

    // Both sides should have only switch actions
    let actions_p1 = legal_actions(&state, 0);
    let actions_p2 = legal_actions(&state, 1);
    assert!(actions_p1.as_slice().iter().all(|&a| a >= ACTION_SWITCH_0),
        "P1 should only have switch actions during PHASE_SWITCH_BOTH");
    assert!(actions_p2.as_slice().iter().all(|&a| a >= ACTION_SWITCH_0),
        "P2 should only have switch actions during PHASE_SWITCH_BOTH");

    // Perform the switches — both switch to slot 1
    execute_switch_turn(&mut state, &keys,
        ACTION_SWITCH_0 + 1, ACTION_SWITCH_0 + 1, &mut dummy_rng);

    // Both replacements should be alive
    assert!(state.active_mon(0).current_hp > 0, "P1 replacement should be alive");
    assert!(state.active_mon(1).current_hp > 0, "P2 replacement should be alive");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_forced_switch_resumes_turn() {
    let (mut state, keys) = setup();
    // Side 1's active has 1 HP, side 0 goes first (speed 100 > 80) and KOs it
    state.sides[1].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    let p1_hp_before = state.sides[0].team[0].current_hp;

    // Side 0 uses Pound (action 0), Side 1 uses Pound (action 0)
    execute_turn(&mut state, &keys, 0, 0, &mut dummy_rng);

    // Should pause for side 1's replacement
    assert_eq!(state.phase, PHASE_SWITCH_P2);
    assert_eq!(state.turn_subphase(), SUBPHASE_AFTER_MOVE1);

    // Side 0 should NOT have taken damage yet (Move 2 hasn't executed)
    assert_eq!(state.sides[0].team[0].current_hp, p1_hp_before,
        "Side 0 should not have taken damage before Move 2 executes");

    // Side 1 switches in slot 1
    execute_switch_turn(&mut state, &keys, 0, ACTION_SWITCH_0 + 1, &mut dummy_rng);

    // The replacement (side 1 slot 1) should have executed Move 2 (Pound on side 0)
    // Side 1 slot 1 has attack=80, side 0 has defense=100, Pound base power=40
    // Some damage should have been dealt to side 0
    // NOTE: The replacement does NOT get to attack — only the original second mover.
    // Since the second mover (side 1) was replaced, Move 2 is skipped.
    // End-of-turn should have run. Phase should be PHASE_ACTIONS.
    assert_eq!(state.phase, PHASE_ACTIONS,
        "After replacement, turn should complete to PHASE_ACTIONS, got {}", state.phase);
    assert_eq!(state.turn_subphase(), SUBPHASE_NORMAL,
        "Turn resume state should be cleared");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_forced_switch_legal_moves_only_switches() {
    let (mut state, keys) = setup();
    state.sides[1].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    // Side 0 OHKOs side 1
    execute_turn(&mut state, &keys, 0, 0, &mut dummy_rng);

    assert_eq!(state.phase, PHASE_SWITCH_P2);

    // Side 1 should only have switch actions
    let actions = legal_actions(&state, 1);
    assert!(!actions.as_slice().is_empty(), "Side 1 should have switch options");
    assert!(actions.as_slice().iter().all(|&a| a >= ACTION_SWITCH_0),
        "All actions should be switches, got {:?}", actions.as_slice());

    // Side 0 (not forced to switch) during PHASE_SWITCH_P2 should have no actions
    let actions_p1 = legal_actions(&state, 0);
    assert!(actions_p1.as_slice().is_empty(),
        "Side 0 should have no actions during PHASE_SWITCH_P2");
}

#[test]
fn test_replacement_does_not_act() {
    let (mut state, keys) = setup();
    // Side 1 has 1 HP, side 0 goes first and KOs it
    state.sides[1].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    let p1_hp_before = state.sides[0].team[0].current_hp;

    // Both use Pound. Side 0 goes first (faster), KOs side 1.
    execute_turn(&mut state, &keys, 0, 0, &mut dummy_rng);
    assert_eq!(state.phase, PHASE_SWITCH_P2);

    // Side 1 switches in slot 1
    execute_switch_turn(&mut state, &keys, 0, ACTION_SWITCH_0 + 1, &mut dummy_rng);

    // The replacement should NOT have attacked side 0
    // Side 0's HP should be unchanged (no Move 2 from fainted/replaced side)
    assert_eq!(state.sides[0].team[0].current_hp, p1_hp_before,
        "Side 0 HP should be unchanged — replacement must not attack");
    assert_eq!(state.phase, PHASE_ACTIONS);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_faint_after_move2_triggers_switch() {
    let (mut state, keys) = setup();
    // Side 0 has 1 HP. Side 0 is faster (speed 100 > 80), so it goes first.
    // Side 0 attacks (doesn't KO). Side 1 attacks, KOs side 0.
    state.sides[0].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_turn(&mut state, &keys, 0, 0, &mut dummy_rng);

    // Side 0 should be fainted from Move 2
    assert_eq!(state.sides[0].team[0].current_hp, 0,
        "Side 0 should be fainted from Move 2");
    assert_eq!(state.phase, PHASE_SWITCH_P1,
        "Should be PHASE_SWITCH_P1, got {}", state.phase);
    assert_eq!(state.turn_subphase(), SUBPHASE_AFTER_MOVE2,
        "Should be SUBPHASE_AFTER_MOVE2");

    // Switch in replacement, end-of-turn should run
    execute_switch_turn(&mut state, &keys, ACTION_SWITCH_0 + 1, 0, &mut dummy_rng);

    assert_eq!(state.phase, PHASE_ACTIONS,
        "After replacement post-Move 2, should return to PHASE_ACTIONS");
    assert_eq!(state.turn_subphase(), SUBPHASE_NORMAL);
    assert!(state.active_mon(0).current_hp > 0, "Replacement should be alive");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_trick_room_inverts_speed() {
    let (mut state, keys) = setup();
    // Side 0 speed = 100, Side 1 speed = 80
    // Under Trick Room, side 1 (slower) should move first
    state.field.trick_room_turns = 5;
    state.sides[0].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_turn(&mut state, &keys, 0, 0, &mut dummy_rng);

    // Side 1 moved first (slower under TR), KO'd side 0
    assert_eq!(state.sides[0].team[0].current_hp, 0,
        "Side 0 should be fainted (side 1 moves first under Trick Room)");
}

#[test]
fn test_trick_room_no_affect_priority() {
    let (mut state, keys) = setup();
    // Give side 0 Quick Attack (98, priority +1) in slot 2
    state.sides[0].team[0].moves[2] = 98;
    state.sides[0].team[0].pp[2] = 24;
    state.field.trick_room_turns = 5;
    // Under TR, side 1 (speed 80) would be "faster" by speed alone
    // But Quick Attack has +1 priority, which should override TR
    state.sides[1].team[0].current_hp = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    // Side 0 uses Quick Attack (action 2), side 1 uses Pound (action 0)
    execute_turn(&mut state, &keys, 2, 0, &mut dummy_rng);

    assert_eq!(state.sides[1].team[0].current_hp, 0,
        "Side 1 should be fainted (priority overrides Trick Room)");
}

// ── Phase 2 Task 13: Encore, Disable, Taunt, Torment ──

#[test]
fn test_encore_forces_move() {
    let (mut state, keys) = setup();
    // Side 0 used Pound (move 1) last turn
    state.sides[0].active.last_move = 1;
    // Side 0 has already moved this turn
    set_volatile(&mut state, &keys, 0, VOL_MOVED_THIS_TURN);
    state.zobrist = compute_full_hash(&state, &keys);

    // Side 1 uses Encore (227) on side 0
    execute_move(&mut state, &keys, 1, 227, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].active.encore_move, 1, "Encore should lock to Pound");
    assert_eq!(state.sides[0].active.encore_turns, 4, "Target already moved => duration 4");

    // Legal actions should only include the encored move slot
    let actions = legal_actions(&state, 0);
    let move_actions: Vec<u8> = actions.as_slice().iter()
        .copied().filter(|&a| a <= 3).collect();
    assert_eq!(move_actions.len(), 1);
    assert_eq!(move_actions[0], 0, "Only slot 0 (Pound) should be legal");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_encore_fails_no_last_move() {
    let (mut state, keys) = setup();
    // Side 0 has no last move
    state.sides[0].active.last_move = 0;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 227, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].active.encore_move, 0, "Encore should fail with no last_move");
    assert_eq!(state.sides[0].active.encore_turns, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_encore_expires() {
    let (mut state, keys) = setup();
    // Set encore manually with 1 turn remaining
    state.sides[0].active.encore_move = 1;
    state.sides[0].active.encore_turns = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].active.encore_turns, 0, "Encore should expire");
    assert_eq!(state.sides[0].active.encore_move, 0, "Encore move should be cleared");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_encore_duration_user_faster() {
    let (mut state, keys) = setup();
    // Side 0 has last_move = Pound, has NOT moved this turn (user is faster)
    state.sides[0].active.last_move = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 227, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].active.encore_turns, 3, "Target hasn't moved => duration 3");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_encore_fails_failencore_move() {
    let (mut state, keys) = setup();
    // Side 0's last move was Encore itself (227) — should fail
    state.sides[0].active.last_move = 227;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 227, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].active.encore_move, 0, "Encore should fail on failencore moves");
    assert_eq!(state.sides[0].active.encore_turns, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_encore_fails_zero_pp() {
    let (mut state, keys) = setup();
    // Side 0 used Pound last turn, but Pound now has 0 PP
    state.sides[0].active.last_move = 1;
    state.sides[0].team[0].pp[0] = 0; // Pound in slot 0 has 0 PP
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 227, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].active.encore_move, 0, "Encore should fail with 0 PP");
    assert_eq!(state.sides[0].active.encore_turns, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_disable_blocks_move() {
    let (mut state, keys) = setup();
    // Side 0 used Thunderbolt (85) last turn
    state.sides[0].active.last_move = 85;
    state.zobrist = compute_full_hash(&state, &keys);

    // Side 1 uses Disable (50) on side 0
    execute_move(&mut state, &keys, 1, 50, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].active.disabled_move, 85);
    assert!(state.sides[0].active.disable_turns > 0);

    // Thunderbolt (slot 1) should not be in legal actions
    let actions = legal_actions(&state, 0);
    let move_actions: Vec<u8> = actions.as_slice().iter()
        .copied().filter(|&a| a <= 3).collect();
    assert!(!move_actions.contains(&1), "Disabled move slot 1 should not be legal");
    assert!(move_actions.len() >= 2, "Other moves should remain legal");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_disable_expires() {
    let (mut state, keys) = setup();
    state.sides[0].active.disabled_move = 85;
    state.sides[0].active.disable_turns = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].active.disable_turns, 0, "Disable should expire");
    assert_eq!(state.sides[0].active.disabled_move, 0, "Disabled move should be cleared");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_disable_duration_turn_order() {
    let (mut state, keys) = setup();
    // Target already moved this turn
    state.sides[0].active.last_move = 85;
    set_volatile(&mut state, &keys, 0, VOL_MOVED_THIS_TURN);
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 50, 0, &mut dummy_rng);
    assert_eq!(state.sides[0].active.disable_turns, 5, "Target already moved => duration 5");

    // Reset and test when target hasn't moved
    let (mut state2, keys2) = setup();
    state2.sides[0].active.last_move = 85;
    state2.zobrist = compute_full_hash(&state2, &keys2);

    execute_move(&mut state2, &keys2, 1, 50, 0, &mut dummy_rng);
    assert_eq!(state2.sides[0].active.disable_turns, 4, "Target hasn't moved => duration 4");
    assert!(validate_hash(&state2, &keys2));
}

#[test]
fn test_disable_fails_no_last_move() {
    let (mut state, keys) = setup();
    state.sides[0].active.last_move = 0;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 50, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].active.disabled_move, 0, "Disable should fail with no last_move");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_disable_fails_zero_pp() {
    let (mut state, keys) = setup();
    state.sides[0].active.last_move = 1; // Pound
    state.sides[0].team[0].pp[0] = 0; // Pound has 0 PP
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 50, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].active.disabled_move, 0, "Disable should fail with 0 PP");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_taunt_blocks_status() {
    let (mut state, keys) = setup();
    state.zobrist = compute_full_hash(&state, &keys);

    // Side 1 uses Taunt (269) on side 0
    execute_move(&mut state, &keys, 1, 269, 0, &mut dummy_rng);

    assert!(state.sides[0].active.taunt_turns > 0, "Taunt should set taunt_turns");

    // Side 0 moves: [1(Pound), 85(Thunderbolt), 269(Taunt), 227(Encore)]
    // Taunt and Encore are Status — they should be blocked
    let actions = legal_actions(&state, 0);
    let move_actions: Vec<u8> = actions.as_slice().iter()
        .copied().filter(|&a| a <= 3).collect();
    // Only Pound (slot 0) and Thunderbolt (slot 1) should be legal
    assert!(move_actions.contains(&0), "Pound should be legal under Taunt");
    assert!(move_actions.contains(&1), "Thunderbolt should be legal under Taunt");
    assert!(!move_actions.contains(&2), "Taunt (Status) should be blocked");
    assert!(!move_actions.contains(&3), "Encore (Status) should be blocked");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_taunt_expires() {
    let (mut state, keys) = setup();
    state.sides[0].active.taunt_turns = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].active.taunt_turns, 0, "Taunt should expire");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_taunt_duration_turn_order() {
    let (mut state, keys) = setup();
    // Target on field for >0 turns and already moved this turn
    state.sides[0].active.turns_active = 1;
    set_volatile(&mut state, &keys, 0, VOL_MOVED_THIS_TURN);
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 269, 0, &mut dummy_rng);
    assert_eq!(state.sides[0].active.taunt_turns, 4, "activeTurns>0 && already moved => 4");

    // First turn on field (turns_active == 0) — even if moved, no +1
    let (mut state2, keys2) = setup();
    state2.sides[0].active.turns_active = 0;
    set_volatile(&mut state2, &keys2, 0, VOL_MOVED_THIS_TURN);
    state2.zobrist = compute_full_hash(&state2, &keys2);

    execute_move(&mut state2, &keys2, 1, 269, 0, &mut dummy_rng);
    assert_eq!(state2.sides[0].active.taunt_turns, 3, "First turn on field => 3 even if moved");
    assert!(validate_hash(&state2, &keys2));
}

#[test]
fn test_taunt_fails_already_taunted() {
    let (mut state, keys) = setup();
    state.sides[0].active.taunt_turns = 2;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 269, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].active.taunt_turns, 2, "Should not re-apply Taunt");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_torment_blocks_repeat() {
    let (mut state, keys) = setup();
    // Apply Torment to side 0
    set_volatile(&mut state, &keys, 0, VOL_TORMENT);
    // Side 0's last move was Pound (1, slot 0)
    state.sides[0].active.last_move = 1;
    state.zobrist = compute_full_hash(&state, &keys);

    let actions = legal_actions(&state, 0);
    let move_actions: Vec<u8> = actions.as_slice().iter()
        .copied().filter(|&a| a <= 3).collect();
    assert!(!move_actions.contains(&0), "Torment should block repeat of Pound (slot 0)");
    assert!(move_actions.len() >= 2, "Other moves should remain legal");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_torment_clears_on_switch() {
    let (mut state, keys) = setup();
    set_volatile(&mut state, &keys, 0, VOL_TORMENT);
    assert!(state.sides[0].active.has_volatile(VOL_TORMENT));
    state.zobrist = compute_full_hash(&state, &keys);

    // Switch side 0 to slot 1
    pkmn_engine::state::switch::switch_out(&mut state, &keys, 0);
    pkmn_engine::state::switch::switch_in(&mut state, &keys, 0, 1);

    assert!(!state.sides[0].active.has_volatile(VOL_TORMENT),
        "Torment should be cleared on switch");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_torment_via_move() {
    let (mut state, keys) = setup();
    // Give side 1 Torment (259) in slot 0
    state.sides[1].team[0].moves[0] = 259;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 259, 0, &mut dummy_rng);

    assert!(state.sides[0].active.has_volatile(VOL_TORMENT),
        "Torment move should set VOL_TORMENT");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_torment_fails_already_set() {
    let (mut state, keys) = setup();
    set_volatile(&mut state, &keys, 0, VOL_TORMENT);
    let hash_before = state.zobrist;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 1, 259, 0, &mut dummy_rng);

    // Should still have Torment, hash unchanged (no double-toggle)
    assert!(state.sides[0].active.has_volatile(VOL_TORMENT));
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_encore_pp_early_termination() {
    let (mut state, keys) = setup();
    // Set encore with 3 turns remaining, but the encored move has 0 PP
    state.sides[0].active.encore_move = 1; // Pound
    state.sides[0].active.encore_turns = 3;
    state.sides[0].team[0].pp[0] = 0; // Pound has 0 PP
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].active.encore_turns, 0, "Encore should end early with 0 PP");
    assert_eq!(state.sides[0].active.encore_move, 0, "Encore move should be cleared");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_healing_wish_user_faints() {
    let (mut state, keys) = setup();
    // Give side 0 Healing Wish as move 0 (move ID 361)
    state.sides[0].team[0].moves[0] = 361;
    state.sides[0].team[0].pp[0] = 10;
    state.zobrist = compute_full_hash(&state, &keys);

    execute_move(&mut state, &keys, 0, 361, 0, &mut dummy_rng);

    assert_eq!(state.sides[0].team[0].current_hp, 0,
        "Healing Wish user should faint");
    assert!(state.sides[0].side_conditions.has_healing_wish(),
        "Healing Wish flag should be set on the side");
    assert!(validate_hash(&state, &keys));
}
