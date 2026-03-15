use pkmn_engine::state::BattleState;
use pkmn_engine::state::accessors::*;
use pkmn_engine::state::structs::{
    VOL_TYPES_OVERRIDDEN, VOL_TRANSFORMED, MON_FLAG_TERASTALLIZED,
    VOL_ABILITY_SUPPRESSED, VOL_ABILITY_OVERRIDDEN, VOL_MAGNET_RISE,
    VOL_SMACKED_DOWN, VOL_INGRAIN
};
use pkmn_engine::data::types::Type;
use pkmn_engine::state::data_bridge::ABILITY_LEVITATE;

#[test]
fn test_effective_types() {
    let mut state = BattleState::default();
    
    // Normal: returns base types. (Default species is 0 -> usually Normal/Normal)
    assert_eq!(effective_types(&state, 0), (Type::Normal as u8, Type::Normal as u8));
    
    // VOL_TYPES_OVERRIDDEN
    state.sides[0].active.override_types = [Type::Fire as u8, Type::Water as u8];
    state.sides[0].active.set_volatile(VOL_TYPES_OVERRIDDEN);
    assert_eq!(effective_types(&state, 0), (Type::Fire as u8, Type::Water as u8));
    
    // Clear VOL_TYPES_OVERRIDDEN, set VOL_TRANSFORMED
    state.sides[0].active.clear_volatile(VOL_TYPES_OVERRIDDEN);
    state.sides[0].active.override_types = [Type::Grass as u8, Type::Ice as u8];
    state.sides[0].active.set_volatile(VOL_TRANSFORMED);
    assert_eq!(effective_types(&state, 0), (Type::Grass as u8, Type::Ice as u8));
    
    // Terastallized
    let mut mon = state.active_mon_mut(0);
    mon.tera_type = Type::Dragon as u8;
    mon.flags |= MON_FLAG_TERASTALLIZED;
    assert_eq!(effective_types(&state, 0), (Type::Dragon as u8, Type::Dragon as u8));
}

#[test]
fn test_effective_ability() {
    let mut state = BattleState::default();
    
    // Default
    state.active_mon_mut(0).ability_id = 10;
    assert_eq!(effective_ability(&state, 0), 10);
    
    // VOL_ABILITY_OVERRIDDEN
    state.sides[0].active.override_ability = 20;
    state.sides[0].active.set_volatile(VOL_ABILITY_OVERRIDDEN);
    assert_eq!(effective_ability(&state, 0), 20);
    
    // VOL_TRANSFORMED
    state.sides[0].active.clear_volatile(VOL_ABILITY_OVERRIDDEN);
    state.sides[0].active.set_volatile(VOL_TRANSFORMED);
    state.sides[0].active.override_ability = 30;
    assert_eq!(effective_ability(&state, 0), 30);
    
    // VOL_ABILITY_SUPPRESSED overrides overridden
    state.sides[0].active.set_volatile(VOL_ABILITY_SUPPRESSED);
    assert_eq!(effective_ability(&state, 0), 0);
}

#[test]
fn test_effective_stat() {
    let mut state = BattleState::default();
    state.active_mon_mut(0).stats = [100, 110, 120, 130, 140];
    
    assert_eq!(effective_stat(&state, 0, 1), 110);
    assert_eq!(effective_stat(&state, 0, 2), 120);
    
    state.sides[0].active.override_stats = [200, 210, 220, 230, 240];
    state.sides[0].active.set_volatile(VOL_TRANSFORMED);
    
    assert_eq!(effective_stat(&state, 0, 1), 210);
    assert_eq!(effective_stat(&state, 0, 2), 220);
}

#[test]
fn test_effective_moves() {
    let mut state = BattleState::default();
    state.active_mon_mut(0).moves = [1, 2, 3, 4];
    
    assert_eq!(effective_moves(&state, 0), [1, 2, 3, 4]);
    
    state.sides[0].active.override_moves = [5, 6, 7, 8];
    state.sides[0].active.set_volatile(VOL_TRANSFORMED);
    
    assert_eq!(effective_moves(&state, 0), [5, 6, 7, 8]);
}

#[test]
fn test_effective_pp() {
    let mut state = BattleState::default();
    state.active_mon_mut(0).pp = [10, 20, 30, 40];
    
    assert_eq!(effective_pp(&state, 0, 0), 10);
    assert_eq!(effective_pp(&state, 0, 1), 20);
    
    state.sides[0].active.override_pp = [15, 25, 35, 45];
    state.sides[0].active.set_volatile(VOL_TRANSFORMED);
    
    assert_eq!(effective_pp(&state, 0, 0), 15);
    assert_eq!(effective_pp(&state, 0, 1), 25);
}

#[test]
fn test_is_grounded() {
    let mut state = BattleState::default();
    
    // Default (Normal, no abilities) -> grounded
    assert!(is_grounded(&state, 0));
    
    // Flying type -> not grounded
    state.sides[0].active.override_types = [Type::Flying as u8, Type::Flying as u8];
    state.sides[0].active.set_volatile(VOL_TYPES_OVERRIDDEN);
    assert!(!is_grounded(&state, 0));
    
    // Gravity overrides Flying -> grounded
    state.field.gravity_turns = 1;
    assert!(is_grounded(&state, 0));
    state.field.gravity_turns = 0;
    
    // SMACKED_DOWN overrides Flying -> grounded
    state.sides[0].active.set_volatile(VOL_SMACKED_DOWN);
    assert!(is_grounded(&state, 0));
    state.sides[0].active.clear_volatile(VOL_SMACKED_DOWN);
    
    // INGRAIN overrides Flying -> grounded
    state.sides[0].active.set_volatile(VOL_INGRAIN);
    assert!(is_grounded(&state, 0));
    state.sides[0].active.clear_volatile(VOL_INGRAIN);
    
    // Levitate -> not grounded
    state.sides[0].active.clear_volatile(VOL_TYPES_OVERRIDDEN); // back to Normal
    state.active_mon_mut(0).ability_id = ABILITY_LEVITATE;
    assert!(!is_grounded(&state, 0));
    
    // Air Balloon (Item 6) -> not grounded
    state.active_mon_mut(0).ability_id = 0;
    state.active_mon_mut(0).item_id = 6;
    assert!(!is_grounded(&state, 0));
    
    // Magnet Rise -> not grounded
    state.active_mon_mut(0).item_id = 0;
    state.sides[0].active.set_volatile(VOL_MAGNET_RISE);
    assert!(!is_grounded(&state, 0));
}

#[test]
fn test_is_trap_immune() {
    let mut state = BattleState::default();
    
    // Normal, no item -> not immune
    assert!(!is_trap_immune(&state, 0));
    
    // Ghost type -> immune
    state.sides[0].active.override_types = [Type::Ghost as u8, Type::Ghost as u8];
    state.sides[0].active.set_volatile(VOL_TYPES_OVERRIDDEN);
    assert!(is_trap_immune(&state, 0));
    
    // Shed Shell (Item 437) -> immune
    state.sides[0].active.clear_volatile(VOL_TYPES_OVERRIDDEN);
    state.active_mon_mut(0).item_id = 437; // Shed Shell
    assert!(is_trap_immune(&state, 0));
}
