//! End-of-turn processing: the 18-step sequence.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag};
use crate::state::accessors::*;
use crate::state::mutations::*;
use crate::state::zobrist::ZobristKeys;

pub fn end_of_turn(state: &mut BattleState, keys: &ZobristKeys) {
    step_weather(state, keys);                                   // 1
    step_terrain_expiry(state, keys);                            // 2
    // 3: Future Sight (not yet implemented)
    step_wish(state, keys);                                      // 4
    for side in 0..2 { step_status_damage(state, keys, side); }  // 5
    step_leech_seed(state, keys);                                // 6
    for side in 0..2 { step_binding_damage(state, keys, side); } // 7
    for side in 0..2 { step_passive_healing(state, keys, side); }// 8
    step_grassy_terrain(state, keys);                            // 9
    for side in 0..2 { step_item_healing(state, keys, side); }   // 10
    for side in 0..2 { step_screen_expiry(state, side); }        // 11
    for side in 0..2 { step_tailwind_expiry(state, side); }      // 12
    step_volatile_counters(state, keys);                         // 13
    for side in 0..2 { step_perish_song(state, keys, side); }    // 14
    for side in 0..2 { step_eot_abilities(state, keys, side); }  // 15
    for side in 0..2 {                                           // 16
        state.sides[side].active.turns_active = state.sides[side].active.turns_active.saturating_add(1);
    }
    for side in 0..2 {                                           // 17
        // Reset protect_consecutive if Protect was NOT used this turn
        if !state.sides[side].active.has_volatile(VOL_PROTECT_THIS_TURN) {
            state.sides[side].active.protect_consecutive = 0;
        }
        let flags_to_clear = state.sides[side].active.volatile_flags & VOL_PER_TURN_MASK;
        for bit in 0..32u32 {
            if flags_to_clear & (1 << bit) != 0 { state.zobrist ^= keys.volatile_bit[side][bit as usize]; }
        }
        state.sides[side].active.volatile_flags &= !VOL_PER_TURN_MASK;
        state.sides[side].active.times_hit = 0;
    }
    state.field.turn += 1;                                       // 18
}

fn step_weather(state: &mut BattleState, keys: &ZobristKeys) {
    let weather = state.field.weather;
    if weather == WEATHER_NONE { return; }

    if weather == WEATHER_SAND {
        for side in 0..2 {
            let slot = state.sides[side].active_index as usize;
            if state.sides[side].team[slot].is_fainted() { continue; }
            if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { continue; }
            let (t1, t2) = effective_types(state, side);
            let immune = [Type::Rock as u8, Type::Ground as u8, Type::Steel as u8];
            if !immune.contains(&t1) && !immune.contains(&t2) {
                deal_proportional_damage(state, keys, side, slot, 1, 16);
            }
        }
    }

    if state.field.weather_turns > 0 && state.field.weather_turns != 255 {
        state.field.weather_turns -= 1;
        if state.field.weather_turns == 0 { clear_weather(state, keys); }
    }
}

fn step_terrain_expiry(state: &mut BattleState, keys: &ZobristKeys) {
    if state.field.terrain == TERRAIN_NONE { return; }
    if state.field.terrain_turns > 0 {
        state.field.terrain_turns -= 1;
        if state.field.terrain_turns == 0 { clear_terrain(state, keys); }
    }
}

fn step_wish(state: &mut BattleState, keys: &ZobristKeys) {
    for side in 0..2 {
        let sc = &state.sides[side].side_conditions;
        if sc.wish_turns == 1 {
            let slot = state.sides[side].active_index as usize;
            let wish_hp = sc.wish_hp;
            heal(state, keys, side, slot, wish_hp);
            state.sides[side].side_conditions.wish_hp = 0;
            state.sides[side].side_conditions.wish_turns = 0;
        } else if state.sides[side].side_conditions.wish_turns > 1 {
            state.sides[side].side_conditions.wish_turns -= 1;
        }
    }
}

fn step_status_damage(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { return; }

    let status = state.sides[side].team[slot].status;
    match status {
        STATUS_BURN   => { deal_proportional_damage(state, keys, side, slot, 1, 16); }
        STATUS_POISON => { deal_proportional_damage(state, keys, side, slot, 1, 8); }
        STATUS_BAD_POISON => {
            let counter = state.sides[side].active.toxic_counter.max(1);
            let max_hp = state.sides[side].team[slot].max_hp;
            let damage = (max_hp as u32 * counter as u32 / 16).max(1) as u16;
            deal_damage(state, keys, side, slot, damage);
            state.sides[side].active.toxic_counter = counter.saturating_add(1);
        }
        _ => {}
    }
}

fn step_leech_seed(state: &mut BattleState, keys: &ZobristKeys) {
    for side in 0..2 {
        let slot = state.sides[side].active_index as usize;
        if !state.sides[side].active.has_volatile(VOL_LEECH_SEED) { continue; }
        if state.sides[side].team[slot].is_fainted() { continue; }
        let drain = (state.sides[side].team[slot].max_hp / 8).max(1);
        deal_damage(state, keys, side, slot, drain);
        let opp = 1 - side;
        let opp_slot = state.sides[opp].active_index as usize;
        if !state.sides[opp].team[opp_slot].is_fainted() { heal(state, keys, opp, opp_slot, drain); }
    }
}

fn step_binding_damage(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if !state.sides[side].active.has_volatile(VOL_BOUND) { return; }
    if state.sides[side].team[slot].is_fainted() { return; }
    let opp_item_id = state.sides[1-side].team[state.sides[1-side].active_index as usize].item_id;
    let has_band = data_bridge::item(opp_item_id).has(ItemFlag::BINDING_BOOST);
    let (n, d) = if has_band { (1, 6) } else { (1, 8) };
    deal_proportional_damage(state, keys, side, slot, n, d);
}

fn step_passive_healing(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    let flags = state.sides[side].active.volatile_flags;
    if flags & VOL_AQUA_RING != 0 { let mhp = state.sides[side].team[slot].max_hp; heal(state, keys, side, slot, mhp / 16); }
    if flags & VOL_INGRAIN != 0 { let mhp = state.sides[side].team[slot].max_hp; heal(state, keys, side, slot, mhp / 16); }
}

fn step_grassy_terrain(state: &mut BattleState, keys: &ZobristKeys) {
    if state.field.terrain != TERRAIN_GRASSY { return; }
    for side in 0..2 {
        let slot = state.sides[side].active_index as usize;
        if state.sides[side].team[slot].is_fainted() { continue; }
        if !is_grounded(state, side) { continue; }
        let mhp = state.sides[side].team[slot].max_hp;
        heal(state, keys, side, slot, mhp / 16);
    }
}

fn step_item_healing(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    let itm = data_bridge::item(state.sides[side].team[slot].item_id);
    if itm.has(ItemFlag::LEFTOVERS) {
        let m = state.sides[side].team[slot].max_hp;
        heal(state, keys, side, slot, m / 16);
    }
    if itm.has(ItemFlag::BLACK_SLUDGE) {
        let (t1, t2) = effective_types(state, side);
        if t1 == Type::Poison as u8 || t2 == Type::Poison as u8 {
            let m = state.sides[side].team[slot].max_hp;
            heal(state, keys, side, slot, m / 16);
        } else {
            deal_proportional_damage(state, keys, side, slot, 1, 8);
        }
    }
}

fn step_screen_expiry(state: &mut BattleState, side: usize) {
    let sc = &mut state.sides[side].side_conditions;
    if sc.reflect_turns > 0 { sc.reflect_turns -= 1; }
    if sc.light_screen_turns > 0 { sc.light_screen_turns -= 1; }
    if sc.aurora_veil_turns > 0 { sc.aurora_veil_turns -= 1; }
}

fn step_tailwind_expiry(state: &mut BattleState, side: usize) {
    let sc = &mut state.sides[side].side_conditions;
    if sc.tailwind_turns > 0 { sc.tailwind_turns -= 1; }
}

fn step_volatile_counters(state: &mut BattleState, keys: &ZobristKeys) {
    for side in 0..2 {
        let a = &mut state.sides[side].active;
        macro_rules! dec { ($f:ident) => { if a.$f > 0 { a.$f -= 1; } } }
        dec!(taunt_turns); dec!(encore_turns); dec!(disable_turns);
        dec!(magnet_rise_turns); dec!(telekinesis_turns); dec!(heal_block_turns);
        if a.encore_turns == 0 && a.encore_move != 0 { a.encore_move = 0; }
        if a.disable_turns == 0 && a.disabled_move != 0 { a.disabled_move = 0; }
        if a.magnet_rise_turns == 0 && a.has_volatile(VOL_MAGNET_RISE) {
            clear_volatile(state, keys, side, VOL_MAGNET_RISE);
        }
    }
    if state.field.trick_room_turns > 0 {
        state.field.trick_room_turns -= 1;
        if state.field.trick_room_turns == 0 { state.zobrist ^= keys.trick_room; }
    }
    if state.field.gravity_turns > 0 {
        state.field.gravity_turns -= 1;
        if state.field.gravity_turns == 0 { state.zobrist ^= keys.gravity; }
    }
}

fn step_perish_song(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    if !state.sides[side].active.has_volatile(VOL_PERISH_SONG) { return; }
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].active.perish_count == 0 {
        let hp = state.sides[side].team[slot].current_hp;
        if hp > 0 { deal_damage(state, keys, side, slot, hp); }
        clear_volatile(state, keys, side, VOL_PERISH_SONG);
    } else if state.sides[side].active.perish_count != 255 {
        state.sides[side].active.perish_count -= 1;
    }
}

fn step_eot_abilities(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    let ability = effective_ability(state, side);
    match ability {
        data_bridge::ABILITY_SPEED_BOOST => {
            if state.sides[side].active.turns_active > 0 { apply_boost(state, keys, side, SPE, 1); }
        }
        data_bridge::ABILITY_POISON_HEAL => {
            let status = state.sides[side].team[slot].status;
            if status == STATUS_POISON || status == STATUS_BAD_POISON {
                let m = state.sides[side].team[slot].max_hp;
                heal(state, keys, side, slot, m / 8);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::zobrist::{compute_full_hash, validate_hash};
    fn setup() -> (BattleState, ZobristKeys) {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        for side in 0..2 { state.sides[side].team[0].species_id = (side as u16 +1)*25; state.sides[side].team[0].current_hp = 200; state.sides[side].team[0].max_hp = 200; }
        state.zobrist = compute_full_hash(&state, &keys);
        (state, keys)
    }
    #[test] fn test_burn() { let (mut s, k) = setup(); s.sides[0].team[0].status = STATUS_BURN; s.zobrist = compute_full_hash(&s, &k); step_status_damage(&mut s, &k, 0); assert_eq!(s.sides[0].team[0].current_hp, 188); assert!(validate_hash(&s, &k)); }
    #[test] fn test_turn_inc() { let (mut s, k) = setup(); end_of_turn(&mut s, &k); assert_eq!(s.field.turn, 1); }
}
