//! Switch-in and switch-out logic.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag};
use crate::state::accessors::{effective_ability, effective_types, effective_stat, effective_species, effective_moves, is_grounded};
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
    // Zero to Hero: mark Palafin for Hero forme on next switch-in
    if ability == data_bridge::ABILITY_ZERO_TO_HERO {
        state.sides[side].team[idx].flags |= MON_FLAG_HERO_ACTIVATED;
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

    // Check berry activation after hazard damage (e.g. Sitrus Berry can save)
    let slot = state.sides[side].active_index as usize;
    if !state.sides[side].team[slot].is_fainted() {
        crate::state::move_exec::check_pinch_berry(state, keys, side, slot);
    }

    // If fainted from hazards, skip ability/item activation
    if state.sides[side].team[slot].is_fainted() {
        return;
    }

    // Palafin Zero to Hero: transform on switch-in
    crate::state::forme::check_palafin_hero(state, keys, side);

    // Healing Wish / Lunar Dance: fully heal the incoming mon
    {
        let sc = &state.sides[side].side_conditions;
        let has_hw = sc.has_healing_wish();
        let has_ld = sc.has_lunar_dance();
        if has_hw || has_ld {
            let max_hp = state.sides[side].team[slot].max_hp;
            heal(state, keys, side, slot, max_hp);
            if state.sides[side].team[slot].status != STATUS_NONE {
                clear_status(state, keys, side, slot);
            }
            if has_ld {
                // Restore PP too
                for i in 0..4 {
                    state.sides[side].team[slot].pp[i] = 255; // max PP (simplified)
                }
            }
            state.sides[side].side_conditions.set_healing_wish(false);
            state.sides[side].side_conditions.set_lunar_dance(false);
        }
    }

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
        let is_steel = t1 == Type::Steel as u8 || t2 == Type::Steel as u8;
        if is_poison {
            // Poison types absorb and remove Toxic Spikes
            state.sides[side].side_conditions.toxic_spikes = 0;
        } else if !is_steel {
            // Steel types are immune to poison; all others get poisoned
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
        // -- Intimidate (blocked by Mist) --
        data_bridge::ABILITY_INTIMIDATE  => {
            if state.sides[opp].side_conditions.mist_turns() == 0 {
                apply_boost(state, keys, opp, ATK, -1);
            }
        }

        // -- Weather setters --
        data_bridge::ABILITY_DRIZZLE     => { set_weather(state, keys, WEATHER_RAIN, 5); }
        data_bridge::ABILITY_DROUGHT     => { set_weather(state, keys, WEATHER_SUN, 5); }
        data_bridge::ABILITY_SAND_STREAM => { set_weather(state, keys, WEATHER_SAND, 5); }
        data_bridge::ABILITY_SNOW_WARNING=> { set_weather(state, keys, WEATHER_SNOW, 5); }

        // -- Terrain setters --
        data_bridge::ABILITY_ELECTRIC_SURGE => { set_terrain(state, keys, TERRAIN_ELECTRIC, 5); }
        data_bridge::ABILITY_GRASSY_SURGE  => { set_terrain(state, keys, TERRAIN_GRASSY, 5); }
        data_bridge::ABILITY_MISTY_SURGE   => { set_terrain(state, keys, TERRAIN_MISTY, 5); }
        data_bridge::ABILITY_PSYCHIC_SURGE => { set_terrain(state, keys, TERRAIN_PSYCHIC, 5); }

        // -- Download: +1 Atk if foe SpD < Def, else +1 SpA --
        data_bridge::ABILITY_DOWNLOAD => {
            let opp_def = state.active_mon(opp).stats[DEF] as u32;
            let opp_spd = state.active_mon(opp).stats[SPD] as u32;
            if opp_spd < opp_def {
                apply_boost(state, keys, side, SPA, 1);
            } else {
                apply_boost(state, keys, side, ATK, 1);
            }
        }

        // -- Trace: copy opponent's ability --
        data_bridge::ABILITY_TRACE => {
            let opp_ability = effective_ability(state, opp);
            if opp_ability != 0
                && opp_ability != data_bridge::ABILITY_TRACE
                && opp_ability != data_bridge::ABILITY_IMPOSTER
            {
                state.sides[side].active.override_ability = opp_ability;
                set_volatile(state, keys, side, VOL_ABILITY_OVERRIDDEN);
            }
        }

        // -- Imposter: transform into opponent --
        data_bridge::ABILITY_IMPOSTER => {
            let opp_mon = state.active_mon(opp);
            if opp_mon.current_hp > 0 {
                let opp_species = effective_species(state, opp);
                let opp_types = effective_types(state, opp);
                let opp_stats = [
                    effective_stat(state, opp, ATK),
                    effective_stat(state, opp, DEF),
                    effective_stat(state, opp, SPA),
                    effective_stat(state, opp, SPD),
                    effective_stat(state, opp, SPE),
                ];
                let opp_moves = effective_moves(state, opp);
                let opp_ability_id = effective_ability(state, opp);

                state.sides[side].active.override_species = opp_species;
                state.sides[side].active.override_types = [opp_types.0, opp_types.1];
                state.sides[side].active.override_stats = opp_stats;
                state.sides[side].active.override_moves = opp_moves;
                state.sides[side].active.override_pp = [5, 5, 5, 5];
                state.sides[side].active.override_ability = opp_ability_id;
                set_volatile(state, keys, side, VOL_TRANSFORMED);
                set_volatile(state, keys, side, VOL_TYPES_OVERRIDDEN);
            }
        }

        // -- Neutralizing Gas: suppress all other abilities --
        data_bridge::ABILITY_NEUTRALIZING_GAS => {
            // Set VOL_ABILITY_SUPPRESSED on opponent
            set_volatile(state, keys, opp, VOL_ABILITY_SUPPRESSED);
        }

        // -- Intrepid Sword: +1 Atk --
        data_bridge::ABILITY_INTREPID_SWORD => {
            apply_boost(state, keys, side, ATK, 1);
        }

        // -- Dauntless Shield: +1 Def --
        data_bridge::ABILITY_DAUNTLESS_SHIELD => {
            apply_boost(state, keys, side, DEF, 1);
        }

        // -- Hospitality: heal ally 25% (in singles, heal self) --
        data_bridge::ABILITY_HOSPITALITY => {
            // In singles, this is a no-op (targets partner slot).
            // For our purposes, skip.
        }

        // -- Supersweet Syrup: -1 Eva on opponent --
        data_bridge::ABILITY_SUPERSWEET_SYRUP => {
            apply_boost(state, keys, opp, EVA, -1);
        }

        // -- Embody Aspect variants --
        data_bridge::ABILITY_EMBODY_ASPECT_TEAL => {
            apply_boost(state, keys, side, SPE, 1);
        }
        data_bridge::ABILITY_EMBODY_ASPECT_WELLSPRING => {
            apply_boost(state, keys, side, SPD, 1);
        }
        data_bridge::ABILITY_EMBODY_ASPECT_HEARTHFLAME => {
            apply_boost(state, keys, side, ATK, 1);
        }
        data_bridge::ABILITY_EMBODY_ASPECT_CORNERSTONE => {
            apply_boost(state, keys, side, DEF, 1);
        }

        // -- Protosynthesis / Quark Drive: identify best stat and set boost --
        data_bridge::ABILITY_PROTOSYNTHESIS => {
            activate_paradox_ability(state, keys, side,
                matches!(state.field.weather, WEATHER_SUN | WEATHER_HARSH_SUN));
        }
        data_bridge::ABILITY_QUARK_DRIVE => {
            activate_paradox_ability(state, keys, side,
                state.field.terrain == TERRAIN_ELECTRIC);
        }

        // -- Unnerve / Air Lock / Cloud Nine: passive effects, no switch-in action --
        // These are checked by other systems (berry activation, weather damage).
        _ => {}
    }
}

/// Activate Protosynthesis or Quark Drive: find highest stat, encode in _padding[3].
/// `field_active` = true if the ability's weather/terrain is currently active.
fn activate_paradox_ability(
    state: &mut BattleState, keys: &ZobristKeys, side: usize, field_active: bool,
) {
    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];

    // Determine if ability should activate
    const ITEM_BOOSTER_ENERGY: u16 = 1880;
    let from_booster = !field_active && mon.item_id == ITEM_BOOSTER_ENERGY;
    if !field_active && !from_booster { return; }

    // Find highest raw stat (0=Atk, 1=Def, 2=SpA, 3=SpD, 4=Spe)
    let stats = &mon.stats;
    let best = (0..5).max_by_key(|&i| stats[i]).unwrap_or(0);

    // Encode in upper nibble of _padding[3]: stat+1 (1-5)
    state.sides[side].active._padding[3] =
        (state.sides[side].active._padding[3] & 0x0F) | (((best as u8) + 1) << 4);

    // Consume Booster Energy if it was the trigger
    if from_booster {
        consume_item(state, keys, side, slot);
        if effective_ability(state, side) == data_bridge::ABILITY_UNBURDEN {
            set_volatile(state, keys, side, VOL_UNBURDEN);
        }
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
        state.zobrist = compute_full_hash(&state, &keys);

        apply_switch_in_item(&mut state, &keys, 0);
        assert_eq!(state.sides[0].active.boosts[DEF], 0);
    }

    // ── Phase 3: Intimidate, hazard damage, switch-out abilities ──

    #[test]
    fn test_intimidate_lowers_opponent_atk() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [100; 5],
            ability_id: data_bridge::ABILITY_INTIMIDATE,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // Switch to Intimidate mon
        perform_switch(&mut state, &keys, 0, 1);

        assert_eq!(state.sides[1].active.boosts[ATK], -1);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_stealth_rock_damage_by_type() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        // Side 0 mon: species 25 (Pikachu, Electric type)
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 25, current_hp: 400, max_hp: 400,
            stats: [100; 5], ..Default::default()
        };
        // Set stealth rock on side 0
        state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        // Should have taken some damage (Rock vs Electric = 1× → 1/8 max HP = 50 damage)
        // The exact amount depends on type effectiveness
        assert!(state.sides[0].team[1].current_hp < 400);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_spikes_damage_by_layer() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 25, current_hp: 400, max_hp: 400,
            stats: [100; 5], ..Default::default()
        };
        // 1 layer of spikes on side 0
        state.sides[0].side_conditions.spikes = 1;
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        // 1 layer = 1/8 max HP = 50 damage
        assert_eq!(state.sides[0].team[1].current_hp, 350);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_spikes_3_layers() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 25, current_hp: 400, max_hp: 400,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].side_conditions.spikes = 3;
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        // 3 layers = 1/4 max HP = 100 damage
        assert_eq!(state.sides[0].team[1].current_hp, 300);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_toxic_spikes_poisons_on_switch() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 25, current_hp: 400, max_hp: 400,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].side_conditions.toxic_spikes = 1;
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        assert_eq!(state.sides[0].team[1].status, STATUS_POISON);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_toxic_spikes_2_layers_badly_poisons() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 25, current_hp: 400, max_hp: 400,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].side_conditions.toxic_spikes = 2;
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        assert_eq!(state.sides[0].team[1].status, STATUS_BAD_POISON);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_faint_from_hazards_skips_ability() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        // Mon with 1 HP switching into stealth rock
        state.sides[0].team[1] = MonSlot {
            species_id: 25, current_hp: 1, max_hp: 400,
            stats: [100; 5],
            ability_id: data_bridge::ABILITY_INTIMIDATE,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        // Mon fainted from hazards → Intimidate should NOT have activated
        assert_eq!(state.sides[0].team[1].current_hp, 0);
        assert_eq!(state.sides[1].active.boosts[ATK], 0);
    }

    #[test]
    fn test_natural_cure_clears_status() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5],
            ability_id: data_bridge::ABILITY_NATURAL_CURE,
            status: STATUS_BURN,
            ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        switch_out(&mut state, &keys, 0);

        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_regenerator_heals_on_switch_out() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 300,
            stats: [100; 5],
            ability_id: data_bridge::ABILITY_REGENERATOR,
            ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        switch_out(&mut state, &keys, 0);

        // Should heal 1/3 of 300 = 100
        assert_eq!(state.sides[0].team[0].current_hp, 300);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_electric_surge_sets_terrain() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [100; 5],
            ability_id: data_bridge::ABILITY_ELECTRIC_SURGE,
            ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        assert_eq!(state.field.terrain, TERRAIN_ELECTRIC);
        assert_eq!(state.field.terrain_turns, 5);
    }

    #[test]
    fn test_download_boosts_spa_when_spd_lower() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [100; 5],
            ability_id: data_bridge::ABILITY_DOWNLOAD,
            ..Default::default()
        };
        // Opponent has lower SpD than Def
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 300, max_hp: 300,
            stats: [100, 120, 100, 80, 100], // Def=120, SpD=80
            ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        assert_eq!(state.sides[0].active.boosts[SPA], 1);
        assert!(validate_hash(&state, &keys));
    }
}
