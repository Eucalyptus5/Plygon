//! State mutation functions.

use crate::state::structs::*;
use crate::state::data_bridge;
use crate::state::accessors::effective_ability;
use crate::data::items::ItemFlag;

pub fn deal_damage(state: &mut BattleState, side: usize, slot: usize, amount: u16) {
    let mon = &mut state.sides[side].team[slot];
    mon.current_hp = mon.current_hp.saturating_sub(amount);
}

pub fn heal(state: &mut BattleState, side: usize, slot: usize, amount: u16) {
    let mon = &mut state.sides[side].team[slot];
    mon.current_hp = mon.current_hp.saturating_add(amount).min(mon.max_hp);
}

pub fn deal_proportional_damage(state: &mut BattleState, side: usize, slot: usize, num: u16, den: u16) {
    let max_hp = state.sides[side].team[slot].max_hp;
    let damage = (max_hp as u32 * num as u32 / den as u32).max(1) as u16;
    deal_damage(state, side, slot, damage);
}

pub fn set_status(state: &mut BattleState, side: usize, slot: usize, status: u8, counter: u8) -> bool {
    let mon = &mut state.sides[side].team[slot];
    if mon.status != STATUS_NONE { return false; }
    mon.status = status;
    mon.status_counter = counter;
    true
}

pub fn clear_status(state: &mut BattleState, side: usize, slot: usize) {
    let mon = &mut state.sides[side].team[slot];
    if mon.status == STATUS_NONE { return; }
    mon.status = STATUS_NONE;
    mon.status_counter = 0;
}

pub fn apply_boost(state: &mut BattleState, side: usize, stat_index: usize, stages: i8) -> i8 {
    // Contrary inverts, Simple doubles (onChangeBoost in Showdown)
    let ability = effective_ability(state, side);
    let stages = if ability == data_bridge::ABILITY_CONTRARY {
        -stages
    } else if ability == data_bridge::ABILITY_SIMPLE {
        (stages as i16 * 2).clamp(-6, 6) as i8
    } else {
        stages
    };
    apply_boost_raw(state, side, stat_index, stages)
}

/// Apply a boost without Contrary/Simple modification.
/// Used when the boost is a reactive response (Competitive/Defiant) that should not be re-inverted.
#[inline(always)]
pub fn apply_boost_raw(state: &mut BattleState, side: usize, stat_index: usize, stages: i8) -> i8 {
    let active = &mut state.sides[side].active;
    let old = active.boosts[stat_index];
    let new = (old as i16 + stages as i16).clamp(-6, 6) as i8;
    let actual = new - old;
    if actual != 0 {
        active.boosts[stat_index] = new;
    }
    actual
}

/// Apply a negative stat change from an external source.
/// Handles: Clear Amulet block, Mirror Armor reflect, Contrary/Simple (via apply_boost),
/// and triggers Competitive/Defiant after application.
///
/// `source` is the side that caused the drop (for Mirror Armor reflect).
#[inline(always)]
pub fn try_opponent_stat_drop_from(
    state: &mut BattleState, target: usize, stat: usize, stages: i8, source: usize,
) -> i8 {
    // Clear Amulet blocks
    if state.field.magic_room_turns() == 0 {
        let item_id = state.active_mon(target).item_id;
        if item_id != 0 && data_bridge::item(item_id).has(ItemFlag::CLEAR_AMULET) {
            return 0;
        }
    }

    // Mirror Armor: reflect negative drops back to source
    let target_ability = effective_ability(state, target);
    if target_ability == data_bridge::ABILITY_MIRROR_ARMOR && source != target {
        // Block the drop on target; reflect to source if source is alive
        if state.active_mon(source).current_hp > 0 {
            apply_boost(state, source, stat, stages);
        }
        return 0;
    }

    let actual = apply_boost(state, target, stat, stages);

    // Competitive/Defiant: +2 SpA/Atk when any stat is lowered by opponent
    // Check the ability AFTER Contrary (which may have inverted the drop into a raise).
    // The trigger condition is: the original intent was a drop (stages < 0) AND after Contrary
    // the actual result was still negative, OR the ability is Contrary and inverted it.
    // In Showdown, onAfterEachBoost checks if any boost[i] < 0 in the ORIGINAL boost object.
    // Since Contrary modifies the boost before onAfterEachBoost, and these abilities fire on
    // the MODIFIED boost: if Contrary made the drop positive, they don't trigger.
    // But the real check is simpler: Showdown's onAfterEachBoost sees the final boost values.
    // With Contrary, a -1 becomes +1, so boost[i] is +1, not <0, so no trigger. Correct.
    // Without Contrary, if stages < 0 and actual < 0, Competitive/Defiant trigger.
    if actual < 0 {
        let abil = effective_ability(state, target);
        if abil == data_bridge::ABILITY_COMPETITIVE {
            apply_boost_raw(state, target, SPA, 2);
        } else if abil == data_bridge::ABILITY_DEFIANT {
            apply_boost_raw(state, target, ATK, 2);
        }
    }
    actual
}

/// Backward-compatible wrapper: opponent stat drop where source is the opponent.
#[inline(always)]
pub fn try_opponent_stat_drop(
    state: &mut BattleState, target: usize, stat: usize, stages: i8,
) -> i8 {
    let source = 1 - target;
    try_opponent_stat_drop_from(state, target, stat, stages, source)
}

#[inline(always)]
fn mirror_herb_core(state: &mut BattleState, herb_side: usize) {
    let herb_slot = state.sides[herb_side].active_index as usize;
    consume_item(state, herb_side, herb_slot);
    if effective_ability(state, herb_side) == data_bridge::ABILITY_UNBURDEN {
        set_volatile(state, herb_side, VOL_UNBURDEN);
    }
}

#[inline(always)]
pub fn try_mirror_herb(
    state: &mut BattleState, boosted_side: usize, boosts: &[(usize, i8)],
) {
    let herb_side = 1 - boosted_side;
    if state.field.magic_room_turns() != 0 { return; }
    let herb_mon = state.active_mon(herb_side);
    if herb_mon.item_id == 0 { return; }
    if !data_bridge::item(herb_mon.item_id).has(ItemFlag::MIRROR_HERB) { return; }
    let mut any = false;
    for &(stat, stages) in boosts {
        if stages > 0 {
            apply_boost(state, herb_side, stat, stages);
            any = true;
        }
    }
    if any { mirror_herb_core(state, herb_side); }
}

#[inline(always)]
pub fn check_mirror_herb_diff(
    state: &mut BattleState, boosted_side: usize, boosts_before: &[i8; 7],
) {
    let herb_side = 1 - boosted_side;
    if state.field.magic_room_turns() != 0 { return; }
    let herb_mon = state.active_mon(herb_side);
    if herb_mon.item_id == 0 { return; }
    if !data_bridge::item(herb_mon.item_id).has(ItemFlag::MIRROR_HERB) { return; }
    let boosts_after = state.sides[boosted_side].active.boosts;
    let mut any = false;
    for i in 0..7 {
        let diff = boosts_after[i] - boosts_before[i];
        if diff > 0 {
            apply_boost(state, herb_side, i, diff);
            any = true;
        }
    }
    if any { mirror_herb_core(state, herb_side); }
}

/// Check and activate White Herb: restore all negative stat changes and consume item.
/// Called after self-drops (Close Combat, etc.) and after opponent-caused drops.
#[inline(always)]
pub fn check_white_herb(state: &mut BattleState, side: usize) {
    if state.field.magic_room_turns() != 0 { return; }
    let item_id = state.active_mon(side).item_id;
    if item_id == 0 { return; }
    if !data_bridge::item(item_id).has(ItemFlag::WHITE_HERB) { return; }
    let mut any_negative = false;
    for i in 0..7 {
        if state.sides[side].active.boosts[i] < 0 {
            any_negative = true;
            break;
        }
    }
    if !any_negative { return; }
    // Restore all negative boosts to 0
    for i in 0..7 {
        if state.sides[side].active.boosts[i] < 0 {
            state.sides[side].active.boosts[i] = 0;
        }
    }
    let slot = state.sides[side].active_index as usize;
    consume_item(state, side, slot);
    if effective_ability(state, side) == data_bridge::ABILITY_UNBURDEN {
        set_volatile(state, side, VOL_UNBURDEN);
    }
}

/// Check and activate Mental Herb: cure Taunt, Encore, Torment, Disable, Heal Block.
/// Called after these volatile conditions are set on a target.
#[inline(always)]
pub fn check_mental_herb(state: &mut BattleState, side: usize) {
    if state.field.magic_room_turns() != 0 { return; }
    let item_id = state.active_mon(side).item_id;
    if item_id == 0 { return; }
    if !data_bridge::item(item_id).has(ItemFlag::MENTAL_HERB) { return; }
    let active = &state.sides[side].active;
    let has_condition = active.taunt_turns > 0
        || active.encore_turns > 0
        || active.has_volatile(VOL_TORMENT)
        || active.disable_turns > 0
        || active.heal_block_turns > 0;
    if !has_condition { return; }
    // Clear all mental conditions
    state.sides[side].active.taunt_turns = 0;
    state.sides[side].active.encore_turns = 0;
    state.sides[side].active.disable_turns = 0;
    state.sides[side].active.disabled_move = 0;
    state.sides[side].active.heal_block_turns = 0;
    clear_volatile(state, side, VOL_TORMENT);
    let slot = state.sides[side].active_index as usize;
    consume_item(state, side, slot);
    if effective_ability(state, side) == data_bridge::ABILITY_UNBURDEN {
        set_volatile(state, side, VOL_UNBURDEN);
    }
}

pub fn set_volatile(state: &mut BattleState, side: usize, flag: u32) {
    let active = &mut state.sides[side].active;
    if active.volatile_flags & flag == 0 {
        active.volatile_flags |= flag;
    }
}

pub fn clear_volatile(state: &mut BattleState, side: usize, flag: u32) {
    let active = &mut state.sides[side].active;
    if active.volatile_flags & flag != 0 {
        active.volatile_flags &= !flag;
    }
}

pub fn set_weather(state: &mut BattleState, weather: u8, turns: u8) {
    // Showdown's Field.setWeather (sim/field.ts:45-53) refuses to re-set the
    // same weather while it's still active — non-sandstorm move sources fail
    // in gen >2, ability sources fail in gen >5 unless duration is 0. The
    // duration counter therefore keeps decrementing on its original schedule.
    // Mirror by no-op when the same non-NONE weather is already active.
    if weather != WEATHER_NONE
        && state.field.weather == weather
        && state.field.weather_turns > 0
    {
        return;
    }
    state.field.weather = weather;
    state.field.weather_turns = turns;
}

pub fn clear_weather(state: &mut BattleState) {
    set_weather(state, WEATHER_NONE, 0);
}

pub fn set_terrain(state: &mut BattleState, terrain: u8, turns: u8) {
    state.field.terrain = terrain;
    state.field.terrain_turns = turns;
}

pub fn clear_terrain(state: &mut BattleState) {
    set_terrain(state, TERRAIN_NONE, 0);
}

pub fn set_trick_room(state: &mut BattleState, turns: u8) {
    state.field.trick_room_turns = turns;
}

pub fn set_gravity(state: &mut BattleState, turns: u8) {
    state.field.gravity_turns = turns;
}

pub fn set_magic_room(state: &mut BattleState, turns: u8) {
    state.field.set_magic_room_turns(turns);
}

pub fn set_wonder_room(state: &mut BattleState, turns: u8) {
    state.field.set_wonder_room_turns(turns);
}

pub fn consume_item(state: &mut BattleState, side: usize, slot: usize) {
    let mon = &mut state.sides[side].team[slot];
    if mon.item_id != 0 {
        mon.item_id = 0;
    }
}

pub fn set_item(state: &mut BattleState, side: usize, slot: usize, item_id: u16) {
    let mon = &mut state.sides[side].team[slot];
    mon.item_id = item_id;
}

pub fn set_phase(state: &mut BattleState, phase: u8) {
    state.phase = phase;
}

pub fn deduct_pp(state: &mut BattleState, side: usize, slot: usize, amount: u8) -> bool {
    if state.sides[side].active.has_volatile(VOL_TRANSFORMED) {
        let pp = &mut state.sides[side].active.override_pp[slot];
        if *pp == 0 { return false; }
        *pp = pp.saturating_sub(amount);
    } else {
        let idx = state.sides[side].active_index as usize;
        let pp = &mut state.sides[side].team[idx].pp[slot];
        if *pp == 0 { return false; }
        *pp = pp.saturating_sub(amount);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> BattleState {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 25;
        state.sides[0].team[0].current_hp = 211;
        state.sides[0].team[0].max_hp = 211;
        state.sides[0].team[0].item_id = 234;
        state
    }

    #[test] fn test_damage() { let mut s = setup(); deal_damage(&mut s, 0, 0, 50); assert_eq!(s.sides[0].team[0].current_hp, 161); }
    #[test] fn test_heal() { let mut s = setup(); deal_damage(&mut s, 0, 0, 100); heal(&mut s, 0, 0, 50); assert_eq!(s.sides[0].team[0].current_hp, 161); }
    #[test] fn test_status() { let mut s = setup(); assert!(set_status(&mut s, 0, 0, STATUS_BURN, 0)); assert!(!set_status(&mut s, 0, 0, STATUS_PARALYSIS, 0)); assert_eq!(s.sides[0].team[0].status, STATUS_BURN); clear_status(&mut s, 0, 0); assert_eq!(s.sides[0].team[0].status, STATUS_NONE); }
    #[test] fn test_boost_clamp() { let mut s = setup(); apply_boost(&mut s, 0, ATK, 4); assert_eq!(apply_boost(&mut s, 0, ATK, 4), 2); assert_eq!(s.sides[0].active.boosts[ATK], 6); }
    #[test] fn test_volatile() { let mut s = setup(); set_volatile(&mut s, 0, VOL_SUBSTITUTE); assert!(s.sides[0].active.has_volatile(VOL_SUBSTITUTE)); clear_volatile(&mut s, 0, VOL_SUBSTITUTE); assert!(!s.sides[0].active.has_volatile(VOL_SUBSTITUTE)); }
    #[test] fn test_weather() { let mut s = setup(); set_weather(&mut s, WEATHER_RAIN, 5); assert_eq!(s.field.weather, WEATHER_RAIN); clear_weather(&mut s); assert_eq!(s.field.weather, WEATHER_NONE); }

    #[test]
    fn test_wonder_room_toggle() {
        let mut s = setup();
        set_wonder_room(&mut s, 5);
        assert_eq!(s.field.wonder_room_turns(), 5);
        set_wonder_room(&mut s, 0);
        assert_eq!(s.field.wonder_room_turns(), 0);
    }

    #[test]
    fn test_magic_room_toggle() {
        let mut s = setup();
        set_magic_room(&mut s, 5);
        assert_eq!(s.field.magic_room_turns(), 5);
        set_magic_room(&mut s, 3);
        assert_eq!(s.field.magic_room_turns(), 3);
        set_magic_room(&mut s, 0);
        assert_eq!(s.field.magic_room_turns(), 0);
    }

    #[test]
    fn test_white_herb_restore() {
        // Guards the check_white_herb de-thread: dropping the boost-restore write fails here.
        let mut s = setup();
        s.sides[0].team[0].item_id = 535; // White Herb
        s.sides[0].active.boosts = [-2, -1, 0, -3, 0, 0, 0];
        check_white_herb(&mut s, 0);
        assert_eq!(s.sides[0].active.boosts, [0i8; 7]);
        assert_eq!(s.sides[0].team[0].item_id, 0);
    }
}
