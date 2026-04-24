//! Phase 2 Task 09: Item damage modifier integration tests.

use pkmn_engine::state::BattleState;
use pkmn_engine::state::calc::calc_damage;
use pkmn_engine::state::structs::*;
use pkmn_engine::state::data_bridge;
use pkmn_engine::state::move_exec::execute_move;
use pkmn_engine::data::types::Type;

fn setup() -> BattleState {
    let mut state = BattleState::default();
    // Attacker: side 0 — Bulbasaur (Grass/Poison), species_id=1
    state.sides[0].team[0] = MonSlot {
        species_id: 1, current_hp: 300, max_hp: 300,
        stats: [150, 100, 150, 100, 100],
        moves: [1, 7, 53, 173], pp: [24, 24, 24, 24],
        level: 100,
        // Pound(1)=Normal/Phys, FirePunch(7)=Fire/Phys, Flamethrower(53)=Fire/Spec, Snore(173)=Normal/Spec/Sound
        ..Default::default()
    };
    state.sides[0].team[1] = MonSlot {
        species_id: 6, current_hp: 200, max_hp: 200,
        stats: [100, 100, 100, 100, 80],
        moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
        level: 100,
        ..Default::default()
    };
    // Defender: side 1 — Charmander (Fire), species_id=4
    state.sides[1].team[0] = MonSlot {
        species_id: 4, current_hp: 300, max_hp: 300,
        stats: [100, 100, 100, 100, 80],
        moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
        level: 100,
        ..Default::default()
    };
    state.sides[1].team[1] = MonSlot {
        species_id: 10, current_hp: 200, max_hp: 200,
        stats: [80, 80, 80, 80, 60],
        moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
        level: 100,
        ..Default::default()
    };
    state.phase = PHASE_ACTIONS;
    state
}

/// No-crit, minimum roll RNG.
fn no_crit_rng(max: u32) -> u32 {
    if max == 24 { 1 } else { 0 }
}

#[test]
fn test_choice_band_physical() {
    let mut state = setup();

    // Baseline: no item, Pound (physical Normal, move_id=1)
    let res_base = calc_damage(&state, 0, 1, 0, &mut no_crit_rng);

    // With Choice Band (item 68)
    state.sides[0].team[0].item_id = 68;
    let res_band = calc_damage(&state, 0, 1, 0, &mut no_crit_rng);

    // Choice Band gives 1.5x Atk on physical moves
    assert!(res_band.damage > res_base.damage,
        "Choice Band should boost physical damage: {} vs {}", res_band.damage, res_base.damage);
    let ratio = res_band.damage as f64 / res_base.damage as f64;
    assert!((ratio - 1.5).abs() < 0.05,
        "Expected ~1.5x boost, got {:.3}x", ratio);
}

#[test]
fn test_eviolite_def_spd() {
    let mut state = setup();

    // Physical baseline (Pound, move_id=1)
    let res_phys_base = calc_damage(&state, 0, 1, 0, &mut no_crit_rng);
    // Special baseline (Flamethrower, move_id=53)
    let res_spec_base = calc_damage(&state, 0, 53, 0, &mut no_crit_rng);

    // Give defender Eviolite (item 130)
    state.sides[1].team[0].item_id = 130;
    let res_phys_evo = calc_damage(&state, 0, 1, 0, &mut no_crit_rng);
    let res_spec_evo = calc_damage(&state, 0, 53, 0, &mut no_crit_rng);

    // Eviolite gives 1.5x Def and SpD — damage should be ~2/3
    assert!(res_phys_evo.damage < res_phys_base.damage,
        "Eviolite should reduce physical damage: {} vs {}", res_phys_evo.damage, res_phys_base.damage);
    assert!(res_spec_evo.damage < res_spec_base.damage,
        "Eviolite should reduce special damage: {} vs {}", res_spec_evo.damage, res_spec_base.damage);

    let phys_ratio = res_phys_evo.damage as f64 / res_phys_base.damage as f64;
    assert!(phys_ratio < 0.72, "Expected ~0.67x physical damage, got {:.3}x", phys_ratio);
}

#[test]
fn test_life_orb_boost_and_recoil() {
    let mut state = setup();

    // Baseline: no item, Pound
    let res_base = calc_damage(&state, 0, 1, 0, &mut no_crit_rng);
    assert_eq!(res_base.recoil_damage, 0);

    // With Life Orb (item 249)
    state.sides[0].team[0].item_id = 249;
    let res_lo = calc_damage(&state, 0, 1, 0, &mut no_crit_rng);

    // 1.3x damage boost (5324/4096)
    assert!(res_lo.damage > res_base.damage,
        "Life Orb should boost damage: {} vs {}", res_lo.damage, res_base.damage);
    let ratio = res_lo.damage as f64 / res_base.damage as f64;
    assert!((ratio - 1.3).abs() < 0.05,
        "Expected ~1.3x boost, got {:.3}x", ratio);

    // 10% max HP recoil
    assert_eq!(res_lo.recoil_damage, 300 / 10,
        "Life Orb recoil should be 10% max HP (30), got {}", res_lo.recoil_damage);
}

#[test]
fn test_type_boost_item() {
    let mut state = setup();

    // Baseline: Fire Punch (move_id=7, Fire/Physical) without item
    let res_base = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);

    // With Charcoal (item 61, boosts Fire 1.2x)
    state.sides[0].team[0].item_id = 61;
    let res_boost = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);

    assert!(res_boost.damage > res_base.damage,
        "Charcoal should boost Fire damage: {} vs {}", res_boost.damage, res_base.damage);
    let ratio = res_boost.damage as f64 / res_base.damage as f64;
    assert!((ratio - 1.2).abs() < 0.05,
        "Expected ~1.2x boost, got {:.3}x", ratio);

    // Wrong type should not boost: Pound (Normal) with Charcoal (Fire boost)
    let res_wrong = calc_damage(&state, 0, 1, 0, &mut no_crit_rng);
    state.sides[0].team[0].item_id = 0;
    let res_no_item = calc_damage(&state, 0, 1, 0, &mut no_crit_rng);
    assert_eq!(res_wrong.damage, res_no_item.damage,
        "Charcoal should not boost non-Fire moves");
}

#[test]
fn test_gem_boost_and_consume() {
    let mut state = setup();

    // Baseline: Fire Punch without item
    let res_base = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);

    // With Fire Gem (item 141)
    state.sides[0].team[0].item_id = 141;
    let res_gem = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);

    // 1.3x damage boost (5325/4096)
    assert!(res_gem.damage > res_base.damage,
        "Fire Gem should boost Fire damage: {} vs {}", res_gem.damage, res_base.damage);
    let ratio = res_gem.damage as f64 / res_base.damage as f64;
    assert!((ratio - 1.3).abs() < 0.05,
        "Expected ~1.3x boost, got {:.3}x", ratio);

    // Gem should be marked for consumption
    assert!(res_gem.item_consumed,
        "Fire Gem should be consumed after boosting");
}

#[test]
fn test_resist_berry_halves() {
    let mut state = setup();

    // Make defender Grass-type so Fire is SE
    state.sides[1].active.override_types = [Type::Grass as u8, Type::Grass as u8];
    state.sides[1].active.set_volatile(VOL_TYPES_OVERRIDDEN);

    // Baseline: Fire Punch (move_id=7) vs Grass defender, no resist berry
    let res_base = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);
    assert!(res_base.effectiveness > 4, "Fire vs Grass should be SE");

    // Give defender Occa Berry (item 311, Fire resist berry)
    state.sides[1].team[0].item_id = 311;
    let res_berry = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);

    // Resist berry halves SE damage
    assert!(res_berry.damage < res_base.damage,
        "Resist berry should reduce SE damage: {} vs {}", res_berry.damage, res_base.damage);
    let ratio = res_berry.damage as f64 / res_base.damage as f64;
    assert!((ratio - 0.5).abs() < 0.05,
        "Expected ~0.5x damage with resist berry, got {:.3}x", ratio);

    // Berry should be marked for consumption
    assert!(res_berry.item_consumed,
        "Resist berry should be consumed");
}

#[test]
fn test_weakness_policy_boost() {
    let mut state = setup();

    // Make defender Grass-type so Fire is SE
    state.sides[1].active.override_types = [Type::Grass as u8, Type::Grass as u8];
    state.sides[1].active.set_volatile(VOL_TYPES_OVERRIDDEN);
    state.sides[1].team[0].item_id = data_bridge::ITEM_WEAKNESS_POLICY;
    // Give defender very high HP/Def to survive the SE hit
    state.sides[1].team[0].current_hp = 999;
    state.sides[1].team[0].max_hp = 999;
    state.sides[1].team[0].stats[1] = 250; // high Def

    // Fire Punch (move_id=7, Fire/Physical) — SE vs Grass
    execute_move(&mut state, &TeamData::default(), 0, 7, 1, &mut no_crit_rng);

    // Defender should have +2 Atk (index 0) and +2 SpA (index 2)
    assert_eq!(state.sides[1].active.boosts[ATK], 2,
        "Weakness Policy should give +2 Atk, got {}", state.sides[1].active.boosts[ATK]);
    assert_eq!(state.sides[1].active.boosts[SPA], 2,
        "Weakness Policy should give +2 SpA, got {}", state.sides[1].active.boosts[SPA]);

    // Item should be consumed
    assert_eq!(state.sides[1].team[0].item_id, 0,
        "Weakness Policy should be consumed");
}

#[test]
fn test_expert_belt_se() {
    let mut state = setup();

    // Make defender Grass-type so Fire is SE
    state.sides[1].active.override_types = [Type::Grass as u8, Type::Grass as u8];
    state.sides[1].active.set_volatile(VOL_TYPES_OVERRIDDEN);

    // Baseline: Fire Punch without Expert Belt
    let res_base = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);
    assert!(res_base.effectiveness > 4, "Fire vs Grass should be SE");

    // With Expert Belt (item 132)
    state.sides[0].team[0].item_id = 132;
    let res_eb = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);

    // Expert Belt: 1.2x on SE moves
    assert!(res_eb.damage > res_base.damage,
        "Expert Belt should boost SE damage: {} vs {}", res_eb.damage, res_base.damage);
    let ratio = res_eb.damage as f64 / res_base.damage as f64;
    assert!((ratio - 1.2).abs() < 0.05,
        "Expected ~1.2x boost, got {:.3}x", ratio);

    // Neutral move should NOT get Expert Belt boost
    // Override defender to Normal type (neutral vs Fire Punch)
    state.sides[1].active.override_types = [Type::Normal as u8, Type::Normal as u8];
    let res_neutral = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);
    state.sides[0].team[0].item_id = 0;
    let res_neutral_no_item = calc_damage(&state, 0, 7, 0, &mut no_crit_rng);
    assert_eq!(res_neutral.damage, res_neutral_no_item.damage,
        "Expert Belt should not boost neutral moves");
}
