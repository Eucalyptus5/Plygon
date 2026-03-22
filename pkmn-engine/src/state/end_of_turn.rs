//! End-of-turn processing: the 18-step sequence.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag};
use crate::state::accessors::*;
use crate::state::mutations::*;
use crate::state::zobrist::ZobristKeys;
use crate::state::forme;
use crate::state::calc_modifiers::terrain_blocks_status;

pub fn end_of_turn(state: &mut BattleState, keys: &ZobristKeys) {
    step_weather(state, keys);                                   // 1
    step_terrain_expiry(state, keys);                            // 2
    // 3: Future Sight (not yet implemented)
    step_wish(state, keys);                                      // 4
    for side in 0..2 {                                            // 5
        step_status_damage(state, keys, side);
        let slot = state.sides[side].active_index as usize;
        if !state.sides[side].team[slot].is_fainted() {
            crate::state::move_exec::check_berry_activation(state, keys, side, slot);
        }
    }
    step_leech_seed(state, keys);                                // 6
    for side in 0..2 { step_binding_damage(state, keys, side); } // 7
    for side in 0..2 { step_passive_healing(state, keys, side); }// 8
    step_grassy_terrain(state, keys);                            // 9
    for side in 0..2 { step_item_healing(state, keys, side); }   // 10
    for side in 0..2 { step_screen_expiry(state, side); }        // 11
    for side in 0..2 { step_tailwind_expiry(state, side); }      // 12
    for side in 0..2 { step_side_condition_expiry(state, side); }// 12b
    for side in 0..2 { step_salt_cure(state, keys, side); }     // 12c
    for side in 0..2 { step_yawn(state, keys, side); }          // 12d
    step_volatile_counters(state, keys);                         // 13
    for side in 0..2 { step_perish_song(state, keys, side); }    // 14
    step_soul_heart(state, keys);                               // 14b
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
    if state.field.weather == WEATHER_NONE { return; }
    let weather = effective_weather(state);

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
        if state.field.weather_turns == 0 {
            clear_weather(state, keys);
            crate::state::switch::check_paradox_deactivation(state);
        }
    }
}

fn step_terrain_expiry(state: &mut BattleState, keys: &ZobristKeys) {
    if state.field.terrain == TERRAIN_NONE { return; }
    if state.field.terrain_turns > 0 {
        state.field.terrain_turns -= 1;
        if state.field.terrain_turns == 0 {
            clear_terrain(state, keys);
            crate::state::switch::check_paradox_deactivation(state);
        }
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
    let is_poison_heal = effective_ability(state, side) == data_bridge::ABILITY_POISON_HEAL;
    match status {
        STATUS_BURN   => { deal_proportional_damage(state, keys, side, slot, 1, 16); }
        STATUS_POISON => { if !is_poison_heal { deal_proportional_damage(state, keys, side, slot, 1, 8); } }
        STATUS_BAD_POISON => { if !is_poison_heal {
            let counter = state.sides[side].active.toxic_counter.max(1);
            let max_hp = state.sides[side].team[slot].max_hp;
            let damage = (max_hp as u32 * counter as u32 / 16).max(1) as u16;
            deal_damage(state, keys, side, slot, damage);
            state.sides[side].active.toxic_counter = counter.saturating_add(1);
        } }
        _ => {}
    }
}

fn step_leech_seed(state: &mut BattleState, keys: &ZobristKeys) {
    for side in 0..2 {
        let slot = state.sides[side].active_index as usize;
        if !state.sides[side].active.has_volatile(VOL_LEECH_SEED) { continue; }
        if state.sides[side].team[slot].is_fainted() { continue; }
        if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { continue; }
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

    // Decrement counter; expire at 0
    let turns = state.sides[side].active.bind_turns();
    if turns == 0 {
        clear_volatile(state, keys, side, VOL_BOUND);
        state.sides[side].active.set_bind_turns(0);
        return;
    }
    state.sides[side].active.set_bind_turns(turns - 1);

    // Magic Guard blocks damage but counter still ticks
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { return; }

    let has_band = if state.field.magic_room_turns() == 0 {
        let opp_item_id = state.sides[1-side].team[state.sides[1-side].active_index as usize].item_id;
        data_bridge::item(opp_item_id).has(ItemFlag::BINDING_BOOST)
    } else { false };
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
    if state.field.magic_room_turns() > 0 { return; }
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
    // Sticky Barb: 1/8 self-damage each turn
    if state.sides[side].team[slot].item_id == data_bridge::ITEM_STICKY_BARB {
        deal_proportional_damage(state, keys, side, slot, 1, 8);
    }
    // Flame Orb: inflict burn at end of turn
    if itm.has(ItemFlag::FLAME_ORB) {
        set_status(state, keys, side, slot, STATUS_BURN, 0);
    }
    // Toxic Orb: inflict bad poison at end of turn
    if itm.has(ItemFlag::TOXIC_ORB) {
        set_status(state, keys, side, slot, STATUS_BAD_POISON, 0);
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

fn step_side_condition_expiry(state: &mut BattleState, side: usize) {
    let sc = &mut state.sides[side].side_conditions;
    let sg = sc.safeguard_turns();
    if sg > 0 { sc.set_safeguard_turns(sg - 1); }
    let mt = sc.mist_turns();
    if mt > 0 { sc.set_mist_turns(mt - 1); }
    let lc = sc.lucky_chant_turns();
    if lc > 0 { sc.set_lucky_chant_turns(lc - 1); }
}

fn step_salt_cure(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    // Salt cure is stored as bit 7 of stockpile field
    if state.sides[side].active.stockpile & 0x80 == 0 { return; }
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { return; }

    let (t1, t2) = effective_types(state, side);
    let is_water_steel = t1 == crate::data::types::Type::Water as u8
        || t2 == crate::data::types::Type::Water as u8
        || t1 == crate::data::types::Type::Steel as u8
        || t2 == crate::data::types::Type::Steel as u8;
    let (n, d) = if is_water_steel { (1, 4) } else { (1, 8) };
    deal_proportional_damage(state, keys, side, slot, n, d);
}

fn step_yawn(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    if !state.sides[side].active.has_volatile(VOL_YAWN) { return; }
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    // Yawn puts the target to sleep the turn after it's used
    clear_volatile(state, keys, side, VOL_YAWN);
    if state.sides[side].team[slot].status == STATUS_NONE {
        if state.sides[side].side_conditions.safeguard_turns() == 0
            && !crate::state::forme::is_minior_meteor_forme(state, side)
            && !terrain_blocks_status(state, side, STATUS_SLEEP)
        {
            set_status(state, keys, side, slot, STATUS_SLEEP, 2); // 1-3 turns
        }
    }
}

fn step_volatile_counters(state: &mut BattleState, keys: &ZobristKeys) {
    for side in 0..2 {
        let a = &mut state.sides[side].active;
        macro_rules! dec { ($f:ident) => { if a.$f > 0 { a.$f -= 1; } } }
        dec!(taunt_turns); dec!(encore_turns); dec!(disable_turns);
        dec!(magnet_rise_turns); dec!(telekinesis_turns); dec!(heal_block_turns);
        if a.encore_turns == 0 && a.encore_move != 0 { a.encore_move = 0; }
        if a.disable_turns == 0 && a.disabled_move != 0 { a.disabled_move = 0; }
        // Encore early termination: end if encored move has 0 PP (Showdown onResidual)
        if state.sides[side].active.encore_turns > 0 {
            let enc_move = state.sides[side].active.encore_move;
            if enc_move != 0 {
                let moves = effective_moves(state, side);
                let mut has_pp = false;
                for i in 0..4 {
                    if moves[i] == enc_move && effective_pp(state, side, i) > 0 {
                        has_pp = true;
                        break;
                    }
                }
                if !has_pp {
                    state.sides[side].active.encore_turns = 0;
                    state.sides[side].active.encore_move = 0;
                }
            }
        }
        if state.sides[side].active.magnet_rise_turns == 0 && state.sides[side].active.has_volatile(VOL_MAGNET_RISE) {
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
    if state.field.magic_room_turns() > 0 {
        let new_turns = state.field.magic_room_turns() - 1;
        set_magic_room(state, keys, new_turns);
    }
    if state.field.wonder_room_turns() > 0 {
        let new_turns = state.field.wonder_room_turns() - 1;
        set_wonder_room(state, keys, new_turns);
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

fn step_eot_abilities(
    state: &mut BattleState, keys: &ZobristKeys, side: usize,
) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    let ability = effective_ability(state, side);
    match ability {
        data_bridge::ABILITY_SPEED_BOOST => {
            if state.sides[side].active.turns_active > 0 {
                apply_boost(state, keys, side, SPE, 1);
            }
        }
        data_bridge::ABILITY_POISON_HEAL => {
            let status = state.sides[side].team[slot].status;
            if status == STATUS_POISON || status == STATUS_BAD_POISON {
                let m = state.sides[side].team[slot].max_hp;
                heal(state, keys, side, slot, m / 8);
            }
        }
        data_bridge::ABILITY_ZEN_MODE => {
            forme::check_zen_mode(state, keys, side);
        }
        data_bridge::ABILITY_SCHOOLING => {
            forme::check_schooling(state, keys, side);
        }
        data_bridge::ABILITY_SHIELDS_DOWN => {
            forme::check_shields_down(state, keys, side);
        }
        data_bridge::ABILITY_MOODY => {
            // +2 to a random stat below +6, -1 to a different stat above -6
            // Simplified: pick stats 0-4 (battle stats only)
            step_moody(state, keys, side);
        }
        data_bridge::ABILITY_BAD_DREAMS => {
            let opp = 1 - side;
            let opp_slot = state.sides[opp].active_index as usize;
            if !state.sides[opp].team[opp_slot].is_fainted()
                && state.sides[opp].team[opp_slot].status == STATUS_SLEEP
            {
                let m = state.sides[opp].team[opp_slot].max_hp;
                deal_damage(state, keys, opp, opp_slot, (m / 8).max(1));
            }
        }
        data_bridge::ABILITY_HYDRATION => {
            if matches!(effective_weather_for(state, side), WEATHER_RAIN | WEATHER_HEAVY_RAIN)
                && state.sides[side].team[slot].status != STATUS_NONE
            {
                clear_status(state, keys, side, slot);
            }
        }
        data_bridge::ABILITY_SHED_SKIN => {
            // 33% chance to cure status — use a simple deterministic approach for MCTS
            // We approximate by always curing (MCTS prefers speed over exact RNG)
            // Actually, we should use the rng. But step_eot_abilities doesn't take rng.
            // For now, skip RNG — cure unconditionally (slight inaccuracy, acceptable for MCTS speed).
            if state.sides[side].team[slot].status != STATUS_NONE {
                // Deterministic: cure if turns_active is divisible by 3 (approx 33%)
                if state.sides[side].active.turns_active % 3 == 0 {
                    clear_status(state, keys, side, slot);
                }
            }
        }
        data_bridge::ABILITY_SOLAR_POWER => {
            if matches!(effective_weather_for(state, side), WEATHER_SUN | WEATHER_HARSH_SUN) {
                let m = state.sides[side].team[slot].max_hp;
                deal_damage(state, keys, side, slot, (m / 8).max(1));
            }
        }
        data_bridge::ABILITY_DRY_SKIN => {
            match effective_weather_for(state, side) {
                WEATHER_SUN | WEATHER_HARSH_SUN => {
                    let m = state.sides[side].team[slot].max_hp;
                    deal_damage(state, keys, side, slot, (m / 8).max(1));
                }
                WEATHER_RAIN | WEATHER_HEAVY_RAIN => {
                    let m = state.sides[side].team[slot].max_hp;
                    heal(state, keys, side, slot, m / 8);
                }
                _ => {}
            }
        }
        data_bridge::ABILITY_RAIN_DISH => {
            if matches!(effective_weather_for(state, side), WEATHER_RAIN | WEATHER_HEAVY_RAIN) {
                let m = state.sides[side].team[slot].max_hp;
                heal(state, keys, side, slot, m / 16);
            }
        }
        data_bridge::ABILITY_ICE_BODY => {
            if effective_weather_for(state, side) == WEATHER_SNOW {
                let m = state.sides[side].team[slot].max_hp;
                heal(state, keys, side, slot, m / 16);
            }
        }
        data_bridge::ABILITY_HARVEST => {
            let berry_id = state.sides[side].last_consumed_berry();
            if berry_id != 0 && state.sides[side].team[slot].item_id == 0 {
                let in_sun = matches!(effective_weather_for(state, side), WEATHER_SUN | WEATHER_HARSH_SUN);
                if in_sun || (state.sides[side].active.turns_active % 2 == 0) {
                    set_item(state, keys, side, slot, berry_id);
                    state.sides[side].set_last_consumed_berry(0);
                }
            }
        }
        _ => {}
    }
}

/// Moody: +2 random stat, -1 different random stat.
/// Uses turns_active as a deterministic seed for MCTS (no rng parameter).
fn step_moody(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let t = state.sides[side].active.turns_active as usize;
    let boost_stat = t % 5;
    let drop_stat = (t + 1) % 5;
    apply_boost(state, keys, side, boost_stat, 2);
    apply_boost(state, keys, side, drop_stat, -1);
}

fn step_soul_heart(state: &mut BattleState, keys: &ZobristKeys) {
    for fainted_side in 0..2 {
        let fs = state.sides[fainted_side].active_index as usize;
        if !state.sides[fainted_side].team[fs].is_fainted() { continue; }
        let other = 1 - fainted_side;
        let os = state.sides[other].active_index as usize;
        if state.sides[other].team[os].is_fainted() { continue; }
        if effective_ability(state, other) == data_bridge::ABILITY_SOUL_HEART {
            apply_boost(state, keys, other, SPA, 1);
        }
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


    #[test]
    fn test_flame_orb_inflicts_burn() {
        let (mut s, k) = setup();
        s.sides[0].team[0].item_id = 145; // Flame Orb
        s.zobrist = compute_full_hash(&s, &k);

        step_item_healing(&mut s, &k, 0);

        assert_eq!(s.sides[0].team[0].status, STATUS_BURN);
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_flame_orb_no_overwrite_existing_status() {
        let (mut s, k) = setup();
        s.sides[0].team[0].item_id = 145;
        s.sides[0].team[0].status = STATUS_PARALYSIS;
        s.zobrist = compute_full_hash(&s, &k);

        step_item_healing(&mut s, &k, 0);

        // set_status returns false if already statused — paralysis stays
        assert_eq!(s.sides[0].team[0].status, STATUS_PARALYSIS);
    }

    #[test]
    fn test_toxic_orb_inflicts_bad_poison() {
        let (mut s, k) = setup();
        s.sides[0].team[0].item_id = 515; // Toxic Orb
        s.zobrist = compute_full_hash(&s, &k);

        step_item_healing(&mut s, &k, 0);

        assert_eq!(s.sides[0].team[0].status, STATUS_BAD_POISON);
        assert!(validate_hash(&s, &k));
    }


    #[test]
    fn test_zen_mode_eot_triggers() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 555, current_hp: 100, max_hp: 400,
            ability_id: data_bridge::ABILITY_ZEN_MODE,
            stats: [280, 110, 60, 110, 190],
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // HP 100/400 = 25% → should trigger Zen Mode
        step_eot_abilities(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 1171); // Darmanitan-Zen
        assert!(validate_hash(&state, &keys));
    }


    #[test]
    fn test_leech_seed_drains_and_heals() {
        let (mut s, k) = setup();
        // Damage side 1 so heal is observable
        s.sides[1].team[0].current_hp = 150;
        set_volatile(&mut s, &k, 0, VOL_LEECH_SEED);
        s.zobrist = compute_full_hash(&s, &k);

        step_leech_seed(&mut s, &k);

        // 200 / 8 = 25 drain
        assert_eq!(s.sides[0].team[0].current_hp, 175); // drained
        assert_eq!(s.sides[1].team[0].current_hp, 175); // healed
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_leech_seed_magic_guard_blocks() {
        let (mut s, k) = setup();
        s.sides[0].team[0].ability_id = data_bridge::ABILITY_MAGIC_GUARD;
        s.sides[1].team[0].current_hp = 150;
        set_volatile(&mut s, &k, 0, VOL_LEECH_SEED);
        s.zobrist = compute_full_hash(&s, &k);

        step_leech_seed(&mut s, &k);

        assert_eq!(s.sides[0].team[0].current_hp, 200); // no drain
        assert_eq!(s.sides[1].team[0].current_hp, 150); // no heal
        assert!(validate_hash(&s, &k));
    }


    #[test]
    fn test_binding_damage_with_counter() {
        let (mut s, k) = setup();
        set_volatile(&mut s, &k, 0, VOL_BOUND);
        s.sides[0].active.set_bind_turns(3);
        s.zobrist = compute_full_hash(&s, &k);

        step_binding_damage(&mut s, &k, 0);

        assert_eq!(s.sides[0].active.bind_turns(), 2);
        assert_eq!(s.sides[0].team[0].current_hp, 175); // 200/8 = 25
        assert!(s.sides[0].active.has_volatile(VOL_BOUND));
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_binding_expiry_at_zero() {
        let (mut s, k) = setup();
        set_volatile(&mut s, &k, 0, VOL_BOUND);
        s.sides[0].active.set_bind_turns(0);
        s.zobrist = compute_full_hash(&s, &k);

        step_binding_damage(&mut s, &k, 0);

        assert!(!s.sides[0].active.has_volatile(VOL_BOUND));
        assert_eq!(s.sides[0].team[0].current_hp, 200); // no damage on expiry
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_binding_damage_magic_guard() {
        let (mut s, k) = setup();
        s.sides[0].team[0].ability_id = data_bridge::ABILITY_MAGIC_GUARD;
        set_volatile(&mut s, &k, 0, VOL_BOUND);
        s.sides[0].active.set_bind_turns(3);
        s.zobrist = compute_full_hash(&s, &k);

        step_binding_damage(&mut s, &k, 0);

        assert_eq!(s.sides[0].active.bind_turns(), 2); // counter still ticks
        assert_eq!(s.sides[0].team[0].current_hp, 200); // no damage
        assert!(s.sides[0].active.has_volatile(VOL_BOUND));
        assert!(validate_hash(&s, &k));
    }


    #[test]
    fn test_perish_song_countdown() {
        let (mut s, k) = setup();
        set_volatile(&mut s, &k, 0, VOL_PERISH_SONG);
        s.sides[0].active.perish_count = 3;
        s.zobrist = compute_full_hash(&s, &k);

        step_perish_song(&mut s, &k, 0);

        assert_eq!(s.sides[0].active.perish_count, 2);
        assert_eq!(s.sides[0].team[0].current_hp, 200); // no damage yet
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_perish_song_faint_at_zero() {
        let (mut s, k) = setup();
        set_volatile(&mut s, &k, 0, VOL_PERISH_SONG);
        s.sides[0].active.perish_count = 0;
        s.zobrist = compute_full_hash(&s, &k);

        step_perish_song(&mut s, &k, 0);

        assert_eq!(s.sides[0].team[0].current_hp, 0); // fainted
        assert!(!s.sides[0].active.has_volatile(VOL_PERISH_SONG)); // cleared
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_terrain_expires() {
        let (mut s, k) = setup();
        s.field.terrain = TERRAIN_ELECTRIC;
        s.field.terrain_turns = 1;
        s.zobrist = compute_full_hash(&s, &k);

        step_terrain_expiry(&mut s, &k);

        assert_eq!(s.field.terrain, TERRAIN_NONE);
        assert_eq!(s.field.terrain_turns, 0);
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_terrain_countdown_not_expired() {
        let (mut s, k) = setup();
        s.field.terrain = TERRAIN_PSYCHIC;
        s.field.terrain_turns = 3;
        s.zobrist = compute_full_hash(&s, &k);

        step_terrain_expiry(&mut s, &k);

        assert_eq!(s.field.terrain, TERRAIN_PSYCHIC);
        assert_eq!(s.field.terrain_turns, 2);
    }

    #[test]
    fn test_grassy_terrain_healing() {
        let (mut s, k) = setup();
        s.field.terrain = TERRAIN_GRASSY;
        s.field.terrain_turns = 5;
        // Reduce HP so healing is visible
        s.sides[0].team[0].current_hp = 100;
        s.sides[1].team[0].current_hp = 100;
        s.zobrist = compute_full_hash(&s, &k);

        step_grassy_terrain(&mut s, &k);

        // Both grounded mons heal 1/16 max HP = 200/16 = 12
        assert_eq!(s.sides[0].team[0].current_hp, 112);
        assert_eq!(s.sides[1].team[0].current_hp, 112);
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_grassy_terrain_no_heal_flying() {
        let (mut s, k) = setup();
        s.field.terrain = TERRAIN_GRASSY;
        s.field.terrain_turns = 5;
        s.sides[0].team[0].current_hp = 100;
        // Make side 0 Flying-type (not grounded)
        s.sides[0].active.override_types = [Type::Flying as u8, Type::Flying as u8];
        s.sides[0].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;
        s.zobrist = compute_full_hash(&s, &k);

        step_grassy_terrain(&mut s, &k);

        // Flying-type (not grounded) doesn't heal
        assert_eq!(s.sides[0].team[0].current_hp, 100);
    }

    #[test]
    fn test_gravity_expires() {
        let (mut s, k) = setup();
        set_gravity(&mut s, &k, 1);
        assert!(s.field.gravity_turns > 0);
        step_volatile_counters(&mut s, &k);
        assert_eq!(s.field.gravity_turns, 0);
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_magic_room_expires() {
        let (mut s, k) = setup();
        set_magic_room(&mut s, &k, 1);
        assert!(s.field.magic_room_turns() > 0);
        step_volatile_counters(&mut s, &k);
        assert_eq!(s.field.magic_room_turns(), 0);
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_wonder_room_expires() {
        let (mut s, k) = setup();
        set_wonder_room(&mut s, &k, 1);
        assert!(s.field.wonder_room_turns() > 0);
        step_volatile_counters(&mut s, &k);
        assert_eq!(s.field.wonder_room_turns(), 0);
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_magic_room_suppresses_item_healing() {
        let (mut s, k) = setup();
        s.sides[0].team[0].item_id = 145; // Flame Orb
        set_magic_room(&mut s, &k, 5);
        s.zobrist = compute_full_hash(&s, &k);
        step_item_healing(&mut s, &k, 0);
        // Magic Room suppresses Flame Orb — no burn
        assert_eq!(s.sides[0].team[0].status, STATUS_NONE);
        assert!(validate_hash(&s, &k));
    }

    #[test]
    fn test_field_flags_packing() {
        let mut f = FieldState::default();
        f.set_magic_room_turns(5);
        assert_eq!(f.magic_room_turns(), 5);
        assert_eq!(f.wonder_room_turns(), 0);

        f.set_wonder_room_turns(3);
        assert_eq!(f.magic_room_turns(), 5);
        assert_eq!(f.wonder_room_turns(), 3);

        // Weather suppressed flag doesn't interfere
        f.field_flags |= FIELD_WEATHER_SUPPRESSED;
        assert_eq!(f.magic_room_turns(), 5);
        assert_eq!(f.wonder_room_turns(), 3);
        assert!(f.field_flags & FIELD_WEATHER_SUPPRESSED != 0);
    }

    #[test]
    fn test_side_conditions_expire_after_eot() {
        let (mut s, _k) = setup();
        s.sides[0].side_conditions.set_safeguard_turns(1);
        s.sides[0].side_conditions.set_mist_turns(1);
        s.sides[0].side_conditions.set_lucky_chant_turns(1);

        step_side_condition_expiry(&mut s, 0);

        assert_eq!(s.sides[0].side_conditions.safeguard_turns(), 0);
        assert_eq!(s.sides[0].side_conditions.mist_turns(), 0);
        assert_eq!(s.sides[0].side_conditions.lucky_chant_turns(), 0);
    }

    #[test]
    fn test_safeguard_does_not_block_flame_orb() {
        let (mut s, k) = setup();
        s.sides[0].team[0].item_id = 145; // Flame Orb
        s.sides[0].side_conditions.set_safeguard_turns(5);
        s.zobrist = compute_full_hash(&s, &k);

        step_item_healing(&mut s, &k, 0);

        // Flame Orb is self-inflicted — Safeguard does not block
        assert_eq!(s.sides[0].team[0].status, STATUS_BURN);
        assert!(validate_hash(&s, &k));
    }
}
