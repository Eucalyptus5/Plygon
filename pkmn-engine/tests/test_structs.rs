use std::mem::size_of;
use pkmn_engine::state::data_bridge::{ItemData, MoveData, MoveMeta};
use pkmn_engine::state::{
    MonSlot, ActiveMon, SideConditions, FieldState, SideState, BattleState,
    boosted_stat, boost_index
};
use pkmn_engine::state::structs::{MonBuildData, MON_FLAG_TERASTALLIZED};

#[test]
fn test_struct_sizes() {
    assert_eq!(size_of::<MonSlot>(), 36);
    assert_eq!(size_of::<ActiveMon>(), 72);
    assert_eq!(size_of::<SideConditions>(), 12);
    assert_eq!(size_of::<FieldState>(), 10);
    assert_eq!(size_of::<SideState>(), 304);
    assert!(size_of::<BattleState>() <= 640);
    assert_eq!(size_of::<MonBuildData>(), 13);
    assert_eq!(size_of::<ItemData>(), 8);
    assert_eq!(size_of::<MoveData>(), 16);
    assert_eq!(size_of::<MoveMeta>(), 2);
}

#[test]
fn test_battle_state_traits() {
    let state = BattleState::default();
    let state_copy = state; // Tests Copy
    let state_clone = state.clone(); // Tests Clone
    assert_eq!(state.zobrist, state_copy.zobrist);
    assert_eq!(state.zobrist, state_clone.zobrist);
}

#[test]
fn test_default_mon_slot() {
    let mon = MonSlot::default();
    assert_eq!(mon.species_id, 0);
    assert_eq!(mon.ability_id, 0);
    assert_eq!(mon.item_id, 0);
    assert_eq!(mon.current_hp, 0);
    assert_eq!(mon.max_hp, 0);
    assert_eq!(mon.stats, [0; 5]);
    assert_eq!(mon.moves, [0; 4]);
    assert_eq!(mon.pp, [0; 4]);
    assert_eq!(mon.status, 0);
    assert_eq!(mon.status_counter, 0);
    assert_eq!(mon.tera_type, 0);
    assert_eq!(mon.flags, 0);
}

#[test]
fn test_default_active_mon() {
    let active = ActiveMon::default();
    assert_eq!(active.volatile_flags, 0);
    assert_eq!(active.substitute_hp, 0);
    assert_eq!(active.last_move, 0);
    assert_eq!(active.last_move_hit_by, 0);
    assert_eq!(active.override_species, 0);
    assert_eq!(active.override_ability, 0);
    assert_eq!(active.override_stats, [0; 5]);
    assert_eq!(active.override_moves, [0; 4]);
    assert_eq!(active.choice_locked_move, 0);
    assert_eq!(active.disabled_move, 0);
    assert_eq!(active.encore_move, 0);
    assert_eq!(active.boosts, [0; 7]);
    assert_eq!(active.override_pp, [0; 4]);
    assert_eq!(active.override_types, [0; 2]);
    assert_eq!(active.confusion_turns, 0);
    assert_eq!(active.taunt_turns, 0);
    assert_eq!(active.encore_turns, 0);
    assert_eq!(active.disable_turns, 0);
    assert_eq!(active.protect_consecutive, 0);
    assert_eq!(active.toxic_counter, 0);
    assert_eq!(active.turns_active, 0);
    assert_eq!(active.times_hit, 0);
    assert_eq!(active.consec_move_count, 0);
    assert_eq!(active.stockpile, 0);
    assert_eq!(active.magnet_rise_turns, 0);
    assert_eq!(active.telekinesis_turns, 0);
    assert_eq!(active.heal_block_turns, 0);
    assert_eq!(active.perish_count, 0);
}

#[test]
fn test_mon_slot_methods() {
    let mut mon = MonSlot::default();
    assert!(mon.is_fainted());
    mon.current_hp = 10;
    assert!(!mon.is_fainted());
    
    assert!(!mon.is_terastallized());
    mon.flags |= MON_FLAG_TERASTALLIZED;
    assert!(mon.is_terastallized());
}

#[test]
fn test_active_mon_volatile() {
    let mut active = ActiveMon::default();
    for i in 0..32 {
        let flag = 1 << i;
        assert!(!active.has_volatile(flag));
        active.set_volatile(flag);
        assert!(active.has_volatile(flag));
        active.clear_volatile(flag);
        assert!(!active.has_volatile(flag));
    }
}

#[test]
fn test_boosted_stat() {
    // 10, 15, 20, 25, 30, 35, 40 / 10
    // Atk/Def/SpA/SpD/Spe boosts
    let base = 100;
    assert_eq!(boosted_stat(base, 0), 100);
    assert_eq!(boosted_stat(base, 1), 150);
    assert_eq!(boosted_stat(base, 2), 200);
    assert_eq!(boosted_stat(base, 3), 250);
    assert_eq!(boosted_stat(base, 4), 300);
    assert_eq!(boosted_stat(base, 5), 350);
    assert_eq!(boosted_stat(base, 6), 400);
    
    // 2/2 = 1.0, 2/3 = 0.66, 2/4 = 0.5, 2/5 = 0.4, 2/6 = 0.33, 2/7 = 0.28, 2/8 = 0.25
    assert_eq!(boosted_stat(base, -1), 66);
    assert_eq!(boosted_stat(base, -2), 50);
    assert_eq!(boosted_stat(base, -3), 40);
    assert_eq!(boosted_stat(base, -4), 33);
    assert_eq!(boosted_stat(base, -5), 28);
    assert_eq!(boosted_stat(base, -6), 25);
}

#[test]
fn test_boost_index() {
    for i in -6..=6 {
        assert_eq!(boost_index(i), (i + 6) as usize);
    }
}
