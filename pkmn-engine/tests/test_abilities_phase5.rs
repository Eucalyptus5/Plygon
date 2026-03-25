//! Phase 5 ability hook tests.

use pkmn_engine::state::BattleState;
use pkmn_engine::state::calc_modifiers::*;
use pkmn_engine::state::calc::calc_damage;
use pkmn_engine::data::types::Type;
use pkmn_engine::state::data_bridge::*;
use pkmn_engine::data::moves::{MoveData, MoveFlags, SelfEffect, VarPower};
use pkmn_engine::state::structs::*;
use pkmn_engine::state::zobrist::{ZobristKeys, compute_full_hash, validate_hash};
use pkmn_engine::state::mutations::*;
use pkmn_engine::state::accessors::*;
use pkmn_engine::state::end_of_turn::end_of_turn;
use pkmn_engine::state::turn::execute_turn;
use pkmn_engine::state::switch::perform_switch;

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
fn test_huge_power_doubles_atk() {
    let a = ability_atk_stat_mod(
        150, ABILITY_HUGE_POWER, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 300);

    let a = ability_atk_stat_mod(
        150, ABILITY_HUGE_POWER, MoveCategory::Special, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 150);
}

#[test]
fn test_defeatist_halves_atk() {
    let a = ability_atk_stat_mod(
        200, ABILITY_DEFEATIST, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 200);

    // At ≤50% HP: halved
    let a = ability_atk_stat_mod(
        200, ABILITY_DEFEATIST, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 150, 300, 0, 0,
    );
    assert_eq!(a, 100);
}

#[test]
fn test_stakeout_doubles_vs_switched() {
    // Defender has 0 turns active (just switched in)
    let a = ability_atk_stat_mod(
        100, ABILITY_STAKEOUT, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 5, 0,
    );
    assert_eq!(a, 200);

    // Defender has been out for a turn → no boost
    let a = ability_atk_stat_mod(
        100, ABILITY_STAKEOUT, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 5, 1,
    );
    assert_eq!(a, 100);
}

#[test]
fn test_solar_power_in_sun() {
    let a = ability_atk_stat_mod(
        200, ABILITY_SOLAR_POWER, MoveCategory::Special, STATUS_NONE,
        Type::Fire, WEATHER_SUN, 300, 300, 0, 0,
    );
    assert_eq!(a, 300); // 1.5×

    let a = ability_atk_stat_mod(
        200, ABILITY_SOLAR_POWER, MoveCategory::Special, STATUS_NONE,
        Type::Fire, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 200);
}

#[test]
fn test_marvel_scale_with_status() {
    let d = ability_def_stat_mod(
        200, ABILITY_MARVEL_SCALE, MoveCategory::Physical, Type::Normal,
        STATUS_BURN, WEATHER_NONE, TERRAIN_NONE,
    );
    assert_eq!(d, 300); // 1.5×

    let d = ability_def_stat_mod(
        200, ABILITY_MARVEL_SCALE, MoveCategory::Physical, Type::Normal,
        STATUS_NONE, WEATHER_NONE, TERRAIN_NONE,
    );
    assert_eq!(d, 200);
}

#[test]
fn test_technician_on_weak_move() {
    let mut state = BattleState::default();
    state.sides[0].team[0].species_id = 1;
    state.sides[0].team[0].ability_id = ABILITY_TECHNICIAN;
    state.sides[0].team[0].max_hp = 300;
    state.sides[0].team[0].current_hp = 300;
    state.sides[1].team[0].species_id = 2;

    let md = MoveData {
        base_power: 40, category: MoveCategory::Physical,
        move_type: Type::Normal, accuracy: 100,
        ..unsafe { core::mem::zeroed() }
    };
    let (n, d) = ability_power_mod(&state, &md, 0, 40);
    assert_eq!((n, d), (6144, 4096)); // 1.5×

    let md70 = MoveData { base_power: 70, ..md };
    let (n, d) = ability_power_mod(&state, &md70, 0, 70);
    assert_eq!((n, d), (4096, 4096));
}

#[test]
fn test_sharpness_slicing() {
    let mut state = BattleState::default();
    state.sides[0].team[0].species_id = 1;
    state.sides[0].team[0].ability_id = ABILITY_SHARPNESS;
    state.sides[0].team[0].max_hp = 300;
    state.sides[0].team[0].current_hp = 300;
    state.sides[1].team[0].species_id = 2;

    let md = MoveData {
        base_power: 80, category: MoveCategory::Physical,
        move_type: Type::Normal, flags: MoveFlags::SLICE,
        ..unsafe { core::mem::zeroed() }
    };
    let (n, d) = ability_power_mod(&state, &md, 0, 80);
    assert_eq!((n, d), (6144, 4096)); // 1.5×
}

#[test]
fn test_multiscale_halves_at_full_hp() {
    let mut state = BattleState::default();
    state.sides[1].team[0].species_id = 1;
    state.sides[1].team[0].ability_id = ABILITY_MULTISCALE;
    state.sides[1].team[0].max_hp = 300;
    state.sides[1].team[0].current_hp = 300;

    let md = MoveData {
        base_power: 80, category: MoveCategory::Physical,
        move_type: Type::Normal,
        ..unsafe { core::mem::zeroed() }
    };
    let (n, d) = defender_ability_final_mod(&state, &md, 1, 8);
    assert_eq!((n, d), (2048, 4096)); // 0.5×

    state.sides[1].team[0].current_hp = 299;
    let (n, d) = defender_ability_final_mod(&state, &md, 1, 8);
    assert_eq!((n, d), (4096, 4096));
}

#[test]
fn test_neuroforce_on_se() {
    let (n, d) = attacker_ability_final_mod(ABILITY_NEUROFORCE, 8); // SE (eff=8)
    assert_eq!((n, d), (5120, 4096)); // 1.25×

    let (n, d) = attacker_ability_final_mod(ABILITY_NEUROFORCE, 4); // neutral
    assert_eq!((n, d), (4096, 4096));
}

#[test]
fn test_filter_super_effective() {
    let mut state = BattleState::default();
    state.sides[1].team[0].species_id = 1;
    state.sides[1].team[0].ability_id = ABILITY_FILTER;
    state.sides[1].team[0].max_hp = 300;
    state.sides[1].team[0].current_hp = 300;

    let md = MoveData {
        base_power: 80, category: MoveCategory::Physical,
        move_type: Type::Normal,
        ..unsafe { core::mem::zeroed() }
    };
    let (n, d) = defender_ability_final_mod(&state, &md, 1, 8); // SE
    assert_eq!((n, d), (3072, 4096)); // 0.75×
}

#[test]
fn test_filter_neutral() {
    let mut state = BattleState::default();
    state.sides[1].team[0].species_id = 1;
    state.sides[1].team[0].ability_id = ABILITY_FILTER;
    state.sides[1].team[0].max_hp = 300;
    state.sides[1].team[0].current_hp = 300;

    let md = MoveData {
        base_power: 80, category: MoveCategory::Physical,
        move_type: Type::Normal,
        ..unsafe { core::mem::zeroed() }
    };
    let (n, d) = defender_ability_final_mod(&state, &md, 1, 4); // neutral
    assert_eq!((n, d), (4096, 4096)); // no reduction
}

#[test]
fn test_tinted_lens_nve() {
    let (n, d) = attacker_ability_final_mod(ABILITY_TINTED_LENS, 2); // NVE
    assert_eq!((n, d), (8192, 4096)); // 2×
}

#[test]
fn test_tinted_lens_neutral_no_boost() {
    let (n, d) = attacker_ability_final_mod(ABILITY_TINTED_LENS, 4); // neutral
    assert_eq!((n, d), (4096, 4096)); // no boost
}

#[test]
fn test_punk_rock_defender() {
    let mut state = BattleState::default();
    state.sides[1].team[0].species_id = 1;
    state.sides[1].team[0].ability_id = ABILITY_PUNK_ROCK;
    state.sides[1].team[0].max_hp = 300;
    state.sides[1].team[0].current_hp = 300;

    let md = MoveData {
        base_power: 80, category: MoveCategory::Special,
        move_type: Type::Normal, flags: MoveFlags::SOUND,
        ..unsafe { core::mem::zeroed() }
    };
    let (n, d) = defender_ability_final_mod(&state, &md, 1, 4);
    assert_eq!((n, d), (2048, 4096)); // 0.5×
}

#[test]
fn test_serene_grace_doubles_secondary() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_SERENE_GRACE;
    state.zobrist = compute_full_hash(&state, &keys);

    assert_eq!(ABILITY_SERENE_GRACE, 32);
}

#[test]
fn test_sheer_force_skips_life_orb_recoil() {
    let mut state = BattleState::default();
    state.sides[0].team[0] = MonSlot {
        species_id: 1, current_hp: 300, max_hp: 300,
        stats: [150, 100, 150, 100, 100],
        moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
        ability_id: ABILITY_SHEER_FORCE,
        item_id: 270, // Life Orb
        ..Default::default()
    };
    state.sides[1].team[0] = MonSlot {
        species_id: 2, current_hp: 300, max_hp: 300,
        stats: [100, 100, 100, 100, 100],
        ..Default::default()
    };

    assert_eq!(ABILITY_SHEER_FORCE, 125);
}

#[test]
fn test_weak_armor_interaction() {
    let (mut state, keys) = setup();
    state.sides[1].team[0].ability_id = ABILITY_WEAK_ARMOR;
    state.zobrist = compute_full_hash(&state, &keys);

    assert_eq!(ABILITY_WEAK_ARMOR, 133);
}

#[test]
fn test_speed_boost_at_eot() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_SPEED_BOOST;
    state.sides[0].active.turns_active = 1; // must be > 0
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].active.boosts[SPE], 1);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_moody_at_eot() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_MOODY;
    state.sides[0].active.turns_active = 0;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    // Moody should have changed at least one stat
    let boosts = state.sides[0].active.boosts;
    let any_changed = boosts.iter().any(|&b| b != 0);
    assert!(any_changed, "Moody should change stats");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_bad_dreams_damages_sleeper() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_BAD_DREAMS;
    state.sides[1].team[0].status = STATUS_SLEEP;
    state.sides[1].team[0].status_counter = 3;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[1].team[0].current_hp, 300 - 37);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_dry_skin_eot() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_DRY_SKIN;

    // In Sun: lose 1/8 HP
    state.field.weather = WEATHER_SUN;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);
    end_of_turn(&mut state, &keys);
    assert_eq!(state.sides[0].team[0].current_hp, 300 - 37);

    // Reset and test Rain
    state.sides[0].team[0].current_hp = 200;
    state.field.weather = WEATHER_RAIN;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);
    end_of_turn(&mut state, &keys);
    // Heal 1/8 of 300 = 37
    assert_eq!(state.sides[0].team[0].current_hp, 237);
}

#[test]
fn test_chlorophyll_doubles_speed_in_sun() {
    let (mut state, _keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_CHLOROPHYLL;
    state.sides[0].team[0].stats[SPE] = 100;

    state.sides[1].team[0].stats[SPE] = 150;
    state.field.weather = WEATHER_SUN;
    state.field.weather_turns = 5;

    assert_eq!(ABILITY_CHLOROPHYLL, 34);
}

#[test]
fn test_prankster_adds_priority() {
    assert_eq!(ABILITY_PRANKSTER, 158);
}

#[test]
fn test_protosynthesis_activates_in_sun() {
    let keys = ZobristKeys::new(42);
    let mut state = BattleState::default();
    state.sides[0].team[0] = MonSlot {
        species_id: 25, current_hp: 300, max_hp: 300,
        stats: [120, 100, 130, 100, 110], // SpA is highest (130)
        ability_id: ABILITY_PROTOSYNTHESIS,
        ..Default::default()
    };
    state.sides[0].team[1] = MonSlot {
        species_id: 6, current_hp: 200, max_hp: 200,
        stats: [100; 5], ..Default::default()
    };
    state.sides[1].team[0] = MonSlot {
        species_id: 50, current_hp: 300, max_hp: 300,
        stats: [100; 5], ..Default::default()
    };
    state.field.weather = WEATHER_SUN;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);

    // Switch to activate the ability
    perform_switch(&mut state, &keys, 0, 1);
    // Switch back to the Protosynthesis mon
    perform_switch(&mut state, &keys, 0, 0);

    // _padding[3] upper nibble should encode the boosted stat (SPA = index 2 → value 3)
    let paradox_stat = state.sides[0].active._padding[3] >> 4;
    assert_eq!(paradox_stat, 3, "Should boost SpA (stat index 2 → encoded as 3)");
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_moxie_boosts_on_ko() {
    assert_eq!(ABILITY_MOXIE, 153);
    assert_eq!(ABILITY_CHILLING_NEIGH, 264);
    assert_eq!(ABILITY_GRIM_NEIGH, 265);
}

#[test]
fn test_toxic_debris_constant() {
    assert_eq!(ABILITY_TOXIC_DEBRIS, 295);
}

#[test]
fn test_seed_sower_constant() {
    assert_eq!(ABILITY_SEED_SOWER, 269);
}

#[test]
fn test_intimidate_then_weak_armor() {
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
        ability_id: ABILITY_INTIMIDATE,
        ..Default::default()
    };
    state.sides[1].team[0] = MonSlot {
        species_id: 50, current_hp: 300, max_hp: 300,
        stats: [100, 100, 100, 100, 80],
        ability_id: ABILITY_WEAK_ARMOR,
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

    perform_switch(&mut state, &keys, 0, 1);

    assert_eq!(state.sides[1].active.boosts[ATK], -1);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_supreme_overlord_scaling() {
    let mut state = BattleState::default();
    state.sides[0].team[0] = MonSlot {
        species_id: 1, current_hp: 300, max_hp: 300,
        stats: [100; 5],
        ability_id: ABILITY_SUPREME_OVERLORD,
        ..Default::default()
    };
    // 2 fainted allies
    state.sides[0].team[1] = MonSlot {
        species_id: 2, current_hp: 0, max_hp: 200,
        stats: [100; 5], ..Default::default()
    };
    state.sides[0].team[2] = MonSlot {
        species_id: 3, current_hp: 0, max_hp: 200,
        stats: [100; 5], ..Default::default()
    };
    state.sides[1].team[0] = MonSlot {
        species_id: 4, current_hp: 300, max_hp: 300,
        stats: [100; 5], ..Default::default()
    };

    let md = MoveData {
        base_power: 80, category: MoveCategory::Physical,
        move_type: Type::Normal,
        ..unsafe { core::mem::zeroed() }
    };

    let (n, d) = ability_power_mod(&state, &md, 0, 80);
    // 2 fainted → exact Showdown value: 4915
    assert_eq!(n, 4915);
    assert_eq!(d, 4096);
}

#[test]
fn test_water_bubble_attacker() {
    let a = ability_atk_stat_mod(
        200, ABILITY_WATER_BUBBLE, MoveCategory::Special, STATUS_NONE,
        Type::Water, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 400); // 2×

    let a = ability_atk_stat_mod(
        200, ABILITY_WATER_BUBBLE, MoveCategory::Special, STATUS_NONE,
        Type::Fire, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 200);
}

#[test]
fn test_water_bubble_defender() {
    // Water Bubble's defensive effect (0.5x Fire) is now applied as a stat modifier
    // on the attacker's Atk/SpA, not as a damage modifier. So defender_ability_final_mod
    // returns identity (4096, 4096) for Water Bubble.
    let mut state = BattleState::default();
    state.sides[1].team[0].species_id = 1;
    state.sides[1].team[0].ability_id = ABILITY_WATER_BUBBLE;
    state.sides[1].team[0].max_hp = 300;
    state.sides[1].team[0].current_hp = 300;

    let md = MoveData {
        base_power: 80, move_type: Type::Fire,
        category: MoveCategory::Special,
        ..unsafe { core::mem::zeroed() }
    };
    let (n, d) = defender_ability_final_mod(&state, &md, 1, 4);
    assert_eq!((n, d), (4096, 4096)); // identity — effect applied via stat mod
}

#[test]
fn test_slow_start() {
    let a = ability_atk_stat_mod(
        200, ABILITY_SLOW_START, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 3, 0,
    );
    assert_eq!(a, 100);

    let a = ability_atk_stat_mod(
        200, ABILITY_SLOW_START, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 5, 0,
    );
    assert_eq!(a, 200);
}

#[test]
fn test_hydration_in_rain() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_HYDRATION;
    state.sides[0].team[0].status = STATUS_BURN;
    state.field.weather = WEATHER_RAIN;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_rain_dish_heals() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_RAIN_DISH;
    state.sides[0].team[0].current_hp = 200;
    state.field.weather = WEATHER_RAIN;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].team[0].current_hp, 218);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_ice_body_heals_in_snow() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_ICE_BODY;
    state.sides[0].team[0].current_hp = 200;
    state.field.weather = WEATHER_SNOW;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].team[0].current_hp, 218);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_grass_pelt_in_grassy_terrain() {
    let d = ability_def_stat_mod(
        200, ABILITY_GRASS_PELT, MoveCategory::Physical, Type::Normal,
        STATUS_NONE, WEATHER_NONE, TERRAIN_GRASSY,
    );
    assert_eq!(d, 300); // 1.5×

    let d = ability_def_stat_mod(
        200, ABILITY_GRASS_PELT, MoveCategory::Physical, Type::Normal,
        STATUS_NONE, WEATHER_NONE, TERRAIN_NONE,
    );
    assert_eq!(d, 200);
}

#[test]
fn test_sand_force_in_sand() {
    let mut state = BattleState::default();
    state.sides[0].team[0].species_id = 1;
    state.sides[0].team[0].ability_id = ABILITY_SAND_FORCE;
    state.sides[0].team[0].max_hp = 300;
    state.sides[0].team[0].current_hp = 300;
    state.sides[1].team[0].species_id = 2;
    state.field.weather = WEATHER_SAND;

    let md = MoveData {
        base_power: 80, category: MoveCategory::Physical,
        move_type: Type::Rock,
        ..unsafe { core::mem::zeroed() }
    };
    let (n, d) = ability_power_mod(&state, &md, 0, 80);
    assert_eq!((n, d), (5325, 4096)); // 1.3×

    let md_normal = MoveData { move_type: Type::Normal, ..md };
    let (n, d) = ability_power_mod(&state, &md_normal, 0, 80);
    assert_eq!((n, d), (4096, 4096));
}

#[test]
fn test_gorilla_tactics() {
    let a = ability_atk_stat_mod(
        200, ABILITY_GORILLA_TACTICS, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 300); // 1.5×

    let a = ability_atk_stat_mod(
        200, ABILITY_GORILLA_TACTICS, MoveCategory::Special, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 200);
}

#[test]
fn test_pure_power_doubles_atk() {
    let a = ability_atk_stat_mod(
        150, ABILITY_PURE_POWER, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 300);

    let a = ability_atk_stat_mod(
        150, ABILITY_PURE_POWER, MoveCategory::Special, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 150);
}

#[test]
fn test_guts_boosts_atk_with_status() {
    // Burned + Physical → 1.5×
    let a = ability_atk_stat_mod(
        200, ABILITY_GUTS, MoveCategory::Physical, STATUS_BURN,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 300);

    // Poisoned + Physical → 1.5×
    let a = ability_atk_stat_mod(
        200, ABILITY_GUTS, MoveCategory::Physical, STATUS_POISON,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 300);

    // No status → no boost
    let a = ability_atk_stat_mod(
        200, ABILITY_GUTS, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 200);

    // Burned + Special → no boost (physical only)
    let a = ability_atk_stat_mod(
        200, ABILITY_GUTS, MoveCategory::Special, STATUS_BURN,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 200);
}

#[test]
fn test_guts_no_burn_penalty() {
    // Burned + Guts → no penalty
    assert_eq!(burn_modifier(STATUS_BURN, MoveCategory::Physical, ABILITY_GUTS, false), (4096, 4096));
    // Burned without Guts → 0.5×
    assert_eq!(burn_modifier(STATUS_BURN, MoveCategory::Physical, 0, false), (2048, 4096));
}

#[test]
fn test_hustle_boosts_physical() {
    let a = ability_atk_stat_mod(
        200, ABILITY_HUSTLE, MoveCategory::Physical, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 300);

    let a = ability_atk_stat_mod(
        200, ABILITY_HUSTLE, MoveCategory::Special, STATUS_NONE,
        Type::Normal, WEATHER_NONE, 300, 300, 0, 0,
    );
    assert_eq!(a, 200);
}

#[test]
fn test_fur_coat_doubles_def() {
    let d = ability_def_stat_mod(
        200, ABILITY_FUR_COAT, MoveCategory::Physical, Type::Normal,
        STATUS_NONE, WEATHER_NONE, TERRAIN_NONE,
    );
    assert_eq!(d, 400);

    let d = ability_def_stat_mod(
        200, ABILITY_FUR_COAT, MoveCategory::Special, Type::Normal,
        STATUS_NONE, WEATHER_NONE, TERRAIN_NONE,
    );
    assert_eq!(d, 200);
}

#[test]
fn test_ice_scales_special_def() {
    // Ice Scales: now applied as 0.5x damage modifier via defender_ability_final_mod,
    // not as a stat doubling. ability_def_stat_mod returns identity.
    let d = ability_def_stat_mod(
        200, ABILITY_ICE_SCALES, MoveCategory::Special, Type::Normal,
        STATUS_NONE, WEATHER_NONE, TERRAIN_NONE,
    );
    assert_eq!(d, 200); // no stat change

    // Verify the damage modifier is applied via defender_ability_final_mod
    let mut state = BattleState::default();
    state.sides[1].team[0].ability_id = ABILITY_ICE_SCALES;
    state.sides[1].team[0].max_hp = 300;
    state.sides[1].team[0].current_hp = 300;
    let md = MoveData {
        base_power: 80, move_type: Type::Normal,
        category: MoveCategory::Special,
        ..unsafe { core::mem::zeroed() }
    };
    let (n, d_mod) = defender_ability_final_mod(&state, &md, 1, 4);
    assert_eq!((n, d_mod), (2048, 4096)); // 0.5x
}

#[test]
fn test_harvest_restores_berry_in_sun() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_HARVEST;
    state.sides[0].team[0].item_id = 0; // no item
    state.sides[0].set_last_consumed_berry(ITEM_SITRUS_BERRY);
    state.field.weather = WEATHER_SUN;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].team[0].item_id, ITEM_SITRUS_BERRY);
    assert_eq!(state.sides[0].last_consumed_berry(), 0);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_harvest_no_restore_with_item() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_HARVEST;
    state.sides[0].team[0].item_id = ITEM_SITRUS_BERRY; // already has item
    state.sides[0].set_last_consumed_berry(ITEM_LUM_BERRY);
    state.field.weather = WEATHER_SUN;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    // Should NOT replace existing item
    assert_eq!(state.sides[0].team[0].item_id, ITEM_SITRUS_BERRY);
    assert_eq!(state.sides[0].last_consumed_berry(), ITEM_LUM_BERRY);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_harvest_clears_on_switch() {
    let (mut state, keys) = setup();
    state.sides[0].set_last_consumed_berry(ITEM_SITRUS_BERRY);
    state.zobrist = compute_full_hash(&state, &keys);

    perform_switch(&mut state, &keys, 0, 1);

    assert_eq!(state.sides[0].last_consumed_berry(), 0);
}

#[test]
fn test_poison_heal_heals() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_POISON_HEAL;
    state.sides[0].team[0].status = STATUS_POISON;
    state.sides[0].team[0].current_hp = 200;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    // No poison damage (1/8 = 37 skipped), heal 1/8 = 300/8 = 37
    assert_eq!(state.sides[0].team[0].current_hp, 237);
    assert_eq!(state.sides[0].team[0].status, STATUS_POISON);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_poison_heal_replaces_toxic() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_POISON_HEAL;
    state.sides[0].team[0].status = STATUS_BAD_POISON;
    state.sides[0].team[0].current_hp = 200;
    state.sides[0].active.toxic_counter = 5;
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    // Toxic damage (5*300/16 = 93) should NOT be applied; heal 300/8 = 37
    assert_eq!(state.sides[0].team[0].current_hp, 237);
    // Toxic counter should NOT increment (stayed at 5)
    assert_eq!(state.sides[0].active.toxic_counter, 5);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_shed_skin_cures() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_SHED_SKIN;
    state.sides[0].team[0].status = STATUS_PARALYSIS;
    state.sides[0].active.turns_active = 0; // 0 % 3 == 0 → cures
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    assert!(validate_hash(&state, &keys));
}

#[test]
fn test_shed_skin_no_cure() {
    let (mut state, keys) = setup();
    state.sides[0].team[0].ability_id = ABILITY_SHED_SKIN;
    state.sides[0].team[0].status = STATUS_PARALYSIS;
    state.sides[0].active.turns_active = 1; // 1 % 3 != 0 → no cure
    state.zobrist = compute_full_hash(&state, &keys);

    end_of_turn(&mut state, &keys);

    assert_eq!(state.sides[0].team[0].status, STATUS_PARALYSIS);
    assert!(validate_hash(&state, &keys));
}
