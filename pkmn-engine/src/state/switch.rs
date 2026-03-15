//! Switch-in and switch-out logic.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag};
use crate::state::accessors::{effective_ability, effective_types, is_grounded};
use crate::state::mutations::*;
use crate::state::zobrist::ZobristKeys;

pub fn switch_out(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let ability = effective_ability(state, side);
    let idx = state.sides[side].active_index as usize;

    if ability == data_bridge::ABILITY_NATURAL_CURE {
        clear_status(state, keys, side, idx);
    }
    if ability == data_bridge::ABILITY_REGENERATOR {
        let max_hp = state.sides[side].team[idx].max_hp;
        heal(state, keys, side, idx, max_hp / 3);
    }

    // Clear volatiles from Zobrist
    let flags = state.sides[side].active.volatile_flags;
    for bit in 0..32u32 {
        if flags & (1 << bit) != 0 { state.zobrist ^= keys.volatile_bit[side][bit as usize]; }
    }
    let boosts = state.sides[side].active.boosts;
    for stat in 0..7 {
        if boosts[stat] != 0 { state.zobrist ^= keys.boosts[side][stat][(boosts[stat] + 6) as usize]; }
    }
    state.sides[side].active.zero();
}

pub fn switch_in(state: &mut BattleState, keys: &ZobristKeys, side: usize, new_index: usize) {
    let old_index = state.sides[side].active_index as usize;
    state.zobrist ^= keys.active_index[side][old_index];
    state.sides[side].active_index = new_index as u8;
    state.zobrist ^= keys.active_index[side][new_index];
    apply_entry_hazards(state, keys, side);
    apply_switch_in_ability(state, keys, side);
}

pub fn perform_switch(state: &mut BattleState, keys: &ZobristKeys, side: usize, new_index: usize) {
    switch_out(state, keys, side);
    switch_in(state, keys, side, new_index);
}

fn apply_entry_hazards(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];
    if data_bridge::item(mon.item_id).has(ItemFlag::HAZARD_IMMUNE) { return; }
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { return; }

    let sc = state.sides[side].side_conditions;

    // Stealth Rock
    if sc.hazard_flags & HAZARD_STEALTH_ROCK != 0 {
        let (t1, t2) = effective_types(state, side);
        // Convert u8 back to Type for the effectiveness call.
        // SAFETY: our u8 type values come from Type as u8, so they're valid.
        let def1 = unsafe { core::mem::transmute::<u8, Type>(t1) };
        let def2 = unsafe { core::mem::transmute::<u8, Type>(t2) };
        let eff = dual_type_effectiveness(Type::Rock, def1, def2);
        let max_hp = state.sides[side].team[slot].max_hp as u32;
        let damage = (max_hp * eff as u32 / 32).max(1) as u16;
        deal_damage(state, keys, side, slot, damage);
    }

    // Spikes (grounded only)
    if sc.spikes > 0 && is_grounded(state, side) {
        let max_hp = state.sides[side].team[slot].max_hp;
        let damage = match sc.spikes { 1 => max_hp / 8, 2 => max_hp / 6, _ => max_hp / 4 };
        deal_damage(state, keys, side, slot, damage.max(1));
    }

    // Toxic Spikes (grounded only)
    if sc.toxic_spikes > 0 && is_grounded(state, side) {
        let (t1, t2) = effective_types(state, side);
        let is_poison = t1 == Type::Poison as u8 || t2 == Type::Poison as u8;
        if is_poison {
            state.sides[side].side_conditions.toxic_spikes = 0;
        } else {
            match sc.toxic_spikes {
                1 => { set_status(state, keys, side, slot, STATUS_POISON, 0); }
                _ => { set_status(state, keys, side, slot, STATUS_BAD_POISON, 0); }
            }
        }
    }

    // Sticky Web (grounded only)
    if sc.hazard_flags & HAZARD_STICKY_WEB != 0 && is_grounded(state, side) {
        apply_boost(state, keys, side, SPE, -1);
    }
}

fn apply_switch_in_ability(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let ability = effective_ability(state, side);
    let opp = 1 - side;
    match ability {
        data_bridge::ABILITY_INTIMIDATE  => { apply_boost(state, keys, opp, ATK, -1); }
        data_bridge::ABILITY_DRIZZLE     => { set_weather(state, keys, WEATHER_RAIN, 5); }
        data_bridge::ABILITY_DROUGHT     => { set_weather(state, keys, WEATHER_SUN, 5); }
        data_bridge::ABILITY_SAND_STREAM => { set_weather(state, keys, WEATHER_SAND, 5); }
        data_bridge::ABILITY_SNOW_WARNING=> { set_weather(state, keys, WEATHER_SNOW, 5); }
        _ => {}
    }
}

// ── Hazard management helpers ───────────────────────────────────────

pub fn add_spikes(state: &mut BattleState, side: usize) {
    let sc = &mut state.sides[side].side_conditions;
    if sc.spikes < 3 { sc.spikes += 1; }
}
pub fn add_toxic_spikes(state: &mut BattleState, side: usize) {
    let sc = &mut state.sides[side].side_conditions;
    if sc.toxic_spikes < 2 { sc.toxic_spikes += 1; }
}
pub fn set_stealth_rock(state: &mut BattleState, side: usize) {
    state.sides[side].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
}
pub fn set_sticky_web(state: &mut BattleState, side: usize) {
    state.sides[side].side_conditions.hazard_flags |= HAZARD_STICKY_WEB;
}
pub fn clear_hazards(state: &mut BattleState, side: usize) {
    let sc = &mut state.sides[side].side_conditions;
    sc.spikes = 0; sc.toxic_spikes = 0; sc.hazard_flags = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::zobrist::{compute_full_hash, validate_hash};

    #[test]
    fn test_switch_out_zeros_active() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 25;
        state.sides[0].team[0].current_hp = 200;
        state.sides[0].team[0].max_hp = 200;
        state.sides[0].active.boosts[ATK] = 2;
        state.sides[0].active.volatile_flags = VOL_SUBSTITUTE | VOL_LEECH_SEED;
        state.zobrist = compute_full_hash(&state, &keys);
        switch_out(&mut state, &keys, 0);
        assert_eq!(state.sides[0].active.volatile_flags, 0);
        assert!(validate_hash(&state, &keys));
    }
}
