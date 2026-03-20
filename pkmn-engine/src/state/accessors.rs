//! Accessor functions: the mux logic for Transform / type changes / ability overrides.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag};

#[inline(always)]
pub fn effective_types(state: &BattleState, side: usize) -> (u8, u8) {
    let mon = state.active_mon(side);
    let active = &state.sides[side].active;

    if mon.is_terastallized() {
        return (mon.tera_type, mon.tera_type);
    }
    if active.has_volatile(VOL_TYPES_OVERRIDDEN) || active.has_volatile(VOL_TRANSFORMED) {
        return (active.override_types[0], active.override_types[1]);
    }

    let sp = data_bridge::species(mon.species_id);
    (sp.type1 as u8, sp.type2 as u8)
}

#[inline(always)]
pub fn effective_weather(state: &BattleState) -> u8 {
    if state.field.field_flags & FIELD_WEATHER_SUPPRESSED != 0 {
        WEATHER_NONE
    } else {
        state.field.weather
    }
}

#[inline(always)]
pub fn effective_ability(state: &BattleState, side: usize) -> u16 {
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_ABILITY_SUPPRESSED) { return 0; }
    if active.has_volatile(VOL_ABILITY_OVERRIDDEN) || active.has_volatile(VOL_TRANSFORMED) {
        return active.override_ability;
    }
    state.active_mon(side).ability_id
}

#[inline(always)]
pub fn effective_stat(state: &BattleState, side: usize, stat_index: usize) -> u16 {
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_TRANSFORMED) { return active.override_stats[stat_index]; }
    // Forme-change stat overrides (Aegislash Blade, Zen Mode, etc.)
    if active.override_stats[0] != 0 { return active.override_stats[stat_index]; }
    state.active_mon(side).stats[stat_index]
}

#[inline(always)]
pub fn effective_moves(state: &BattleState, side: usize) -> [u16; 4] {
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_TRANSFORMED) { return active.override_moves; }
    state.active_mon(side).moves
}

#[inline(always)]
pub fn effective_pp(state: &BattleState, side: usize, move_slot: usize) -> u8 {
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_TRANSFORMED) { return active.override_pp[move_slot]; }
    state.active_mon(side).pp[move_slot]
}

#[inline(always)]
pub fn effective_species(state: &BattleState, side: usize) -> u16 {
    let active = &state.sides[side].active;
    if active.override_species != 0 {
        return active.override_species;
    }
    state.active_mon(side).species_id
}

#[inline]
pub fn has_type(state: &BattleState, side: usize, check_type: u8) -> bool {
    let (t1, t2) = effective_types(state, side);
    t1 == check_type || t2 == check_type
}

/// Check if the active Pokémon is grounded.
/// Grounded = NOT (Flying-type OR Levitate OR Air Balloon OR Magnet Rise OR Telekinesis)
/// unless Gravity is active (overrides all).
#[inline]
pub fn is_grounded(state: &BattleState, side: usize) -> bool {
    let field = &state.field;
    if field.gravity_turns > 0 { return true; }

    let active = &state.sides[side].active;
    if active.has_volatile(VOL_SMACKED_DOWN) { return true; }
    if active.has_volatile(VOL_INGRAIN) { return true; }
    if has_type(state, side, Type::Flying as u8) { return false; }
    if effective_ability(state, side) == data_bridge::ABILITY_LEVITATE { return false; }
    if state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(side).item_id).has(ItemFlag::AIR_BALLOON) { return false; }
    if active.has_volatile(VOL_MAGNET_RISE) { return false; }
    if active.telekinesis_turns > 0 { return false; }

    true
}

#[inline]
pub fn is_trap_immune(state: &BattleState, side: usize) -> bool {
    if has_type(state, side, Type::Ghost as u8) { return true; }
    state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(side).item_id).has(ItemFlag::TRAP_IMMUNE)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_transform_overrides() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 132;
        state.sides[0].team[0].stats = [48, 48, 48, 48, 48];
        state.sides[0].active.volatile_flags |= VOL_TRANSFORMED;
        state.sides[0].active.override_stats = [84, 78, 109, 85, 100];
        state.sides[0].active.override_moves = [53, 126, 257, 394];
        state.sides[0].active.override_pp = [5, 5, 5, 5];
        assert_eq!(effective_stat(&state, 0, ATK), 84);
        assert_eq!(effective_moves(&state, 0), [53, 126, 257, 394]);
    }

    #[test]
    fn test_gravity_grounds_flying() {
        use crate::data::types::Type;
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 18; // Pidgeot (Flying type)
        // Without Gravity: Flying type is not grounded
        assert!(!is_grounded(&state, 0));
        // With Gravity: everything is grounded
        state.field.gravity_turns = 5;
        assert!(is_grounded(&state, 0));
    }
}
