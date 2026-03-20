//! Turn executor: the single function the MCTS loop calls to advance the game.
//!
//! Takes two player actions (chosen simultaneously), resolves turn order,
//! executes both actions, runs end-of-turn, checks for faints, and updates
//! the phase.
//!
//! Zero heap allocations.  All integer math.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag, MoveCategory};
use crate::state::accessors::*;
use crate::state::mutations::*;
use crate::state::zobrist::ZobristKeys;
use crate::state::switch::perform_switch;
use crate::state::end_of_turn::end_of_turn;
use crate::state::move_exec::execute_move;
use crate::data::moves::MoveFlags;
use crate::data::types::Type;

/// Decoded action. Move variant carries the resolved move_id so we
/// never call `effective_moves` twice for the same action.
#[derive(Clone, Copy, Debug)]
pub enum ActionKind {
    Move { slot: u8, move_id: u16 },
    Switch { target: u8 },
    Tera { move_id: u16 },
    Struggle,
}

/// Decode a `u8` action, resolving the move_id eagerly.
#[inline]
fn decode_action(state: &BattleState, side: usize, action: u8) -> ActionKind {
    match action {
        0..=3 => {
            let move_id = effective_moves(state, side)[action as usize];
            ActionKind::Move { slot: action, move_id }
        }
        4..=9 => ActionKind::Switch { target: action - 4 },
        ACTION_TERA => {
            // MCTS simplification: tera is combined with move 0
            let move_id = effective_moves(state, side)[0];
            ActionKind::Tera { move_id }
        }
        _ => ActionKind::Struggle,
    }
}

#[derive(Clone, Copy)]
struct OrderedAction {
    side: usize,
    action: ActionKind,
}

/// Compute effective speed for turn order comparison.
#[inline]
fn resolve_speed(state: &BattleState, side: usize) -> u32 {
    let mon = state.active_mon(side);
    let ability = effective_ability(state, side);
    let mut speed = boosted_stat(
        effective_stat(state, side, SPE),
        state.sides[side].active.boosts[SPE],
    ) as u32;

    if mon.status == STATUS_PARALYSIS && ability != data_bridge::ABILITY_QUICK_FEET {
        speed /= 2;
    }

    if data_bridge::item(mon.item_id).has(ItemFlag::CHOICE_SPE) {
        speed = speed * 3 / 2;
    }

    if state.sides[side].active.has_volatile(VOL_UNBURDEN) {
        speed *= 2;
    }

    if state.sides[side].side_conditions.tailwind_turns > 0 {
        speed *= 2;
    }

    let weather = effective_weather(state);
    match ability {
        data_bridge::ABILITY_CHLOROPHYLL
            if matches!(weather, WEATHER_SUN | WEATHER_HARSH_SUN)
            => { speed *= 2; }
        data_bridge::ABILITY_SWIFT_SWIM
            if matches!(weather, WEATHER_RAIN | WEATHER_HEAVY_RAIN)
            => { speed *= 2; }
        data_bridge::ABILITY_SAND_RUSH if weather == WEATHER_SAND
            => { speed *= 2; }
        data_bridge::ABILITY_SLUSH_RUSH if weather == WEATHER_SNOW
            => { speed *= 2; }
        data_bridge::ABILITY_SURGE_SURFER if state.field.terrain == TERRAIN_ELECTRIC
            => { speed *= 2; }
        data_bridge::ABILITY_SLOW_START
            if state.sides[side].active.turns_active < 5
            => { speed /= 2; }
        data_bridge::ABILITY_QUICK_FEET if mon.status != STATUS_NONE
            => { speed = speed * 3 / 2; }
        _ => {}
    }

    // Protosynthesis/Quark Drive Spe boost (suppressed by Neutralizing Gas)
    let paradox_stat = state.sides[side].active.paradox_stat();
    if paradox_stat == (SPE as u8 + 1) && !state.sides[side].active.has_volatile(VOL_ABILITY_SUPPRESSED) {
        speed = speed * 3 / 2; // 1.5× for Spe
    }

    speed
}

#[inline(always)]
fn action_priority(state: &BattleState, side: usize, action: &ActionKind) -> i8 {
    match action {
        ActionKind::Switch { .. } => 7,
        ActionKind::Struggle => 0,
        ActionKind::Tera { move_id } | ActionKind::Move { move_id, .. } => {
            if *move_id == 0 { return 0; }
            let md = data_bridge::move_hot(*move_id);
            let mut pri = md.priority;
            let ability = effective_ability(state, side);

            // Grassy Glide: +1 priority in Grassy Terrain if user is grounded
            if md.effect == data_bridge::MoveEffect::GrassyGlide
                && state.field.terrain == TERRAIN_GRASSY
                && is_grounded(state, side)
            {
                pri += 1;
            }

            // Prankster: +1 to Status moves
            if ability == data_bridge::ABILITY_PRANKSTER
                && md.category == MoveCategory::Status
            {
                pri += 1;
            }

            // Gale Wings: +1 to Flying moves at full HP
            if ability == data_bridge::ABILITY_GALE_WINGS
                && md.move_type == Type::Flying
            {
                let mon = state.active_mon(side);
                if mon.current_hp == mon.max_hp {
                    pri += 1;
                }
            }

            // Triage: +3 to healing moves
            if ability == data_bridge::ABILITY_TRIAGE
                && md.flags & MoveFlags::HEAL != 0
            {
                pri += 3;
            }

            // Stall / Mycelium Might: effectively go last (handled via fractional priority)
            // We model this as -1 priority shift for applicable moves
            if ability == data_bridge::ABILITY_STALL {
                pri -= 1;
            }
            if ability == data_bridge::ABILITY_MYCELIUM_MIGHT
                && md.category == MoveCategory::Status
            {
                pri -= 1;
            }

            pri
        }
    }
}

#[inline]
fn resolve_order(
    state: &BattleState,
    side_a: usize, act_a: ActionKind,
    side_b: usize, act_b: ActionKind,
    rng: &mut impl FnMut(u32) -> u32,
) -> (OrderedAction, OrderedAction) {
    let a = OrderedAction { side: side_a, action: act_a };
    let b = OrderedAction { side: side_b, action: act_b };

    let pri_a = action_priority(state, side_a, &act_a);
    let pri_b = action_priority(state, side_b, &act_b);

    if pri_a != pri_b {
        return if pri_a > pri_b { (a, b) } else { (b, a) };
    }

    if matches!(act_a, ActionKind::Switch { .. }) && matches!(act_b, ActionKind::Switch { .. }) {
        return (a, b);
    }

    let a_quick = matches!(act_a, ActionKind::Move { move_id, .. } | ActionKind::Tera { move_id } if {
        let md = data_bridge::move_hot(move_id);
        md.category != MoveCategory::Status
    }) && effective_ability(state, side_a) == data_bridge::ABILITY_QUICK_DRAW
        && rng(10) < 3;
    let b_quick = matches!(act_b, ActionKind::Move { move_id, .. } | ActionKind::Tera { move_id } if {
        let md = data_bridge::move_hot(move_id);
        md.category != MoveCategory::Status
    }) && effective_ability(state, side_b) == data_bridge::ABILITY_QUICK_DRAW
        && rng(10) < 3;

    if a_quick && !b_quick { return (a, b); }
    if b_quick && !a_quick { return (b, a); }

    let spd_a = resolve_speed(state, side_a);
    let spd_b = resolve_speed(state, side_b);
    let trick_room = state.field.trick_room_turns > 0;

    let a_faster = if spd_a != spd_b {
        if trick_room { spd_a < spd_b } else { spd_a > spd_b }
    } else {
        rng(2) == 0
    };

    if a_faster { (a, b) } else { (b, a) }
}

#[inline]
fn execute_action(
    state: &mut BattleState,
    keys: &ZobristKeys,
    side: usize,
    action: &ActionKind,
    rng: &mut impl FnMut(u32) -> u32,
) {
    match *action {
        ActionKind::Switch { target } => {
            perform_switch(state, keys, side, target as usize);
        }
        ActionKind::Move { slot, move_id } => {
            execute_move(state, keys, side, move_id, slot, rng);
        }
        ActionKind::Tera { move_id } => {
            apply_tera(state, keys, side);
            execute_move(state, keys, side, move_id, 0, rng);
        }
        ActionKind::Struggle => {
            execute_move(state, keys, side, 0, 0, rng);
        }
    }
}

/// Apply Terastallization to the active Pokémon.
#[inline]
fn apply_tera(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    let mon = &mut state.sides[side].team[slot];
    if mon.is_fainted() || mon.is_terastallized() { return; }
    mon.flags |= MON_FLAG_TERASTALLIZED;
    state.sides[side]._padding[0] |= 1;
    state.zobrist ^= keys.species[side][slot][0];

    // Embody Aspect: boost stat on Terastallization
    let ability = effective_ability(state, side);
    match ability {
        data_bridge::ABILITY_EMBODY_ASPECT_TEAL => { apply_boost(state, keys, side, SPE, 1); }
        data_bridge::ABILITY_EMBODY_ASPECT_WELLSPRING => { apply_boost(state, keys, side, SPD, 1); }
        data_bridge::ABILITY_EMBODY_ASPECT_HEARTHFLAME => { apply_boost(state, keys, side, ATK, 1); }
        data_bridge::ABILITY_EMBODY_ASPECT_CORNERSTONE => { apply_boost(state, keys, side, DEF, 1); }
        _ => {}
    }
}

fn faint_sweep(state: &mut BattleState, keys: &ZobristKeys) {
    let p1_alive = (0..6).any(|i| {
        let m = &state.sides[0].team[i];
        m.species_id != 0 && m.current_hp > 0
    });
    let p2_alive = (0..6).any(|i| {
        let m = &state.sides[1].team[i];
        m.species_id != 0 && m.current_hp > 0
    });

    if !p1_alive || !p2_alive {
        set_phase(state, keys, PHASE_GAME_OVER);
        return;
    }

    let p1_fainted = state.active_mon(0).is_fainted();
    let p2_fainted = state.active_mon(1).is_fainted();

    let p1_must = state.sides[0].active.has_volatile(VOL_MUST_SWITCH) && !p1_fainted;
    let p2_must = state.sides[1].active.has_volatile(VOL_MUST_SWITCH) && !p2_fainted;

    if p1_must { clear_volatile(state, keys, 0, VOL_MUST_SWITCH); }
    if p2_must { clear_volatile(state, keys, 1, VOL_MUST_SWITCH); }

    let phase = match (p1_fainted || p1_must, p2_fainted || p2_must) {
        (true, true)   => PHASE_SWITCH_BOTH,
        (true, false)  => PHASE_SWITCH_P1,
        (false, true)  => PHASE_SWITCH_P2,
        (false, false) => PHASE_ACTIONS,
    };
    set_phase(state, keys, phase);
}

pub fn execute_turn(
    state: &mut BattleState,
    keys: &ZobristKeys,
    action_p1: u8,
    action_p2: u8,
    rng: &mut impl FnMut(u32) -> u32,
) {
    let act0 = decode_action(state, 0, action_p1);
    let act1 = decode_action(state, 1, action_p2);
    let (first, second) = resolve_order(state, 0, act0, 1, act1, rng);

    let second_raw = if second.side == 0 { action_p1 } else { action_p2 };

    execute_action(state, keys, first.side, &first.action, rng);

    // Faint after move 1: pause for forced replacement before continuing
    if state.active_mon(0).is_fainted() || state.active_mon(1).is_fainted() {
        state.set_turn_resume(SUBPHASE_AFTER_MOVE1, second.side, second_raw);
        faint_sweep(state, keys);
        return;
    }

    execute_action(state, keys, second.side, &second.action, rng);

    if state.active_mon(0).is_fainted() || state.active_mon(1).is_fainted() {
        state.set_turn_resume(SUBPHASE_AFTER_MOVE2, 0, 0);
        faint_sweep(state, keys);
        return;
    }

    // Pivot moves: transition to switch phase before end-of-turn
    let p1_pivot = state.sides[0].active.has_volatile(VOL_MUST_SWITCH);
    let p2_pivot = state.sides[1].active.has_volatile(VOL_MUST_SWITCH);
    if p1_pivot || p2_pivot {
        faint_sweep(state, keys);
        return;
    }

    end_of_turn(state, keys);
    faint_sweep(state, keys);
    state.clear_turn_resume();
}

pub fn execute_switch_turn(
    state: &mut BattleState,
    keys: &ZobristKeys,
    action_p1: u8,
    action_p2: u8,
    rng: &mut impl FnMut(u32) -> u32,
) {
    let phase = state.phase;

    if phase == PHASE_SWITCH_P1 || phase == PHASE_SWITCH_BOTH {
        if let ActionKind::Switch { target } = decode_action(state, 0, action_p1) {
            perform_switch(state, keys, 0, target as usize);
        }
    }
    if phase == PHASE_SWITCH_P2 || phase == PHASE_SWITCH_BOTH {
        if let ActionKind::Switch { target } = decode_action(state, 1, action_p2) {
            perform_switch(state, keys, 1, target as usize);
        }
    }

    let subphase = state.turn_subphase();

    match subphase {
        SUBPHASE_AFTER_MOVE1 => {
            faint_sweep(state, keys);
            if state.phase != PHASE_ACTIONS { return; }

            let second_side = state.second_mover_side();
            let second_raw = state.pending_action();

            let second_replaced = match phase {
                PHASE_SWITCH_P1   => second_side == 0,
                PHASE_SWITCH_P2   => second_side == 1,
                PHASE_SWITCH_BOTH => true,
                _ => false,
            };

            if !second_replaced && !state.active_mon(second_side).is_fainted() {
                let second_action = decode_action(state, second_side, second_raw);
                execute_action(state, keys, second_side, &second_action, rng);
            }

            if state.active_mon(0).is_fainted() || state.active_mon(1).is_fainted() {
                state.set_turn_resume(SUBPHASE_AFTER_MOVE2, 0, 0);
                faint_sweep(state, keys);
                return;
            }

            let p1_pivot = state.sides[0].active.has_volatile(VOL_MUST_SWITCH);
            let p2_pivot = state.sides[1].active.has_volatile(VOL_MUST_SWITCH);
            if p1_pivot || p2_pivot {
                state.clear_turn_resume();
                faint_sweep(state, keys);
                return;
            }

            end_of_turn(state, keys);
            faint_sweep(state, keys);
            state.clear_turn_resume();
        }

        SUBPHASE_AFTER_MOVE2 => {
            faint_sweep(state, keys);
            if state.phase != PHASE_ACTIONS { return; }

            let p1_pivot = state.sides[0].active.has_volatile(VOL_MUST_SWITCH);
            let p2_pivot = state.sides[1].active.has_volatile(VOL_MUST_SWITCH);
            if p1_pivot || p2_pivot {
                state.clear_turn_resume();
                faint_sweep(state, keys);
                return;
            }

            end_of_turn(state, keys);
            faint_sweep(state, keys);
            state.clear_turn_resume();
        }

        _ => {
            faint_sweep(state, keys);
        }
    }
}
