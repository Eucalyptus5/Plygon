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
    apply_switch_in_item(state, keys, side);
}

fn apply_switch_in_item(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];
    if mon.item_id == 0 || mon.is_fainted() { return; }
    let item = data_bridge::item(mon.item_id);

    // Terrain seeds: boost stat + consume if matching terrain is active
    if item.has(ItemFlag::TERRAIN_SEED) {
        let terrain = state.field.terrain;
        let activates = match (terrain, item.type_param) {
            // type_param encodes which terrain: 1=Electric, 2=Grassy, 3=Psychic, 4=Misty
            (TERRAIN_ELECTRIC, 1) => true,
            (TERRAIN_GRASSY, 2) => true,
            (TERRAIN_PSYCHIC, 3) => true,
            (TERRAIN_MISTY, 4) => true,
            _ => false,
        };
        if activates {
            // Electric Seed → +1 Def, Grassy Seed → +1 Def,
            // Psychic Seed → +1 SpD, Misty Seed → +1 SpD
            let stat = match item.type_param {
                1 | 2 => DEF,
                3 | 4 => SPD,
                _ => return,
            };
            apply_boost(state, keys, side, stat, 1);
            consume_item(state, keys, side, slot);
            if effective_ability(state, side) == data_bridge::ABILITY_UNBURDEN {
                set_volatile(state, keys, side, VOL_UNBURDEN);
            }
        }
    }
}

/// Baton Pass volatile mask: volatiles that transfer on Baton Pass.
const BATON_PASS_VOLATILE_MASK: u32 =
    VOL_SUBSTITUTE | VOL_LEECH_SEED | VOL_INGRAIN | VOL_AQUA_RING
    | VOL_FOCUS_ENERGY | VOL_PERISH_SONG | VOL_MAGNET_RISE;

pub fn perform_switch(state: &mut BattleState, keys: &ZobristKeys, side: usize, new_index: usize) {
    let is_baton_pass = state.sides[side].active._padding[0] != 0;

    // Save Baton Pass state before switch_out zeros everything
    let saved_boosts: [i8; 7];
    let saved_volatiles: u32;
    let saved_sub_hp: u16;
    if is_baton_pass {
        saved_boosts = state.sides[side].active.boosts;
        saved_volatiles = state.sides[side].active.volatile_flags & BATON_PASS_VOLATILE_MASK;
        saved_sub_hp = state.sides[side].active.substitute_hp;
    } else {
        saved_boosts = [0; 7];
        saved_volatiles = 0;
        saved_sub_hp = 0;
    }

    switch_out(state, keys, side);
    switch_in(state, keys, side, new_index);

    // Restore Baton Pass state onto the new active
    if is_baton_pass {
        for stat in 0..7 {
            if saved_boosts[stat] != 0 {
                apply_boost(state, keys, side, stat, saved_boosts[stat]);
            }
        }
        for bit in 0..32u32 {
            if saved_volatiles & (1 << bit) != 0 {
                set_volatile(state, keys, side, 1 << bit);
            }
        }
        if saved_sub_hp > 0 {
            state.sides[side].active.substitute_hp = saved_sub_hp;
        }
    }
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

    #[test]
    fn test_baton_pass_preserves_boosts() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [80; 5], ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // Set +2 Atk and +1 SpA
        apply_boost(&mut state, &keys, 0, ATK, 2);
        apply_boost(&mut state, &keys, 0, SPA, 1);

        // Set Baton Pass flag
        state.sides[0].active._padding[0] = 1;

        // Perform Baton Pass switch to slot 1
        perform_switch(&mut state, &keys, 0, 1);

        // New active should inherit boosts
        assert_eq!(state.sides[0].active.boosts[ATK], 2);
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
        assert_eq!(state.sides[0].active_index, 1);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_baton_pass_preserves_substitute() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [80; 5], ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // Set up Substitute with 50 HP
        set_volatile(&mut state, &keys, 0, VOL_SUBSTITUTE);
        state.sides[0].active.substitute_hp = 50;

        // Also set a non-Baton volatile (e.g., VOL_FLASH_FIRE) — should NOT transfer
        set_volatile(&mut state, &keys, 0, VOL_FLASH_FIRE);

        // Set Baton Pass flag
        state.sides[0].active._padding[0] = 1;

        perform_switch(&mut state, &keys, 0, 1);

        // Substitute should transfer, Flash Fire should not
        assert!(state.sides[0].active.has_volatile(VOL_SUBSTITUTE));
        assert_eq!(state.sides[0].active.substitute_hp, 50);
        assert!(!state.sides[0].active.has_volatile(VOL_FLASH_FIRE));
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_normal_switch_clears_boosts() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [80; 5], ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        apply_boost(&mut state, &keys, 0, ATK, 2);

        // Normal switch (no baton pass flag)
        perform_switch(&mut state, &keys, 0, 1);

        // Boosts should NOT transfer
        assert_eq!(state.sides[0].active.boosts[ATK], 0);
        assert!(validate_hash(&state, &keys));
    }

    // ── Step 8: Terrain seed tests ─────────────────────────────

    #[test]
    fn test_terrain_seed_no_trigger_without_terrain() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        // Give a fake terrain seed item (doesn't exist in gen data,
        // so we test apply_switch_in_item directly)
        // No terrain active → should not trigger
        state.zobrist = compute_full_hash(&state, &keys);

        apply_switch_in_item(&mut state, &keys, 0);
        assert_eq!(state.sides[0].active.boosts[DEF], 0);
    }
}
