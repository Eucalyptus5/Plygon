//! State mutation functions.  Every mutation updates the Zobrist hash.

use crate::state::structs::*;
use crate::state::zobrist::{ZobristKeys, hp_bucket};

pub fn deal_damage(state: &mut BattleState, keys: &ZobristKeys, side: usize, slot: usize, amount: u16) {
    let mon = &mut state.sides[side].team[slot];
    let old_bucket = hp_bucket(mon.current_hp, mon.max_hp);
    mon.current_hp = mon.current_hp.saturating_sub(amount);
    let new_bucket = hp_bucket(mon.current_hp, mon.max_hp);
    if old_bucket != new_bucket {
        state.zobrist ^= keys.hp_bucket[side][slot][old_bucket];
        state.zobrist ^= keys.hp_bucket[side][slot][new_bucket];
    }
}

pub fn heal(state: &mut BattleState, keys: &ZobristKeys, side: usize, slot: usize, amount: u16) {
    let mon = &mut state.sides[side].team[slot];
    let old_bucket = hp_bucket(mon.current_hp, mon.max_hp);
    mon.current_hp = mon.current_hp.saturating_add(amount).min(mon.max_hp);
    let new_bucket = hp_bucket(mon.current_hp, mon.max_hp);
    if old_bucket != new_bucket {
        state.zobrist ^= keys.hp_bucket[side][slot][old_bucket];
        state.zobrist ^= keys.hp_bucket[side][slot][new_bucket];
    }
}

pub fn deal_proportional_damage(state: &mut BattleState, keys: &ZobristKeys, side: usize, slot: usize, num: u16, den: u16) {
    let max_hp = state.sides[side].team[slot].max_hp;
    let damage = (max_hp as u32 * num as u32 / den as u32).max(1) as u16;
    deal_damage(state, keys, side, slot, damage);
}

pub fn set_status(state: &mut BattleState, keys: &ZobristKeys, side: usize, slot: usize, status: u8, counter: u8) -> bool {
    let mon = &mut state.sides[side].team[slot];
    if mon.status != STATUS_NONE { return false; }
    state.zobrist ^= keys.status[side][slot][STATUS_NONE as usize];
    mon.status = status;
    mon.status_counter = counter;
    state.zobrist ^= keys.status[side][slot][status as usize];
    true
}

pub fn clear_status(state: &mut BattleState, keys: &ZobristKeys, side: usize, slot: usize) {
    let mon = &mut state.sides[side].team[slot];
    if mon.status == STATUS_NONE { return; }
    state.zobrist ^= keys.status[side][slot][mon.status as usize];
    mon.status = STATUS_NONE;
    mon.status_counter = 0;
    state.zobrist ^= keys.status[side][slot][STATUS_NONE as usize];
}

pub fn apply_boost(state: &mut BattleState, keys: &ZobristKeys, side: usize, stat_index: usize, stages: i8) -> i8 {
    let active = &mut state.sides[side].active;
    let old = active.boosts[stat_index];
    let new = (old as i16 + stages as i16).clamp(-6, 6) as i8;
    let actual = new - old;
    if actual != 0 {
        if old != 0 { state.zobrist ^= keys.boosts[side][stat_index][(old + 6) as usize]; }
        active.boosts[stat_index] = new;
        if new != 0 { state.zobrist ^= keys.boosts[side][stat_index][(new + 6) as usize]; }
    }
    actual
}

pub fn set_volatile(state: &mut BattleState, keys: &ZobristKeys, side: usize, flag: u32) {
    let active = &mut state.sides[side].active;
    if active.volatile_flags & flag == 0 {
        active.volatile_flags |= flag;
        state.zobrist ^= keys.volatile_bit[side][flag.trailing_zeros() as usize];
    }
}

pub fn clear_volatile(state: &mut BattleState, keys: &ZobristKeys, side: usize, flag: u32) {
    let active = &mut state.sides[side].active;
    if active.volatile_flags & flag != 0 {
        active.volatile_flags &= !flag;
        state.zobrist ^= keys.volatile_bit[side][flag.trailing_zeros() as usize];
    }
}

pub fn set_weather(state: &mut BattleState, keys: &ZobristKeys, weather: u8, turns: u8) {
    state.zobrist ^= keys.weather[state.field.weather as usize];
    state.field.weather = weather;
    state.field.weather_turns = turns;
    state.zobrist ^= keys.weather[weather as usize];
}

pub fn clear_weather(state: &mut BattleState, keys: &ZobristKeys) {
    set_weather(state, keys, WEATHER_NONE, 0);
}

pub fn set_terrain(state: &mut BattleState, keys: &ZobristKeys, terrain: u8, turns: u8) {
    state.zobrist ^= keys.terrain[state.field.terrain as usize];
    state.field.terrain = terrain;
    state.field.terrain_turns = turns;
    state.zobrist ^= keys.terrain[terrain as usize];
}

pub fn clear_terrain(state: &mut BattleState, keys: &ZobristKeys) {
    set_terrain(state, keys, TERRAIN_NONE, 0);
}

pub fn set_trick_room(state: &mut BattleState, keys: &ZobristKeys, turns: u8) {
    let was = state.field.trick_room_turns > 0;
    state.field.trick_room_turns = turns;
    if was != (turns > 0) { state.zobrist ^= keys.trick_room; }
}

pub fn set_gravity(state: &mut BattleState, keys: &ZobristKeys, turns: u8) {
    let was = state.field.gravity_turns > 0;
    state.field.gravity_turns = turns;
    if was != (turns > 0) { state.zobrist ^= keys.gravity; }
}

pub fn consume_item(state: &mut BattleState, keys: &ZobristKeys, side: usize, slot: usize) {
    let mon = &mut state.sides[side].team[slot];
    if mon.item_id != 0 {
        state.zobrist ^= keys.item[side][slot][mon.item_id as usize];
        mon.item_id = 0;
    }
}

pub fn set_item(state: &mut BattleState, keys: &ZobristKeys, side: usize, slot: usize, item_id: u16) {
    let mon = &mut state.sides[side].team[slot];
    if mon.item_id != 0 { state.zobrist ^= keys.item[side][slot][mon.item_id as usize]; }
    mon.item_id = item_id;
    if item_id != 0 { state.zobrist ^= keys.item[side][slot][item_id as usize]; }
}

pub fn set_phase(state: &mut BattleState, keys: &ZobristKeys, phase: u8) {
    state.zobrist ^= keys.phase[state.phase as usize];
    state.phase = phase;
    state.zobrist ^= keys.phase[phase as usize];
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
    use crate::state::zobrist::{compute_full_hash, validate_hash};

    fn setup() -> (BattleState, ZobristKeys) {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 25;
        state.sides[0].team[0].current_hp = 211;
        state.sides[0].team[0].max_hp = 211;
        state.sides[0].team[0].item_id = 234;
        state.zobrist = compute_full_hash(&state, &keys);
        (state, keys)
    }

    #[test] fn test_damage() { let (mut s, k) = setup(); deal_damage(&mut s, &k, 0, 0, 50); assert_eq!(s.sides[0].team[0].current_hp, 161); assert!(validate_hash(&s, &k)); }
    #[test] fn test_heal() { let (mut s, k) = setup(); deal_damage(&mut s, &k, 0, 0, 100); heal(&mut s, &k, 0, 0, 50); assert_eq!(s.sides[0].team[0].current_hp, 161); assert!(validate_hash(&s, &k)); }
    #[test] fn test_status() { let (mut s, k) = setup(); assert!(set_status(&mut s, &k, 0, 0, STATUS_BURN, 0)); assert!(!set_status(&mut s, &k, 0, 0, STATUS_PARALYSIS, 0)); clear_status(&mut s, &k, 0, 0); assert!(validate_hash(&s, &k)); }
    #[test] fn test_boost_clamp() { let (mut s, k) = setup(); apply_boost(&mut s, &k, 0, ATK, 4); assert_eq!(apply_boost(&mut s, &k, 0, ATK, 4), 2); assert!(validate_hash(&s, &k)); }
    #[test] fn test_volatile() { let (mut s, k) = setup(); set_volatile(&mut s, &k, 0, VOL_SUBSTITUTE); assert!(s.sides[0].active.has_volatile(VOL_SUBSTITUTE)); clear_volatile(&mut s, &k, 0, VOL_SUBSTITUTE); assert!(validate_hash(&s, &k)); }
    #[test] fn test_weather() { let (mut s, k) = setup(); set_weather(&mut s, &k, WEATHER_RAIN, 5); clear_weather(&mut s, &k); assert!(validate_hash(&s, &k)); }
}
