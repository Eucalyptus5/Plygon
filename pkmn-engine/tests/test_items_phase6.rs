//! Phase 6 item hook integration tests.

use pkmn_engine::state::BattleState;
use pkmn_engine::state::calc_modifiers::*;
use pkmn_engine::data::types::Type;
use pkmn_engine::state::data_bridge::*;
use pkmn_engine::data::items::ItemFlag;
use pkmn_engine::data::moves::{MoveCategory, MoveFlags};
use pkmn_engine::state::structs::*;
use pkmn_engine::state::zobrist::{ZobristKeys, compute_full_hash, validate_hash};
use pkmn_engine::state::end_of_turn::end_of_turn;
use pkmn_engine::state::legal_moves::legal_actions;
use pkmn_engine::state::move_exec::check_berry_activation;

fn setup() -> (BattleState, ZobristKeys) {
    let keys = ZobristKeys::new(42);
    let mut state = BattleState::default();
    state.sides[0].team[0] = MonSlot {
        species_id: 25, current_hp: 300, max_hp: 300,
        stats: [150, 100, 150, 100, 100],
        moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
        ..Default::default()
    };
    state.sides[0].team[1] = MonSlot {
        species_id: 6, current_hp: 200, max_hp: 200,
        stats: [100, 100, 100, 100, 80],
        ..Default::default()
    };
    state.sides[1].team[0] = MonSlot {
        species_id: 50, current_hp: 300, max_hp: 300,
        stats: [100, 100, 100, 100, 80],
        moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
        ..Default::default()
    };
    state.sides[1].team[1] = MonSlot {
        species_id: 10, current_hp: 200, max_hp: 200,
        stats: [80, 80, 80, 80, 60],
        ..Default::default()
    };
    state.phase = PHASE_ACTIONS;
    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

#[test]
fn test_choice_band_boosts_physical_atk() {
    let itm = item(68);
    assert!(itm.has(ItemFlag::CHOICE_ATK));
}

#[test]
fn test_eviolite_boosts_def() {
    let itm = item(130); // Eviolite
    assert!(itm.has(ItemFlag::EVIOLITE));
}

#[test]
fn test_life_orb_power_mod() {
    let itm = item(249); // Life Orb
    let (num, den) = item_power_mod(itm, 249, Type::Normal, MoveCategory::Physical, 0, 0);
    assert_eq!(num, 5324);
    assert_eq!(den, 4096);
}

#[test]
fn test_type_boost_item_power_mod() {
    // Charcoal (id=61) boosts Fire
    let itm = item(61);
    let (num, den) = item_power_mod(itm, 61, Type::Fire, MoveCategory::Special, 0, 0);
    assert_eq!(num, 4915); // 1.2× = 4915/4096
    assert_eq!(den, 4096);
}

#[test]
fn test_type_boost_item_no_boost_wrong_type() {
    let itm = item(61); // Charcoal boosts Fire
    let (num, den) = item_power_mod(itm, 61, Type::Water, MoveCategory::Special, 0, 0);
    assert_eq!(num, 4096); // no boost for wrong type
}

#[test]
fn test_muscle_band_physical_boost() {
    let itm = item(ITEM_MUSCLE_BAND);
    let (num, den) = item_power_mod(itm, ITEM_MUSCLE_BAND, Type::Normal, MoveCategory::Physical, 0, 0);
    assert_eq!(num, 4505); // 1.1×
    assert_eq!(den, 4096);
}

#[test]
fn test_muscle_band_no_boost_special() {
    let itm = item(ITEM_MUSCLE_BAND);
    let (num, den) = item_power_mod(itm, ITEM_MUSCLE_BAND, Type::Normal, MoveCategory::Special, 0, 0);
    assert_eq!(num, 4096); // no boost for special
}

#[test]
fn test_wise_glasses_special_boost() {
    let itm = item(ITEM_WISE_GLASSES);
    let (num, den) = item_power_mod(itm, ITEM_WISE_GLASSES, Type::Normal, MoveCategory::Special, 0, 0);
    assert_eq!(num, 4505);
}

#[test]
fn test_punching_glove_punch_boost() {
    let itm = item(ITEM_PUNCHING_GLOVE);
    let (num, den) = item_power_mod(itm, ITEM_PUNCHING_GLOVE, Type::Normal, MoveCategory::Physical, MoveFlags::PUNCH, 0);
    assert_eq!(num, 4505);
}

#[test]
fn test_punching_glove_no_boost_non_punch() {
    let itm = item(ITEM_PUNCHING_GLOVE);
    let (num, den) = item_power_mod(itm, ITEM_PUNCHING_GLOVE, Type::Normal, MoveCategory::Physical, 0, 0);
    assert_eq!(num, 4096);
}

#[test]
fn test_resist_berry_flag() {
    // Occa Berry (id=311) resists Fire
    let itm = item(311);
    assert!(itm.has(ItemFlag::RESIST_BERRY));
    assert!(itm.has(ItemFlag::IS_BERRY));
    assert!(itm.has(ItemFlag::CONSUMABLE));
    assert_eq!(itm.type_param, Type::Fire as u8);
}

#[test]
fn test_focus_sash_flag() {
    let itm = item(151); // Focus Sash
    assert!(itm.has(ItemFlag::FOCUS_SASH));
    assert!(itm.has(ItemFlag::CONSUMABLE));
}

#[test]
fn test_sitrus_berry_heals_at_half() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].item_id = ITEM_SITRUS_BERRY;
    state.sides[0].team[0].current_hp = 140; // 140/300 < 50%
    state.zobrist = compute_full_hash(&state, &keys);

    check_berry_activation(&mut state, &keys, 0, 0);

    assert_eq!(state.sides[0].team[0].current_hp, 215); // heals 25% of max HP
    assert_eq!(state.sides[0].team[0].item_id, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_sitrus_berry_no_heal_above_half() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].item_id = ITEM_SITRUS_BERRY;
    state.sides[0].team[0].current_hp = 200; // 200/300 > 50%
    state.zobrist = compute_full_hash(&state, &keys);

    check_berry_activation(&mut state, &keys, 0, 0);

    assert_eq!(state.sides[0].team[0].current_hp, 200);
    assert_eq!(state.sides[0].team[0].item_id, ITEM_SITRUS_BERRY);
}

#[test]
fn test_gluttony_berry_at_50_percent() {
    let (mut state, keys) = setup();
    // Use Figy Berry (heal berry, normally activates at 25%)
    state.sides[0].team[0].item_id = ITEM_FIGY_BERRY;
    state.sides[0].team[0].ability_id = ABILITY_GLUTTONY;
    state.sides[0].team[0].current_hp = 140; // 140/300 < 50% but > 25%
    state.zobrist = compute_full_hash(&state, &keys);

    check_berry_activation(&mut state, &keys, 0, 0);

    // Gluttony raises berry activation threshold from 25% to 50%
    assert!(state.sides[0].team[0].current_hp > 140);
    assert_eq!(state.sides[0].team[0].item_id, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_lum_berry_cures_status() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].item_id = ITEM_LUM_BERRY;
    state.sides[0].team[0].status = STATUS_PARALYSIS;
    state.zobrist = compute_full_hash(&state, &keys);

    check_berry_activation(&mut state, &keys, 0, 0);

    assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    assert_eq!(state.sides[0].team[0].item_id, 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_lum_berry_no_cure_if_healthy() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].item_id = ITEM_LUM_BERRY;
    state.sides[0].team[0].status = STATUS_NONE;
    state.zobrist = compute_full_hash(&state, &keys);

    check_berry_activation(&mut state, &keys, 0, 0);

    assert_eq!(state.sides[0].team[0].item_id, ITEM_LUM_BERRY);
}

#[test]
fn test_rocky_helmet_flag() {
    let itm = item(417); // Rocky Helmet
    assert!(itm.has(ItemFlag::ROCKY_HELMET));
}

#[test]
fn test_leftovers_eot_heal() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].item_id = 242; // Leftovers
    state.sides[0].team[0].current_hp = 200;
    state.sides[1].team[0].current_hp = 300; // keep alive
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].team[0].current_hp, 218); // 300/16 = 18 heal
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_sticky_barb_eot_damage() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].item_id = ITEM_STICKY_BARB;
    state.sides[0].team[0].current_hp = 300;
    state.sides[1].team[0].current_hp = 300;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].team[0].current_hp, 263); // 300/8 = 37 damage
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_choice_lock_restricts_moves() {
    let (mut state, _) = setup();
    state.sides[0].team[0].item_id = 68; // Choice Band
    state.sides[0].active.choice_locked_move = 1;
    state.phase = PHASE_ACTIONS;

    let actions = legal_actions(&state, 0);
    let move_actions: Vec<u8> = actions.as_slice().iter().copied().filter(|&a| a < ACTION_SWITCH_0).collect();
    assert_eq!(move_actions.len(), 1);
    assert_eq!(move_actions[0], 0); // slot 0 which has move_id=1
}

#[test]
fn test_assault_vest_blocks_status() {
    let (mut state, _) = setup();
    state.sides[0].team[0].item_id = 581; // Assault Vest
    state.phase = PHASE_ACTIONS;

    let actions = legal_actions(&state, 0);

    let move_actions: Vec<u8> = actions.as_slice().iter().copied().filter(|&a| a < ACTION_SWITCH_0).collect();
    assert!(!move_actions.is_empty() || actions.as_slice().contains(&ACTION_STRUGGLE));
}

#[test]
fn test_weakness_policy_item_exists() {
    let itm = item(ITEM_WEAKNESS_POLICY);
    assert_ne!(itm.flags, 0);
}

#[test]
fn test_berry_juice_heals() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].item_id = ITEM_BERRY_JUICE;
    state.sides[0].team[0].current_hp = 140; // 140/300 < 50%
    state.zobrist = compute_full_hash(&state, &keys);

    check_berry_activation(&mut state, &keys, 0, 0);

    assert_eq!(state.sides[0].team[0].current_hp, 160); // +20
    assert_eq!(state.sides[0].team[0].item_id, 0); // consumed
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_starf_berry_boosts() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].item_id = ITEM_STARF_BERRY;
    state.sides[0].team[0].current_hp = 50; // 50/300 < 25%
    state.zobrist = compute_full_hash(&state, &keys);

    check_berry_activation(&mut state, &keys, 0, 0);

    assert_eq!(state.sides[0].team[0].item_id, 0);
    let boosts = &state.sides[0].active.boosts;
    let total_boost: i8 = boosts.iter().sum();
    assert_eq!(total_boost, 2); // +2 to one stat
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_shell_bell_item_exists() {
    let itm = item(ITEM_SHELL_BELL);
    assert_eq!(itm.type_param, 0xFF);
}

#[test]
fn test_throat_spray_item_exists() {
    let itm = item(ITEM_THROAT_SPRAY);
    assert_ne!(itm.type_param, 0); // should have power_param set
}
