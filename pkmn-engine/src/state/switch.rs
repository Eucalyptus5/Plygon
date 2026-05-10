//! Switch-in and switch-out logic.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag};
use crate::state::accessors::{effective_ability, effective_weather_for, battle_types, effective_stat, effective_species, effective_moves, is_grounded, is_trapped};
use crate::state::mutations::*;

pub fn switch_out(state: &mut BattleState, teams: &TeamData, side: usize) {
    let ability = effective_ability(state, side);
    let idx = state.sides[side].active_index as usize;

    if ability == data_bridge::ABILITY_NATURAL_CURE {
        clear_status(state, side, idx);
    }
    if ability == data_bridge::ABILITY_REGENERATOR {
        let max_hp = state.sides[side].team[idx].max_hp;
        heal(state, side, idx, max_hp / 3);
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
        clear_volatile(state, opp, VOL_ABILITY_SUPPRESSED);
        let opp_slot = state.sides[opp].active_index as usize;
        if !state.sides[opp].team[opp_slot].is_fainted() {
            let opp_ability = effective_ability(state, opp);
            // Imposter only fires on actual switch-in, not ability reactivation
            if opp_ability != data_bridge::ABILITY_IMPOSTER {
                apply_switch_in_ability(state, teams, opp);
            }
        }
    }

    // Partial-trap source leaves field → release the opposite side's bind.
    // Showdown: data/conditions.ts:234-242 deletes the partiallytrapped volatile
    // when source.isActive is false. Single-battle invariant: if A's mon was
    // binding B's mon, A leaving releases B; engine doesn't track source so it
    // assumes the binder was always A's outgoing mon.
    let opp = 1 - side;
    if state.sides[opp].active.has_volatile(VOL_BOUND) {
        clear_volatile(state, opp, VOL_BOUND);
        state.sides[opp].active.set_bind_turns(0);
    }

    // Skill Swap restore: Showdown's clearVolatile resets this.ability to
    // baseAbility on switch-out. Mirror that by restoring the stashed pre-swap
    // ability before the active (which holds the stash) is zeroed.
    if state.sides[side].team[idx].flags & MON_FLAG_ABILITY_SWAPPED != 0 {
        state.sides[side].team[idx].ability_id = state.sides[side].active.override_ability;
        state.sides[side].team[idx].flags &= !MON_FLAG_ABILITY_SWAPPED;
    }
    // Transform restore: the live base ability_id was overwritten with the copied
    // one; revert to the stashed native ability (Showdown's clearVolatile path).
    if state.sides[side].team[idx].flags & MON_FLAG_TRANSFORMED != 0 {
        state.sides[side].team[idx].ability_id = state.sides[side].active.transform_orig_ability;
        state.sides[side].team[idx].flags &= !MON_FLAG_TRANSFORMED;
    }

    state.sides[side].active.zero();
    state.sides[side].set_last_consumed_berry(0);
    // Showdown clears statsLoweredThisTurn on switch-out (clearVolatile); the incoming
    // mon must not inherit the departing mon's this-turn stat-drop state.
    state.sides[side].clear_stats_lowered_this_turn();
    // Reset the move-action flag so the incoming mon's first move (e.g. Fake Out)
    // is treated as its first turn out (Showdown resets activeMoveActions on switch).
    state.sides[side].clear_acted_since_switch_in();
    // Showdown clearVolatile resets moveThisTurnResult/moveLastTurnResult: a
    // freshly-switched-in mon never doubles Stomping Tantrum / Temper Flare.
    state.sides[side].clear_move_failed_state();
    // Showdown clearVolatile removes the Ghost-Curse volatile on switch-out: the
    // incoming mon (incl. a faint replacement) never inherits the curse residual.
    state.sides[side].clear_cursed();
}

pub fn switch_in(state: &mut BattleState, teams: &TeamData, side: usize, new_index: usize) {
    if switch_in_phase_a(state, teams, side, new_index) {
        apply_switch_in_ability(state, teams, side);
        apply_switch_in_item(state, side);
    }
}

/// Phase A of switch-in: state install only — set active_index, apply entry
/// hazards, check pinch berry, run faint short-circuit, fire Healing Wish /
/// Lunar Dance, install Neutralizing-Gas ability suppression. Does NOT run
/// the switch-in ability (Phase B) or the terrain-seed item check (which
/// must observe Phase-B-installed terrain).
///
/// Returns `true` if the switch-in ability/item should still run, `false` if
/// the incoming mon fainted from hazards (skip ability/item for this side).
///
/// On a same-turn double switch Showdown does NOT batch both switch-ins: it
/// runs each switch's `runSwitch` (ability + item) immediately, in
/// outgoing-speed order, so the faster-switching side's switch-in ability
/// (e.g. Intimidate) fires against the foe present at that instant. The split
/// from the ability/item step exists so `perform_double_switch` can install
/// each incoming mon, then fire its ability before the other side switches.
pub fn switch_in_phase_a(
    state: &mut BattleState, teams: &TeamData, side: usize, new_index: usize,
) -> bool {
    state.sides[side].active_index = new_index as u8;
    apply_entry_hazards(state, side);

    // Check berry activation after hazard damage (e.g. Sitrus Berry can save)
    let slot = state.sides[side].active_index as usize;
    if !state.sides[side].team[slot].is_fainted() {
        crate::state::move_exec::check_pinch_berry(state, side, slot);
    }

    // If fainted from hazards, skip ability/item activation
    if state.sides[side].team[slot].is_fainted() {
        return false;
    }

    // Palafin Zero to Hero: transform on switch-in
    crate::state::forme::check_palafin_hero(state, teams, side);

    // Terapagos Tera Shift -> Tera Shell (Terapagos-Terastal) on switch-in
    crate::state::forme::check_tera_shift(state, teams, side);

    // Healing Wish / Lunar Dance: fully heal the incoming mon
    {
        let sc = &state.sides[side].side_conditions;
        let has_hw = sc.has_healing_wish();
        let has_ld = sc.has_lunar_dance();
        if has_hw || has_ld {
            let max_hp = state.sides[side].team[slot].max_hp;
            heal(state, side, slot, max_hp);
            if state.sides[side].team[slot].status != STATUS_NONE {
                clear_status(state, side, slot);
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

    // BUG-P3-A-301: If opposing active has Neutralizing Gas, suppress the
    // incoming side's ability before its switch-in effect fires (e.g. Intimidate).
    // NG's own switch-in is dispatched via ABILITY_NEUTRALIZING_GAS in switch_out
    // for the opposing side, so this guard does not block NG itself.
    {
        let opp = 1 - side;
        let opp_slot = state.sides[opp].active_index as usize;
        if !state.sides[opp].team[opp_slot].is_fainted() {
            let opp_ability_id = state.sides[opp].team[opp_slot].ability_id;
            if opp_ability_id == data_bridge::ABILITY_NEUTRALIZING_GAS
                && !state.sides[opp].active.has_volatile(VOL_ABILITY_SUPPRESSED)
            {
                let slot = state.sides[side].active_index as usize;
                let has_shield = state.field.magic_room_turns() == 0
                    && data_bridge::item(state.sides[side].team[slot].item_id).has(ItemFlag::ABILITY_SHIELD);
                if !has_shield {
                    set_volatile(state, side, VOL_ABILITY_SUPPRESSED);
                }
            }
        }
    }

    true
}

/// Check and activate a terrain seed for the given side.
/// Called on switch-in and when terrain is set by a move or ability.
#[inline]
pub fn check_terrain_seed(state: &mut BattleState, side: usize) {
    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];
    if mon.item_id == 0 || mon.is_fainted() { return; }
    if state.field.magic_room_turns() > 0 { return; }
    let item = data_bridge::item(mon.item_id);

    if !item.has(ItemFlag::TERRAIN_SEED) { return; }

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
        apply_boost(state, side, stat, 1);
        consume_item(state, side, slot);
        if effective_ability(state, side) == data_bridge::ABILITY_UNBURDEN {
            set_volatile(state, side, VOL_UNBURDEN);
        }
    }
}

pub(crate) fn apply_switch_in_item(state: &mut BattleState, side: usize) {
    check_terrain_seed(state, side);
}

/// Baton Pass volatile mask: volatiles that transfer on Baton Pass.
const BATON_PASS_VOLATILE_MASK: u32 =
    VOL_SUBSTITUTE | VOL_LEECH_SEED | VOL_INGRAIN | VOL_AQUA_RING
    | VOL_FOCUS_ENERGY | VOL_PERISH_SONG | VOL_MAGNET_RISE;

/// Perform a voluntary (PHASE_ACTIONS) switch. Returns `true` if the switch
/// happened, `false` if rejected because the user is trapped (Magnet Pull /
/// Arena Trap / Shadow Tag / binding volatiles / etc.).
///
/// Forced switches (faint replacement, U-turn) must bypass the trap gate —
/// use `perform_switch_forced` or call `switch_out` + `switch_in` directly.
pub fn perform_switch(state: &mut BattleState, teams: &TeamData, side: usize, new_index: usize) -> bool {
    if is_trapped(state, side) {
        return false;
    }
    perform_switch_forced(state, teams, side, new_index);
    true
}

/// Both sides chose Switch on the same turn. Showdown sorts the two `switch`
/// actions by the outgoing active's speed and, via `insertChoice`, runs each
/// switch's `runSwitch` (switch-in ability + item) immediately — the faster
/// side's switch-in ability fires while the slower side is still its OLD mon.
/// So Intimidate from the faster-switching side targets the foe present at that
/// instant, not the foe's incoming replacement. We resolve each side fully
/// (switch_out + Phase A install + ability + item) in `first`/`second` order;
/// the caller has already ordered them by outgoing speed.
///
/// Trapping (Magnet Pull / Arena Trap / Shadow Tag / binding volatiles) is
/// honored per side — a trapped side's switch silently no-ops, matching
/// `perform_switch`'s singleton-path behavior.
pub fn perform_double_switch(
    state: &mut BattleState, teams: &TeamData,
    first_side: usize, first_target: usize,
    second_side: usize, second_target: usize,
    _rng: &mut impl FnMut(u32) -> u32,
) {
    if !is_trapped(state, first_side) {
        switch_out(state, teams, first_side);
        if switch_in_phase_a(state, teams, first_side, first_target) {
            apply_switch_in_ability(state, teams, first_side);
            apply_switch_in_item(state, first_side);
        }
    }
    if !is_trapped(state, second_side) {
        switch_out(state, teams, second_side);
        if switch_in_phase_a(state, teams, second_side, second_target) {
            apply_switch_in_ability(state, teams, second_side);
            apply_switch_in_item(state, second_side);
        }
    }
}

/// Force a switch regardless of trap status. Used by faint replacement and
/// pivot moves (U-turn, Volt Switch, Baton Pass, etc.) which bypass trapping.
pub fn perform_switch_forced(state: &mut BattleState, teams: &TeamData, side: usize, new_index: usize) {
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

    switch_out(state, teams, side);
    switch_in(state, teams, side, new_index);

    if is_baton_pass {
        for stat in 0..7 {
            if saved_boosts[stat] != 0 {
                apply_boost(state, side, stat, saved_boosts[stat]);
            }
        }
        for bit in 0..32u32 {
            if saved_volatiles & (1 << bit) != 0 {
                set_volatile(state, side, 1 << bit);
            }
        }
        if saved_sub_hp > 0 {
            state.sides[side].active.substitute_hp = saved_sub_hp;
        }
    }
}

fn apply_entry_hazards(state: &mut BattleState, side: usize) {
    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];
    if state.field.magic_room_turns() == 0
        && data_bridge::item(mon.item_id).has(ItemFlag::HAZARD_IMMUNE) { return; }
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD { return; }

    let sc = state.sides[side].side_conditions;

    if sc.hazard_flags & HAZARD_STEALTH_ROCK != 0 {
        let (t1, t2) = battle_types(state, side);
        // Convert u8 back to Type for the effectiveness call.
        // SAFETY: our u8 type values come from Type as u8, so they're valid.
        let def1 = unsafe { core::mem::transmute::<u8, Type>(t1) };
        let def2 = unsafe { core::mem::transmute::<u8, Type>(t2) };
        let eff = dual_type_effectiveness(Type::Rock, def1, def2);
        let max_hp = state.sides[side].team[slot].max_hp as u32;
        let damage = (max_hp * eff as u32 / 32).max(1) as u16;
        deal_damage(state, side, slot, damage);
    }

    // Spikes (grounded only)
    if sc.spikes > 0 && is_grounded(state, side) {
        let max_hp = state.sides[side].team[slot].max_hp;
        let damage = match sc.spikes { 1 => max_hp / 8, 2 => max_hp / 6, _ => max_hp / 4 };
        deal_damage(state, side, slot, damage.max(1));
    }

    // Toxic Spikes (grounded only)
    if sc.toxic_spikes > 0 && is_grounded(state, side) {
        let (t1, t2) = battle_types(state, side);
        let is_poison = t1 == Type::Poison as u8 || t2 == Type::Poison as u8;
        let is_steel = t1 == Type::Steel as u8 || t2 == Type::Steel as u8;
        if is_poison {
            // Poison types absorb and remove Toxic Spikes
            state.sides[side].side_conditions.toxic_spikes = 0;
        } else if !is_steel && state.sides[side].side_conditions.safeguard_turns() == 0
            && !crate::state::forme::is_minior_meteor_forme(state, side)
        {
            // BUG-P5-M-009: Immunity / Pastel Veil / Purifying Salt block Toxic Spikes poison.
            let ability = effective_ability(state, side);
            let poison_immune = ability == data_bridge::ABILITY_IMMUNITY
                || ability == data_bridge::ABILITY_PASTEL_VEIL
                || ability == data_bridge::ABILITY_PURIFYING_SALT;
            if !poison_immune {
                // Steel types are immune to poison; Safeguard blocks status
                match sc.toxic_spikes {
                    1 => { set_status(state, side, slot, STATUS_POISON, 0); }
                    _ => { set_status(state, side, slot, STATUS_BAD_POISON, 0); }
                }
            }
        }
    }

    if sc.hazard_flags & HAZARD_STICKY_WEB != 0 && is_grounded(state, side)
        && state.sides[side].side_conditions.mist_turns() == 0
    {
        try_opponent_stat_drop(state, side, SPE, -1);
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

pub(crate) fn apply_switch_in_ability(state: &mut BattleState, teams: &TeamData, side: usize) {
    let ability = effective_ability(state, side);
    let opp = 1 - side;
    let side_mirror = state.field.magic_room_turns() == 0
        && state.active_mon(opp).item_id != 0;
    let opp_mirror = state.field.magic_room_turns() == 0
        && state.active_mon(side).item_id != 0;
    let side_boosts_before = if side_mirror { state.sides[side].active.boosts } else { [0; 7] };
    let opp_boosts_before = if opp_mirror { state.sides[opp].active.boosts } else { [0; 7] };
    match ability {
        data_bridge::ABILITY_INTIMIDATE  => {
            let opp_ability = effective_ability(state, opp);
            if !state.sides[opp].active.has_volatile(VOL_SUBSTITUTE) {
                if opp_ability == data_bridge::ABILITY_GUARD_DOG {
                    // Guard Dog reverses Intimidate into +1 Atk (bypasses Mist)
                    apply_boost(state, opp, ATK, 1);
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
                        try_opponent_stat_drop(state, opp, ATK, -1);
                    }
                }
            }
            // Rattled: +1 Speed when targeted by Intimidate, regardless of blocking
            if opp_ability == data_bridge::ABILITY_RATTLED {
                apply_boost(state, opp, SPE, 1);
            }
        }

        data_bridge::ABILITY_DRIZZLE     => { set_weather(state, WEATHER_RAIN, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_DROUGHT     => { set_weather(state, WEATHER_SUN, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_SAND_STREAM => { set_weather(state, WEATHER_SAND, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_SNOW_WARNING=> { set_weather(state, WEATHER_SNOW, 5); check_paradox_deactivation(state); }

        data_bridge::ABILITY_AIR_LOCK | data_bridge::ABILITY_CLOUD_NINE => {
            state.field.field_flags |= FIELD_WEATHER_SUPPRESSED;
            check_paradox_deactivation(state);
        }

        data_bridge::ABILITY_ELECTRIC_SURGE => { set_terrain(state, TERRAIN_ELECTRIC, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_GRASSY_SURGE  => { set_terrain(state, TERRAIN_GRASSY, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_MISTY_SURGE   => { set_terrain(state, TERRAIN_MISTY, 5); check_paradox_deactivation(state); }
        data_bridge::ABILITY_PSYCHIC_SURGE => { set_terrain(state, TERRAIN_PSYCHIC, 5); check_paradox_deactivation(state); }

        // +1 SpA if foe SpD < Def, else +1 Atk
        data_bridge::ABILITY_DOWNLOAD => {
            let opp_def = state.active_mon(opp).stats[DEF] as u32;
            let opp_spd = state.active_mon(opp).stats[SPD] as u32;
            if opp_spd < opp_def {
                apply_boost(state, side, SPA, 1);
            } else {
                apply_boost(state, side, ATK, 1);
            }
        }

        data_bridge::ABILITY_TRACE => {
            let opp_ability = effective_ability(state, opp);
            if opp_ability != 0 && !is_untraceable(opp_ability) {
                state.sides[side].active.override_ability = opp_ability;
                set_volatile(state, side, VOL_ABILITY_OVERRIDDEN);
                // Trigger the traced ability's switch-in effect
                apply_switch_in_ability(state, teams, side);
            }
        }

        data_bridge::ABILITY_IMPOSTER => {
            let opp_mon = state.active_mon(opp);
            if opp_mon.current_hp > 0 {
                let opp_species = effective_species(state, opp);
                let opp_types = battle_types(state, opp);
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
                set_volatile(state, side, VOL_TRANSFORMED);
                set_volatile(state, side, VOL_TYPES_OVERRIDDEN);
            }
        }

        data_bridge::ABILITY_NEUTRALIZING_GAS => {
            let has_shield = state.field.magic_room_turns() == 0
                && data_bridge::item(state.active_mon(opp).item_id).has(ItemFlag::ABILITY_SHIELD);
            if !has_shield {
                set_volatile(state, opp, VOL_ABILITY_SUPPRESSED);
            }
        }

        data_bridge::ABILITY_INTREPID_SWORD => {
            let slot = state.sides[side].active_index as usize;
            if state.sides[side].team[slot].flags & MON_FLAG_SWORD_BOOSTED == 0 {
                state.sides[side].team[slot].flags |= MON_FLAG_SWORD_BOOSTED;
                apply_boost(state, side, ATK, 1);
            }
        }

        data_bridge::ABILITY_DAUNTLESS_SHIELD => {
            let slot = state.sides[side].active_index as usize;
            if state.sides[side].team[slot].flags & MON_FLAG_SHIELD_BOOSTED == 0 {
                state.sides[side].team[slot].flags |= MON_FLAG_SHIELD_BOOSTED;
                apply_boost(state, side, DEF, 1);
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
                    try_opponent_stat_drop(state, opp, EVA, -1);
                }
            }
        }

        data_bridge::ABILITY_SCHOOLING => {
            crate::state::forme::check_schooling(state, teams, side);
        }
        data_bridge::ABILITY_SHIELDS_DOWN => {
            crate::state::forme::check_shields_down(state, teams, side);
        }

        data_bridge::ABILITY_PROTOSYNTHESIS => {
            activate_paradox_ability(state, side,
                matches!(effective_weather_for(state, side), WEATHER_SUN | WEATHER_HARSH_SUN));
        }
        data_bridge::ABILITY_QUARK_DRIVE => {
            activate_paradox_ability(state, side,
                state.field.terrain == TERRAIN_ELECTRIC);
        }

        // Orichalcum Pulse: set sun on switch-in (like Drought)
        data_bridge::ABILITY_ORICHALCUM_PULSE => {
            set_weather(state, WEATHER_SUN, 5);
            check_paradox_deactivation(state);
        }
        // Hadron Engine: set Electric Terrain on switch-in (like Electric Surge)
        data_bridge::ABILITY_HADRON_ENGINE => {
            set_terrain(state, TERRAIN_ELECTRIC, 5);
            check_paradox_deactivation(state);
        }

        _ => {}
    }
    if side_mirror { check_mirror_herb_diff(state, side, &side_boosts_before); }
    if opp_mirror { check_mirror_herb_diff(state, opp, &opp_boosts_before); }
}

/// Find the highest stat index (first-wins on ties, matching Showdown's getBestStat).
#[inline]
fn find_best_stat(stats: &[u16; 5], boosts: &[i8; 7]) -> usize {
    let mut best_idx = 0usize;
    let mut best_val = 0u16;
    for i in 0..5 {
        let v = boosted_stat(stats[i], boosts[i]);
        if v > best_val {
            best_val = v;
            best_idx = i;
        }
    }
    best_idx
}

/// Activate Protosynthesis or Quark Drive: find highest stat, encode in _padding[3].
/// `field_active` = true if the ability's weather/terrain is currently active.
fn activate_paradox_ability(
    state: &mut BattleState, side: usize, field_active: bool,
) {
    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];

    // Determine if ability should activate
    let from_booster = !field_active && mon.item_id == data_bridge::ITEM_BOOSTER_ENERGY;
    if !field_active && !from_booster { return; }

    // Find highest stat accounting for stat stages (Showdown getBestStat(false, true))
    // Showdown uses strictly-greater-than, so first stat wins ties.
    let stats = &mon.stats;
    let boosts = &state.sides[side].active.boosts;
    let best = find_best_stat(stats, boosts);

    // Store paradox stat+1 and fromBooster flag in _padding[3]
    state.sides[side].active.set_paradox(best as u8 + 1, from_booster);

    // Consume Booster Energy if it was the trigger
    if from_booster {
        consume_item(state, side, slot);
        if effective_ability(state, side) == data_bridge::ABILITY_UNBURDEN {
            set_volatile(state, side, VOL_UNBURDEN);
        }
    }
}

/// After any weather/terrain change, check if Protosynthesis/Quark Drive should
/// deactivate OR activate. Deactivates field-sourced boosts when conditions end.
/// Activates paradox abilities for mons already on the field when conditions start.
pub fn check_paradox_deactivation(state: &mut BattleState) {
    for side in 0..2 {
        let slot = state.sides[side].active_index as usize;
        if state.sides[side].team[slot].is_fainted() { continue; }
        if state.sides[side].active.has_volatile(VOL_ABILITY_SUPPRESSED) { continue; }

        let ability = effective_ability(state, side);
        let already_active = state.sides[side].active.paradox_stat() > 0;
        let from_booster = state.sides[side].active.paradox_from_booster();

        if already_active && !from_booster {
            // Check if should deactivate
            let should_deactivate = match ability {
                data_bridge::ABILITY_PROTOSYNTHESIS => {
                    !matches!(effective_weather_for(state, side), WEATHER_SUN | WEATHER_HARSH_SUN)
                }
                data_bridge::ABILITY_QUARK_DRIVE => {
                    state.field.terrain != TERRAIN_ELECTRIC
                }
                _ => true, // ability changed/suppressed — deactivate
            };
            if should_deactivate {
                state.sides[side].active.clear_paradox();
            }
        } else if !already_active {
            // Check if should activate (field condition now present)
            let should_activate = match ability {
                data_bridge::ABILITY_PROTOSYNTHESIS => {
                    matches!(effective_weather_for(state, side), WEATHER_SUN | WEATHER_HARSH_SUN)
                }
                data_bridge::ABILITY_QUARK_DRIVE => {
                    state.field.terrain == TERRAIN_ELECTRIC
                }
                _ => false,
            };
            if should_activate {
                // Find best stat and set paradox (first-wins on ties)
                let mon = &state.sides[side].team[slot];
                let stats = &mon.stats;
                let boosts = &state.sides[side].active.boosts;
                let best = find_best_stat(stats, boosts);
                state.sides[side].active.set_paradox(best as u8 + 1, false);
            }
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

    #[test]
    fn test_switch_out_zeros_active() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 25;
        state.sides[0].team[0].current_hp = 200;
        state.sides[0].team[0].max_hp = 200;
        state.sides[0].active.boosts[ATK] = 2;
        state.sides[0].active.volatile_flags = VOL_SUBSTITUTE | VOL_LEECH_SEED;
        switch_out(&mut state, &TeamData::default(), 0);
        assert_eq!(state.sides[0].active.volatile_flags, 0);
    }

    #[test]
    fn test_baton_pass_preserves_boosts() {
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [80; 5], ..Default::default()
        };

        // Set +2 Atk and +1 SpA
        apply_boost(&mut state, 0, ATK, 2);
        apply_boost(&mut state, 0, SPA, 1);

        // Set Baton Pass flag
        state.sides[0].active._padding[0] = 1;

        // Perform Baton Pass switch to slot 1
        perform_switch(&mut state, &TeamData::default(), 0, 1);

        // New active should inherit boosts
        assert_eq!(state.sides[0].active.boosts[ATK], 2);
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
        assert_eq!(state.sides[0].active_index, 1);
    }

    #[test]
    fn test_baton_pass_preserves_substitute() {
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [80; 5], ..Default::default()
        };

        // Set up Substitute with 50 HP
        set_volatile(&mut state, 0, VOL_SUBSTITUTE);
        state.sides[0].active.substitute_hp = 50;

        // Also set a non-Baton volatile (e.g., VOL_FLASH_FIRE) — should NOT transfer
        set_volatile(&mut state, 0, VOL_FLASH_FIRE);

        // Set Baton Pass flag
        state.sides[0].active._padding[0] = 1;

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        // Substitute should transfer, Flash Fire should not
        assert!(state.sides[0].active.has_volatile(VOL_SUBSTITUTE));
        assert_eq!(state.sides[0].active.substitute_hp, 50);
        assert!(!state.sides[0].active.has_volatile(VOL_FLASH_FIRE));
    }

    #[test]
    fn test_normal_switch_clears_boosts() {
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [80; 5], ..Default::default()
        };

        apply_boost(&mut state, 0, ATK, 2);

        // Normal switch (no baton pass flag)
        perform_switch(&mut state, &TeamData::default(), 0, 1);

        // Boosts should NOT transfer
        assert_eq!(state.sides[0].active.boosts[ATK], 0);
    }


    #[test]
    fn test_terrain_seed_no_trigger_without_terrain() {
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };

        apply_switch_in_item(&mut state, 0);
        assert_eq!(state.sides[0].active.boosts[DEF], 0);
    }


    #[test]
    fn test_intimidate_lowers_opponent_atk() {
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

        // Switch to Intimidate mon
        perform_switch(&mut state, &TeamData::default(), 0, 1);

        assert_eq!(state.sides[1].active.boosts[ATK], -1);
    }

    #[test]
    fn test_stealth_rock_damage_by_type() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        // Should have taken some damage (Rock vs Electric = 1× → 1/8 max HP = 50 damage)
        // The exact amount depends on type effectiveness
        assert!(state.sides[0].team[1].current_hp < 400);
    }

    #[test]
    fn test_spikes_damage_by_layer() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        // 1 layer = 1/8 max HP = 50 damage
        assert_eq!(state.sides[0].team[1].current_hp, 350);
    }

    #[test]
    fn test_spikes_3_layers() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        // 3 layers = 1/4 max HP = 100 damage
        assert_eq!(state.sides[0].team[1].current_hp, 300);
    }

    #[test]
    fn test_toxic_spikes_poisons_on_switch() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        assert_eq!(state.sides[0].team[1].status, STATUS_POISON);
    }

    #[test]
    fn test_toxic_spikes_2_layers_badly_poisons() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        assert_eq!(state.sides[0].team[1].status, STATUS_BAD_POISON);
    }

    #[test]
    fn test_faint_from_hazards_skips_ability() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        // Mon fainted from hazards → Intimidate should NOT have activated
        assert_eq!(state.sides[0].team[1].current_hp, 0);
        assert_eq!(state.sides[1].active.boosts[ATK], 0);
    }

    #[test]
    fn test_natural_cure_clears_status() {
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

        switch_out(&mut state, &TeamData::default(), 0);

        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_regenerator_heals_on_switch_out() {
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

        switch_out(&mut state, &TeamData::default(), 0);

        // Should heal 1/3 of 300 = 100
        assert_eq!(state.sides[0].team[0].current_hp, 300);
    }

    #[test]
    fn test_electric_surge_sets_terrain() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        assert_eq!(state.field.terrain, TERRAIN_ELECTRIC);
        assert_eq!(state.field.terrain_turns, 5);
    }

    #[test]
    fn test_download_boosts_spa_when_spd_lower() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        assert_eq!(state.sides[0].active.boosts[SPA], 1);
    }

    #[test]
    fn test_protosynthesis_sun_boost() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 0);

        assert_eq!(state.sides[0].active.paradox_stat(), ATK as u8 + 1);
        assert!(!state.sides[0].active.paradox_from_booster());
    }

    #[test]
    fn test_protosynthesis_booster_energy() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 0);

        assert_eq!(state.sides[0].active.paradox_stat(), SPA as u8 + 1);
        assert!(state.sides[0].active.paradox_from_booster());
        assert_eq!(state.sides[0].team[0].item_id, 0); // consumed
    }

    #[test]
    fn test_quark_drive_electric_terrain() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 0);

        assert_eq!(state.sides[0].active.paradox_stat(), SPE as u8 + 1);
        assert!(!state.sides[0].active.paradox_from_booster());
    }

    #[test]
    fn test_paradox_deactivates_weather_end() {
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

        crate::state::end_of_turn::end_of_turn(&mut state, &TeamData::default(), &mut crate::state::BattleRng::from_closure(&mut |_| 0u32));

        // Weather expired, paradox should deactivate
        assert_eq!(state.field.weather, WEATHER_NONE);
        assert_eq!(state.sides[0].active.paradox_stat(), 0);
    }

    #[test]
    fn test_paradox_booster_persists_weather_end() {
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

        crate::state::end_of_turn::end_of_turn(&mut state, &TeamData::default(), &mut crate::state::BattleRng::from_closure(&mut |_| 0u32));

        // Weather expired but Booster Energy boost persists
        assert_eq!(state.field.weather, WEATHER_NONE);
        assert_eq!(state.sides[0].active.paradox_stat(), ATK as u8 + 1);
        assert!(state.sides[0].active.paradox_from_booster());
    }

    #[test]
    fn test_paradox_no_activate_without_condition() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 0);

        assert_eq!(state.sides[0].active.paradox_stat(), 0);
    }

    #[test]
    fn test_paradox_deactivates_terrain_end() {
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

        crate::state::end_of_turn::end_of_turn(&mut state, &TeamData::default(), &mut crate::state::BattleRng::from_closure(&mut |_| 0u32));

        assert_eq!(state.field.terrain, TERRAIN_NONE);
        assert_eq!(state.sides[0].active.paradox_stat(), 0);
    }

    #[test]
    fn test_drizzle_deactivates_protosynthesis() {
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

        perform_switch(&mut state, &TeamData::default(), 1, 1);

        // Drizzle replaced sun -> Protosynthesis deactivated
        assert_eq!(state.field.weather, WEATHER_RAIN);
        assert_eq!(state.sides[0].active.paradox_stat(), 0);
    }

    #[test]
    fn test_safeguard_blocks_toxic_spikes() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        // Safeguard blocks the poison from toxic spikes
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
        // Toxic spikes remain on the field (not absorbed)
        assert_eq!(state.sides[0].side_conditions.toxic_spikes, 1);
    }

    #[test]
    fn test_mist_blocks_sticky_web() {
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

        perform_switch(&mut state, &TeamData::default(), 0, 1);

        // Mist blocks the speed drop from sticky web
        assert_eq!(state.sides[0].active.boosts[SPE], 0);
    }

    #[test]
    fn test_ability_shield_blocks_neutralizing_gas() {
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            ability_id: data_bridge::ABILITY_NEUTRALIZING_GAS,
            stats: [100; 5], ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 300, max_hp: 300,
            ability_id: data_bridge::ABILITY_INTIMIDATE,
            item_id: data_bridge::ITEM_ABILITY_SHIELD,
            stats: [100; 5], ..Default::default()
        };
        perform_switch(&mut state, &TeamData::default(), 0, 1);
        assert!(!state.sides[1].active.has_volatile(VOL_ABILITY_SUPPRESSED));
    }

    #[test]
    fn test_clear_amulet_blocks_intimidate() {
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            ability_id: data_bridge::ABILITY_INTIMIDATE,
            stats: [100; 5], ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 300, max_hp: 300,
            item_id: data_bridge::ITEM_CLEAR_AMULET,
            stats: [100; 5], ..Default::default()
        };
        perform_switch(&mut state, &TeamData::default(), 0, 1);
        assert_eq!(state.sides[1].active.boosts[ATK], 0);
    }

    #[test]
    fn test_clear_amulet_blocks_sticky_web() {
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            item_id: data_bridge::ITEM_CLEAR_AMULET,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            item_id: data_bridge::ITEM_CLEAR_AMULET,
            stats: [100; 5], ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 300, max_hp: 300,
            stats: [100; 5], ..Default::default()
        };
        state.sides[0].side_conditions.hazard_flags = HAZARD_STICKY_WEB;
        perform_switch(&mut state, &TeamData::default(), 0, 1);
        assert_eq!(state.sides[0].active.boosts[SPE], 0);
    }
}
