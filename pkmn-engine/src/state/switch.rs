//! Switch-in and switch-out logic.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag};
use crate::state::accessors::{effective_ability, effective_weather, effective_types, effective_stat, effective_species, effective_moves, is_grounded};
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

    // Air Lock / Cloud Nine: clear weather suppression if opponent doesn't also suppress
    if ability == data_bridge::ABILITY_AIR_LOCK || ability == data_bridge::ABILITY_CLOUD_NINE {
        let opp = 1 - side;
        let opp_ability = effective_ability(state, opp);
        if opp_ability != data_bridge::ABILITY_AIR_LOCK && opp_ability != data_bridge::ABILITY_CLOUD_NINE {
            state.field.field_flags &= !FIELD_WEATHER_SUPPRESSED;
        }
    }

    // Neutralizing Gas: re-trigger opponent's switch-in ability
    if ability == data_bridge::ABILITY_NEUTRALIZING_GAS {
        let opp = 1 - side;
        clear_volatile(state, keys, opp, VOL_ABILITY_SUPPRESSED);
        let opp_slot = state.sides[opp].active_index as usize;
        if !state.sides[opp].team[opp_slot].is_fainted() {
            let opp_ability = effective_ability(state, opp);
            // Imposter only fires on actual switch-in, not ability reactivation
            if opp_ability != data_bridge::ABILITY_IMPOSTER {
                apply_switch_in_ability(state, keys, opp);
            }
        }
    }

    let flags = state.sides[side].active.volatile_flags;
    for bit in 0..32u32 {
        if flags & (1 << bit) != 0 { state.zobrist ^= keys.volatile_bit[side][bit as usize]; }
    }
    let boosts = state.sides[side].active.boosts;
    for stat in 0..7 {
        if boosts[stat] != 0 { state.zobrist ^= keys.boosts[side][stat][(boosts[stat] + 6) as usize]; }
    }
    state.sides[side].active.zero();
    state.sides[side].set_last_consumed_berry(0);
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
    if state.field.magic_room_turns() > 0 { return; }
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
    if state.field.magic_room_turns() == 0
        && data_bridge::item(mon.item_id).has(ItemFlag::HAZARD_IMMUNE) { return; }
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { return; }

    let sc = state.sides[side].side_conditions;

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
        } else if !is_steel && state.sides[side].side_conditions.safeguard_turns() == 0
            && !crate::state::forme::is_minior_meteor_forme(state, side)
        {
            // Steel types are immune to poison; Safeguard blocks status
            match sc.toxic_spikes {
                1 => { set_status(state, keys, side, slot, STATUS_POISON, 0); }
                _ => { set_status(state, keys, side, slot, STATUS_BAD_POISON, 0); }
            }
        }
    }

    // Sticky Web (grounded only)
    if sc.hazard_flags & HAZARD_STICKY_WEB != 0 && is_grounded(state, side)
        && state.sides[side].side_conditions.mist_turns() == 0
    {
        apply_boost(state, keys, side, SPE, -1);
    }
}

#[inline]
fn is_untraceable(ability: u16) -> bool {
    matches!(ability,
        0
        | data_bridge::ABILITY_AS_ONE_GLASTRIER
        | data_bridge::ABILITY_AS_ONE_SPECTRIER
        | data_bridge::ABILITY_BATTLE_BOND
        | data_bridge::ABILITY_COMATOSE
        | data_bridge::ABILITY_COMMANDER
        | data_bridge::ABILITY_DISGUISE
        | data_bridge::ABILITY_EMBODY_ASPECT_TEAL
        | data_bridge::ABILITY_EMBODY_ASPECT_WELLSPRING
        | data_bridge::ABILITY_EMBODY_ASPECT_HEARTHFLAME
        | data_bridge::ABILITY_EMBODY_ASPECT_CORNERSTONE
        | data_bridge::ABILITY_FLOWER_GIFT
        | data_bridge::ABILITY_FORECAST
        | data_bridge::ABILITY_GULP_MISSILE
        | data_bridge::ABILITY_HUNGER_SWITCH
        | data_bridge::ABILITY_ICE_FACE
        | data_bridge::ABILITY_ILLUSION
        | data_bridge::ABILITY_IMPOSTER
        | data_bridge::ABILITY_MULTITYPE
        | data_bridge::ABILITY_NEUTRALIZING_GAS
        | data_bridge::ABILITY_POISON_PUPPETEER
        | data_bridge::ABILITY_POWER_CONSTRUCT
        | data_bridge::ABILITY_POWER_OF_ALCHEMY
        | data_bridge::ABILITY_PROTOSYNTHESIS
        | data_bridge::ABILITY_QUARK_DRIVE
        | data_bridge::ABILITY_RECEIVER
        | data_bridge::ABILITY_RKS_SYSTEM
        | data_bridge::ABILITY_SCHOOLING
        | data_bridge::ABILITY_SHIELDS_DOWN
        | data_bridge::ABILITY_STANCE_CHANGE
        | data_bridge::ABILITY_TERA_SHIFT
        | data_bridge::ABILITY_TERA_SHELL
        | data_bridge::ABILITY_TERAFORM_ZERO
        | data_bridge::ABILITY_TRACE
        | data_bridge::ABILITY_ZEN_MODE
        | data_bridge::ABILITY_ZERO_TO_HERO
    )
}

fn apply_switch_in_ability(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let ability = effective_ability(state, side);
    let opp = 1 - side;
    match ability {
        data_bridge::ABILITY_INTIMIDATE  => {
            let opp_ability = effective_ability(state, opp);
            if !state.sides[opp].active.has_volatile(VOL_SUBSTITUTE) {
                if opp_ability == data_bridge::ABILITY_GUARD_DOG {
                    // Guard Dog reverses Intimidate into +1 Atk (bypasses Mist)
                    apply_boost(state, keys, opp, ATK, 1);
                } else {
                    let blocked = matches!(opp_ability,
                        data_bridge::ABILITY_CLEAR_BODY
                        | data_bridge::ABILITY_WHITE_SMOKE
                        | data_bridge::ABILITY_FULL_METAL_BODY
                        | data_bridge::ABILITY_INNER_FOCUS
                        | data_bridge::ABILITY_OBLIVIOUS
                        | data_bridge::ABILITY_OWN_TEMPO
                        | data_bridge::ABILITY_SCRAPPY
                        | data_bridge::ABILITY_HYPER_CUTTER
                    ) || state.sides[opp].side_conditions.mist_turns() > 0;
                    if !blocked {
                        apply_boost(state, keys, opp, ATK, -1);
                    }
                }
            }
            // Rattled: +1 Speed when targeted by Intimidate, regardless of blocking
            if opp_ability == data_bridge::ABILITY_RATTLED {
                apply_boost(state, keys, opp, SPE, 1);
            }
        }

        data_bridge::ABILITY_DRIZZLE     => { set_weather(state, keys, WEATHER_RAIN, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_DROUGHT     => { set_weather(state, keys, WEATHER_SUN, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_SAND_STREAM => { set_weather(state, keys, WEATHER_SAND, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_SNOW_WARNING=> { set_weather(state, keys, WEATHER_SNOW, 5); check_paradox_deactivation(state); }

        data_bridge::ABILITY_AIR_LOCK | data_bridge::ABILITY_CLOUD_NINE => {
            state.field.field_flags |= FIELD_WEATHER_SUPPRESSED;
            check_paradox_deactivation(state);
        }

        data_bridge::ABILITY_ELECTRIC_SURGE => { set_terrain(state, keys, TERRAIN_ELECTRIC, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_GRASSY_SURGE  => { set_terrain(state, keys, TERRAIN_GRASSY, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_MISTY_SURGE   => { set_terrain(state, keys, TERRAIN_MISTY, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_PSYCHIC_SURGE => { set_terrain(state, keys, TERRAIN_PSYCHIC, 5); check_paradox_deactivation(state); }

        // +1 SpA if foe SpD < Def, else +1 Atk
        data_bridge::ABILITY_DOWNLOAD => {
            let opp_def = state.active_mon(opp).stats[DEF] as u32;
            let opp_spd = state.active_mon(opp).stats[SPD] as u32;
            if opp_spd < opp_def {
                apply_boost(state, keys, side, SPA, 1);
            } else {
                apply_boost(state, keys, side, ATK, 1);
            }
        }

        data_bridge::ABILITY_TRACE => {
            let opp_ability = effective_ability(state, opp);
            if opp_ability != 0 && !is_untraceable(opp_ability) {
                state.sides[side].active.override_ability = opp_ability;
                set_volatile(state, keys, side, VOL_ABILITY_OVERRIDDEN);
                // Trigger the traced ability's switch-in effect
                apply_switch_in_ability(state, keys, side);
            }
        }

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

        data_bridge::ABILITY_NEUTRALIZING_GAS => {
            set_volatile(state, keys, opp, VOL_ABILITY_SUPPRESSED);
        }

        data_bridge::ABILITY_INTREPID_SWORD => {
            let slot = state.sides[side].active_index as usize;
            if state.sides[side].team[slot].flags & MON_FLAG_SWORD_BOOSTED == 0 {
                state.sides[side].team[slot].flags |= MON_FLAG_SWORD_BOOSTED;
                apply_boost(state, keys, side, ATK, 1);
            }
        }

        data_bridge::ABILITY_DAUNTLESS_SHIELD => {
            let slot = state.sides[side].active_index as usize;
            if state.sides[side].team[slot].flags & MON_FLAG_SHIELD_BOOSTED == 0 {
                state.sides[side].team[slot].flags |= MON_FLAG_SHIELD_BOOSTED;
                apply_boost(state, keys, side, DEF, 1);
            }
        }

        // No-op in singles (targets partner slot)
        data_bridge::ABILITY_HOSPITALITY => {
        }

        data_bridge::ABILITY_SUPERSWEET_SYRUP => {
            let slot = state.sides[side].active_index as usize;
            if state.sides[side].team[slot].flags & MON_FLAG_SYRUP_TRIGGERED == 0 {
                state.sides[side].team[slot].flags |= MON_FLAG_SYRUP_TRIGGERED;
                if !state.sides[opp].active.has_volatile(VOL_SUBSTITUTE) {
                    apply_boost(state, keys, opp, EVA, -1);
                }
            }
        }

        data_bridge::ABILITY_SCHOOLING => {
            crate::state::forme::check_schooling(state, keys, side);
        }
        data_bridge::ABILITY_SHIELDS_DOWN => {
            crate::state::forme::check_shields_down(state, keys, side);
        }

        data_bridge::ABILITY_PROTOSYNTHESIS => {
            activate_paradox_ability(state, keys, side,
                matches!(effective_weather(state), WEATHER_SUN | WEATHER_HARSH_SUN));
        }
        data_bridge::ABILITY_QUARK_DRIVE => {
            activate_paradox_ability(state, keys, side,
                state.field.terrain == TERRAIN_ELECTRIC);
        }

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
    let from_booster = !field_active && mon.item_id == data_bridge::ITEM_BOOSTER_ENERGY;
    if !field_active && !from_booster { return; }

    // Find highest stat accounting for stat stages (Showdown getBestStat(false, true))
    let stats = &mon.stats;
    let boosts = &state.sides[side].active.boosts;
    let best = (0..5).max_by_key(|&i| boosted_stat(stats[i], boosts[i])).unwrap_or(0);

    // Store paradox stat+1 and fromBooster flag in _padding[3]
    state.sides[side].active.set_paradox(best as u8 + 1, from_booster);

    // Consume Booster Energy if it was the trigger
    if from_booster {
        consume_item(state, keys, side, slot);
        if effective_ability(state, side) == data_bridge::ABILITY_UNBURDEN {
            set_volatile(state, keys, side, VOL_UNBURDEN);
        }
    }
}

/// After any weather/terrain change, check if Protosynthesis/Quark Drive should deactivate.
/// Only deactivates field-sourced boosts (not Booster Energy).
pub fn check_paradox_deactivation(state: &mut BattleState) {
    for side in 0..2 {
        let slot = state.sides[side].active_index as usize;
        if state.sides[side].team[slot].is_fainted() { continue; }
        if state.sides[side].active.paradox_stat() == 0 { continue; }
        if state.sides[side].active.paradox_from_booster() { continue; }

        let ability = effective_ability(state, side);
        let should_deactivate = match ability {
            data_bridge::ABILITY_PROTOSYNTHESIS => {
                !matches!(effective_weather(state), WEATHER_SUN | WEATHER_HARSH_SUN)
            }
            data_bridge::ABILITY_QUARK_DRIVE => {
                state.field.terrain != TERRAIN_ELECTRIC
            }
            _ => true, // ability changed/suppressed — deactivate
        };
        if should_deactivate {
            state.sides[side].active.clear_paradox();
        }
    }
}

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

    #[test]
    fn test_protosynthesis_sun_boost() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [150, 100, 120, 110, 130], // Atk highest
            ability_id: data_bridge::ABILITY_PROTOSYNTHESIS,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.field.weather = WEATHER_SUN;
        state.field.weather_turns = 5;
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 0);

        assert_eq!(state.sides[0].active.paradox_stat(), ATK as u8 + 1);
        assert!(!state.sides[0].active.paradox_from_booster());
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_protosynthesis_booster_energy() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100, 100, 150, 100, 100], // SpA highest
            ability_id: data_bridge::ABILITY_PROTOSYNTHESIS,
            item_id: data_bridge::ITEM_BOOSTER_ENERGY,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        // No sun — triggers Booster Energy consumption
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 0);

        assert_eq!(state.sides[0].active.paradox_stat(), SPA as u8 + 1);
        assert!(state.sides[0].active.paradox_from_booster());
        assert_eq!(state.sides[0].team[0].item_id, 0); // consumed
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_quark_drive_electric_terrain() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100, 100, 100, 100, 160], // Spe highest
            ability_id: data_bridge::ABILITY_QUARK_DRIVE,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 0);

        assert_eq!(state.sides[0].active.paradox_stat(), SPE as u8 + 1);
        assert!(!state.sides[0].active.paradox_from_booster());
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_paradox_deactivates_weather_end() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [150, 100, 120, 110, 130],
            ability_id: data_bridge::ABILITY_PROTOSYNTHESIS,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        // Activate paradox from sun
        state.sides[0].active.set_paradox(ATK as u8 + 1, false);
        // Sun about to expire
        state.field.weather = WEATHER_SUN;
        state.field.weather_turns = 1;
        state.zobrist = compute_full_hash(&state, &keys);

        crate::state::end_of_turn::end_of_turn(&mut state, &keys);

        // Weather expired, paradox should deactivate
        assert_eq!(state.field.weather, WEATHER_NONE);
        assert_eq!(state.sides[0].active.paradox_stat(), 0);
    }

    #[test]
    fn test_paradox_booster_persists_weather_end() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [150, 100, 120, 110, 130],
            ability_id: data_bridge::ABILITY_PROTOSYNTHESIS,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        // Activated from Booster Energy (item already consumed)
        state.sides[0].active.set_paradox(ATK as u8 + 1, true);
        state.field.weather = WEATHER_SUN;
        state.field.weather_turns = 1;
        state.zobrist = compute_full_hash(&state, &keys);

        crate::state::end_of_turn::end_of_turn(&mut state, &keys);

        // Weather expired but Booster Energy boost persists
        assert_eq!(state.field.weather, WEATHER_NONE);
        assert_eq!(state.sides[0].active.paradox_stat(), ATK as u8 + 1);
        assert!(state.sides[0].active.paradox_from_booster());
    }

    #[test]
    fn test_paradox_no_activate_without_condition() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [150, 100, 120, 110, 130],
            ability_id: data_bridge::ABILITY_PROTOSYNTHESIS,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        // No sun, no Booster Energy
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 0);

        assert_eq!(state.sides[0].active.paradox_stat(), 0);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_paradox_deactivates_terrain_end() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100, 100, 100, 100, 160],
            ability_id: data_bridge::ABILITY_QUARK_DRIVE,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        // Activate paradox from Electric Terrain
        state.sides[0].active.set_paradox(SPE as u8 + 1, false);
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 1;
        state.zobrist = compute_full_hash(&state, &keys);

        crate::state::end_of_turn::end_of_turn(&mut state, &keys);

        assert_eq!(state.field.terrain, TERRAIN_NONE);
        assert_eq!(state.sides[0].active.paradox_stat(), 0);
    }

    #[test]
    fn test_drizzle_deactivates_protosynthesis() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        // Side 0: Protosynthesis mon active in sun
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [150, 100, 120, 110, 130],
            ability_id: data_bridge::ABILITY_PROTOSYNTHESIS,
            ..Default::default()
        };
        state.sides[0].active.set_paradox(ATK as u8 + 1, false);
        // Side 1: Drizzle mon switching in
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.sides[1].team[1] = MonSlot {
            species_id: 60, current_hp: 200, max_hp: 200,
            stats: [100; 5],
            ability_id: data_bridge::ABILITY_DRIZZLE,
            ..Default::default()
        };
        state.field.weather = WEATHER_SUN;
        state.field.weather_turns = 5;
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 1, 1);

        // Drizzle replaced sun -> Protosynthesis deactivated
        assert_eq!(state.field.weather, WEATHER_RAIN);
        assert_eq!(state.sides[0].active.paradox_stat(), 0);
    }

    #[test]
    fn test_safeguard_blocks_toxic_spikes() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 400, max_hp: 400,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 26, current_hp: 400, max_hp: 400,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].side_conditions.toxic_spikes = 1;
        state.sides[0].side_conditions.set_safeguard_turns(5);
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        // Safeguard blocks the poison from toxic spikes
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
        // Toxic spikes remain on the field (not absorbed)
        assert_eq!(state.sides[0].side_conditions.toxic_spikes, 1);
    }

    #[test]
    fn test_mist_blocks_sticky_web() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 400, max_hp: 400,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 26, current_hp: 400, max_hp: 400,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].side_conditions.hazard_flags = HAZARD_STICKY_WEB;
        state.sides[0].side_conditions.set_mist_turns(5);
        state.zobrist = compute_full_hash(&state, &keys);

        perform_switch(&mut state, &keys, 0, 1);

        // Mist blocks the speed drop from sticky web
        assert_eq!(state.sides[0].active.boosts[SPE], 0);
    }
}
