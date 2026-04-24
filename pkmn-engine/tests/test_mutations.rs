use pkmn_engine::state::BattleState;
use pkmn_engine::state::mutations::*;
use pkmn_engine::state::structs::{VOL_TRANSFORMED, VOL_TYPES_OVERRIDDEN};

fn setup() -> BattleState {
    let mut state = BattleState::default();
    // Setup a basic mon
    state.sides[0].team[0].species_id = 1;
    state.sides[0].team[0].current_hp = 300;
    state.sides[0].team[0].max_hp = 300;
    state.sides[0].team[0].item_id = 242; // Leftovers
    state.sides[0].team[0].status = 0;
    state.sides[0].team[0].pp = [10, 10, 10, 10];
    
    // Recompute since we modified directly without mutations
    
    state
}

#[test]
fn test_deal_damage() {
    let mut state = setup();
    
    deal_damage(&mut state, 0, 0, 50);
    assert_eq!(state.sides[0].team[0].current_hp, 250);
    
    // Floor at 0
    deal_damage(&mut state, 0, 0, 500);
    assert_eq!(state.sides[0].team[0].current_hp, 0);
}

#[test]
fn test_heal() {
    let mut state = setup();
    
    state.sides[0].team[0].current_hp = 100;
    
    heal(&mut state, 0, 0, 50);
    assert_eq!(state.sides[0].team[0].current_hp, 150);
    
    // Cap at max_hp
    heal(&mut state, 0, 0, 500);
    assert_eq!(state.sides[0].team[0].current_hp, 300);
    
    // Heal from 0
    state.sides[0].team[0].current_hp = 0;
    heal(&mut state, 0, 0, 50);
    assert_eq!(state.sides[0].team[0].current_hp, 50);
}

#[test]
fn test_deal_proportional_damage() {
    let mut state = setup();
    
    deal_proportional_damage(&mut state, 0, 0, 1, 16);
    // 300 / 16 = 18
    assert_eq!(state.sides[0].team[0].current_hp, 282);
    
    // Minimum 1 damage
    state.sides[0].team[0].max_hp = 10;
    state.sides[0].team[0].current_hp = 10;
    
    deal_proportional_damage(&mut state, 0, 0, 1, 16);
    assert_eq!(state.sides[0].team[0].current_hp, 9);
}

#[test]
fn test_set_status() {
    let mut state = setup();
    
    assert!(set_status(&mut state, 0, 0, 1, 0)); // Set Burn (or something)
    assert_eq!(state.sides[0].team[0].status, 1);
    
    // Fails when already set
    assert!(!set_status(&mut state, 0, 0, 2, 0));
    assert_eq!(state.sides[0].team[0].status, 1); // Still 1
}

#[test]
fn test_clear_status() {
    let mut state = setup();
    
    set_status(&mut state, 0, 0, 1, 5);
    clear_status(&mut state, 0, 0);
    assert_eq!(state.sides[0].team[0].status, 0);
    assert_eq!(state.sides[0].team[0].status_counter, 0);
}

#[test]
fn test_apply_boost() {
    let mut state = setup();
    
    // apply_boost(state, side, stat_index, stages)
    let applied = apply_boost(&mut state, 0, 0, 4); // +4 Atk
    assert_eq!(applied, 4);
    assert_eq!(state.sides[0].active.boosts[0], 4);
    
    let applied2 = apply_boost(&mut state, 0, 0, 4); // +4 more, but capped at +6
    assert_eq!(applied2, 2);
    assert_eq!(state.sides[0].active.boosts[0], 6);
    
    let applied3 = apply_boost(&mut state, 0, 0, -10); // +6 - 10 = -4
    assert_eq!(applied3, -10);
    assert_eq!(state.sides[0].active.boosts[0], -4);

    let applied4 = apply_boost(&mut state, 0, 0, -5); // -4 - 5 = -9, capped at -6
    assert_eq!(applied4, -2);
    assert_eq!(state.sides[0].active.boosts[0], -6);
}

#[test]
fn test_volatile() {
    let mut state = setup();
    
    let flag = VOL_TYPES_OVERRIDDEN;
    set_volatile(&mut state, 0, flag);
    assert!(state.sides[0].active.has_volatile(flag));
    
    set_volatile(&mut state, 0, flag); // Already set, no change
    
    clear_volatile(&mut state, 0, flag);
    assert!(!state.sides[0].active.has_volatile(flag));
}

#[test]
fn test_weather_terrain() {
    let mut state = setup();
    
    set_weather(&mut state, 1, 5);
    assert_eq!(state.field.weather, 1);
    assert_eq!(state.field.weather_turns, 5);
    
    clear_weather(&mut state);
    assert_eq!(state.field.weather, 0);
    assert_eq!(state.field.weather_turns, 0);
    
    set_terrain(&mut state, 2, 5);
    assert_eq!(state.field.terrain, 2);
    assert_eq!(state.field.terrain_turns, 5);
    
    clear_terrain(&mut state);
    assert_eq!(state.field.terrain, 0);
    assert_eq!(state.field.terrain_turns, 0);
}

#[test]
fn test_trick_room_gravity() {
    let mut state = setup();
    
    set_trick_room(&mut state, 5);
    assert_eq!(state.field.trick_room_turns, 5);
    
    set_gravity(&mut state, 5);
    assert_eq!(state.field.gravity_turns, 5);
}

#[test]
fn test_consume_item() {
    let mut state = setup();
    assert_eq!(state.sides[0].team[0].item_id, 242);
    
    consume_item(&mut state, 0, 0);
    assert_eq!(state.sides[0].team[0].item_id, 0);
    
    consume_item(&mut state, 0, 0); // already 0
}

#[test]
fn test_set_item() {
    let mut state = setup();
    
    set_item(&mut state, 0, 0, 100);
    assert_eq!(state.sides[0].team[0].item_id, 100);
}

#[test]
fn test_deduct_pp() {
    let mut state = setup();
    
    assert!(deduct_pp(&mut state, 0, 0, 1));
    assert_eq!(state.sides[0].team[0].pp[0], 9);
    
    // Try to deduct more than we have -> goes to 0, returns true
    assert!(deduct_pp(&mut state, 0, 0, 10));
    assert_eq!(state.sides[0].team[0].pp[0], 0);

    // Try to deduct when 0 -> returns false
    assert!(!deduct_pp(&mut state, 0, 0, 1));
    
    // VOL_TRANSFORMED -> deducts from override_pp
    state.sides[0].active.override_pp = [5, 5, 5, 5];
    state.sides[0].active.set_volatile(VOL_TRANSFORMED);
    
    assert!(deduct_pp(&mut state, 0, 0, 1));
    assert_eq!(state.sides[0].active.override_pp[0], 4);
    assert_eq!(state.sides[0].team[0].pp[0], 0); // Original PP unchanged (was 0)
}
