//! End-of-turn processing: the 18-step sequence.

use crate::state::structs::*;
use crate::state::battle_rng::BattleRng;
use crate::state::data_bridge::{self, ItemFlag};
use crate::state::accessors::*;
use crate::state::mutations::*;
use crate::state::forme;
use crate::state::calc_modifiers::{terrain_blocks_status, is_heatproof_effective, chain_mod};

pub fn end_of_turn(state: &mut BattleState, teams: &TeamData, rng: &mut BattleRng) {
    step_weather(state);                                   // 1
    step_terrain_countdown(state);                               // 2a: decrement only
    for side in 0..2 { step_future_move(state, side, rng); }     // 3: Future Sight / Doom Desire
    step_wish(state);                                      // 4
    for side in 0..2 { step_hydration(state, side); }      // 5a (order 5, sub 3)
    for side in 0..2 { step_item_healing(state, side); }   // 5b (order 5, sub 4)
    step_grassy_terrain(state);                            // 5c (terrain heal)
    step_terrain_expiry(state);                            // 2b: clear if counter==0
    for side in 0..2 { step_passive_healing(state, side); }// 6 (Aqua Ring, Ingrain)
    // 6b: Weather-residual abilities (Dry Skin, Rain Dish, Ice Body, Solar Power).
    // Showdown fires these during weather residuals, BEFORE status damage.
    for side in 0..2 { step_weather_abilities(state, side); }
    // 7-8: Berry activation (status-cure berries must fire BEFORE status damage)
    for side in 0..2 {
        let slot = state.sides[side].active_index as usize;
        if !state.sides[side].team[slot].is_fainted() {
            crate::state::move_exec::check_berry_activation(state, teams, side, slot, &mut |n| rng.next(n));
        }
    }
    for side in 0..2 {                                            // 9-10 status damage
        step_status_damage(state, side);
        let slot = state.sides[side].active_index as usize;
        if !state.sides[side].team[slot].is_fainted() {
            crate::state::move_exec::check_berry_activation(state, teams, side, slot, &mut |n| rng.next(n));
        }
    }
    step_leech_seed(state);                                // 11
    for side in 0..2 { step_curse(state, side); }          // 12 (Curse residual, onResidualOrder 12)
    for side in 0..2 { step_binding_damage(state, side); } // 13
    for side in 0..2 { step_screen_expiry(state, side); }        // 11
    for side in 0..2 { step_tailwind_expiry(state, side); }      // 12
    for side in 0..2 { step_side_condition_expiry(state, side); }// 12b
    for side in 0..2 { step_salt_cure(state, side); }     // 12c
    for side in 0..2 { step_yawn(state, side); }          // 12d
    step_volatile_counters(state);                         // 13
    for side in 0..2 { step_perish_song(state, side); }    // 14
    for side in 0..2 { step_status_orbs(state, side); }   // 14c (order 28: orbs/barb)
    step_soul_heart(state);                               // 14b
    for side in 0..2 { step_eot_abilities(state, teams, side, rng); }  // 15
    for side in 0..2 {                                           // 16
        state.sides[side].active.turns_active = state.sides[side].active.turns_active.saturating_add(1);
    }
    for side in 0..2 {                                           // 17
        // Reset protect_consecutive if neither Protect nor Endure was used this turn
        // (Showdown's `stall` volatile is shared by the Protect/Detect/Endure ladder)
        if !state.sides[side].active.has_volatile(VOL_PROTECT_THIS_TURN)
            && !state.sides[side].active.has_volatile(VOL_ENDURE)
        {
            state.sides[side].active.protect_consecutive = 0;
        }
        state.sides[side].active.volatile_flags &= !VOL_PER_TURN_MASK;
        state.sides[side].active.times_hit = 0;
        state.sides[side].active.damage_taken_this_turn = 0;
        // Mirror Showdown's nextTurn reset of statsLoweredThisTurn (sim/battle.ts:1665).
        state.sides[side].clear_stats_lowered_this_turn();
        state.sides[side].clear_stats_raised_this_turn();
        // Mirror nextTurn's moveLastTurnResult = moveThisTurnResult (sim/battle.ts:1660):
        // promote this turn's move-failure bit into last-turn for Stomping Tantrum / Temper Flare.
        state.sides[side].promote_move_failed();
    }
    state.field.turn += 1;                                       // 18
}

fn step_weather(state: &mut BattleState) {
    if state.field.weather == WEATHER_NONE { return; }

    // Decrement weather counter FIRST (Showdown: damage only on non-expiry turns)
    if state.field.weather_turns > 0 && state.field.weather_turns != 255 {
        state.field.weather_turns -= 1;
        if state.field.weather_turns == 0 {
            clear_weather(state);
            crate::state::switch::check_paradox_deactivation(state);
            return; // Weather expired this turn — no damage
        }
    }

    let weather = effective_weather(state);

    if weather == WEATHER_SAND {
        for side in 0..2 {
            let slot = state.sides[side].active_index as usize;
            if state.sides[side].team[slot].is_fainted() { continue; }
            if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { continue; }
            let ab = effective_ability(state, side);
            if ab == data_bridge::ABILITY_OVERCOAT
                || ab == data_bridge::ABILITY_SAND_VEIL
                || ab == data_bridge::ABILITY_SAND_RUSH
                || ab == data_bridge::ABILITY_SAND_FORCE
            { continue; }
            // Safety Goggles blocks sandstorm residual (suppressed by Magic Room).
            if state.field.magic_room_turns() == 0
                && data_bridge::item(state.active_mon(side).item_id).has(ItemFlag::SAFETY_GOGGLES)
            { continue; }
            let (t1, t2) = battle_types(state, side);
            let immune = [Type::Rock as u8, Type::Ground as u8, Type::Steel as u8];
            if !immune.contains(&t1) && !immune.contains(&t2) {
                deal_proportional_damage(state, side, slot, 1, 16);
            }
        }
    }
}

/// Decrement terrain counter (does NOT clear terrain yet).
fn step_terrain_countdown(state: &mut BattleState) {
    if state.field.terrain == TERRAIN_NONE { return; }
    if state.field.terrain_turns > 0 {
        state.field.terrain_turns -= 1;
    }
}

/// Clear terrain if counter reached 0. Called after grassy terrain heal.
fn step_terrain_expiry(state: &mut BattleState) {
    if state.field.terrain == TERRAIN_NONE { return; }
    if state.field.terrain_turns == 0 {
        clear_terrain(state);
        crate::state::switch::check_paradox_deactivation(state);
    }
}

/// Step 3 (Showdown futuremove `onResidualOrder: 3`): resolve a pending Future
/// Sight / Doom Desire. The `future_move() == 0` fast-exit keeps the common
/// no-pending path (every bench turn) to a single byte read. On the resolution
/// tick, deal special damage to the CURRENT slot occupant from the use-time
/// snapshot (the user may have switched out or fainted), then clear the record.
fn step_future_move(state: &mut BattleState, side: usize, rng: &mut BattleRng) {
    let which = state.sides[side].side_conditions.future_move();
    if which == 0 { return; }
    let countdown = state.sides[side].side_conditions.future_countdown();
    if countdown > 1 {
        state.sides[side].side_conditions.set_future_countdown(countdown - 1);
        return;
    }
    // Resolution tick (countdown reaches 1): snapshot offense, then clear the
    // record regardless of whether the hit lands.
    let level = state.sides[side].side_conditions.fut_level;
    let spa = state.sides[side].side_conditions.fut_spa as u32;
    let stab_kind = state.sides[side].side_conditions.future_stab_kind();
    state.sides[side].side_conditions.clear_future_move();

    let slot = state.sides[side].active_index as usize;
    // Showdown onEnd skips a fainted occupant (the record is still consumed).
    // Singles: the target slot can never hold the user, so no self-target check.
    if state.sides[side].team[slot].is_fainted() { return; }

    let (power, move_type): (u32, Type) =
        if which == 1 { (120, Type::Psychic) } else { (140, Type::Steel) };
    let (dt1, dt2) = battle_types(state, side);
    let def_type1 = unsafe { core::mem::transmute::<u8, Type>(dt1) };
    let def_type2 = unsafe { core::mem::transmute::<u8, Type>(dt2) };
    let eff = dual_type_effectiveness(move_type, def_type1, def_type2);
    if eff == 0 { return; } // type-immune (Future Sight Psychic vs Dark)

    let d = boosted_stat(
        effective_stat(state, side, SPD),
        state.sides[side].active.boosts[SPD],
    ).max(1) as u32;
    let lf = 2 * level as u32 / 5 + 2;
    let mut dmg = (lf * power * spa / d) / 50 + 2;
    let roll = 85 + rng.next(16);
    dmg = dmg * roll / 100;
    let sn: u32 = match stab_kind { 1 => 6144, 2 => 8192, 3 => 9216, _ => 4096 };
    dmg = chain_mod(dmg, sn);
    dmg = dmg * eff as u32 / 4;
    if dmg == 0 { dmg = 1; }
    let dmg = dmg.min(u16::MAX as u32) as u16;
    deal_damage(state, side, slot, dmg);
}

fn step_wish(state: &mut BattleState) {
    for side in 0..2 {
        let sc = &state.sides[side].side_conditions;
        if sc.wish_turns == 1 {
            let slot = state.sides[side].active_index as usize;
            let wish_hp = sc.wish_hp;
            // Showdown wish onEnd gates on !target.fainted; a KO'd mon stays fainted.
            if !state.sides[side].team[slot].is_fainted() {
                heal(state, side, slot, wish_hp);
            }
            state.sides[side].side_conditions.wish_hp = 0;
            state.sides[side].side_conditions.wish_turns = 0;
        } else if state.sides[side].side_conditions.wish_turns > 1 {
            state.sides[side].side_conditions.wish_turns -= 1;
        }
    }
}

fn step_hydration(state: &mut BattleState, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    if effective_ability(state, side) == data_bridge::ABILITY_HYDRATION
        && matches!(effective_weather_for(state, side), WEATHER_RAIN | WEATHER_HEAVY_RAIN)
        && state.sides[side].team[slot].status != STATUS_NONE
    {
        clear_status(state, side, slot);
    }
}

fn step_status_damage(state: &mut BattleState, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { return; }

    let status = state.sides[side].team[slot].status;
    let is_poison_heal = effective_ability(state, side) == data_bridge::ABILITY_POISON_HEAL;
    match status {
        STATUS_BURN   => {
            // Heatproof halves burn residual damage (Showdown: onDamage id==='brn' -> damage/2).
            let den = if is_heatproof_effective(state, side) { 32 } else { 16 };
            deal_proportional_damage(state, side, slot, 1, den);
        }
        STATUS_POISON => { if !is_poison_heal { deal_proportional_damage(state, side, slot, 1, 8); } }
        STATUS_BAD_POISON => { if !is_poison_heal {
            let counter = state.sides[side].active.toxic_counter.max(1);
            let max_hp = state.sides[side].team[slot].max_hp;
            let damage = ((max_hp as u32 / 16).max(1) * counter as u32) as u16;
            deal_damage(state, side, slot, damage);
            state.sides[side].active.toxic_counter = counter.saturating_add(1);
        } }
        _ => {}
    }
}

fn step_leech_seed(state: &mut BattleState) {
    for side in 0..2 {
        let slot = state.sides[side].active_index as usize;
        if !state.sides[side].active.has_volatile(VOL_LEECH_SEED) { continue; }
        if state.sides[side].team[slot].is_fainted() { continue; }
        if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { continue; }
        let drain = (state.sides[side].team[slot].max_hp / 8).max(1);
        // Showdown's leechseed onResidual (moves.ts:10246-10249) heals the source
        // by `this.damage()`'s return value — the ACTUAL HP removed, clamped to the
        // seeded mon's remaining HP — not the nominal max/8. A seeded mon dying to
        // the drain itself transfers only what it had left.
        let before = state.sides[side].team[slot].current_hp;
        deal_damage(state, side, slot, drain);
        let dealt = before - state.sides[side].team[slot].current_hp;
        let opp = 1 - side;
        let opp_slot = state.sides[opp].active_index as usize;
        if dealt > 0 && !state.sides[opp].team[opp_slot].is_fainted() {
            heal(state, opp, opp_slot, dealt);
        }
    }
}

/// Step (Showdown curse volatile `onResidualOrder: 12`): a Ghost-cursed active loses
/// ¼ of its max HP each end-of-turn (can KO). The bit-gated fast-exit keeps the common
/// uncursed path to a single byte read; Magic Guard blocks the indirect residual.
fn step_curse(state: &mut BattleState, side: usize) {
    if !state.sides[side].is_cursed() { return; }
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { return; }
    deal_proportional_damage(state, side, slot, 1, 4);
}

fn step_binding_damage(state: &mut BattleState, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if !state.sides[side].active.has_volatile(VOL_BOUND) { return; }
    if state.sides[side].team[slot].is_fainted() { return; }

    // Showdown's partiallytrapped onResidual (conditions.ts:234-242) ends the trap
    // silently (no damage) when the source has left: !source.isActive || hp<=0.
    // Singles invariant: the binder is the opposing active (same assumption as
    // switch.rs's voluntary-switch release). A faint of the trapper is the only
    // case not already handled by switch_out, since the faint replacement runs
    // after EOT — release the bind here so no orphan tick lands.
    let opp = 1 - side;
    let opp_slot = state.sides[opp].active_index as usize;
    if state.sides[opp].team[opp_slot].is_fainted() {
        clear_volatile(state, side, VOL_BOUND);
        state.sides[side].active.set_bind_turns(0);
        return;
    }

    // Decrement counter; expire at 0
    let turns = state.sides[side].active.bind_turns();
    if turns == 0 {
        clear_volatile(state, side, VOL_BOUND);
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
    deal_proportional_damage(state, side, slot, n, d);
}

fn step_passive_healing(state: &mut BattleState, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    let flags = state.sides[side].active.volatile_flags;
    if flags & VOL_AQUA_RING != 0 { let mhp = state.sides[side].team[slot].max_hp; heal(state, side, slot, mhp / 16); }
    if flags & VOL_INGRAIN != 0 { let mhp = state.sides[side].team[slot].max_hp; heal(state, side, slot, mhp / 16); }
}

fn step_grassy_terrain(state: &mut BattleState) {
    if state.field.terrain != TERRAIN_GRASSY { return; }
    for side in 0..2 {
        let slot = state.sides[side].active_index as usize;
        if state.sides[side].team[slot].is_fainted() { continue; }
        if !is_grounded(state, side) { continue; }
        let mhp = state.sides[side].team[slot].max_hp;
        heal(state, side, slot, mhp / 16);
    }
}

fn step_item_healing(state: &mut BattleState, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    if state.field.magic_room_turns() > 0 { return; }
    let itm = data_bridge::item(state.sides[side].team[slot].item_id);
    if itm.has(ItemFlag::LEFTOVERS) {
        let m = state.sides[side].team[slot].max_hp;
        heal(state, side, slot, m / 16);
    }
    if itm.has(ItemFlag::BLACK_SLUDGE) {
        let (t1, t2) = battle_types(state, side);
        if t1 == Type::Poison as u8 || t2 == Type::Poison as u8 {
            let m = state.sides[side].team[slot].max_hp;
            heal(state, side, slot, m / 16);
        } else {
            deal_proportional_damage(state, side, slot, 1, 8);
        }
    }
}

fn step_status_orbs(state: &mut BattleState, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    if state.field.magic_room_turns() > 0 { return; }
    let itm = data_bridge::item(state.sides[side].team[slot].item_id);
    // Sticky Barb: 1/8 self-damage each turn
    if state.sides[side].team[slot].item_id == data_bridge::ITEM_STICKY_BARB {
        deal_proportional_damage(state, side, slot, 1, 8);
    }
    // Flame Orb: inflict burn at end of turn (Fire types immune)
    if itm.has(ItemFlag::FLAME_ORB) && !type_immune_to_status(state, side, STATUS_BURN) {
        set_status(state, side, slot, STATUS_BURN, 0);
    }
    // Toxic Orb: inflict bad poison at end of turn (Poison/Steel types immune)
    if itm.has(ItemFlag::TOXIC_ORB) && !type_immune_to_status(state, side, STATUS_BAD_POISON) {
        set_status(state, side, slot, STATUS_BAD_POISON, 0);
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

fn step_salt_cure(state: &mut BattleState, side: usize) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    // Salt cure is stored as bit 7 of stockpile field
    if state.sides[side].active.stockpile & 0x80 == 0 { return; }
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { return; }

    let (t1, t2) = battle_types(state, side);
    let is_water_steel = t1 == crate::data::types::Type::Water as u8
        || t2 == crate::data::types::Type::Water as u8
        || t1 == crate::data::types::Type::Steel as u8
        || t2 == crate::data::types::Type::Steel as u8;
    let (n, d) = if is_water_steel { (1, 4) } else { (1, 8) };
    deal_proportional_damage(state, side, slot, n, d);
}

fn step_yawn(state: &mut BattleState, side: usize) {
    if !state.sides[side].active.has_volatile(VOL_YAWN) { return; }
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    // Yawn has a 2-turn delay: set on hit, first EOT decrements, second EOT applies sleep.
    // Bit 7 of _padding[4] = yawn first tick (1 = needs one more turn before sleep).
    if state.sides[side].active._padding[4] & 0x80 != 0 {
        // First tick: decrement the marker, don't apply sleep yet
        state.sides[side].active._padding[4] &= !0x80;
        return;
    }
    // Second tick: apply sleep and clear volatile
    clear_volatile(state, side, VOL_YAWN);
    if state.sides[side].team[slot].status == STATUS_NONE {
        if state.sides[side].side_conditions.safeguard_turns() == 0
            && !crate::state::forme::is_minior_meteor_forme(state, side)
            && !terrain_blocks_status(state, side, STATUS_SLEEP)
            && !ability_blocks_status(effective_ability(state, side), STATUS_SLEEP)
        {
            set_status(state, side, slot, STATUS_SLEEP, 3); // 1-3 turns (MCTS: use median)
        }
    }
}

fn step_volatile_counters(state: &mut BattleState) {
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
            clear_volatile(state, side, VOL_MAGNET_RISE);
        }
    }
    if state.field.trick_room_turns > 0 {
        state.field.trick_room_turns -= 1;
    }
    if state.field.gravity_turns > 0 {
        state.field.gravity_turns -= 1;
    }
    if state.field.magic_room_turns() > 0 {
        let new_turns = state.field.magic_room_turns() - 1;
        set_magic_room(state, new_turns);
    }
    if state.field.wonder_room_turns() > 0 {
        let new_turns = state.field.wonder_room_turns() - 1;
        set_wonder_room(state, new_turns);
    }
}

fn step_perish_song(state: &mut BattleState, side: usize) {
    if !state.sides[side].active.has_volatile(VOL_PERISH_SONG) { return; }
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].active.perish_count == 0 {
        let hp = state.sides[side].team[slot].current_hp;
        if hp > 0 { deal_damage(state, side, slot, hp); }
        clear_volatile(state, side, VOL_PERISH_SONG);
    } else if state.sides[side].active.perish_count != 255 {
        state.sides[side].active.perish_count -= 1;
    }
}

/// Weather-residual abilities (Dry Skin, Rain Dish, Ice Body, Solar Power).
/// Showdown fires these during the weather-residual phase, BEFORE status damage.
/// Split out from `step_eot_abilities` so they can run at the correct EOT index.
#[inline]
fn step_weather_abilities(
    state: &mut BattleState, side: usize,
) {
    // All abilities in this step require active weather; fast-exit when weather is none.
    if state.field.weather == WEATHER_NONE { return; }
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    let ability = effective_ability(state, side);
    match ability {
        data_bridge::ABILITY_SOLAR_POWER => {
            if matches!(effective_weather_for(state, side), WEATHER_SUN | WEATHER_HARSH_SUN) {
                let m = state.sides[side].team[slot].max_hp;
                deal_damage(state, side, slot, (m / 8).max(1));
            }
        }
        data_bridge::ABILITY_DRY_SKIN => {
            match effective_weather_for(state, side) {
                WEATHER_SUN | WEATHER_HARSH_SUN => {
                    let m = state.sides[side].team[slot].max_hp;
                    deal_damage(state, side, slot, (m / 8).max(1));
                }
                WEATHER_RAIN | WEATHER_HEAVY_RAIN => {
                    let m = state.sides[side].team[slot].max_hp;
                    heal(state, side, slot, m / 8);
                }
                _ => {}
            }
        }
        data_bridge::ABILITY_RAIN_DISH => {
            if matches!(effective_weather_for(state, side), WEATHER_RAIN | WEATHER_HEAVY_RAIN) {
                let m = state.sides[side].team[slot].max_hp;
                heal(state, side, slot, m / 16);
            }
        }
        data_bridge::ABILITY_ICE_BODY => {
            if effective_weather_for(state, side) == WEATHER_SNOW {
                let m = state.sides[side].team[slot].max_hp;
                heal(state, side, slot, m / 16);
            }
        }
        _ => {}
    }
}

fn step_eot_abilities(
    state: &mut BattleState, teams: &TeamData, side: usize, rng: &mut BattleRng,
) {
    let slot = state.sides[side].active_index as usize;
    if state.sides[side].team[slot].is_fainted() { return; }
    let ability = effective_ability(state, side);
    match ability {
        data_bridge::ABILITY_SPEED_BOOST => {
            // Showdown gates on pokemon.activeTurns: a mid-battle switch-in has
            // activeTurns==0 at that EOT and gets no boost. turns_active==0 on
            // both the turn-1 lead and a mid-battle switch-in; field.turn is
            // still 0 only during the turn-1 EOT (incremented later), so this
            // disjunct keeps the lead boost while skipping the switch-in case.
            if state.sides[side].active.turns_active >= 1 || state.field.turn == 0 {
                apply_boost(state, side, SPE, 1);
            }
        }
        data_bridge::ABILITY_POISON_HEAL => {
            let status = state.sides[side].team[slot].status;
            if status == STATUS_POISON || status == STATUS_BAD_POISON {
                let m = state.sides[side].team[slot].max_hp;
                heal(state, side, slot, m / 8);
            }
        }
        data_bridge::ABILITY_ZEN_MODE => {
            forme::check_zen_mode(state, teams, side);
        }
        data_bridge::ABILITY_SCHOOLING => {
            forme::check_schooling(state, teams, side);
        }
        data_bridge::ABILITY_SHIELDS_DOWN => {
            forme::check_shields_down(state, teams, side);
        }
        data_bridge::ABILITY_MOODY => {
            step_moody(state, side, rng);
        }
        data_bridge::ABILITY_BAD_DREAMS => {
            let opp = 1 - side;
            let opp_slot = state.sides[opp].active_index as usize;
            if !state.sides[opp].team[opp_slot].is_fainted()
                && state.sides[opp].team[opp_slot].status == STATUS_SLEEP
            {
                let m = state.sides[opp].team[opp_slot].max_hp;
                deal_damage(state, opp, opp_slot, (m / 8).max(1));
            }
        }
        data_bridge::ABILITY_HYDRATION => {
            // Handled earlier in step_hydration (before status damage)
        }
        data_bridge::ABILITY_SHED_SKIN => {
            // Showdown: randomChance(33, 100) = random(100) < 33
            if state.sides[side].team[slot].status != STATUS_NONE && rng.next(100) < 33 {
                clear_status(state, side, slot);
            }
        }
        data_bridge::ABILITY_HARVEST => {
            let berry_id = state.sides[side].last_consumed_berry();
            if berry_id != 0 && state.sides[side].team[slot].item_id == 0 {
                let in_sun = matches!(effective_weather_for(state, side), WEATHER_SUN | WEATHER_HARSH_SUN);
                if in_sun || (state.sides[side].active.turns_active % 2 == 0) {
                    set_item(state, side, slot, berry_id);
                    state.sides[side].set_last_consumed_berry(0);
                }
            }
        }
        _ => {}
    }
}

/// Moody: +2 to a random stat below +6, -1 to a different random stat above -6.
/// The lower list is built before the raise lands and excludes the just-raised
/// stat, matching the sample order in Showdown's moody.onResidual. Accuracy and
/// evasion are excluded; only the five battle stats are eligible.
fn step_moody(state: &mut BattleState, side: usize, rng: &mut BattleRng) {
    let boosts = state.sides[side].active.boosts;

    let mut raise_list = [0usize; 5];
    let mut raise_len = 0;
    for stat in 0..5 {
        if (boosts[stat] as i8) < 6 {
            raise_list[raise_len] = stat;
            raise_len += 1;
        }
    }
    let raised = if raise_len > 0 {
        Some(raise_list[rng.next(raise_len as u32) as usize])
    } else {
        None
    };

    let mut drop_list = [0usize; 5];
    let mut drop_len = 0;
    for stat in 0..5 {
        if (boosts[stat] as i8) > -6 && Some(stat) != raised {
            drop_list[drop_len] = stat;
            drop_len += 1;
        }
    }
    let dropped = if drop_len > 0 {
        Some(drop_list[rng.next(drop_len as u32) as usize])
    } else {
        None
    };

    if let Some(stat) = raised {
        apply_boost(state, side, stat, 2);
    }
    if let Some(stat) = dropped {
        apply_boost(state, side, stat, -1);
    }
}

fn step_soul_heart(state: &mut BattleState) {
    for fainted_side in 0..2 {
        let fs = state.sides[fainted_side].active_index as usize;
        if !state.sides[fainted_side].team[fs].is_fainted() { continue; }
        let other = 1 - fainted_side;
        let os = state.sides[other].active_index as usize;
        if state.sides[other].team[os].is_fainted() { continue; }
        if effective_ability(state, other) == data_bridge::ABILITY_SOUL_HEART {
            apply_boost(state, other, SPA, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> BattleState {
        let mut state = BattleState::default();
        for side in 0..2 { state.sides[side].team[0].species_id = (side as u16 +1)*25; state.sides[side].team[0].current_hp = 200; state.sides[side].team[0].max_hp = 200; }
        state
    }
    #[test] fn test_burn() { let mut s = setup(); s.sides[0].team[0].status = STATUS_BURN; step_status_damage(&mut s, 0); assert_eq!(s.sides[0].team[0].current_hp, 188); }
    #[test] fn test_turn_inc() { let mut s = setup(); end_of_turn(&mut s, &TeamData::default(), &mut BattleRng::from_closure(&mut |_| 0u32)); assert_eq!(s.field.turn, 1); }

    #[test]
    fn test_shed_skin_cures_on_successful_roll() {
        let mut s = setup();
        s.sides[0].team[0].ability_id = data_bridge::ABILITY_SHED_SKIN;
        s.sides[0].team[0].status = STATUS_BURN;
        step_eot_abilities(&mut s, &TeamData::default(), 0, &mut BattleRng::from_closure(&mut |_| 0u32));
        assert_eq!(s.sides[0].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_shed_skin_no_cure_on_failed_roll() {
        let mut s = setup();
        s.sides[0].team[0].ability_id = data_bridge::ABILITY_SHED_SKIN;
        s.sides[0].team[0].status = STATUS_BURN;
        step_eot_abilities(&mut s, &TeamData::default(), 0, &mut BattleRng::from_closure(&mut |_| 50u32));
        assert_eq!(s.sides[0].team[0].status, STATUS_BURN);
    }

    #[test]
    fn test_shed_skin_no_status_is_noop() {
        let mut s = setup();
        s.sides[0].team[0].ability_id = data_bridge::ABILITY_SHED_SKIN;
        step_eot_abilities(&mut s, &TeamData::default(), 0, &mut BattleRng::from_closure(&mut |_| 0u32));
        assert_eq!(s.sides[0].team[0].status, STATUS_NONE);
    }


    #[test]
    fn test_flame_orb_inflicts_burn() {
        let mut s = setup();
        s.sides[0].team[0].item_id = 145; // Flame Orb

        step_status_orbs(&mut s, 0);

        assert_eq!(s.sides[0].team[0].status, STATUS_BURN);
    }

    #[test]
    fn test_flame_orb_no_overwrite_existing_status() {
        let mut s = setup();
        s.sides[0].team[0].item_id = 145;
        s.sides[0].team[0].status = STATUS_PARALYSIS;

        step_status_orbs(&mut s, 0);

        // set_status returns false if already statused — paralysis stays
        assert_eq!(s.sides[0].team[0].status, STATUS_PARALYSIS);
    }

    #[test]
    fn test_toxic_orb_inflicts_bad_poison() {
        let mut s = setup();
        s.sides[0].team[0].item_id = 515; // Toxic Orb

        step_status_orbs(&mut s, 0);

        assert_eq!(s.sides[0].team[0].status, STATUS_BAD_POISON);
    }


    #[test]
    fn test_zen_mode_eot_triggers() {
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

        // HP 100/400 = 25% → should trigger Zen Mode
        step_eot_abilities(&mut state, &TeamData::default(), 0, &mut BattleRng::from_closure(&mut |_| 0u32));
        assert_eq!(effective_species(&state, 0), 1171); // Darmanitan-Zen
    }


    #[test]
    fn test_leech_seed_drains_and_heals() {
        let mut s = setup();
        // Damage side 1 so heal is observable
        s.sides[1].team[0].current_hp = 150;
        set_volatile(&mut s, 0, VOL_LEECH_SEED);

        step_leech_seed(&mut s);

        // 200 / 8 = 25 drain
        assert_eq!(s.sides[0].team[0].current_hp, 175); // drained
        assert_eq!(s.sides[1].team[0].current_hp, 175); // healed
    }

    #[test]
    fn test_leech_seed_dying_mon_heals_only_actual() {
        let mut s = setup(); // team[0] 200/200 both sides
        // Seeded mon has less HP than the nominal drain (200/8 = 25).
        s.sides[0].team[0].current_hp = 10;
        s.sides[1].team[0].current_hp = 150; // damaged seeder so heal is observable
        set_volatile(&mut s, 0, VOL_LEECH_SEED);

        step_leech_seed(&mut s);

        // Seeded mon faints; seeder heals only the 10 actually removed, not 25.
        assert_eq!(s.sides[0].team[0].current_hp, 0);
        assert_eq!(s.sides[1].team[0].current_hp, 160);
    }

    #[test]
    fn test_leech_seed_magic_guard_blocks() {
        let mut s = setup();
        s.sides[0].team[0].ability_id = data_bridge::ABILITY_MAGIC_GUARD;
        s.sides[1].team[0].current_hp = 150;
        set_volatile(&mut s, 0, VOL_LEECH_SEED);

        step_leech_seed(&mut s);

        assert_eq!(s.sides[0].team[0].current_hp, 200); // no drain
        assert_eq!(s.sides[1].team[0].current_hp, 150); // no heal
    }


    #[test]
    fn test_binding_damage_with_counter() {
        let mut s = setup();
        set_volatile(&mut s, 0, VOL_BOUND);
        s.sides[0].active.set_bind_turns(3);

        step_binding_damage(&mut s, 0);

        assert_eq!(s.sides[0].active.bind_turns(), 2);
        assert_eq!(s.sides[0].team[0].current_hp, 175); // 200/8 = 25
        assert!(s.sides[0].active.has_volatile(VOL_BOUND));
    }

    #[test]
    fn test_binding_expiry_at_zero() {
        let mut s = setup();
        set_volatile(&mut s, 0, VOL_BOUND);
        s.sides[0].active.set_bind_turns(0);

        step_binding_damage(&mut s, 0);

        assert!(!s.sides[0].active.has_volatile(VOL_BOUND));
        assert_eq!(s.sides[0].team[0].current_hp, 200); // no damage on expiry
    }

    #[test]
    fn test_binding_tick_continues_while_trapper_active() {
        let mut s = setup();
        set_volatile(&mut s, 0, VOL_BOUND);
        s.sides[0].active.set_bind_turns(3);
        // Trapper (opposing active) alive → tick lands.
        step_binding_damage(&mut s, 0);
        assert_eq!(s.sides[0].active.bind_turns(), 2);
        assert_eq!(s.sides[0].team[0].current_hp, 175); // 200/8 = 25
        assert!(s.sides[0].active.has_volatile(VOL_BOUND));
    }

    #[test]
    fn test_binding_ends_when_trapper_faints() {
        let mut s = setup();
        set_volatile(&mut s, 0, VOL_BOUND);
        s.sides[0].active.set_bind_turns(3);
        // Trapper (side 1 active) fainted → trap ends silently, no damage.
        s.sides[1].team[0].current_hp = 0;
        step_binding_damage(&mut s, 0);
        assert!(!s.sides[0].active.has_volatile(VOL_BOUND));
        assert_eq!(s.sides[0].active.bind_turns(), 0);
        assert_eq!(s.sides[0].team[0].current_hp, 200); // no orphan tick
    }

    #[test]
    fn test_binding_damage_magic_guard() {
        let mut s = setup();
        s.sides[0].team[0].ability_id = data_bridge::ABILITY_MAGIC_GUARD;
        set_volatile(&mut s, 0, VOL_BOUND);
        s.sides[0].active.set_bind_turns(3);

        step_binding_damage(&mut s, 0);

        assert_eq!(s.sides[0].active.bind_turns(), 2); // counter still ticks
        assert_eq!(s.sides[0].team[0].current_hp, 200); // no damage
        assert!(s.sides[0].active.has_volatile(VOL_BOUND));
    }


    #[test]
    fn test_perish_song_countdown() {
        let mut s = setup();
        set_volatile(&mut s, 0, VOL_PERISH_SONG);
        s.sides[0].active.perish_count = 3;

        step_perish_song(&mut s, 0);

        assert_eq!(s.sides[0].active.perish_count, 2);
        assert_eq!(s.sides[0].team[0].current_hp, 200); // no damage yet
    }

    #[test]
    fn test_perish_song_faint_at_zero() {
        let mut s = setup();
        set_volatile(&mut s, 0, VOL_PERISH_SONG);
        s.sides[0].active.perish_count = 0;

        step_perish_song(&mut s, 0);

        assert_eq!(s.sides[0].team[0].current_hp, 0); // fainted
        assert!(!s.sides[0].active.has_volatile(VOL_PERISH_SONG)); // cleared
    }

    #[test]
    fn test_terrain_expires() {
        let mut s = setup();
        s.field.terrain = TERRAIN_ELECTRIC;
        s.field.terrain_turns = 1;

        // Countdown decrements to 0, then expiry clears terrain
        step_terrain_countdown(&mut s);
        step_terrain_expiry(&mut s);

        assert_eq!(s.field.terrain, TERRAIN_NONE);
        assert_eq!(s.field.terrain_turns, 0);
    }

    #[test]
    fn test_terrain_countdown_not_expired() {
        let mut s = setup();
        s.field.terrain = TERRAIN_PSYCHIC;
        s.field.terrain_turns = 3;

        // Countdown decrements but doesn't clear
        step_terrain_countdown(&mut s);
        step_terrain_expiry(&mut s);

        assert_eq!(s.field.terrain, TERRAIN_PSYCHIC);
        assert_eq!(s.field.terrain_turns, 2);
    }

    #[test]
    fn test_grassy_terrain_healing() {
        let mut s = setup();
        s.field.terrain = TERRAIN_GRASSY;
        s.field.terrain_turns = 5;
        // Reduce HP so healing is visible
        s.sides[0].team[0].current_hp = 100;
        s.sides[1].team[0].current_hp = 100;

        step_grassy_terrain(&mut s);

        // Both grounded mons heal 1/16 max HP = 200/16 = 12
        assert_eq!(s.sides[0].team[0].current_hp, 112);
        assert_eq!(s.sides[1].team[0].current_hp, 112);
    }

    #[test]
    fn test_grassy_terrain_no_heal_flying() {
        let mut s = setup();
        s.field.terrain = TERRAIN_GRASSY;
        s.field.terrain_turns = 5;
        s.sides[0].team[0].current_hp = 100;
        // Make side 0 Flying-type (not grounded)
        s.sides[0].active.override_types = [Type::Flying as u8, Type::Flying as u8];
        s.sides[0].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;

        step_grassy_terrain(&mut s);

        // Flying-type (not grounded) doesn't heal
        assert_eq!(s.sides[0].team[0].current_hp, 100);
    }

    #[test]
    fn test_gravity_expires() {
        let mut s = setup();
        set_gravity(&mut s, 1);
        assert!(s.field.gravity_turns > 0);
        step_volatile_counters(&mut s);
        assert_eq!(s.field.gravity_turns, 0);
    }

    #[test]
    fn test_magic_room_expires() {
        let mut s = setup();
        set_magic_room(&mut s, 1);
        assert!(s.field.magic_room_turns() > 0);
        step_volatile_counters(&mut s);
        assert_eq!(s.field.magic_room_turns(), 0);
    }

    #[test]
    fn test_wonder_room_expires() {
        let mut s = setup();
        set_wonder_room(&mut s, 1);
        assert!(s.field.wonder_room_turns() > 0);
        step_volatile_counters(&mut s);
        assert_eq!(s.field.wonder_room_turns(), 0);
    }

    #[test]
    fn test_magic_room_suppresses_item_healing() {
        let mut s = setup();
        s.sides[0].team[0].item_id = 145; // Flame Orb
        set_magic_room(&mut s, 5);
        step_item_healing(&mut s, 0);
        // Magic Room suppresses Flame Orb — no burn
        assert_eq!(s.sides[0].team[0].status, STATUS_NONE);
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
        let mut s = setup();
        s.sides[0].side_conditions.set_safeguard_turns(1);
        s.sides[0].side_conditions.set_mist_turns(1);
        s.sides[0].side_conditions.set_lucky_chant_turns(1);

        step_side_condition_expiry(&mut s, 0);

        assert_eq!(s.sides[0].side_conditions.safeguard_turns(), 0);
        assert_eq!(s.sides[0].side_conditions.mist_turns(), 0);
        assert_eq!(s.sides[0].side_conditions.lucky_chant_turns(), 0);
    }

    // ── Future Sight / Doom Desire delayed-attack resolution ──────────────────
    fn rng0() -> impl FnMut(u32) -> u32 { |_| 0u32 }

    #[test]
    fn test_future_move_resolves_at_third_eot() {
        let mut s = setup(); // side 1 team[0]: species 50 (Ground, Psychic-neutral), hp 200
        s.sides[1].team[0].current_hp = 500;
        s.sides[1].team[0].max_hp = 500;
        s.sides[1].team[0].stats[SPD] = 100;
        s.sides[1].team[0].level = 100;
        // Future Sight (kind 1), countdown 3, snapshot level 100 / SpA 200 / STAB ×1.5.
        s.sides[1].side_conditions.set_future_move(1, 3, 1, 100, 200);
        let mut c = rng0(); let mut rng = BattleRng::from_closure(&mut c);
        step_future_move(&mut s, 1, &mut rng);
        assert_eq!(s.sides[1].team[0].current_hp, 500, "no damage at 1st EOT");
        assert_eq!(s.sides[1].side_conditions.future_countdown(), 2);
        step_future_move(&mut s, 1, &mut rng);
        assert_eq!(s.sides[1].team[0].current_hp, 500, "no damage at 2nd EOT");
        assert_eq!(s.sides[1].side_conditions.future_countdown(), 1);
        step_future_move(&mut s, 1, &mut rng);
        // base=(42*120*200/100)/50+2=203; roll85→172; STAB×1.5→258; neutral→258.
        assert_eq!(s.sides[1].team[0].current_hp, 242, "damage lands at 3rd EOT");
        assert_eq!(s.sides[1].side_conditions.future_move(), 0, "record cleared after resolution");
    }

    #[test]
    fn test_future_move_no_pending_is_noop() {
        let mut s = setup();
        let mut c = rng0(); let mut rng = BattleRng::from_closure(&mut c);
        step_future_move(&mut s, 1, &mut rng);
        assert_eq!(s.sides[1].team[0].current_hp, 200, "no pending move → no damage");
    }

    #[test]
    fn test_future_sight_immune_vs_dark_clears_without_damage() {
        let mut s = setup();
        s.sides[1].team[0].stats[SPD] = 100;
        s.sides[1].active.override_types = [Type::Dark as u8, Type::Dark as u8];
        s.sides[1].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;
        s.sides[1].side_conditions.set_future_move(1, 1, 1, 100, 200); // resolve this tick
        let mut c = rng0(); let mut rng = BattleRng::from_closure(&mut c);
        step_future_move(&mut s, 1, &mut rng);
        assert_eq!(s.sides[1].team[0].current_hp, 200, "Dark is immune to Future Sight");
        assert_eq!(s.sides[1].side_conditions.future_move(), 0, "record still consumed");
    }

    #[test]
    fn test_future_move_skips_fainted_occupant() {
        let mut s = setup();
        s.sides[1].team[0].current_hp = 0; // fainted occupant
        s.sides[1].side_conditions.set_future_move(1, 1, 1, 100, 200);
        let mut c = rng0(); let mut rng = BattleRng::from_closure(&mut c);
        step_future_move(&mut s, 1, &mut rng);
        assert_eq!(s.sides[1].team[0].current_hp, 0);
        assert_eq!(s.sides[1].side_conditions.future_move(), 0, "consumed even when occupant fainted");
    }

    #[test]
    fn test_curse_residual_chips_quarter_max_hp() {
        let mut s = setup(); // team[0] 200/200
        s.sides[0].set_cursed();
        step_curse(&mut s, 0);
        assert_eq!(s.sides[0].team[0].current_hp, 150); // 200/4 = 50
    }

    #[test]
    fn test_curse_residual_can_ko() {
        let mut s = setup();
        s.sides[0].team[0].current_hp = 30;
        s.sides[0].set_cursed();
        step_curse(&mut s, 0);
        assert_eq!(s.sides[0].team[0].current_hp, 0);
        assert!(s.sides[0].team[0].is_fainted());
    }

    #[test]
    fn test_curse_residual_magic_guard_blocks() {
        let mut s = setup();
        s.sides[0].team[0].ability_id = data_bridge::ABILITY_MAGIC_GUARD;
        s.sides[0].set_cursed();
        step_curse(&mut s, 0);
        assert_eq!(s.sides[0].team[0].current_hp, 200);
    }

    #[test]
    fn test_curse_residual_uncursed_is_noop() {
        let mut s = setup();
        step_curse(&mut s, 0);
        assert_eq!(s.sides[0].team[0].current_hp, 200);
    }

    #[test]
    fn test_safeguard_does_not_block_flame_orb() {
        let mut s = setup();
        s.sides[0].team[0].item_id = 145; // Flame Orb
        s.sides[0].side_conditions.set_safeguard_turns(5);

        step_status_orbs(&mut s, 0);

        // Flame Orb is self-inflicted — Safeguard does not block
        assert_eq!(s.sides[0].team[0].status, STATUS_BURN);
    }
}
