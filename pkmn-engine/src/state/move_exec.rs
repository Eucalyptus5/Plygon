//! Move execution: the per-move sequence called by the turn executor.
//!
//! Handles pre-move checks (flinch, paralysis, sleep, freeze, confusion),
//! accuracy, damage application, drain/recoil, secondary effects,
//! contact aftermath, status moves, and Protect logic.
//!
//! Zero heap allocations.  All integer math.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag, MoveCategory, MoveEffect};
use crate::state::accessors::*;
use crate::state::mutations::*;
use crate::state::zobrist::ZobristKeys;
use crate::state::calc::calc_damage;
use crate::state::switch::{
    set_stealth_rock, add_spikes, add_toxic_spikes, set_sticky_web, clear_hazards,
};
use crate::data::moves::{MoveData, MoveFlags};
use crate::data::types::Type;

// ── Accuracy tables ─────────────────────────────────────────────────

const ACC_NUM: [u32; 13] = [3, 3, 3, 3, 3, 3, 3, 4, 5, 6, 7, 8, 9];
const ACC_DEN: [u32; 13] = [9, 8, 7, 6, 5, 4, 3, 3, 3, 3, 3, 3, 3];

// ── Accuracy check ──────────────────────────────────────────────────

#[inline]
fn accuracy_check(
    state: &BattleState,
    atk_side: usize,
    md: &MoveData,
    rng: &mut impl FnMut(u32) -> u32,
) -> bool {
    if md.accuracy == 0 { return true; }

    let def_side = 1 - atk_side;
    let atk_ability = effective_ability(state, atk_side);
    let def_ability = effective_ability(state, def_side);

    if atk_ability == data_bridge::ABILITY_NO_GUARD
        || def_ability == data_bridge::ABILITY_NO_GUARD
    {
        return true;
    }

    let acc_idx = (state.sides[atk_side].active.boosts[ACC] + 6) as usize;
    let eva_idx = (state.sides[def_side].active.boosts[EVA] + 6) as usize;

    let mut accuracy = md.accuracy as u32
        * ACC_NUM[acc_idx] / ACC_DEN[acc_idx]
        * ACC_DEN[eva_idx] / ACC_NUM[eva_idx];

    if atk_ability == data_bridge::ABILITY_COMPOUND_EYES { accuracy = accuracy * 13 / 10; }
    if atk_ability == data_bridge::ABILITY_HUSTLE && md.category == MoveCategory::Physical {
        accuracy = accuracy * 4 / 5;
    }
    if atk_ability == data_bridge::ABILITY_VICTORY_STAR { accuracy = accuracy * 11 / 10; }

    if def_ability == data_bridge::ABILITY_SAND_VEIL && state.field.weather == WEATHER_SAND {
        accuracy = accuracy * 4 / 5;
    }
    if def_ability == data_bridge::ABILITY_SNOW_CLOAK && state.field.weather == WEATHER_SNOW {
        accuracy = accuracy * 4 / 5;
    }

    if data_bridge::item(state.active_mon(atk_side).item_id).has(ItemFlag::WIDE_LENS) {
        accuracy = accuracy * 11 / 10;
    }

    if state.field.gravity_turns > 0 { accuracy = accuracy * 5 / 3; }

    rng(100) < accuracy
}

// ── Secondary effect application ────────────────────────────────────

#[inline]
fn apply_secondary(
    state: &mut BattleState,
    keys: &ZobristKeys,
    atk_side: usize,
    def_side: usize,
    md: &MoveData,
    rng: &mut impl FnMut(u32) -> u32,
) {
    if md.secondary_chance == 0 { return; }

    let atk_ability = effective_ability(state, atk_side);

    // Sheer Force: secondary suppressed (power boost already applied in calc)
    if atk_ability == data_bridge::ABILITY_SHEER_FORCE { return; }

    let mut chance = md.secondary_chance as u32;
    if atk_ability == data_bridge::ABILITY_SERENE_GRACE { chance = (chance * 2).min(100); }

    if rng(100) >= chance { return; }

    let def_slot = state.sides[def_side].active_index as usize;

    // ── Stat changes (secondary_stat != 0) ──────────────────────
    if md.secondary_stat > 0 {
        let stat = if md.category == MoveCategory::Physical { ATK } else { SPA };
        apply_boost(state, keys, atk_side, stat, md.secondary_stat as i8);
        return;
    }
    if md.secondary_stat < 0 {
        let stat = if md.category == MoveCategory::Physical { DEF } else { SPD };
        apply_boost(state, keys, def_side, stat, md.secondary_stat as i8);
        return;
    }

    // ── Status effect ───────────────────────────────────────────
    // Prefer the explicit field (covers Scald→burn, Body Slam→paralysis).
    // Fall back to type heuristic for moves where the field isn't populated.
    let status = if md.secondary_status != STATUS_NONE {
        md.secondary_status
    } else {
        match md.move_type {
            Type::Fire     => STATUS_BURN,
            Type::Electric => STATUS_PARALYSIS,
            Type::Ice      => STATUS_FREEZE,
            Type::Poison   => STATUS_POISON,
            _ => STATUS_NONE,
        }
    };

    if status != STATUS_NONE {
        set_status(state, keys, def_side, def_slot, status, 0);
        return;
    }

    // ── Flinch (Rock Slide, Iron Head, Air Slash, etc.) ─────────
    // Only works if the defender hasn't moved yet this turn.
    if !state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN) {
        set_volatile(state, keys, def_side, VOL_FLINCHED);
    }
}

// ── Status move dispatch ────────────────────────────────────────────

fn execute_status_move(
    state: &mut BattleState,
    keys: &ZobristKeys,
    atk_side: usize,
    def_side: usize,
    md: &MoveData,
    rng: &mut impl FnMut(u32) -> u32,
) {
    let atk_slot = state.sides[atk_side].active_index as usize;
    let def_slot = state.sides[def_side].active_index as usize;

    // ── Protect / Detect ─────────────────────────────────────────
    if md.effect == MoveEffect::Protect {
        execute_protect(state, keys, atk_side, rng);
        return;
    }

    // ── Recovery (HEAL flag) ─────────────────────────────────────
    if md.flags & MoveFlags::HEAL != 0 {
        let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
        heal(state, keys, atk_side, atk_slot, max_hp / 2);
        return;
    }

    // ── Targeting, Protect blocking, accuracy ────────────────────
    let targets_self = is_self_targeting(md);

    if !targets_self && state.sides[def_side].active.has_volatile(VOL_PROTECT_THIS_TURN) {
        return;
    }
    if !targets_self && md.accuracy != 0 && !accuracy_check(state, atk_side, md, rng) {
        return;
    }

    // ── Dispatch on MoveEffect ───────────────────────────────────
    match md.effect {
        // -- Hazard setters --
        MoveEffect::StealthRock => { set_stealth_rock(state, def_side); }
        MoveEffect::Spikes      => { add_spikes(state, def_side); }
        MoveEffect::ToxicSpikes => { add_toxic_spikes(state, def_side); }
        MoveEffect::StickyWeb   => { set_sticky_web(state, def_side); }

        // -- Hazard removal --
        MoveEffect::Defog => {
            clear_hazards(state, def_side);
            clear_hazards(state, atk_side);
            state.sides[def_side].side_conditions.reflect_turns = 0;
            state.sides[def_side].side_conditions.light_screen_turns = 0;
            state.sides[def_side].side_conditions.aurora_veil_turns = 0;
        }

        // -- Status infliction --
        MoveEffect::WillOWisp   => { set_status(state, keys, def_side, def_slot, STATUS_BURN, 0); }
        MoveEffect::ThunderWave => { set_status(state, keys, def_side, def_slot, STATUS_PARALYSIS, 0); }
        MoveEffect::Toxic       => { set_status(state, keys, def_side, def_slot, STATUS_BAD_POISON, 0); }
        MoveEffect::Sleep       => {
            let turns = (rng(3) + 1) as u8;
            set_status(state, keys, def_side, def_slot, STATUS_SLEEP, turns);
        }

        // -- Self-boosts --
        MoveEffect::SwordsDance => { apply_boost(state, keys, atk_side, ATK, 2); }
        MoveEffect::NastyPlot   => { apply_boost(state, keys, atk_side, SPA, 2); }
        MoveEffect::DragonDance => {
            apply_boost(state, keys, atk_side, ATK, 1);
            apply_boost(state, keys, atk_side, SPE, 1);
        }
        MoveEffect::CalmMind => {
            apply_boost(state, keys, atk_side, SPA, 1);
            apply_boost(state, keys, atk_side, SPD, 1);
        }
        MoveEffect::BulkUp => {
            apply_boost(state, keys, atk_side, ATK, 1);
            apply_boost(state, keys, atk_side, DEF, 1);
        }
        MoveEffect::IronDefense => { apply_boost(state, keys, atk_side, DEF, 2); }
        MoveEffect::Agility     => { apply_boost(state, keys, atk_side, SPE, 2); }
        MoveEffect::QuiverDance => {
            apply_boost(state, keys, atk_side, SPA, 1);
            apply_boost(state, keys, atk_side, SPD, 1);
            apply_boost(state, keys, atk_side, SPE, 1);
        }
        MoveEffect::ShellSmash => {
            apply_boost(state, keys, atk_side, ATK, 2);
            apply_boost(state, keys, atk_side, SPA, 2);
            apply_boost(state, keys, atk_side, SPE, 2);
            apply_boost(state, keys, atk_side, DEF, -1);
            apply_boost(state, keys, atk_side, SPD, -1);
        }
        MoveEffect::Coil => {
            apply_boost(state, keys, atk_side, ATK, 1);
            apply_boost(state, keys, atk_side, DEF, 1);
            apply_boost(state, keys, atk_side, ACC, 1);
        }
        MoveEffect::ShiftGear => {
            apply_boost(state, keys, atk_side, ATK, 1);
            apply_boost(state, keys, atk_side, SPE, 2);
        }

        // -- Screens --
        MoveEffect::Reflect => {
            state.sides[atk_side].side_conditions.reflect_turns = screen_duration(state, atk_side);
        }
        MoveEffect::LightScreen => {
            state.sides[atk_side].side_conditions.light_screen_turns = screen_duration(state, atk_side);
        }
        MoveEffect::AuroraVeil => {
            if state.field.weather == WEATHER_SNOW {
                state.sides[atk_side].side_conditions.aurora_veil_turns = screen_duration(state, atk_side);
            }
        }

        // -- Field effects --
        MoveEffect::Tailwind => {
            state.sides[atk_side].side_conditions.tailwind_turns = 4;
        }
        MoveEffect::TrickRoom => {
            if state.field.trick_room_turns > 0 {
                set_trick_room(state, keys, 0);
            } else {
                set_trick_room(state, keys, 5);
            }
        }

        // -- Substitute --
        MoveEffect::Substitute => {
            let cost = state.sides[atk_side].team[atk_slot].max_hp / 4;
            if state.sides[atk_side].team[atk_slot].current_hp > cost
                && !state.sides[atk_side].active.has_volatile(VOL_SUBSTITUTE)
            {
                deal_damage(state, keys, atk_side, atk_slot, cost);
                state.sides[atk_side].active.substitute_hp = cost;
                set_volatile(state, keys, atk_side, VOL_SUBSTITUTE);
            }
        }

        // -- Wish --
        MoveEffect::Wish => {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            state.sides[atk_side].side_conditions.wish_hp = max_hp / 2;
            state.sides[atk_side].side_conditions.wish_turns = 2;
        }

        // -- Taunt --
        MoveEffect::Taunt => {
            state.sides[def_side].active.taunt_turns = 3;
        }

        // -- Leech Seed --
        MoveEffect::LeechSeed => {
            if !has_type(state, def_side, Type::Grass as u8) {
                set_volatile(state, keys, def_side, VOL_LEECH_SEED);
            }
        }

        // -- Encore --
        MoveEffect::Encore => {
            let last = state.sides[def_side].active.last_move;
            if last != 0 {
                state.sides[def_side].active.encore_move = last;
                state.sides[def_side].active.encore_turns = 3;
            }
        }

        // -- Fallback for MoveEffect::None and damaging effects --
        _ => {
            if md.secondary_stat > 0 {
                apply_boost(state, keys, atk_side, ATK, md.secondary_stat as i8);
            } else if md.secondary_stat < 0 {
                apply_boost(state, keys, def_side, ATK, md.secondary_stat as i8);
            }
        }
    }
}

/// Returns true if a status move targets self / field / own side
/// (not blocked by Protect, doesn't miss).
#[inline]
fn is_self_targeting(md: &MoveData) -> bool {
    match md.effect {
        // Self-boost moves
        MoveEffect::SwordsDance | MoveEffect::NastyPlot | MoveEffect::DragonDance |
        MoveEffect::CalmMind | MoveEffect::BulkUp | MoveEffect::IronDefense |
        MoveEffect::Agility | MoveEffect::QuiverDance | MoveEffect::ShellSmash |
        MoveEffect::Coil | MoveEffect::ShiftGear => true,

        // Screens (set on own side)
        MoveEffect::Reflect | MoveEffect::LightScreen | MoveEffect::AuroraVeil => true,

        // Field / own side
        MoveEffect::Tailwind | MoveEffect::TrickRoom => true,

        // Utility targeting self/own side
        MoveEffect::Substitute | MoveEffect::Wish => true,

        // Hazard setters target the opponent's side, not the active mon.
        // They're not blocked by the opponent's Protect.
        MoveEffect::StealthRock | MoveEffect::Spikes |
        MoveEffect::ToxicSpikes | MoveEffect::StickyWeb => true,

        // Fallback: secondary_stat > 0 implies self-boost
        MoveEffect::None => md.secondary_stat > 0,

        _ => false,
    }
}

/// Screen duration: 5 turns, or 8 with Light Clay.
#[inline]
fn screen_duration(state: &BattleState, side: usize) -> u8 {
    if data_bridge::item(state.active_mon(side).item_id).has(ItemFlag::EXTENDS_SCREENS) { 8 } else { 5 }
}

// ── Protect logic ───────────────────────────────────────────────────

#[inline]
fn execute_protect(
    state: &mut BattleState,
    keys: &ZobristKeys,
    side: usize,
    rng: &mut impl FnMut(u32) -> u32,
) {
    let consecutive = state.sides[side].active.protect_consecutive;
    let succeeds = match consecutive {
        0 => true,
        1 => rng(3) == 0,
        2 => rng(9) == 0,
        _ => false,
    };
    if succeeds {
        set_volatile(state, keys, side, VOL_PROTECT_THIS_TURN);
        state.sides[side].active.protect_consecutive += 1;
    }
}

// ── Main move execution ─────────────────────────────────────────────

/// Execute a single move.
///
/// `move_id`: the actual move ID (from `effective_moves`), or 0 for Struggle.
/// `move_slot`: 0-3 index for PP deduction.
pub fn execute_move(
    state: &mut BattleState,
    keys: &ZobristKeys,
    atk_side: usize,
    move_id: u16,
    move_slot: u8,
    rng: &mut impl FnMut(u32) -> u32,
) {
    let def_side = 1 - atk_side;
    let atk_slot = state.sides[atk_side].active_index as usize;
    let def_slot = state.sides[def_side].active_index as usize;
    let is_struggle = move_id == 0;
    let md = data_bridge::move_hot(move_id);

    // ── Pre-move checks ─────────────────────────────────────────

    if state.sides[atk_side].team[atk_slot].is_fainted() { return; }

    if state.sides[atk_side].active.has_volatile(VOL_RECHARGING) {
        clear_volatile(state, keys, atk_side, VOL_RECHARGING);
        return;
    }

    if state.sides[atk_side].active.has_volatile(VOL_FLINCHED) { return; }

    if state.sides[atk_side].team[atk_slot].status == STATUS_PARALYSIS {
        if rng(4) == 0 { return; }
    }

    // Sleep: decrement counter, fail unless waking up.
    // This is the ONLY place the sleep counter is decremented (not in end_of_turn).
    if state.sides[atk_side].team[atk_slot].status == STATUS_SLEEP {
        let counter = state.sides[atk_side].team[atk_slot].status_counter;
        if counter > 0 {
            state.sides[atk_side].team[atk_slot].status_counter = counter - 1;
            if counter > 1 { return; }
            clear_status(state, keys, atk_side, atk_slot);
        }
    }

    // Freeze: 20% thaw, fire moves always thaw
    if state.sides[atk_side].team[atk_slot].status == STATUS_FREEZE {
        if md.move_type == Type::Fire || rng(5) == 0 {
            clear_status(state, keys, atk_side, atk_slot);
        } else {
            return;
        }
    }

    // Confusion: 33% self-hit
    if state.sides[atk_side].active.confusion_turns > 0 {
        state.sides[atk_side].active.confusion_turns -= 1;
        if rng(3) == 0 {
            let a = boosted_stat(
                effective_stat(state, atk_side, ATK),
                state.sides[atk_side].active.boosts[ATK],
            ) as u32;
            let d = boosted_stat(
                effective_stat(state, atk_side, DEF),
                state.sides[atk_side].active.boosts[DEF],
            ).max(1) as u32;
            let dmg = ((42u32 * 40 * a / d) / 50 + 2) as u16;
            deal_damage(state, keys, atk_side, atk_slot, dmg);
            return;
        }
    }

    // ── PP and bookkeeping ──────────────────────────────────────

    if !is_struggle {
        deduct_pp(state, atk_side, move_slot as usize, 1);
    }

    {
        let active = &mut state.sides[atk_side].active;
        if active.last_move == move_id && move_id != 0 {
            active.consec_move_count = active.consec_move_count.saturating_add(1);
        } else {
            active.consec_move_count = 1;
        }
        active.last_move = move_id;
    }

    set_volatile(state, keys, atk_side, VOL_MOVED_THIS_TURN);

    if !is_struggle {
        let atk_item = data_bridge::item(state.active_mon(atk_side).item_id);
        if atk_item.has(ItemFlag::IS_CHOICE)
            && state.sides[atk_side].active.choice_locked_move == 0
        {
            state.sides[atk_side].active.choice_locked_move = move_id;
        }
    }

    // ── Status moves (Struggle is always damaging, skip) ────────

    if !is_struggle && md.category == MoveCategory::Status {
        execute_status_move(state, keys, atk_side, def_side, md, rng);
        return;
    }

    // ── Protect check (damaging moves) ──────────────────────────

    if state.sides[def_side].active.has_volatile(VOL_PROTECT_THIS_TURN) {
        return;
    }

    // ── Accuracy check (Struggle always hits) ───────────────────

    if !is_struggle && !accuracy_check(state, atk_side, md, rng) {
        return;
    }

    // ── Damage calculation ──────────────────────────────────────

    let result = calc_damage(state, atk_side, move_id, rng);

    if result.type_immune { return; }

    // ── Apply damage ────────────────────────────────────────────

    if result.hits_substitute {
        let sub = &mut state.sides[def_side].active.substitute_hp;
        *sub = sub.saturating_sub(result.damage);
        if *sub == 0 {
            clear_volatile(state, keys, def_side, VOL_SUBSTITUTE);
        }
    } else {
        deal_damage(state, keys, def_side, def_slot, result.damage);
        state.sides[def_side].active.last_move_hit_by = move_id;
    }

    // ── Drain / recoil ──────────────────────────────────────────

    if result.drain_heal > 0 {
        heal(state, keys, atk_side, atk_slot, result.drain_heal);
    }
    if result.recoil_damage > 0 {
        deal_damage(state, keys, atk_side, atk_slot, result.recoil_damage);
    }

    // ── Item consumption ────────────────────────────────────────

    if result.item_consumed {
        let atk_itm = data_bridge::item(state.active_mon(atk_side).item_id);
        if atk_itm.has(ItemFlag::GEM) {
            consume_item(state, keys, atk_side, atk_slot);
            if effective_ability(state, atk_side) == data_bridge::ABILITY_UNBURDEN {
                set_volatile(state, keys, atk_side, VOL_UNBURDEN);
            }
        }
        let def_itm = data_bridge::item(state.active_mon(def_side).item_id);
        if def_itm.has(ItemFlag::RESIST_BERRY) {
            consume_item(state, keys, def_side, def_slot);
            if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                set_volatile(state, keys, def_side, VOL_UNBURDEN);
            }
        }
    }

    // ── Secondary effects ───────────────────────────────────────

    if !state.sides[def_side].team[def_slot].is_fainted() {
        apply_secondary(state, keys, atk_side, def_side, md, rng);
    }

    // ── Contact aftermath ───────────────────────────────────────

    if md.flags & MoveFlags::CONTACT != 0
        && !state.sides[atk_side].team[atk_slot].is_fainted()
        && !state.sides[def_side].team[def_slot].is_fainted()
    {
        let def_itm = data_bridge::item(state.active_mon(def_side).item_id);
        if def_itm.has(ItemFlag::ROCKY_HELMET) {
            let atk_max = state.active_mon(atk_side).max_hp;
            deal_damage(state, keys, atk_side, atk_slot, atk_max / 6);
        }
    }

    // ── Recharge ────────────────────────────────────────────────

    if md.flags & MoveFlags::RECHARGE != 0 {
        set_volatile(state, keys, atk_side, VOL_RECHARGING);
    }

    // ── Rapid Spin: clear own hazards + Speed boost ─────────────

    if md.effect == MoveEffect::RapidSpin {
        clear_hazards(state, atk_side);
        apply_boost(state, keys, atk_side, SPE, 1);
    }

    // ── Force switch (U-turn, Volt Switch, Flip Turn) ───────────

    if md.effect == MoveEffect::ForceSwitch
        && !state.sides[atk_side].team[atk_slot].is_fainted()
    {
        let has_bench = (0..6).any(|i| {
            i != atk_slot
                && state.sides[atk_side].team[i].species_id != 0
                && state.sides[atk_side].team[i].current_hp > 0
        });
        if has_bench {
            set_volatile(state, keys, atk_side, VOL_MUST_SWITCH);
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::zobrist::{compute_full_hash, validate_hash};

    fn fixed_rng(val: u32) -> impl FnMut(u32) -> u32 { move |max| val % max }

    fn setup() -> (BattleState, ZobristKeys) {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [150, 100, 150, 100, 100],
            moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
            ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [100, 100, 100, 100, 80],
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 300, max_hp: 300,
            stats: [100, 100, 100, 100, 80],
            moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
            ..Default::default()
        };
        state.sides[1].team[1] = MonSlot {
            species_id: 10, current_hp: 200, max_hp: 200,
            stats: [80, 80, 80, 80, 60],
            ..Default::default()
        };
        state.phase = PHASE_ACTIONS;
        state.zobrist = compute_full_hash(&state, &keys);
        (state, keys)
    }

    #[test]
    fn test_accuracy_always_hit() {
        let (state, _) = setup();
        let md = MoveData { accuracy: 0, ..unsafe { core::mem::zeroed() } };
        assert!(accuracy_check(&state, 0, &md, &mut fixed_rng(99)));
    }

    #[test]
    fn test_accuracy_miss() {
        let (state, _) = setup();
        let md = MoveData { accuracy: 50, ..unsafe { core::mem::zeroed() } };
        assert!(!accuracy_check(&state, 0, &md, &mut fixed_rng(99)));
    }

    #[test]
    fn test_evasion_stages() {
        let (mut state, _) = setup();
        state.sides[1].active.boosts[EVA] = 2;
        let md = MoveData { accuracy: 100, ..unsafe { core::mem::zeroed() } };
        assert!(accuracy_check(&state, 0, &md, &mut fixed_rng(59)));
        assert!(!accuracy_check(&state, 0, &md, &mut fixed_rng(60)));
    }

    #[test]
    fn test_full_paralysis() {
        let (mut state, keys) = setup();
        state.sides[0].team[0].status = STATUS_PARALYSIS;
        state.zobrist = compute_full_hash(&state, &keys);
        let pp_before = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].pp[0], pp_before);
    }

    #[test]
    fn test_sleep_wake() {
        let (mut state, keys) = setup();
        state.sides[0].team[0].status = STATUS_SLEEP;
        state.sides[0].team[0].status_counter = 2;
        state.zobrist = compute_full_hash(&state, &keys);
        // Turn 1: counter 2→1, still asleep
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].status, STATUS_SLEEP);
        assert_eq!(state.sides[0].team[0].status_counter, 1);
        // Turn 2: counter 1→0, wake up
        state.zobrist = compute_full_hash(&state, &keys);
        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
        assert_eq!(state.sides[0].team[0].pp[0], pp - 1);
    }

    #[test]
    fn test_confusion_self_hit() {
        let (mut state, keys) = setup();
        state.sides[0].active.confusion_turns = 3;
        state.zobrist = compute_full_hash(&state, &keys);
        let hp = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));
        assert!(state.sides[0].team[0].current_hp < hp);
    }

    #[test]
    fn test_protect() {
        let (mut state, keys) = setup();
        execute_protect(&mut state, &keys, 1, &mut fixed_rng(0));
        assert!(state.sides[1].active.has_volatile(VOL_PROTECT_THIS_TURN));
        let hp = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[1].team[0].current_hp, hp);
    }

    #[test]
    fn test_recharging_skips() {
        let (mut state, keys) = setup();
        set_volatile(&mut state, &keys, 0, VOL_RECHARGING);
        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));
        assert!(!state.sides[0].active.has_volatile(VOL_RECHARGING));
        assert_eq!(state.sides[0].team[0].pp[0], pp);
    }

    #[test]
    fn test_is_self_targeting() {
        let none_md = MoveData { effect: MoveEffect::None, ..unsafe { core::mem::zeroed() } };
        let sd_md = MoveData { effect: MoveEffect::SwordsDance, ..unsafe { core::mem::zeroed() } };
        let ww_md = MoveData { effect: MoveEffect::WillOWisp, ..unsafe { core::mem::zeroed() } };
        assert!(!is_self_targeting(&none_md));
        assert!(is_self_targeting(&sd_md));
        assert!(!is_self_targeting(&ww_md));
    }
}
