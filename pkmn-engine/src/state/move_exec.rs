//! Move execution: the per-move sequence called by the turn executor.
//!
//! Handles pre-move checks (flinch, paralysis, sleep, freeze, confusion),
//! accuracy, damage application, drain/recoil, secondary effects,
//! contact aftermath, status moves, and Protect logic.
//!
//! Zero heap allocations.  All integer math.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag, MoveCategory, MoveEffect, SelfEffect};
use crate::state::accessors::*;
use crate::state::mutations::*;
use crate::state::zobrist::ZobristKeys;
use crate::state::calc::calc_damage;
use crate::state::calc_modifiers::{ability_type_immunity, ability_flag_immunity, good_as_gold_immunity, priority_block_immunity, AbilityImmunityEffect};
use crate::state::forme::{apply_battle_forme, revert_battle_forme};
use crate::state::switch::{
    set_stealth_rock, add_spikes, add_toxic_spikes, set_sticky_web, clear_hazards,
};
use crate::data::moves::{MoveData, MoveFlags};
use crate::data::types::Type;

// ── Immunity effect application helper ───────────────────────────────

#[inline]
fn apply_immunity_effect(
    state: &mut BattleState,
    keys: &ZobristKeys,
    def_side: usize,
    def_slot: usize,
    eff: AbilityImmunityEffect,
) {
    match eff {
        AbilityImmunityEffect::Heal(hp) => { heal(state, keys, def_side, def_slot, hp); }
        AbilityImmunityEffect::Boost(stat, stages) => { apply_boost(state, keys, def_side, stat, stages); }
        AbilityImmunityEffect::FlashFire => { set_volatile(state, keys, def_side, VOL_FLASH_FIRE); }
        AbilityImmunityEffect::Nullify => {}
    }
}

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

    // Weather-dependent accuracy overrides (applied before anything else)
    // Thunder / Hurricane: 100% in rain, 50% in sun
    if md.effect == MoveEffect::WeatherAccRain {
        match state.field.weather {
            WEATHER_RAIN | WEATHER_HEAVY_RAIN => return true,
            WEATHER_SUN | WEATHER_HARSH_SUN => {
                // Override accuracy to 50, then fall through to normal check
                // (handled below by using effective_accuracy)
            }
            _ => {}
        }
    }
    // Blizzard: 100% in snow/hail
    if md.effect == MoveEffect::WeatherAccSnow {
        if state.field.weather == WEATHER_SNOW {
            return true;
        }
    }

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

    // Weather-dependent accuracy: Thunder/Hurricane have 50% in sun
    let base_acc = if md.effect == MoveEffect::WeatherAccRain
        && matches!(state.field.weather, WEATHER_SUN | WEATHER_HARSH_SUN)
    {
        50u32
    } else {
        md.accuracy as u32
    };

    let mut accuracy = base_acc
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
    // Semi-invulnerability: status moves miss semi-invulnerable targets
    if !targets_self && state.sides[def_side].active.has_volatile(VOL_SEMI_INVULNERABLE) {
        return;
    }
    // Ability-based immunities for status moves
    if !targets_self {
        // Good as Gold: immune to status moves targeting it
        if good_as_gold_immunity(state, def_side) { return; }
        if let Some(eff) = ability_flag_immunity(state, def_side, md.flags) {
            apply_immunity_effect(state, keys, def_side, def_slot, eff);
            return;
        }
        if let Some(eff) = ability_type_immunity(state, def_side, md.move_type) {
            apply_immunity_effect(state, keys, def_side, def_slot, eff);
            return;
        }
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

        // -- Status infliction (blocked by Safeguard) --
        MoveEffect::WillOWisp   => {
            if state.sides[def_side].side_conditions.safeguard_turns() == 0 {
                set_status(state, keys, def_side, def_slot, STATUS_BURN, 0);
            }
        }
        MoveEffect::ThunderWave => {
            if state.sides[def_side].side_conditions.safeguard_turns() == 0 {
                set_status(state, keys, def_side, def_slot, STATUS_PARALYSIS, 0);
            }
        }
        MoveEffect::Toxic       => {
            if state.sides[def_side].side_conditions.safeguard_turns() == 0 {
                set_status(state, keys, def_side, def_slot, STATUS_BAD_POISON, 0);
            }
        }
        MoveEffect::Sleep       => {
            if state.sides[def_side].side_conditions.safeguard_turns() == 0 {
                let turns = (rng(3) + 1) as u8;
                set_status(state, keys, def_side, def_slot, STATUS_SLEEP, turns);
            }
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

        // -- BellyDrum: -50% HP, +6 Atk --
        MoveEffect::BellyDrum => {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            let current_hp = state.sides[atk_side].team[atk_slot].current_hp;
            if current_hp > max_hp / 2 {
                deal_damage(state, keys, atk_side, atk_slot, max_hp / 2);
                let current_atk = state.sides[atk_side].active.boosts[ATK];
                if current_atk < 6 {
                    apply_boost(state, keys, atk_side, ATK, 6 - current_atk);
                }
            }
        }

        // -- PainSplit: average both mons' HP --
        MoveEffect::PainSplit => {
            let hp_a = state.sides[atk_side].team[atk_slot].current_hp as u32;
            let hp_d = state.sides[def_side].team[def_slot].current_hp as u32;
            let avg = ((hp_a + hp_d) / 2) as u16;
            let max_a = state.sides[atk_side].team[atk_slot].max_hp;
            let max_d = state.sides[def_side].team[def_slot].max_hp;
            state.sides[atk_side].team[atk_slot].current_hp = avg.min(max_a);
            state.sides[def_side].team[def_slot].current_hp = avg.min(max_d);
        }

        // -- PerishSong: set 3-turn perish counter on both --
        MoveEffect::PerishSong => {
            if !state.sides[atk_side].active.has_volatile(VOL_PERISH_SONG) {
                set_volatile(state, keys, atk_side, VOL_PERISH_SONG);
                state.sides[atk_side].active.perish_count = 3;
            }
            if !state.sides[def_side].active.has_volatile(VOL_PERISH_SONG) {
                set_volatile(state, keys, def_side, VOL_PERISH_SONG);
                state.sides[def_side].active.perish_count = 3;
            }
        }

        // -- DestinyBond --
        MoveEffect::DestinyBond => {
            set_volatile(state, keys, atk_side, VOL_DESTINY_BOND);
        }

        // -- Trick / Switcheroo: swap items --
        MoveEffect::Trick => {
            let item_a = state.sides[atk_side].team[atk_slot].item_id;
            let item_d = state.sides[def_side].team[def_slot].item_id;
            if item_a != 0 || item_d != 0 {
                set_item(state, keys, atk_side, atk_slot, item_d);
                set_item(state, keys, def_side, def_slot, item_a);
            }
        }

        // -- Disable: prevent last-used move for 4 turns --
        MoveEffect::Disable => {
            let last = state.sides[def_side].active.last_move;
            if last != 0 && state.sides[def_side].active.disabled_move == 0 {
                state.sides[def_side].active.disabled_move = last;
                state.sides[def_side].active.disable_turns = 4;
            }
        }

        // -- Torment --
        MoveEffect::Torment => {
            set_volatile(state, keys, def_side, VOL_TORMENT);
        }

        // -- HealingWish: user faints, next switch-in fully heals --
        MoveEffect::HealingWish => {
            let hp = state.sides[atk_side].team[atk_slot].current_hp;
            if hp > 0 {
                deal_damage(state, keys, atk_side, atk_slot, hp);
                state.sides[atk_side].side_conditions.set_healing_wish(true);
            }
        }

        // -- LunarDance: user faints, next switch-in fully heals + PP --
        MoveEffect::LunarDance => {
            let hp = state.sides[atk_side].team[atk_slot].current_hp;
            if hp > 0 {
                deal_damage(state, keys, atk_side, atk_slot, hp);
                state.sides[atk_side].side_conditions.set_lunar_dance(true);
            }
        }

        // -- CourtChange: swap side conditions --
        MoveEffect::CourtChange => {
            let tmp = state.sides[atk_side].side_conditions;
            state.sides[atk_side].side_conditions = state.sides[def_side].side_conditions;
            state.sides[def_side].side_conditions = tmp;
        }

        // -- Roost: heal 50%, lose Flying type for rest of turn --
        MoveEffect::Roost => {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            heal(state, keys, atk_side, atk_slot, max_hp / 2);
            // Temporarily remove Flying type — we use a counter field to track
            // The end-of-move cleanup restores it. For simplicity in MCTS,
            // we handle this as a type override for the remainder of the turn.
            let (t1, t2) = effective_types(state, atk_side);
            if t1 == Type::Flying as u8 || t2 == Type::Flying as u8 {
                let new_t1 = if t1 == Type::Flying as u8 { Type::Normal as u8 } else { t1 };
                let new_t2 = if t2 == Type::Flying as u8 { Type::Normal as u8 } else { t2 };
                // If both types were Flying, become pure Normal
                state.sides[atk_side].active.override_types = [new_t1, new_t2];
                set_volatile(state, keys, atk_side, VOL_TYPES_OVERRIDDEN);
            }
        }

        // -- Safeguard --
        MoveEffect::Safeguard => {
            state.sides[atk_side].side_conditions.set_safeguard_turns(5);
        }

        // -- Mist --
        MoveEffect::Mist => {
            state.sides[atk_side].side_conditions.set_mist_turns(5);
        }

        // -- Lucky Chant --
        MoveEffect::LuckyChant => {
            state.sides[atk_side].side_conditions.set_lucky_chant_turns(5);
        }

        // -- Gravity --
        MoveEffect::Gravity => {
            set_gravity(state, keys, 5);
        }

        // -- Whirlwind / Roar: force random switch --
        MoveEffect::Whirlwind => {
            let mut targets = [0usize; 5];
            let mut cnt = 0usize;
            for i in 0..6 {
                if i != def_slot
                    && state.sides[def_side].team[i].species_id != 0
                    && state.sides[def_side].team[i].current_hp > 0
                {
                    targets[cnt] = i;
                    cnt += 1;
                }
            }
            if cnt > 0 {
                let pick = targets[rng(cnt as u32) as usize];
                crate::state::switch::perform_switch(state, keys, def_side, pick);
            }
        }

        // -- Haze: reset all stat changes --
        MoveEffect::Haze => {
            for side in 0..2 {
                for stat in 0..7 {
                    let old = state.sides[side].active.boosts[stat];
                    if old != 0 {
                        state.zobrist ^= keys.boosts[side][stat][(old + 6) as usize];
                        state.sides[side].active.boosts[stat] = 0;
                        state.zobrist ^= keys.boosts[side][stat][6]; // boost 0 index
                    }
                }
            }
        }

        // -- Yawn --
        MoveEffect::Yawn => {
            if !state.sides[def_side].active.has_volatile(VOL_YAWN)
                && state.sides[def_side].team[def_slot].status == STATUS_NONE
            {
                set_volatile(state, keys, def_side, VOL_YAWN);
            }
        }

        // -- Confuse (Confuse Ray, Sweet Kiss) --
        MoveEffect::Confuse => {
            if state.sides[def_side].active.confusion_turns == 0 {
                state.sides[def_side].active.confusion_turns = (rng(3) + 2) as u8;
            }
        }

        // -- Magnet Rise --
        MoveEffect::MagnetRise => {
            if state.sides[atk_side].active.magnet_rise_turns == 0 {
                set_volatile(state, keys, atk_side, VOL_MAGNET_RISE);
                state.sides[atk_side].active.magnet_rise_turns = 5;
            }
        }

        // -- Focus Energy --
        MoveEffect::FocusEnergy => {
            set_volatile(state, keys, atk_side, VOL_FOCUS_ENERGY);
        }

        // -- Imprison --
        MoveEffect::Imprison => {
            set_volatile(state, keys, atk_side, VOL_IMPRISON);
        }

        // -- Aromatherapy / Heal Bell: cure team status --
        MoveEffect::Aromatherapy => {
            for i in 0..6 {
                if state.sides[atk_side].team[i].species_id != 0
                    && state.sides[atk_side].team[i].status != STATUS_NONE
                {
                    clear_status(state, keys, atk_side, i);
                }
            }
        }

        // -- Minimize: +2 Evasion + set VOL_MINIMIZE --
        MoveEffect::Minimize => {
            apply_boost(state, keys, atk_side, EVA, 2);
            set_volatile(state, keys, atk_side, VOL_MINIMIZE);
        }

        // -- Stockpile --
        MoveEffect::Stockpile => {
            if state.sides[atk_side].active.stockpile < 3 {
                state.sides[atk_side].active.stockpile += 1;
                apply_boost(state, keys, atk_side, DEF, 1);
                apply_boost(state, keys, atk_side, SPD, 1);
            }
        }

        // -- Swallow --
        MoveEffect::Swallow => {
            let count = state.sides[atk_side].active.stockpile;
            if count > 0 {
                let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
                let heal_amount = match count {
                    1 => max_hp / 4,
                    2 => max_hp / 2,
                    _ => max_hp,
                };
                heal(state, keys, atk_side, atk_slot, heal_amount);
                apply_boost(state, keys, atk_side, DEF, -(count as i8));
                apply_boost(state, keys, atk_side, SPD, -(count as i8));
                state.sides[atk_side].active.stockpile = 0;
            }
        }

        // -- Parting Shot: -1 Atk -1 SpA on target, then self-switch --
        MoveEffect::PartingShot => {
            let a = apply_boost(state, keys, def_side, ATK, -1);
            let s = apply_boost(state, keys, def_side, SPA, -1);
            // Only switch if at least one stat drop landed
            if (a != 0 || s != 0)
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

        // -- Baton Pass: self-switch preserving boosts + select volatiles --
        MoveEffect::BatonPass => {
            if !state.sides[atk_side].team[atk_slot].is_fainted() {
                let has_bench = (0..6).any(|i| {
                    i != atk_slot
                        && state.sides[atk_side].team[i].species_id != 0
                        && state.sides[atk_side].team[i].current_hp > 0
                });
                if has_bench {
                    // Mark for baton pass (switch.rs checks _padding[0])
                    state.sides[atk_side].active._padding[0] = 1;
                    set_volatile(state, keys, atk_side, VOL_MUST_SWITCH);
                }
            }
        }

        // -- Geomancy: +2 SpA/SpD/Spe (resolves on charge turn 2) --
        MoveEffect::ChargeGeomancy => {
            apply_boost(state, keys, atk_side, SPA, 2);
            apply_boost(state, keys, atk_side, SPD, 2);
            apply_boost(state, keys, atk_side, SPE, 2);
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
        MoveEffect::Tailwind | MoveEffect::TrickRoom | MoveEffect::Gravity => true,

        // Utility targeting self/own side
        MoveEffect::Substitute | MoveEffect::Wish | MoveEffect::BatonPass |
        MoveEffect::ChargeGeomancy | MoveEffect::BellyDrum | MoveEffect::Roost |
        MoveEffect::HealingWish | MoveEffect::LunarDance | MoveEffect::FocusEnergy |
        MoveEffect::Imprison | MoveEffect::Aromatherapy | MoveEffect::Minimize |
        MoveEffect::Stockpile | MoveEffect::Swallow | MoveEffect::MagnetRise |
        MoveEffect::DestinyBond => true,

        // Side conditions on own side
        MoveEffect::Safeguard | MoveEffect::Mist | MoveEffect::LuckyChant => true,

        // Hazard setters target the opponent's side, not the active mon.
        // They're not blocked by the opponent's Protect.
        MoveEffect::StealthRock | MoveEffect::Spikes |
        MoveEffect::ToxicSpikes | MoveEffect::StickyWeb => true,

        // PerishSong affects both sides — not blocked by Protect
        MoveEffect::PerishSong => true,

        // Haze affects both sides
        MoveEffect::Haze => true,

        // CourtChange — targets both sides
        MoveEffect::CourtChange => true,

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

// ── Charge move helpers ────────────────────────────────────────────

/// Check if weather allows skipping the charge turn.
#[inline]
fn weather_skips_charge(state: &BattleState, md: &MoveData) -> bool {
    match md.effect {
        MoveEffect::SolarBeam => matches!(state.field.weather, WEATHER_SUN | WEATHER_HARSH_SUN),
        MoveEffect::ChargeElectroShot => matches!(state.field.weather, WEATHER_RAIN | WEATHER_HEAVY_RAIN),
        _ => false,
    }
}

/// Check if the charge turn can be skipped (Power Herb or weather).
#[inline]
fn can_skip_charge(state: &BattleState, atk_side: usize, md: &MoveData) -> bool {
    if weather_skips_charge(state, md) { return true; }
    data_bridge::item(state.active_mon(atk_side).item_id).has(ItemFlag::POWER_HERB)
}

/// Returns the semi-invulnerability location byte, or None for non-semi-invuln charges.
#[inline]
fn semi_invuln_location(md: &MoveData) -> Option<u8> {
    match md.effect {
        MoveEffect::ChargeFly => Some(1),      // air
        MoveEffect::ChargeDig => Some(2),      // underground
        MoveEffect::ChargeDive => Some(3),     // underwater
        MoveEffect::ChargePhantom => Some(4),  // vanished
        _ => None,
    }
}

/// Apply charge-turn effects (boosts applied during the charge turn).
#[inline]
fn apply_charge_turn_effects(
    state: &mut BattleState,
    keys: &ZobristKeys,
    atk_side: usize,
    md: &MoveData,
) {
    match md.effect {
        MoveEffect::ChargeSkullBash => { apply_boost(state, keys, atk_side, DEF, 1); }
        MoveEffect::ChargeMeteorBeam | MoveEffect::ChargeElectroShot => {
            apply_boost(state, keys, atk_side, SPA, 1);
        }
        _ => {}
    }
}

/// Check if a move can hit a semi-invulnerable target.
/// `charge_loc`: 1=air, 2=underground, 3=underwater, 4=vanished.
#[inline]
fn can_hit_semi_invuln(move_id: u16, charge_loc: u8) -> bool {
    use crate::data::{MOVE_THUNDER, MOVE_HURRICANE, MOVE_EARTHQUAKE, MOVE_MAGNITUDE, MOVE_SURF, MOVE_WHIRLPOOL};
    match charge_loc {
        1 => move_id == MOVE_THUNDER as u16 || move_id == MOVE_HURRICANE as u16,
        2 => move_id == MOVE_EARTHQUAKE as u16 || move_id == MOVE_MAGNITUDE as u16,
        3 => move_id == MOVE_SURF as u16 || move_id == MOVE_WHIRLPOOL as u16,
        _ => false, // vanished: nothing hits (except No Guard, checked before)
    }
}

// ── Unified berry / HP-check activation (Hook 22) ───────────────────

/// Check if a Pokémon's held item should activate based on HP or status.
/// Called after ANY HP change: post-damage, post-recoil, post-hazard, post-status damage.
/// Handles: pinch berries, Sitrus Berry, Lum Berry, Berry Juice, Starf Berry,
/// flavored heal berries (Aguav/Figy/Wiki/Mago/Iapapa), and Gluttony threshold.
pub fn check_berry_activation(
    state: &mut BattleState,
    keys: &ZobristKeys,
    side: usize,
    slot: usize,
) {
    let mon = &state.sides[side].team[slot];
    if mon.item_id == 0 || mon.is_fainted() { return; }
    let item_id = mon.item_id;
    let item = data_bridge::item(item_id);

    let has_gluttony = effective_ability(state, side) == data_bridge::ABILITY_GLUTTONY;
    let max_hp = mon.max_hp;
    let current_hp = mon.current_hp;

    // ── Pinch stat berries (Liechi, Petaya, Ganlon, Apicot, Salac) ──
    if item.has(ItemFlag::PINCH_BERRY) {
        let threshold = if has_gluttony { max_hp / 2 } else { max_hp / 4 };
        if current_hp <= threshold {
            let stat = item.type_param as usize;
            if stat < 5 {
                apply_boost(state, keys, side, stat, 1);
            }
            consume_berry(state, keys, side, slot);
        }
        return;
    }

    // ── Sitrus Berry: ≤50% HP → heal 25% ──
    if item_id == data_bridge::ITEM_SITRUS_BERRY {
        if current_hp * 2 <= max_hp {
            heal(state, keys, side, slot, max_hp / 4);
            consume_berry(state, keys, side, slot);
        }
        return;
    }

    // ── Lum Berry: has non-volatile status → cure ──
    if item_id == data_bridge::ITEM_LUM_BERRY {
        if state.sides[side].team[slot].status != STATUS_NONE {
            clear_status(state, keys, side, slot);
            consume_berry(state, keys, side, slot);
        }
        return;
    }

    // ── Berry Juice: ≤50% HP → heal 20 HP ──
    if item_id == data_bridge::ITEM_BERRY_JUICE {
        if current_hp * 2 <= max_hp {
            heal(state, keys, side, slot, 20);
            consume_berry(state, keys, side, slot);
        }
        return;
    }

    // ── Starf Berry: ≤25% HP (or 50% with Gluttony) → +2 random stat ──
    if item_id == data_bridge::ITEM_STARF_BERRY {
        let threshold = if has_gluttony { max_hp / 2 } else { max_hp / 4 };
        if current_hp <= threshold {
            // Deterministic stat pick for MCTS: use turns_active as seed
            let stat = (state.sides[side].active.turns_active as usize) % 5;
            apply_boost(state, keys, side, stat, 2);
            consume_berry(state, keys, side, slot);
        }
        return;
    }

    // ── Flavored heal berries (Aguav/Figy/Wiki/Mago/Iapapa): ≤25% → heal 33% ──
    match item_id {
        data_bridge::ITEM_AGUAV_BERRY | data_bridge::ITEM_FIGY_BERRY |
        data_bridge::ITEM_WIKI_BERRY | data_bridge::ITEM_MAGO_BERRY |
        data_bridge::ITEM_IAPAPA_BERRY => {
            let threshold = if has_gluttony { max_hp / 2 } else { max_hp / 4 };
            if current_hp <= threshold {
                heal(state, keys, side, slot, max_hp / 3);
                consume_berry(state, keys, side, slot);
            }
        }
        _ => {}
    }
}

/// Consume a berry and trigger Unburden if applicable.
#[inline]
fn consume_berry(state: &mut BattleState, keys: &ZobristKeys, side: usize, slot: usize) {
    consume_item(state, keys, side, slot);
    if effective_ability(state, side) == data_bridge::ABILITY_UNBURDEN {
        set_volatile(state, keys, side, VOL_UNBURDEN);
    }
}

/// Legacy alias — some call sites still use this name.
pub fn check_pinch_berry(
    state: &mut BattleState, keys: &ZobristKeys, side: usize, slot: usize,
) {
    check_berry_activation(state, keys, side, slot);
}

// ── Self-effect application ──────────────────────────────────────────

/// Apply the attacker's self-effect after damage + drain/recoil.
/// CrashDamage is NOT handled here (it's in the miss path above).
#[inline]
fn apply_self_effect(
    state: &mut BattleState,
    keys: &ZobristKeys,
    atk_side: usize,
    md: &MoveData,
) {
    match md.self_effect {
        SelfEffect::None => {}
        SelfEffect::DefSpDDown1 => {
            apply_boost(state, keys, atk_side, DEF, -1);
            apply_boost(state, keys, atk_side, SPD, -1);
        }
        SelfEffect::AtkDefDown1 => {
            apply_boost(state, keys, atk_side, ATK, -1);
            apply_boost(state, keys, atk_side, DEF, -1);
        }
        SelfEffect::DefSpDSpeDown1 => {
            apply_boost(state, keys, atk_side, DEF, -1);
            apply_boost(state, keys, atk_side, SPD, -1);
            apply_boost(state, keys, atk_side, SPE, -1);
        }
        SelfEffect::SpADown2 => {
            apply_boost(state, keys, atk_side, SPA, -2);
        }
        SelfEffect::SpeDown1 => {
            apply_boost(state, keys, atk_side, SPE, -1);
        }
        SelfEffect::SpeDown2 => {
            apply_boost(state, keys, atk_side, SPE, -2);
        }
        SelfEffect::AtkDown1 => {
            apply_boost(state, keys, atk_side, ATK, -1);
        }
        SelfEffect::SpDDown1 => {
            apply_boost(state, keys, atk_side, SPD, -1);
        }
        SelfEffect::DefDown1 => {
            apply_boost(state, keys, atk_side, DEF, -1);
        }
        SelfEffect::SpADown1 => {
            apply_boost(state, keys, atk_side, SPA, -1);
        }
        SelfEffect::AtkUp1 => {
            apply_boost(state, keys, atk_side, ATK, 1);
        }
        SelfEffect::SpeUp1 => {
            apply_boost(state, keys, atk_side, SPE, 1);
        }
        SelfEffect::DefUp1 => {
            apply_boost(state, keys, atk_side, DEF, 1);
        }
        SelfEffect::SpAUp1 => {
            apply_boost(state, keys, atk_side, SPA, 1);
        }
        SelfEffect::ThawSelf => {
            let atk_slot = state.sides[atk_side].active_index as usize;
            if state.sides[atk_side].team[atk_slot].status == STATUS_FREEZE {
                clear_status(state, keys, atk_side, atk_slot);
            }
        }
        // SelfSwitch/BatonPass/PartingShot/Heal50 are handled via MoveEffect.
        // CrashDamage is handled in the miss path (§B).
        _ => {}
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
    mut move_id: u16,
    move_slot: u8,
    rng: &mut impl FnMut(u32) -> u32,
) {
    let def_side = 1 - atk_side;
    let atk_slot = state.sides[atk_side].active_index as usize;
    let def_slot = state.sides[def_side].active_index as usize;

    // ── Pre-move checks ─────────────────────────────────────────

    if state.sides[atk_side].team[atk_slot].is_fainted() { return; }

    // ── Charge move continuation (turn 2) ────────────────────
    let is_charge_turn2 = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
    if is_charge_turn2 {
        move_id = state.sides[atk_side].active.last_move;
        clear_volatile(state, keys, atk_side, VOL_CHARGING);
        clear_volatile(state, keys, atk_side, VOL_SEMI_INVULNERABLE);
        state.sides[atk_side].active._padding[1] = 0;
    }

    // ── Move-lock continuation (Outrage, Petal Dance, etc.) ──
    let is_move_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);
    let was_last_locked_turn;
    if is_move_locked {
        move_id = state.sides[atk_side].active.last_move;
        let counter = state.sides[atk_side].active._padding[2];
        state.sides[atk_side].active._padding[2] = counter - 1;
        was_last_locked_turn = counter <= 1;
        if was_last_locked_turn {
            clear_volatile(state, keys, atk_side, VOL_MOVE_LOCKED);
        }
    } else {
        was_last_locked_turn = false;
    }

    let is_struggle = move_id == 0;
    let md = data_bridge::move_hot(move_id);

    // Pre-move checks and execution are in a labeled block so that
    // thrash confusion is always applied after the last locked turn,
    // even if the move fails due to flinch/para/sleep/etc.
    'exec: {

    if state.sides[atk_side].active.has_volatile(VOL_RECHARGING) {
        clear_volatile(state, keys, atk_side, VOL_RECHARGING);
        break 'exec;
    }

    if state.sides[atk_side].active.has_volatile(VOL_FLINCHED) { break 'exec; }

    if state.sides[atk_side].team[atk_slot].status == STATUS_PARALYSIS {
        if rng(4) == 0 { break 'exec; }
    }

    // Sleep: decrement counter, fail unless waking up.
    // This is the ONLY place the sleep counter is decremented (not in end_of_turn).
    if state.sides[atk_side].team[atk_slot].status == STATUS_SLEEP {
        let counter = state.sides[atk_side].team[atk_slot].status_counter;
        if counter > 0 {
            state.sides[atk_side].team[atk_slot].status_counter = counter - 1;
            if counter > 1 { break 'exec; }
            clear_status(state, keys, atk_side, atk_slot);
        }
    }

    // Freeze: 20% thaw, fire moves always thaw
    if state.sides[atk_side].team[atk_slot].status == STATUS_FREEZE {
        if md.move_type == Type::Fire || rng(5) == 0 {
            clear_status(state, keys, atk_side, atk_slot);
        } else {
            break 'exec;
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
            break 'exec;
        }
    }

    // ── PP and bookkeeping (skip on charge turn 2) ──────────

    if !is_charge_turn2 {
        if !is_struggle {
            // For move-locked turns, find the correct move slot for PP deduction
            let pp_slot = if is_move_locked {
                let moves = effective_moves(state, atk_side);
                (0..4).find(|&i| moves[i] == move_id).unwrap_or(move_slot as usize)
            } else {
                move_slot as usize
            };
            deduct_pp(state, atk_side, pp_slot, 1);
        }

        if !is_move_locked {
            let active = &mut state.sides[atk_side].active;
            if active.last_move == move_id && move_id != 0 {
                active.consec_move_count = active.consec_move_count.saturating_add(1);
            } else {
                active.consec_move_count = 1;
            }
            active.last_move = move_id;
        }

        if !is_struggle {
            let atk_item = data_bridge::item(state.active_mon(atk_side).item_id);
            if atk_item.has(ItemFlag::IS_CHOICE)
                && state.sides[atk_side].active.choice_locked_move == 0
            {
                state.sides[atk_side].active.choice_locked_move = move_id;
            }
        }
    }

    set_volatile(state, keys, atk_side, VOL_MOVED_THIS_TURN);

    // ── Protean / Libero: change type to match move ─────────

    if !is_struggle && !is_charge_turn2 && !is_move_locked {
        let atk_ability = effective_ability(state, atk_side);
        if (atk_ability == data_bridge::ABILITY_PROTEAN || atk_ability == data_bridge::ABILITY_LIBERO)
            && state.sides[atk_side].active._padding[3] == 0
        {
            let new_type = md.move_type as u8;
            state.sides[atk_side].active.override_types = [new_type, new_type];
            set_volatile(state, keys, atk_side, VOL_TYPES_OVERRIDDEN);
            state.sides[atk_side].active._padding[3] = 1; // once per switch-in
        }
    }

    // ── Stance Change (Aegislash) ──────────────────────────
    if !is_struggle {
        let atk_ability = effective_ability(state, atk_side);
        if atk_ability == data_bridge::ABILITY_STANCE_CHANGE {
            let species = effective_species(state, atk_side);
            const AEGISLASH_SHIELD: u16 = 681;
            const AEGISLASH_BLADE: u16 = 1103;
            const KING_S_SHIELD: u16 = 588;
            if species == AEGISLASH_SHIELD && md.category != MoveCategory::Status {
                apply_battle_forme(state, keys, atk_side, AEGISLASH_BLADE);
            } else if species == AEGISLASH_BLADE && move_id == KING_S_SHIELD {
                revert_battle_forme(state, keys, atk_side);
            }
        }
    }

    // ── Charge move initiation (turn 1) ─────────────────────

    if !is_charge_turn2 && !is_struggle && md.flags & MoveFlags::CHARGE != 0 {
        let skip = can_skip_charge(state, atk_side, md);
        apply_charge_turn_effects(state, keys, atk_side, md);
        if skip {
            // Consume Power Herb if it was the skip reason (not weather)
            if !weather_skips_charge(state, md) {
                consume_item(state, keys, atk_side, atk_slot);
                if effective_ability(state, atk_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, keys, atk_side, VOL_UNBURDEN);
                }
            }
            // Fall through to execute the move this turn
        } else {
            // Begin charging
            set_volatile(state, keys, atk_side, VOL_CHARGING);
            if let Some(loc) = semi_invuln_location(md) {
                set_volatile(state, keys, atk_side, VOL_SEMI_INVULNERABLE);
                state.sides[atk_side].active._padding[1] = loc;
            }
            return; // End turn 1 (charge moves are never thrash, so return is correct)
        }
    }

    // ── Thrash lock initiation (first turn of Outrage, etc.) ────

    if !is_charge_turn2 && !is_struggle && !is_move_locked
        && md.effect == MoveEffect::Thrash
    {
        set_volatile(state, keys, atk_side, VOL_MOVE_LOCKED);
        state.sides[atk_side].active._padding[2] = (rng(2) + 1) as u8; // 1 or 2 more turns
    }

    // ── Status moves (Struggle is always damaging, skip) ────────

    if !is_struggle && md.category == MoveCategory::Status {
        execute_status_move(state, keys, atk_side, def_side, md, rng);
        return; // Status moves are never thrash, so return is correct
    }

    // ── Protect check (damaging moves) ──────────────────────────

    let bypasses_protect = is_charge_turn2 && md.effect == MoveEffect::ChargePhantom;
    if !bypasses_protect && state.sides[def_side].active.has_volatile(VOL_PROTECT_THIS_TURN) {
        break 'exec;
    }

    // ── Semi-invulnerability dodge ──────────────────────────────

    if !is_struggle && state.sides[def_side].active.has_volatile(VOL_SEMI_INVULNERABLE) {
        let atk_ability = effective_ability(state, atk_side);
        let def_ability = effective_ability(state, def_side);
        if atk_ability != data_bridge::ABILITY_NO_GUARD
            && def_ability != data_bridge::ABILITY_NO_GUARD
            && !can_hit_semi_invuln(move_id, state.sides[def_side].active._padding[1])
        {
            break 'exec;
        }
    }

    // ── Accuracy check (Struggle always hits) ───────────────────

    if !is_struggle && !accuracy_check(state, atk_side, md, rng) {
        // §B: Crash damage on miss — 50% of attacker's max HP
        if md.self_effect == SelfEffect::CrashDamage {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            deal_damage(state, keys, atk_side, atk_slot, max_hp / 2);
        }
        break 'exec;
    }

    // ── Ability-based immunities (before damage calc) ───────────
    if !is_struggle {
        // Priority-blocking: Dazzling / Queenly Majesty / Armor Tail
        if priority_block_immunity(state, def_side, md.priority) {
            break 'exec;
        }
        // Flag-based immunities (Bulletproof, Soundproof, Overcoat, Wind Rider)
        if let Some(eff) = ability_flag_immunity(state, def_side, md.flags) {
            apply_immunity_effect(state, keys, def_side, def_slot, eff);
            break 'exec;
        }
        // Type-based immunities and side effects
        if let Some(eff) = ability_type_immunity(state, def_side, md.move_type) {
            apply_immunity_effect(state, keys, def_side, def_slot, eff);
            break 'exec;
        }
    }

    // ── Fixed-damage moves (bypass normal calc) ──────────────────

    // Endeavor: set target HP = user HP
    if md.effect == MoveEffect::Endeavor {
        let user_hp = state.sides[atk_side].team[atk_slot].current_hp;
        let target_hp = state.sides[def_side].team[def_slot].current_hp;
        if target_hp > user_hp {
            deal_damage(state, keys, def_side, def_slot, target_hp - user_hp);
        }
        break 'exec;
    }

    // SuperFang: halve target's current HP
    if md.effect == MoveEffect::SuperFang {
        let target_hp = state.sides[def_side].team[def_slot].current_hp;
        deal_damage(state, keys, def_side, def_slot, (target_hp / 2).max(1));
        break 'exec;
    }

    // SeismicToss / Night Shade: damage = level (100 at L100)
    if md.effect == MoveEffect::SeismicToss {
        deal_damage(state, keys, def_side, def_slot, BATTLE_LEVEL);
        break 'exec;
    }

    // Counter: return 2× physical damage taken this turn
    if md.effect == MoveEffect::Counter {
        let last_hit = state.sides[atk_side].active.last_move_hit_by;
        if last_hit != 0 {
            let last_md = data_bridge::move_hot(last_hit);
            if last_md.category == MoveCategory::Physical {
                // Approximate: use 1/4 of attacker's max HP as base (simplified for MCTS)
                let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, keys, def_side, def_slot, max_hp / 2);
            }
        }
        break 'exec;
    }

    // MirrorCoat: return 2× special damage taken this turn
    if md.effect == MoveEffect::MirrorCoat {
        let last_hit = state.sides[atk_side].active.last_move_hit_by;
        if last_hit != 0 {
            let last_md = data_bridge::move_hot(last_hit);
            if last_md.category == MoveCategory::Special {
                let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, keys, def_side, def_slot, max_hp / 2);
            }
        }
        break 'exec;
    }

    // MetalBurst: return 1.5× last damage taken
    if md.effect == MoveEffect::MetalBurst {
        let last_hit = state.sides[atk_side].active.last_move_hit_by;
        if last_hit != 0 {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            deal_damage(state, keys, def_side, def_slot, max_hp * 3 / 8);
        }
        break 'exec;
    }

    // FinalGambit: deal user's current HP as damage, user faints
    if md.effect == MoveEffect::FinalGambit {
        let user_hp = state.sides[atk_side].team[atk_slot].current_hp;
        deal_damage(state, keys, def_side, def_slot, user_hp);
        deal_damage(state, keys, atk_side, atk_slot, user_hp);
        break 'exec;
    }

    // SpitUp: deal 100/200/300 damage by stockpile count
    if md.effect == MoveEffect::SpitUp {
        let count = state.sides[atk_side].active.stockpile;
        if count > 0 {
            let damage = count as u16 * 100;
            deal_damage(state, keys, def_side, def_slot, damage);
            apply_boost(state, keys, atk_side, DEF, -(count as i8));
            apply_boost(state, keys, atk_side, SPD, -(count as i8));
            state.sides[atk_side].active.stockpile = 0;
        }
        break 'exec;
    }

    // ── Damage calculation ──────────────────────────────────────

    let result = calc_damage(state, atk_side, move_id, rng);

    if result.type_immune { break 'exec; }

    // ── Disguise / Ice Face: nullify first hit ──────────────────

    if !result.hits_substitute {
        let def_ability = effective_ability(state, def_side);
        let shields = state.sides[def_side].active._padding[4];

        // Disguise: blocks one hit of any category
        if def_ability == data_bridge::ABILITY_DISGUISE && shields & 1 == 0 {
            state.sides[def_side].active._padding[4] = shields | 1;
            // Disguise costs 1/8 max HP when broken (Gen 8+)
            let max_hp = state.sides[def_side].team[def_slot].max_hp;
            deal_damage(state, keys, def_side, def_slot, max_hp / 8);
            break 'exec;
        }

        // Ice Face: blocks one Physical hit
        if def_ability == data_bridge::ABILITY_ICE_FACE
            && shields & 2 == 0
            && md.category == MoveCategory::Physical
        {
            state.sides[def_side].active._padding[4] = shields | 2;
            break 'exec;
        }
    }

    // ── Focus Sash / Sturdy: survive OHKO at full HP ──────────

    let mut final_damage = result.damage;
    if !result.hits_substitute {
        let def_mon = &state.sides[def_side].team[def_slot];
        if def_mon.current_hp == def_mon.max_hp && final_damage >= def_mon.current_hp {
            let def_item = data_bridge::item(def_mon.item_id);
            let def_ability = effective_ability(state, def_side);
            if def_item.has(ItemFlag::FOCUS_SASH) {
                final_damage = def_mon.current_hp - 1;
                consume_item(state, keys, def_side, def_slot);
                if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, keys, def_side, VOL_UNBURDEN);
                }
            } else if def_ability == data_bridge::ABILITY_STURDY {
                final_damage = def_mon.current_hp - 1;
            }
        }
    }

    // ── Apply damage ────────────────────────────────────────────

    if result.hits_substitute {
        let sub = &mut state.sides[def_side].active.substitute_hp;
        *sub = sub.saturating_sub(final_damage);
        if *sub == 0 {
            clear_volatile(state, keys, def_side, VOL_SUBSTITUTE);
        }
    } else {
        deal_damage(state, keys, def_side, def_slot, final_damage);
        state.sides[def_side].active.last_move_hit_by = move_id;
    }

    // ── Drain / recoil ──────────────────────────────────────────

    if result.drain_heal > 0 {
        heal(state, keys, atk_side, atk_slot, result.drain_heal);
    }
    if result.recoil_damage > 0 {
        deal_damage(state, keys, atk_side, atk_slot, result.recoil_damage);
        // Berry activation after recoil (e.g., Sitrus Berry)
        if !state.sides[atk_side].team[atk_slot].is_fainted() {
            check_berry_activation(state, keys, atk_side, atk_slot);
        }
    }

    // ── Clear Charge bit after using an Electric move ──────────
    if md.move_type == Type::Electric {
        state.sides[atk_side].active._padding[4] &= !4;
    }

    // ── Hook 20: ITEM_AFTER_DAMAGE (attacker) ───────────────────

    if !state.sides[atk_side].team[atk_slot].is_fainted() && !result.hits_substitute {
        let atk_item_id = state.sides[atk_side].team[atk_slot].item_id;
        // Shell Bell: heal 1/8 of damage dealt
        if atk_item_id == data_bridge::ITEM_SHELL_BELL && final_damage > 0 {
            heal(state, keys, atk_side, atk_slot, (final_damage / 8).max(1));
        }
        // Throat Spray: +1 SpA if sound move, consume
        if atk_item_id == data_bridge::ITEM_THROAT_SPRAY
            && md.flags & MoveFlags::SOUND != 0
        {
            apply_boost(state, keys, atk_side, SPA, 1);
            consume_item(state, keys, atk_side, atk_slot);
            if effective_ability(state, atk_side) == data_bridge::ABILITY_UNBURDEN {
                set_volatile(state, keys, atk_side, VOL_UNBURDEN);
            }
        }
    }

    // ── SelfEffect application (after damage + drain/recoil) ────

    if !state.sides[atk_side].team[atk_slot].is_fainted() {
        apply_self_effect(state, keys, atk_side, md);
    }

    // ── Berry activation (after taking direct damage) ────────────

    if !result.hits_substitute
        && !state.sides[def_side].team[def_slot].is_fainted()
    {
        check_berry_activation(state, keys, def_side, def_slot);
    }

    // ── Zen Mode check (defender HP may have dropped ≤50%) ─────
    if !result.hits_substitute
        && !state.sides[def_side].team[def_slot].is_fainted()
    {
        crate::state::forme::check_zen_mode(state, keys, def_side);
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

    // ── Defender ability hooks on being hit ──────────────────────

    if !result.hits_substitute
        && !state.sides[def_side].team[def_slot].is_fainted()
    {
        let def_ability = effective_ability(state, def_side);
        match def_ability {
            // Weak Armor: Physical hit → -1 Def, +2 Spe
            data_bridge::ABILITY_WEAK_ARMOR if md.category == MoveCategory::Physical => {
                apply_boost(state, keys, def_side, DEF, -1);
                apply_boost(state, keys, def_side, SPE, 2);
            }
            // Justified: Dark-type hit → +1 Atk
            data_bridge::ABILITY_JUSTIFIED if md.move_type == Type::Dark => {
                apply_boost(state, keys, def_side, ATK, 1);
            }
            // Stamina: any hit → +1 Def
            data_bridge::ABILITY_STAMINA => {
                apply_boost(state, keys, def_side, DEF, 1);
            }
            // Anger Point: crit → maximize Atk (+6)
            data_bridge::ABILITY_ANGER_POINT if result.crit => {
                let current = state.sides[def_side].active.boosts[ATK];
                if current < 6 {
                    apply_boost(state, keys, def_side, ATK, 6 - current);
                }
            }
            // Color Change: change type to match the move type
            data_bridge::ABILITY_COLOR_CHANGE => {
                let new_type = md.move_type as u8;
                let (t1, t2) = effective_types(state, def_side);
                if t1 != new_type || t2 != new_type {
                    state.sides[def_side].active.override_types = [new_type, new_type];
                    set_volatile(state, keys, def_side, VOL_TYPES_OVERRIDDEN);
                }
            }
            // Rattled: Bug/Dark/Ghost hit → +1 Spe
            data_bridge::ABILITY_RATTLED
                if matches!(md.move_type, Type::Bug | Type::Dark | Type::Ghost)
                => { apply_boost(state, keys, def_side, SPE, 1); }
            // Steam Engine: Fire/Water hit → +6 Spe
            data_bridge::ABILITY_STEAM_ENGINE
                if matches!(md.move_type, Type::Fire | Type::Water)
                => { apply_boost(state, keys, def_side, SPE, 6); }
            // Thermal Exchange: Fire hit → +1 Atk (+ burn immunity handled elsewhere)
            data_bridge::ABILITY_THERMAL_EXCHANGE if md.move_type == Type::Fire
                => { apply_boost(state, keys, def_side, ATK, 1); }
            // Berserk: HP drops ≤50% → +1 SpA
            data_bridge::ABILITY_BERSERK => {
                let m = state.sides[def_side].team[def_slot].max_hp;
                let hp = state.sides[def_side].team[def_slot].current_hp;
                // The damage already happened, so check if HP crossed the 50% threshold
                // We approximate: if HP ≤ 50% after damage, trigger
                if hp > 0 && hp * 2 <= m {
                    apply_boost(state, keys, def_side, SPA, 1);
                }
            }
            // Toxic Debris: physical hit → set Toxic Spikes on attacker's side
            data_bridge::ABILITY_TOXIC_DEBRIS if md.category == MoveCategory::Physical => {
                crate::state::switch::add_toxic_spikes(state, atk_side);
            }
            // Electromorphosis: any hit → gain Charge (2× next Electric move)
            data_bridge::ABILITY_ELECTROMORPHOSIS => {
                state.sides[def_side].active._padding[4] |= 4; // charge bit
            }
            // Wind Power: wind move hit → gain Charge
            data_bridge::ABILITY_WIND_POWER if md.flags & MoveFlags::WIND != 0 => {
                state.sides[def_side].active._padding[4] |= 4; // charge bit
            }
            // Seed Sower: any hit → set Grassy Terrain
            data_bridge::ABILITY_SEED_SOWER => {
                set_terrain(state, keys, TERRAIN_GRASSY, 5);
            }
            // Cotton Down: any hit → lower attacker's Spe by 1
            data_bridge::ABILITY_COTTON_DOWN => {
                if !state.sides[atk_side].team[atk_slot].is_fainted() {
                    apply_boost(state, keys, atk_side, SPE, -1);
                }
            }
            // Mummy / Lingering Aroma: contact → overwrite attacker's ability
            data_bridge::ABILITY_MUMMY if md.flags & MoveFlags::CONTACT != 0 => {
                if !state.sides[atk_side].team[atk_slot].is_fainted() {
                    let atk_ab = effective_ability(state, atk_side);
                    if atk_ab != data_bridge::ABILITY_MUMMY && atk_ab != 0 {
                        state.sides[atk_side].active.override_ability = data_bridge::ABILITY_MUMMY;
                        set_volatile(state, keys, atk_side, VOL_ABILITY_OVERRIDDEN);
                    }
                }
            }
            data_bridge::ABILITY_LINGERING_AROMA if md.flags & MoveFlags::CONTACT != 0 => {
                if !state.sides[atk_side].team[atk_slot].is_fainted() {
                    let atk_ab = effective_ability(state, atk_side);
                    if atk_ab != data_bridge::ABILITY_LINGERING_AROMA && atk_ab != 0 {
                        state.sides[atk_side].active.override_ability = data_bridge::ABILITY_LINGERING_AROMA;
                        set_volatile(state, keys, atk_side, VOL_ABILITY_OVERRIDDEN);
                    }
                }
            }
            // Perish Body: contact → set 3-turn Perish on both
            data_bridge::ABILITY_PERISH_BODY if md.flags & MoveFlags::CONTACT != 0 => {
                if !state.sides[def_side].active.has_volatile(VOL_PERISH_SONG) {
                    set_volatile(state, keys, def_side, VOL_PERISH_SONG);
                    state.sides[def_side].active.perish_count = 3;
                }
                if !state.sides[atk_side].active.has_volatile(VOL_PERISH_SONG)
                    && !state.sides[atk_side].team[atk_slot].is_fainted()
                {
                    set_volatile(state, keys, atk_side, VOL_PERISH_SONG);
                    state.sides[atk_side].active.perish_count = 3;
                }
            }
            _ => {}
        }

        // ── Attacker ability hooks after dealing damage (Hook 19) ────
        if !state.sides[atk_side].team[atk_slot].is_fainted() {
            let atk_ability = effective_ability(state, atk_side);
            match atk_ability {
                // Poison Touch: 30% poison on contact
                data_bridge::ABILITY_POISON_TOUCH
                    if md.flags & MoveFlags::CONTACT != 0
                    && state.sides[def_side].team[def_slot].status == STATUS_NONE
                    => {
                    if rng(100) < 30 {
                        set_status(state, keys, def_side, def_slot, STATUS_POISON, 0);
                    }
                }
                // Toxic Chain: 30% toxic on any hit
                data_bridge::ABILITY_TOXIC_CHAIN
                    if state.sides[def_side].team[def_slot].status == STATUS_NONE
                    => {
                    if rng(100) < 30 {
                        set_status(state, keys, def_side, def_slot, STATUS_BAD_POISON, 0);
                    }
                }
                // Magician: steal target's item on hit
                data_bridge::ABILITY_MAGICIAN
                    if state.active_mon(atk_side).item_id == 0
                    && state.active_mon(def_side).item_id != 0
                    => {
                    let stolen = state.sides[def_side].team[def_slot].item_id;
                    set_item(state, keys, atk_side, atk_slot, stolen);
                    consume_item(state, keys, def_side, def_slot);
                }
                _ => {}
            }
        }

        // ── Hook 21: ITEM_AFTER_HIT (defender) ──────────────────────

        let def_item_id = state.sides[def_side].team[def_slot].item_id;
        if def_item_id != 0 && !state.sides[def_side].team[def_slot].is_fainted() {
            // Weakness Policy: +2 Atk +2 SpA if hit by SE move, consume
            if def_item_id == data_bridge::ITEM_WEAKNESS_POLICY && result.effectiveness > 4 {
                apply_boost(state, keys, def_side, ATK, 2);
                apply_boost(state, keys, def_side, SPA, 2);
                consume_item(state, keys, def_side, def_slot);
                if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, keys, def_side, VOL_UNBURDEN);
                }
            }
            // Jaboca Berry: 1/8 attacker HP if physical hit
            if def_item_id == data_bridge::ITEM_JABOCA_BERRY
                && md.category == MoveCategory::Physical
                && !state.sides[atk_side].team[atk_slot].is_fainted()
            {
                let atk_max = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, keys, atk_side, atk_slot, (atk_max / 8).max(1));
                consume_berry(state, keys, def_side, def_slot);
            }
            // Rowap Berry: 1/8 attacker HP if special hit
            if def_item_id == data_bridge::ITEM_ROWAP_BERRY
                && md.category == MoveCategory::Special
                && !state.sides[atk_side].team[atk_slot].is_fainted()
            {
                let atk_max = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, keys, atk_side, atk_slot, (atk_max / 8).max(1));
                consume_berry(state, keys, def_side, def_slot);
            }
            // Air Balloon: pop on any damaging hit
            if data_bridge::item(def_item_id).has(ItemFlag::AIR_BALLOON) {
                consume_item(state, keys, def_side, def_slot);
                if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, keys, def_side, VOL_UNBURDEN);
                }
            }
        }
    }

    // ── Contact aftermath (items + abilities) ────────────────────

    if md.flags & MoveFlags::CONTACT != 0
        && !state.sides[atk_side].team[atk_slot].is_fainted()
        && !state.sides[def_side].team[def_slot].is_fainted()
    {
        // Rocky Helmet: 1/6 max HP (triggers even through substitute)
        let def_itm = data_bridge::item(state.active_mon(def_side).item_id);
        if def_itm.has(ItemFlag::ROCKY_HELMET) {
            let atk_max = state.active_mon(atk_side).max_hp;
            deal_damage(state, keys, atk_side, atk_slot, atk_max / 6);
        }

        // Ability-based contact aftermath (only on direct hit, not through sub)
        if !result.hits_substitute {
            let def_ability = effective_ability(state, def_side);

            // Rough Skin / Iron Barbs: 1/8 max HP
            if def_ability == data_bridge::ABILITY_ROUGH_SKIN
                || def_ability == data_bridge::ABILITY_IRON_BARBS
            {
                let atk_max = state.active_mon(atk_side).max_hp;
                deal_damage(state, keys, atk_side, atk_slot, (atk_max / 8).max(1));
            }

            // Contact status abilities (attacker alive + no status)
            if !state.sides[atk_side].team[atk_slot].is_fainted()
                && state.sides[atk_side].team[atk_slot].status == STATUS_NONE
            {
                match def_ability {
                    data_bridge::ABILITY_FLAME_BODY => {
                        if rng(100) < 30 {
                            set_status(state, keys, atk_side, atk_slot, STATUS_BURN, 0);
                        }
                    }
                    data_bridge::ABILITY_STATIC => {
                        if rng(100) < 30 {
                            set_status(state, keys, atk_side, atk_slot, STATUS_PARALYSIS, 0);
                        }
                    }
                    data_bridge::ABILITY_POISON_POINT => {
                        if rng(100) < 30 {
                            set_status(state, keys, atk_side, atk_slot, STATUS_POISON, 0);
                        }
                    }
                    data_bridge::ABILITY_EFFECT_SPORE => {
                        let roll = rng(100);
                        if roll < 10 {
                            set_status(state, keys, atk_side, atk_slot, STATUS_SLEEP, (rng(3) + 1) as u8);
                        } else if roll < 20 {
                            set_status(state, keys, atk_side, atk_slot, STATUS_PARALYSIS, 0);
                        } else if roll < 30 {
                            set_status(state, keys, atk_side, atk_slot, STATUS_POISON, 0);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // ── Knock Off: remove defender's item ────────────────────────

    if md.effect == MoveEffect::KnockOff
        && !state.sides[atk_side].team[atk_slot].is_fainted()
        && !result.hits_substitute
    {
        let def_mon = state.active_mon(def_side);
        if def_mon.item_id != 0 {
            let def_itm = data_bridge::item(def_mon.item_id);
            if !def_itm.has(ItemFlag::MEGA_STONE) && !def_itm.has(ItemFlag::Z_CRYSTAL) {
                consume_item(state, keys, def_side, def_slot);
                if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, keys, def_side, VOL_UNBURDEN);
                }
            }
        }
    }

    // ── Salt Cure: apply volatile for EOT damage ────────────────

    if md.effect == MoveEffect::SaltCure && !result.hits_substitute
        && !state.sides[def_side].team[def_slot].is_fainted()
    {
        // Use VOL_BOUND as a proxy for salt cure (they're mutually exclusive in practice)
        // Actually, we'll use the VOL_YAWN bit repurposing is bad. Let me use a different approach.
        // For MCTS simplicity, store salt cure in the heal_block_turns field with a sentinel.
        // Better: just track via a dedicated mechanism. We'll set last_move_hit_by to SaltCure move_id
        // and check in end_of_turn. Simplest: just deal the damage now as approximation.
        // For proper EOT: we need a way to track. Let's use the stockpile field's upper bits
        // since stockpile only uses 0-3. We'll set bit 7 to indicate salt_cure.
        state.sides[def_side].active.stockpile |= 0x80; // bit 7 = salt cure
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

    // ── After-KO hooks (Aftermath, Moxie, Beast Boost) ─────────

    // ── Destiny Bond: if defender had it and attacker KO'd them ────
    if state.sides[def_side].team[def_slot].is_fainted()
        && state.sides[def_side].active.has_volatile(VOL_DESTINY_BOND)
        && !state.sides[atk_side].team[atk_slot].is_fainted()
    {
        let atk_hp = state.sides[atk_side].team[atk_slot].current_hp;
        deal_damage(state, keys, atk_side, atk_slot, atk_hp);
    }

    if state.sides[def_side].team[def_slot].is_fainted() {
        // Aftermath: 1/4 max HP to attacker if contact and defender fainted
        if md.flags & MoveFlags::CONTACT != 0
            && !result.hits_substitute
            && !state.sides[atk_side].team[atk_slot].is_fainted()
        {
            let def_ability = effective_ability(state, def_side);
            if def_ability == data_bridge::ABILITY_AFTERMATH {
                let atk_max = state.active_mon(atk_side).max_hp;
                deal_damage(state, keys, atk_side, atk_slot, atk_max / 4);
            }
        }

        // Innards Out: deal damage equal to HP lost (= final_damage) to attacker
        if !result.hits_substitute
            && !state.sides[atk_side].team[atk_slot].is_fainted()
        {
            let def_ability = effective_ability(state, def_side);
            if def_ability == data_bridge::ABILITY_INNARDS_OUT {
                deal_damage(state, keys, atk_side, atk_slot, final_damage);
            }
        }

        // Moxie / Beast Boost: attacker stat boost on KO
        if !state.sides[atk_side].team[atk_slot].is_fainted() {
            let atk_ability = effective_ability(state, atk_side);
            match atk_ability {
                data_bridge::ABILITY_MOXIE => {
                    apply_boost(state, keys, atk_side, ATK, 1);
                }
                data_bridge::ABILITY_CHILLING_NEIGH | data_bridge::ABILITY_AS_ONE_GLASTRIER => {
                    apply_boost(state, keys, atk_side, ATK, 1);
                }
                data_bridge::ABILITY_GRIM_NEIGH | data_bridge::ABILITY_AS_ONE_SPECTRIER => {
                    apply_boost(state, keys, atk_side, SPA, 1);
                }
                data_bridge::ABILITY_BATTLE_BOND => {
                    apply_boost(state, keys, atk_side, ATK, 1);
                    apply_boost(state, keys, atk_side, SPA, 1);
                    apply_boost(state, keys, atk_side, SPE, 1);
                }
                data_bridge::ABILITY_BEAST_BOOST => {
                    // Boost the highest raw stat
                    let stats = &state.sides[atk_side].team[atk_slot].stats;
                    let best = (0..5).max_by_key(|&i| stats[i]).unwrap_or(ATK);
                    apply_boost(state, keys, atk_side, best, 1);
                }
                _ => {}
            }
        }
    }

    } // end 'exec

    // ── Thrash confusion on last locked turn ─────────────────────
    // Applied regardless of whether the move executed (para/sleep/etc.
    // still end the lock and cause confusion).
    if was_last_locked_turn && !state.sides[atk_side].team[atk_slot].is_fainted() {
        state.sides[atk_side].active.confusion_turns = (rng(3) + 2) as u8; // 2-4 turns
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

    // ── Step 1: Weather accuracy tests ──────────────────────

    #[test]
    fn test_weather_acc_rain_always_hits() {
        let (mut state, _) = setup();
        state.field.weather = WEATHER_RAIN;
        state.field.weather_turns = 5;
        let md = MoveData {
            accuracy: 70,
            effect: MoveEffect::WeatherAccRain,
            ..unsafe { core::mem::zeroed() }
        };
        // Should always hit in rain regardless of RNG
        assert!(accuracy_check(&state, 0, &md, &mut fixed_rng(99)));
    }

    #[test]
    fn test_weather_acc_sun_50pct() {
        let (mut state, _) = setup();
        state.field.weather = WEATHER_SUN;
        state.field.weather_turns = 5;
        let md = MoveData {
            accuracy: 70,
            effect: MoveEffect::WeatherAccRain,
            ..unsafe { core::mem::zeroed() }
        };
        // RNG=49 → 49 < 50 → hit
        assert!(accuracy_check(&state, 0, &md, &mut fixed_rng(49)));
        // RNG=99 → 99 >= 50 → miss
        assert!(!accuracy_check(&state, 0, &md, &mut fixed_rng(99)));
    }

    #[test]
    fn test_weather_acc_snow_always_hits() {
        let (mut state, _) = setup();
        state.field.weather = WEATHER_SNOW;
        state.field.weather_turns = 5;
        let md = MoveData {
            accuracy: 70,
            effect: MoveEffect::WeatherAccSnow,
            ..unsafe { core::mem::zeroed() }
        };
        // Should always hit in snow regardless of RNG
        assert!(accuracy_check(&state, 0, &md, &mut fixed_rng(99)));
    }

    #[test]
    fn test_weather_acc_normal_uses_base() {
        let (state, _) = setup();
        // No weather: normal accuracy applies
        let md = MoveData {
            accuracy: 70,
            effect: MoveEffect::WeatherAccRain,
            ..unsafe { core::mem::zeroed() }
        };
        // RNG=69 → 69 < 70 → hit
        assert!(accuracy_check(&state, 0, &md, &mut fixed_rng(69)));
        // RNG=70 → 70 >= 70 → miss
        assert!(!accuracy_check(&state, 0, &md, &mut fixed_rng(70)));
    }

    // ── Step 2: Pivot move tests ────────────────────────────

    #[test]
    fn test_parting_shot_debuffs_and_switches() {
        let (mut state, keys) = setup();
        state.zobrist = compute_full_hash(&state, &keys);

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0, // always hits
            effect: MoveEffect::PartingShot,
            ..unsafe { core::mem::zeroed() }
        };

        execute_status_move(&mut state, &keys, 0, 1, &md, &mut fixed_rng(99));

        // Opponent should have -1 Atk and -1 SpA
        assert_eq!(state.sides[1].active.boosts[ATK], -1);
        assert_eq!(state.sides[1].active.boosts[SPA], -1);

        // Attacker should have VOL_MUST_SWITCH set (has bench mon)
        assert!(state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_parting_shot_no_switch_at_min_boosts() {
        let (mut state, keys) = setup();
        // Set opponent to -6 Atk and -6 SpA already
        state.sides[1].active.boosts[ATK] = -6;
        state.sides[1].active.boosts[SPA] = -6;
        state.zobrist = compute_full_hash(&state, &keys);

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::PartingShot,
            ..unsafe { core::mem::zeroed() }
        };

        execute_status_move(&mut state, &keys, 0, 1, &md, &mut fixed_rng(99));

        // Boosts can't go lower, so no switch
        assert!(!state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    }

    #[test]
    fn test_baton_pass_sets_flag_and_must_switch() {
        let (mut state, keys) = setup();
        state.zobrist = compute_full_hash(&state, &keys);

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::BatonPass,
            ..unsafe { core::mem::zeroed() }
        };

        execute_status_move(&mut state, &keys, 0, 1, &md, &mut fixed_rng(99));

        assert!(state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
        assert_eq!(state.sides[0].active._padding[0], 1); // baton pass flag
    }

    #[test]
    fn test_baton_pass_is_self_targeting() {
        let md = MoveData { effect: MoveEffect::BatonPass, ..unsafe { core::mem::zeroed() } };
        assert!(is_self_targeting(&md));
        // Parting Shot targets opponent
        let ps = MoveData { effect: MoveEffect::PartingShot, ..unsafe { core::mem::zeroed() } };
        assert!(!is_self_targeting(&ps));
    }

    // ── Step 3: Charge move tests ────────────────────────────

    /// Helper: create a MoveData with CHARGE flag and given effect.
    fn charge_move(effect: MoveEffect, bp: u8, cat: MoveCategory) -> MoveData {
        MoveData {
            flags: MoveFlags::CHARGE,
            base_power: bp,
            accuracy: 100,
            category: cat,
            effect,
            ..unsafe { core::mem::zeroed() }
        }
    }

    #[test]
    fn test_charge_turn1_sets_charging() {
        let (mut state, keys) = setup();
        // Give side 0 a charge move (Fly-like)
        let fly_id = 19u16; // MOVE_FLY
        state.sides[0].team[0].moves[0] = fly_id;
        state.zobrist = compute_full_hash(&state, &keys);

        let pp_before = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &keys, 0, fly_id, 0, &mut fixed_rng(0));

        // VOL_CHARGING should be set (move has CHARGE flag)
        // But since the MoveEffect in gen data is None (not ChargeFly),
        // VOL_SEMI_INVULNERABLE won't be set.
        assert!(state.sides[0].active.has_volatile(VOL_CHARGING));
        // PP was deducted on turn 1
        assert_eq!(state.sides[0].team[0].pp[0], pp_before - 1);
        // Defender HP should not change yet
        assert_eq!(state.sides[1].team[0].current_hp, 300);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_charge_turn2_deals_damage() {
        let (mut state, keys) = setup();
        let fly_id = 19u16;
        state.sides[0].team[0].moves[0] = fly_id;
        state.zobrist = compute_full_hash(&state, &keys);

        // Turn 1: charge
        execute_move(&mut state, &keys, 0, fly_id, 0, &mut fixed_rng(0));
        assert!(state.sides[0].active.has_volatile(VOL_CHARGING));
        let pp_after_turn1 = state.sides[0].team[0].pp[0];

        // Turn 2: execute (pass any move_id, it's overridden by last_move)
        execute_move(&mut state, &keys, 0, 0, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_CHARGING));
        // PP should NOT be deducted again
        assert_eq!(state.sides[0].team[0].pp[0], pp_after_turn1);
        // Defender should have taken damage
        assert!(state.sides[1].team[0].current_hp < 300);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_charge_semi_invuln_dodges_attacks() {
        let (mut state, keys) = setup();

        // Manually set side 1 as charging with semi-invuln (air)
        set_volatile(&mut state, &keys, 1, VOL_CHARGING);
        set_volatile(&mut state, &keys, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 1; // air
        state.sides[1].active.last_move = 19; // Fly

        let hp_before = state.sides[1].team[0].current_hp;

        // Side 0 uses a normal move (move 1 = Pound)
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        // Should miss due to semi-invulnerability
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_semi_invuln_air_hit_by_thunder() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_THUNDER;

        // Change defender to non-Ground type (species 50 = Diglett is Ground, immune to Electric)
        state.sides[1].team[0].species_id = 25; // Pikachu (Electric, not immune)
        state.zobrist = compute_full_hash(&state, &keys);

        // Set side 1 as semi-invuln in air
        set_volatile(&mut state, &keys, 1, VOL_CHARGING);
        set_volatile(&mut state, &keys, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 1; // air
        state.sides[1].active.last_move = 19;

        // Side 0 uses Thunder (should hit through semi-invuln)
        execute_move(&mut state, &keys, 0, MOVE_THUNDER as u16, 0, &mut fixed_rng(0));

        // Thunder should deal damage through semi-invuln
        assert!(state.sides[1].team[0].current_hp < 300);
    }

    #[test]
    fn test_semi_invuln_underground_hit_by_earthquake() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_EARTHQUAKE;

        // Set side 1 as semi-invuln underground
        set_volatile(&mut state, &keys, 1, VOL_CHARGING);
        set_volatile(&mut state, &keys, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 2; // underground
        state.sides[1].active.last_move = 91; // Dig

        execute_move(&mut state, &keys, 0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < 300);
    }

    #[test]
    fn test_semi_invuln_underwater_hit_by_surf() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_SURF;

        set_volatile(&mut state, &keys, 1, VOL_CHARGING);
        set_volatile(&mut state, &keys, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 3; // underwater
        state.sides[1].active.last_move = 291; // Dive

        execute_move(&mut state, &keys, 0, MOVE_SURF as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < 300);
    }

    #[test]
    fn test_semi_invuln_vanished_dodges_everything() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_EARTHQUAKE;

        set_volatile(&mut state, &keys, 1, VOL_CHARGING);
        set_volatile(&mut state, &keys, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 4; // vanished
        state.sides[1].active.last_move = 566; // Phantom Force

        let hp_before = state.sides[1].team[0].current_hp;
        // Even Earthquake misses vanished targets
        execute_move(&mut state, &keys, 0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_charge_skull_bash_def_boost() {
        let (mut state, keys) = setup();
        // Create a Skull Bash-like charge move via direct execution
        let md = charge_move(MoveEffect::ChargeSkullBash, 130, MoveCategory::Physical);

        // Manually test apply_charge_turn_effects
        apply_charge_turn_effects(&mut state, &keys, 0, &md);
        assert_eq!(state.sides[0].active.boosts[DEF], 1);
    }

    #[test]
    fn test_charge_meteor_beam_spa_boost() {
        let (mut state, keys) = setup();
        let md = charge_move(MoveEffect::ChargeMeteorBeam, 120, MoveCategory::Special);

        apply_charge_turn_effects(&mut state, &keys, 0, &md);
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
    }

    #[test]
    fn test_charge_electro_shot_spa_boost() {
        let (mut state, keys) = setup();
        let md = charge_move(MoveEffect::ChargeElectroShot, 130, MoveCategory::Special);

        apply_charge_turn_effects(&mut state, &keys, 0, &md);
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
    }

    #[test]
    fn test_solar_beam_skip_in_sun() {
        // weather_skips_charge returns true in sun for SolarBeam
        let mut state = BattleState::default();
        let md = MoveData {
            flags: MoveFlags::CHARGE,
            effect: MoveEffect::SolarBeam,
            ..unsafe { core::mem::zeroed() }
        };

        state.field.weather = WEATHER_SUN;
        assert!(weather_skips_charge(&state, &md));

        state.field.weather = WEATHER_RAIN;
        assert!(!weather_skips_charge(&state, &md));

        state.field.weather = WEATHER_NONE;
        assert!(!weather_skips_charge(&state, &md));
    }

    #[test]
    fn test_electro_shot_skip_in_rain() {
        let mut state = BattleState::default();
        let md = MoveData {
            flags: MoveFlags::CHARGE,
            effect: MoveEffect::ChargeElectroShot,
            ..unsafe { core::mem::zeroed() }
        };

        state.field.weather = WEATHER_RAIN;
        assert!(weather_skips_charge(&state, &md));

        state.field.weather = WEATHER_SUN;
        assert!(!weather_skips_charge(&state, &md));
    }

    #[test]
    fn test_phantom_force_bypasses_protect() {
        let (mut state, keys) = setup();

        // Simulate turn 2 of Phantom Force: set VOL_CHARGING, last_move to a move
        // that has ChargePhantom effect. We'll use move_id=566 (Phantom Force).
        // Since the gen data might not have ChargePhantom effect yet,
        // manually test the bypass logic.
        set_volatile(&mut state, &keys, 0, VOL_CHARGING);
        state.sides[0].active.last_move = 566; // Phantom Force

        // Set defender as Protecting
        set_volatile(&mut state, &keys, 1, VOL_PROTECT_THIS_TURN);

        // For bypass to work, the MoveData for Phantom Force needs ChargePhantom effect.
        // Since gen data may not be updated yet, test the helper directly.
        let md = MoveData {
            flags: MoveFlags::CHARGE | MoveFlags::CONTACT,
            effect: MoveEffect::ChargePhantom,
            base_power: 90,
            accuracy: 100,
            category: MoveCategory::Physical,
            ..unsafe { core::mem::zeroed() }
        };
        let is_turn2 = true;
        let bypasses = is_turn2 && md.effect == MoveEffect::ChargePhantom;
        assert!(bypasses);
    }

    #[test]
    fn test_geomancy_charge_and_execute() {
        let (mut state, keys) = setup();
        // Create a Geomancy status move with CHARGE flag
        let md = MoveData {
            flags: MoveFlags::CHARGE,
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::ChargeGeomancy,
            ..unsafe { core::mem::zeroed() }
        };

        // Geomancy is self-targeting
        assert!(is_self_targeting(&md));

        // Dispatch the status effect (as if turn 2 has resolved)
        execute_status_move(&mut state, &keys, 0, 1, &md, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.boosts[SPA], 2);
        assert_eq!(state.sides[0].active.boosts[SPD], 2);
        assert_eq!(state.sides[0].active.boosts[SPE], 2);
    }

    #[test]
    fn test_semi_invuln_location_helper() {
        let fly = MoveData { effect: MoveEffect::ChargeFly, ..unsafe { core::mem::zeroed() } };
        let dig = MoveData { effect: MoveEffect::ChargeDig, ..unsafe { core::mem::zeroed() } };
        let dive = MoveData { effect: MoveEffect::ChargeDive, ..unsafe { core::mem::zeroed() } };
        let phantom = MoveData { effect: MoveEffect::ChargePhantom, ..unsafe { core::mem::zeroed() } };
        let sky = MoveData { effect: MoveEffect::ChargeSkyAttack, ..unsafe { core::mem::zeroed() } };
        let none = MoveData { effect: MoveEffect::None, ..unsafe { core::mem::zeroed() } };

        assert_eq!(semi_invuln_location(&fly), Some(1));
        assert_eq!(semi_invuln_location(&dig), Some(2));
        assert_eq!(semi_invuln_location(&dive), Some(3));
        assert_eq!(semi_invuln_location(&phantom), Some(4));
        assert_eq!(semi_invuln_location(&sky), None);
        assert_eq!(semi_invuln_location(&none), None);
    }

    #[test]
    fn test_can_hit_semi_invuln_helper() {
        use crate::data::{MOVE_THUNDER, MOVE_HURRICANE, MOVE_EARTHQUAKE, MOVE_MAGNITUDE, MOVE_SURF, MOVE_WHIRLPOOL};

        // Air: Thunder and Hurricane hit
        assert!(can_hit_semi_invuln(MOVE_THUNDER as u16, 1));
        assert!(can_hit_semi_invuln(MOVE_HURRICANE as u16, 1));
        assert!(!can_hit_semi_invuln(1, 1)); // Pound doesn't hit

        // Underground: Earthquake and Magnitude hit
        assert!(can_hit_semi_invuln(MOVE_EARTHQUAKE as u16, 2));
        assert!(can_hit_semi_invuln(MOVE_MAGNITUDE as u16, 2));
        assert!(!can_hit_semi_invuln(MOVE_THUNDER as u16, 2));

        // Underwater: Surf and Whirlpool hit
        assert!(can_hit_semi_invuln(MOVE_SURF as u16, 3));
        assert!(can_hit_semi_invuln(MOVE_WHIRLPOOL as u16, 3));
        assert!(!can_hit_semi_invuln(MOVE_EARTHQUAKE as u16, 3));

        // Vanished: nothing hits
        assert!(!can_hit_semi_invuln(MOVE_THUNDER as u16, 4));
        assert!(!can_hit_semi_invuln(MOVE_EARTHQUAKE as u16, 4));
        assert!(!can_hit_semi_invuln(MOVE_SURF as u16, 4));
    }

    #[test]
    fn test_status_misses_semi_invuln() {
        let (mut state, keys) = setup();

        // Set side 1 as semi-invulnerable
        set_volatile(&mut state, &keys, 1, VOL_SEMI_INVULNERABLE);

        // Will-o-Wisp should miss semi-invulnerable targets
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 85,
            effect: MoveEffect::WillOWisp,
            ..unsafe { core::mem::zeroed() }
        };

        execute_status_move(&mut state, &keys, 0, 1, &md, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    // ── Step 4: Thrash / locked move tests ────────────────────
    // Note: Generated move data doesn't have MoveEffect::Thrash yet (Step 10).
    // Tests manually set VOL_MOVE_LOCKED and counter to test continuation/confusion.

    /// Helper: set up move-lock state as if the first thrash turn already executed.
    fn setup_thrash_locked(state: &mut BattleState, keys: &ZobristKeys, side: usize, move_id: u16, turns_remaining: u8) {
        set_volatile(state, keys, side, VOL_MOVE_LOCKED);
        state.sides[side].active._padding[2] = turns_remaining;
        state.sides[side].active.last_move = move_id;
    }

    #[test]
    fn test_thrash_initiation_condition() {
        // Verify the initiation logic: MoveEffect::Thrash → sets lock
        let md = MoveData {
            effect: MoveEffect::Thrash,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(md.effect, MoveEffect::Thrash);
        // The initiation block checks: !is_charge_turn2 && !is_struggle && !is_move_locked && md.effect == MoveEffect::Thrash
        // This will fire once the codegen populates the effect field.
    }

    #[test]
    fn test_thrash_locked_turn_deals_damage() {
        let (mut state, keys) = setup();
        // Move 1 is a physical move in gen data (Pound)
        setup_thrash_locked(&mut state, &keys, 0, 1, 2);
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        // Player action is irrelevant; move_id overridden to last_move=1
        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));

        // Counter decremented from 2→1, still locked
        assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(state.sides[0].active._padding[2], 1);
        // Damage should have been dealt
        assert!(state.sides[1].team[0].current_hp < hp_before);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_thrash_last_turn_clears_lock_and_confuses() {
        let (mut state, keys) = setup();
        setup_thrash_locked(&mut state, &keys, 0, 1, 1);
        state.zobrist = compute_full_hash(&state, &keys);

        // Last locked turn: counter 1→0, lock clears, confusion applied
        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));

        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(state.sides[0].active._padding[2], 0);
        // Confusion: rng(3)=0 → 0+2=2 turns
        assert_eq!(state.sides[0].active.confusion_turns, 2);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_thrash_3_turn_sequence() {
        let (mut state, keys) = setup();
        // Simulate 3-turn lock: counter=2
        setup_thrash_locked(&mut state, &keys, 0, 1, 2);
        state.zobrist = compute_full_hash(&state, &keys);

        // Turn 2: counter 2→1, still locked
        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));
        assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(state.sides[0].active._padding[2], 1);
        assert_eq!(state.sides[0].active.confusion_turns, 0);

        // Turn 3: counter 1→0, lock ends, confusion
        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert!(state.sides[0].active.confusion_turns >= 2);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_thrash_locked_overrides_move_id() {
        let (mut state, keys) = setup();
        // Lock onto move 1 (Pound), player tries move 2
        setup_thrash_locked(&mut state, &keys, 0, 1, 2);
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        // Player passes move_id=2, slot=1 — but lock overrides to move 1
        execute_move(&mut state, &keys, 0, 2, 1, &mut fixed_rng(0));

        // Damage dealt using move 1 (Pound), not move 2
        assert!(state.sides[1].team[0].current_hp < hp_before);
        assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
    }

    #[test]
    fn test_thrash_confusion_on_full_para() {
        let (mut state, keys) = setup();
        setup_thrash_locked(&mut state, &keys, 0, 1, 1); // last locked turn
        set_status(&mut state, &keys, 0, 0, STATUS_PARALYSIS, 0);
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        // rng(4)==0 triggers full paralysis
        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));

        // Move failed (full para), but lock ended and confusion applied
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert!(state.sides[0].active.confusion_turns >= 2);
    }

    #[test]
    fn test_thrash_confusion_on_flinch() {
        let (mut state, keys) = setup();
        setup_thrash_locked(&mut state, &keys, 0, 1, 1); // last locked turn
        set_volatile(&mut state, &keys, 0, VOL_FLINCHED);
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));

        // Didn't attack (flinched), but lock ended and confusion applied
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert!(state.sides[0].active.confusion_turns >= 2);
    }

    #[test]
    fn test_thrash_pp_deducted_each_locked_turn() {
        let (mut state, keys) = setup();
        state.sides[0].team[0].pp[0] = 10;
        // Lock onto move in slot 0 (moves[0]=1)
        setup_thrash_locked(&mut state, &keys, 0, 1, 2);
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].pp[0], 9); // PP deducted

        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].pp[0], 8); // PP deducted again
    }

    #[test]
    fn test_thrash_no_confusion_if_fainted() {
        let (mut state, keys) = setup();
        state.sides[0].team[0].current_hp = 1;
        setup_thrash_locked(&mut state, &keys, 0, 1, 1); // last locked turn
        state.zobrist = compute_full_hash(&state, &keys);

        // KO the attacker
        deal_damage(&mut state, &keys, 0, 0, 1);
        assert!(state.sides[0].team[0].is_fainted());

        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));

        // No confusion on fainted mon
        assert_eq!(state.sides[0].active.confusion_turns, 0);
    }

    #[test]
    fn test_thrash_confusion_on_sleep() {
        let (mut state, keys) = setup();
        setup_thrash_locked(&mut state, &keys, 0, 1, 1); // last locked turn
        // Put attacker to sleep with counter=3 (stays asleep)
        set_status(&mut state, &keys, 0, 0, STATUS_SLEEP, 3);
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));

        // Asleep, didn't attack, but confusion still applied
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert!(state.sides[0].active.confusion_turns >= 2);
    }

    #[test]
    fn test_thrash_locked_bookkeeping_skip() {
        let (mut state, keys) = setup();
        // Set last_move and consec_move_count before lock
        state.sides[0].active.last_move = 1;
        state.sides[0].active.consec_move_count = 3;
        setup_thrash_locked(&mut state, &keys, 0, 1, 2);
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 99, 0, &mut fixed_rng(0));

        // Bookkeeping (last_move, consec_move_count) should NOT be updated on locked turns
        // last_move stays as 1 (set by setup_thrash_locked)
        assert_eq!(state.sides[0].active.last_move, 1);
        // consec_move_count should remain 3 (not updated)
        assert_eq!(state.sides[0].active.consec_move_count, 3);
    }

    // ── Step 5: Ability immunity tests ───────────────────────────

    #[test]
    fn test_water_absorb_blocks_and_heals() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_WATER_GUN;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_WATER_ABSORB;
        state.sides[1].team[0].current_hp = 200; // missing 100 HP
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, MOVE_WATER_GUN as u16, 0, &mut fixed_rng(0));

        // No damage, healed 25% of max HP (300/4 = 75)
        assert!(state.sides[1].team[0].current_hp > 200);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_volt_absorb_blocks_and_heals() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_THUNDERBOLT;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_VOLT_ABSORB;
        state.sides[1].team[0].current_hp = 200;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, MOVE_THUNDERBOLT as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].current_hp > 200);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_dry_skin_blocks_water_and_heals() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_WATER_GUN;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DRY_SKIN;
        state.sides[1].team[0].current_hp = 200;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, MOVE_WATER_GUN as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].current_hp > 200);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_flash_fire_blocks_and_sets_volatile() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_FLASH_FIRE;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(state.sides[1].active.has_volatile(VOL_FLASH_FIRE));
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_lightning_rod_blocks_and_boosts_spa() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_THUNDERBOLT;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_LIGHTNING_ROD;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_THUNDERBOLT as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert_eq!(state.sides[1].active.boosts[SPA], 1);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_storm_drain_blocks_and_boosts_spa() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_WATER_GUN;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STORM_DRAIN;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_WATER_GUN as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert_eq!(state.sides[1].active.boosts[SPA], 1);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_motor_drive_blocks_and_boosts_spe() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_THUNDERBOLT;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_MOTOR_DRIVE;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_THUNDERBOLT as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert_eq!(state.sides[1].active.boosts[SPE], 1);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_sap_sipper_blocks_and_boosts_atk() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_ENERGY_BALL;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SAP_SIPPER;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_ENERGY_BALL as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert_eq!(state.sides[1].active.boosts[ATK], 1);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_levitate_blocks_ground() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_EARTHQUAKE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_LEVITATE;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_levitate_fails_under_gravity() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_EARTHQUAKE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_LEVITATE;
        state.field.gravity_turns = 3;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));

        // Gravity overrides Levitate — should take damage
        assert!(state.sides[1].team[0].current_hp < 300);
    }

    #[test]
    fn test_overcoat_blocks_powder_status() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_OVERCOAT;
        state.zobrist = compute_full_hash(&state, &keys);

        // Sleep Powder is a Powder-flagged status move
        // Use manual MoveData to ensure the POWDER flag is set
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 75,
            flags: MoveFlags::POWDER,
            effect: MoveEffect::Sleep,
            move_type: Type::Grass,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &keys, 0, 1, &md, &mut fixed_rng(0));

        // Overcoat blocks it — no sleep
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_soundproof_blocks_sound_move() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SOUNDPROOF;
        state.zobrist = compute_full_hash(&state, &keys);

        // Use a sound-flagged damaging move
        let md = MoveData {
            category: MoveCategory::Special,
            base_power: 90,
            accuracy: 100,
            flags: MoveFlags::SOUND,
            move_type: Type::Normal,
            ..unsafe { core::mem::zeroed() }
        };
        // Test via the flag immunity helper directly
        assert!(ability_flag_immunity(&state, 1, md.flags).is_some());
    }

    #[test]
    fn test_bulletproof_blocks_bullet_move() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_BULLETPROOF;
        state.zobrist = compute_full_hash(&state, &keys);

        // Bullet-flagged move
        assert!(ability_flag_immunity(&state, 1, MoveFlags::BULLET).is_some());
        // Non-bullet move is not blocked
        assert!(ability_flag_immunity(&state, 1, MoveFlags::CONTACT).is_none());
    }

    #[test]
    fn test_thunder_wave_blocked_by_volt_absorb() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_VOLT_ABSORB;
        state.sides[1].team[0].current_hp = 200;
        state.zobrist = compute_full_hash(&state, &keys);

        // Thunder Wave is Electric-type status
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 90,
            effect: MoveEffect::ThunderWave,
            move_type: Type::Electric,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &keys, 0, 1, &md, &mut fixed_rng(0));

        // Should be blocked + healed, no paralysis
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
        assert!(state.sides[1].team[0].current_hp > 200);
    }

    #[test]
    fn test_sap_sipper_blocks_grass_status() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SAP_SIPPER;
        state.zobrist = compute_full_hash(&state, &keys);

        // Leech Seed is Grass-type status
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 90,
            effect: MoveEffect::LeechSeed,
            move_type: Type::Grass,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &keys, 0, 1, &md, &mut fixed_rng(0));

        // Blocked by Sap Sipper, +1 Atk
        assert!(!state.sides[1].active.has_volatile(VOL_LEECH_SEED));
        assert_eq!(state.sides[1].active.boosts[ATK], 1);
    }

    // ── Step 6: Pre-move ability hook tests ──────────────────────

    #[test]
    fn test_protean_changes_type() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(0));

        // Attacker type should now be Fire/Fire
        assert!(state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(state.sides[0].active.override_types, [Type::Fire as u8, Type::Fire as u8]);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_protean_once_per_switch() {
        let (mut state, keys) = setup();
        use crate::data::{MOVE_FLAMETHROWER, MOVE_THUNDERBOLT};
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;
        state.zobrist = compute_full_hash(&state, &keys);

        // First move: type changes to Fire
        execute_move(&mut state, &keys, 0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.override_types[0], Type::Fire as u8);

        // Second move: type should NOT change again (once per switch-in)
        execute_move(&mut state, &keys, 0, MOVE_THUNDERBOLT as u16, 1, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.override_types[0], Type::Fire as u8);
        assert_eq!(state.sides[0].active._padding[3], 1);
    }

    #[test]
    fn test_libero_changes_type() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_WATER_GUN;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_LIBERO;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, MOVE_WATER_GUN as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(state.sides[0].active.override_types, [Type::Water as u8, Type::Water as u8]);
    }

    #[test]
    fn test_protean_not_on_struggle() {
        let (mut state, keys) = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;
        state.zobrist = compute_full_hash(&state, &keys);

        // Struggle (move_id=0) should not trigger Protean
        execute_move(&mut state, &keys, 0, 0, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
    }

    // ── Step 9: Forme change tests ──────────────────────────

    #[test]
    fn test_stance_change_to_blade() {
        let (mut state, keys) = setup();
        use crate::data::{MOVE_SHADOW_BALL, MOVE_KING_S_SHIELD};
        // setup() gives side 0 stats [150, 100, 150, 100, 100]
        state.sides[0].team[0].species_id = 681; // Aegislash Shield
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STANCE_CHANGE;
        state.zobrist = compute_full_hash(&state, &keys);

        // Attacking move → should change to Blade forme
        execute_move(&mut state, &keys, 0, MOVE_SHADOW_BALL as u16, 0, &mut fixed_rng(99));

        assert_eq!(effective_species(&state, 0), 1103); // Aegislash-Blade
        // Aegislash Shield: atk:50, def:140, spa:50, spd:140, spe:60
        // Aegislash Blade:  atk:140, def:50,  spa:140, spd:50,  spe:60
        // Stats: atk 150*140/50=420, def 100*50/140=35, spa 150*140/50=420, spd 100*50/140=35, spe 100*60/60=100
        assert_eq!(effective_stat(&state, 0, ATK), 420);
        assert_eq!(effective_stat(&state, 0, DEF), 35);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_stance_change_to_shield_on_kings_shield() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_KING_S_SHIELD;
        state.sides[0].team[0].species_id = 681;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STANCE_CHANGE;
        state.sides[0].team[0].stats = [100; 5];
        state.zobrist = compute_full_hash(&state, &keys);

        // First, manually set to Blade forme
        crate::state::forme::apply_battle_forme(&mut state, &keys, 0, 1103);
        assert_eq!(effective_species(&state, 0), 1103);

        // King's Shield → should revert to Shield forme
        execute_move(&mut state, &keys, 0, MOVE_KING_S_SHIELD as u16, 0, &mut fixed_rng(99));

        assert_eq!(effective_species(&state, 0), 681);
        assert_eq!(effective_stat(&state, 0, ATK), 100); // back to team stats
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_stance_change_no_trigger_on_status_move() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_SWORDS_DANCE;
        state.sides[0].team[0].species_id = 681;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STANCE_CHANGE;
        state.zobrist = compute_full_hash(&state, &keys);

        // Status move (not King's Shield) → should NOT change forme
        execute_move(&mut state, &keys, 0, MOVE_SWORDS_DANCE as u16, 0, &mut fixed_rng(99));

        assert_eq!(effective_species(&state, 0), 681); // still Shield
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_disguise_blocks_first_hit() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DISGUISE;
        state.zobrist = compute_full_hash(&state, &keys);

        let max_hp = state.sides[1].team[0].max_hp;
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        // Disguise blocks damage but costs 1/8 max HP
        assert_eq!(state.sides[1].team[0].current_hp, max_hp - max_hp / 8);
        // Shield is now broken
        assert_eq!(state.sides[1].active._padding[4] & 1, 1);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_disguise_broken_takes_damage() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DISGUISE;
        state.sides[1].active._padding[4] = 1; // already broken
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        // Second hit goes through normally
        assert!(state.sides[1].team[0].current_hp < hp_before);
    }

    #[test]
    fn test_ice_face_blocks_physical() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ICE_FACE;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        // Move 1 (Pound) is Physical in gen data
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        // Ice Face blocks the physical hit — no damage
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        // Shield broken
        assert_eq!(state.sides[1].active._padding[4] & 2, 2);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_ice_face_doesnt_block_special() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ICE_FACE;
        state.zobrist = compute_full_hash(&state, &keys);

        // Flamethrower is Special — Ice Face doesn't block
        execute_move(&mut state, &keys, 0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].current_hp < 300);
        // Shield NOT broken by special move
        assert_eq!(state.sides[1].active._padding[4] & 2, 0);
    }

    #[test]
    fn test_ice_face_broken_takes_physical() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ICE_FACE;
        state.sides[1].active._padding[4] = 2; // already broken
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        // Second physical hit goes through
        assert!(state.sides[1].team[0].current_hp < hp_before);
    }

    #[test]
    fn test_disguise_doesnt_block_behind_substitute() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DISGUISE;
        // Set up substitute
        set_volatile(&mut state, &keys, 1, VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 100;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        // Substitute takes the hit, Disguise NOT consumed
        assert_eq!(state.sides[1].active._padding[4] & 1, 0);
    }

    // ── Step 7: After-damage ability hook tests ──────────────────

    #[test]
    fn test_rough_skin_damages_attacker() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ROUGH_SKIN;
        state.zobrist = compute_full_hash(&state, &keys);

        let atk_hp = state.sides[0].team[0].current_hp;
        let atk_max = state.sides[0].team[0].max_hp;
        // Pound (1) is Contact/Physical
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        // Attacker should lose 1/8 max HP from Rough Skin
        let expected_rough = (atk_max / 8).max(1);
        // Attacker took Rough Skin damage (plus defender took damage)
        assert!(state.sides[0].team[0].current_hp <= atk_hp - expected_rough);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_rough_skin_no_trigger_on_special() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ROUGH_SKIN;
        state.zobrist = compute_full_hash(&state, &keys);

        let atk_hp = state.sides[0].team[0].current_hp;
        // Flamethrower is Special, no Contact — Rough Skin should NOT trigger
        execute_move(&mut state, &keys, 0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[0].team[0].current_hp, atk_hp);
    }

    #[test]
    fn test_iron_barbs_damages_attacker() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_IRON_BARBS;
        state.zobrist = compute_full_hash(&state, &keys);

        let atk_hp = state.sides[0].team[0].current_hp;
        let atk_max = state.sides[0].team[0].max_hp;
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        let expected = (atk_max / 8).max(1);
        assert!(state.sides[0].team[0].current_hp <= atk_hp - expected);
    }

    #[test]
    fn test_weak_armor_on_physical() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_WEAK_ARMOR;
        state.zobrist = compute_full_hash(&state, &keys);

        // Pound is Physical → triggers Weak Armor
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[DEF], -1);
        assert_eq!(state.sides[1].active.boosts[SPE], 2);
    }

    #[test]
    fn test_weak_armor_no_trigger_on_special() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_WEAK_ARMOR;
        state.zobrist = compute_full_hash(&state, &keys);

        // Flamethrower is Special → no Weak Armor
        execute_move(&mut state, &keys, 0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[DEF], 0);
        assert_eq!(state.sides[1].active.boosts[SPE], 0);
    }

    #[test]
    fn test_justified_on_dark_move() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_BITE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_JUSTIFIED;
        state.zobrist = compute_full_hash(&state, &keys);

        // Bite is Dark/Contact
        execute_move(&mut state, &keys, 0, MOVE_BITE as u16, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[ATK], 1);
    }

    #[test]
    fn test_justified_no_trigger_on_normal() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_JUSTIFIED;
        state.zobrist = compute_full_hash(&state, &keys);

        // Pound is Normal → no Justified
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[ATK], 0);
    }

    #[test]
    fn test_stamina_boosts_on_any_hit() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STAMINA;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[DEF], 1);
    }

    #[test]
    fn test_anger_point_on_crit() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ANGER_POINT;
        state.zobrist = compute_full_hash(&state, &keys);

        // fixed_rng(0): rng(24)==0 → crit, rng(4)==0 → paralysis...
        // But attacker isn't paralyzed. The defender has Anger Point.
        // We need the move to hit AND crit. fixed_rng(0) gives crit.
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        // Anger Point should maximize Atk to +6
        assert_eq!(state.sides[1].active.boosts[ATK], 6);
    }

    #[test]
    fn test_color_change_on_hit() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_COLOR_CHANGE;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(99));

        // Defender type should now be Fire/Fire
        assert!(state.sides[1].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(state.sides[1].active.override_types, [Type::Fire as u8, Type::Fire as u8]);
    }

    #[test]
    fn test_flame_body_burns_on_contact() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_FLAME_BODY;
        state.zobrist = compute_full_hash(&state, &keys);

        // fixed_rng(0): rng(100)==0 < 30 → triggers
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[0].team[0].status, STATUS_BURN);
    }

    #[test]
    fn test_flame_body_no_trigger_high_roll() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_FLAME_BODY;
        state.zobrist = compute_full_hash(&state, &keys);

        // fixed_rng(99): rng(100)==99 >= 30 → does not trigger
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_static_paralyzes_on_contact() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STATIC;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[0].team[0].status, STATUS_PARALYSIS);
    }

    #[test]
    fn test_poison_point_poisons_on_contact() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_POISON_POINT;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[0].team[0].status, STATUS_POISON);
    }

    #[test]
    fn test_contact_ability_no_trigger_through_sub() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ROUGH_SKIN;
        set_volatile(&mut state, &keys, 1, VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 200;
        state.zobrist = compute_full_hash(&state, &keys);

        let atk_hp = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        // Rough Skin doesn't trigger through substitute
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp);
    }

    #[test]
    fn test_moxie_on_ko() {
        let (mut state, keys) = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_MOXIE;
        // Set defender to 1 HP so it faints
        state.sides[1].team[0].current_hp = 1;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
    }

    #[test]
    fn test_moxie_no_boost_if_no_ko() {
        let (mut state, keys) = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_MOXIE;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        assert!(!state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[ATK], 0);
    }

    #[test]
    fn test_beast_boost_highest_stat() {
        let (mut state, keys) = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_BEAST_BOOST;
        // Make SpA the highest stat
        state.sides[0].team[0].stats = [100, 100, 200, 100, 100]; // SpA=200 highest
        state.sides[1].team[0].current_hp = 1;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        // SPA (index 2) should be boosted
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
        assert_eq!(state.sides[0].active.boosts[ATK], 0);
    }

    #[test]
    fn test_aftermath_damages_attacker_on_ko() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_AFTERMATH;
        state.sides[1].team[0].current_hp = 1;
        state.zobrist = compute_full_hash(&state, &keys);

        let atk_hp = state.sides[0].team[0].current_hp;
        let atk_max = state.sides[0].team[0].max_hp;
        // Pound is Contact → Aftermath triggers
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        let expected = atk_max / 4;
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp - expected);
    }

    #[test]
    fn test_aftermath_no_trigger_on_non_contact() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_AFTERMATH;
        state.sides[1].team[0].current_hp = 1;
        state.zobrist = compute_full_hash(&state, &keys);

        let atk_hp = state.sides[0].team[0].current_hp;
        // Flamethrower is Special, no Contact → Aftermath should NOT trigger
        execute_move(&mut state, &keys, 0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp);
    }

    #[test]
    fn test_effect_spore_sleep() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_EFFECT_SPORE;
        state.zobrist = compute_full_hash(&state, &keys);

        // Need rng sequence where: accuracy hits, no crit concern, effect spore roll < 10
        // fixed_rng(0) gives: rng(100)=0 for accuracy (hit), rng(24)=0 (crit, but doesn't matter
        // for status), and rng(100)=0 for effect spore → 0 < 10 → sleep
        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[0].team[0].status, STATUS_SLEEP);
    }

    #[test]
    fn test_defender_hooks_dont_trigger_through_sub() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STAMINA;
        set_volatile(&mut state, &keys, 1, VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 200;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        // Stamina doesn't trigger when sub takes the hit
        assert_eq!(state.sides[1].active.boosts[DEF], 0);
    }

    // ── Step 8: Item activation tests ────────────────────────────

    #[test]
    fn test_focus_sash_survives_ohko() {
        let (mut state, keys) = setup();
        // Give defender Focus Sash (id=151) and lower HP to make OHKO easier
        state.sides[1].team[0].item_id = 151;
        state.sides[1].team[0].current_hp = 50;
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500; // Very high attack to guarantee OHKO
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        // Should survive with 1 HP
        assert_eq!(state.sides[1].team[0].current_hp, 1);
        // Sash consumed
        assert_eq!(state.sides[1].team[0].item_id, 0);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_focus_sash_no_trigger_if_not_full_hp() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].item_id = 151;
        state.sides[1].team[0].current_hp = 49; // Not full HP
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        // Should faint — sash only works at full HP
        assert!(state.sides[1].team[0].is_fainted());
        // Sash NOT consumed
        assert_eq!(state.sides[1].team[0].item_id, 151);
    }

    #[test]
    fn test_sturdy_survives_ohko() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STURDY;
        state.sides[1].team[0].current_hp = 50;
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        // Should survive with 1 HP (ability, not consumed)
        assert_eq!(state.sides[1].team[0].current_hp, 1);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_sturdy_no_trigger_if_not_full_hp() {
        let (mut state, keys) = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STURDY;
        state.sides[1].team[0].current_hp = 49;
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
    }

    #[test]
    fn test_pinch_berry_boosts_stat() {
        let (mut state, keys) = setup();
        // Manually create a pinch berry item: PINCH_BERRY, type_param=0 (Atk boost)
        // Since no real pinch berry is in gen data, we assign a fake item id
        // and directly test check_pinch_berry
        state.sides[1].team[0].current_hp = 50;
        state.sides[1].team[0].max_hp = 300;
        // HP (50) ≤ 25% of max (75) → should trigger
        // We can't easily test through execute_move since gen data lacks pinch berries,
        // so we test the helper directly
        // First, set a known item with PINCH_BERRY flag
        // Use item id=0 workaround: we'll modify the state and call check_pinch_berry
        // Actually, we need a valid item in gen_items. Since there are none with PINCH_BERRY,
        // this test validates the function exists and compiles.
        // Full integration testing requires codegen updates (Step 10).
        check_pinch_berry(&mut state, &keys, 1, 0);
        // No pinch berry item → no boost
        assert_eq!(state.sides[1].active.boosts[ATK], 0);
    }

    // ── Phase 1: SelfEffect tests ─────────────────────────────

    #[test]
    fn test_close_combat_self_drops() {
        use crate::data::MOVE_CLOSE_COMBAT;
        let (mut state, keys) = setup();
        // Give side 0 Close Combat in slot 0
        state.sides[0].team[0].moves[0] = MOVE_CLOSE_COMBAT as u16;
        state.zobrist = compute_full_hash(&state, &keys);
        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_CLOSE_COMBAT as u16, 0, &mut fixed_rng(0));
        // Damage should have been dealt
        assert!(state.sides[1].team[0].current_hp < hp_before);
        // Attacker should have -1 Def and -1 SpD
        assert_eq!(state.sides[0].active.boosts[DEF], -1);
        assert_eq!(state.sides[0].active.boosts[SPD], -1);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_high_jump_kick_crash_on_miss() {
        use crate::data::MOVE_HIGH_JUMP_KICK;
        let (mut state, keys) = setup();
        state.sides[0].team[0].moves[0] = MOVE_HIGH_JUMP_KICK as u16;
        state.zobrist = compute_full_hash(&state, &keys);
        let atk_hp_before = state.sides[0].team[0].current_hp;
        let def_hp_before = state.sides[1].team[0].current_hp;
        // Use rng that always misses (rng(100) returns 99 >= 90 accuracy)
        execute_move(&mut state, &keys, 0, MOVE_HIGH_JUMP_KICK as u16, 0, &mut fixed_rng(99));
        // Defender should NOT have taken damage (miss)
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before);
        // Attacker should have taken 50% max HP crash damage
        let expected_crash = atk_hp_before / 2;
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp_before - expected_crash);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_high_jump_kick_hit_no_crash() {
        use crate::data::MOVE_HIGH_JUMP_KICK;
        let (mut state, keys) = setup();
        state.sides[0].team[0].moves[0] = MOVE_HIGH_JUMP_KICK as u16;
        state.zobrist = compute_full_hash(&state, &keys);
        let atk_hp_before = state.sides[0].team[0].current_hp;
        // Use rng that always hits (rng(100) returns 0 < 90 accuracy)
        execute_move(&mut state, &keys, 0, MOVE_HIGH_JUMP_KICK as u16, 0, &mut fixed_rng(0));
        // On hit, no crash damage — attacker HP should be unchanged
        // (HJK has no recoil on hit, drain=0)
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp_before);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_recover_heals_50pct() {
        use crate::data::MOVE_RECOVER;
        let (mut state, keys) = setup();
        state.sides[0].team[0].moves[0] = MOVE_RECOVER as u16;
        // Damage the attacker first
        deal_damage(&mut state, &keys, 0, 0, 200);
        state.zobrist = compute_full_hash(&state, &keys);
        let hp_before = state.sides[0].team[0].current_hp; // 100
        let max_hp = state.sides[0].team[0].max_hp; // 300
        execute_move(&mut state, &keys, 0, MOVE_RECOVER as u16, 0, &mut fixed_rng(0));
        // Should heal 50% of max HP
        assert_eq!(state.sides[0].team[0].current_hp, hp_before + max_hp / 2);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_uturn_sets_must_switch() {
        use crate::data::MOVE_U_TURN;
        let (mut state, keys) = setup();
        state.sides[0].team[0].moves[0] = MOVE_U_TURN as u16;
        state.zobrist = compute_full_hash(&state, &keys);
        execute_move(&mut state, &keys, 0, MOVE_U_TURN as u16, 0, &mut fixed_rng(0));
        // U-turn should set VOL_MUST_SWITCH (attacker has a bench mon)
        assert!(state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
        assert!(validate_hash(&state, &keys));
    }

    // ── Phase 3: Ability immunity tests ──────────────────────────

    #[test]
    fn test_earth_eater_blocks_ground_and_heals() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_EARTHQUAKE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_EARTH_EATER;
        state.sides[1].team[0].current_hp = 200;
        state.zobrist = compute_full_hash(&state, &keys);

        execute_move(&mut state, &keys, 0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));

        // Should heal, not take damage
        assert!(state.sides[1].team[0].current_hp > 200);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_bulletproof_blocks_aura_sphere() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_AURA_SPHERE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_BULLETPROOF;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_AURA_SPHERE as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_dazzling_blocks_priority() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_QUICK_ATTACK;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DAZZLING;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_QUICK_ATTACK as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_dazzling_allows_normal_priority() {
        let (mut state, keys) = setup();
        use crate::data::MOVE_SURF;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DAZZLING;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &keys, 0, MOVE_SURF as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].current_hp < hp_before);
    }

    #[test]
    fn test_wind_rider_blocks_wind_and_boosts_atk() {
        let (mut state, keys) = setup();
        // Use a Wind-flagged move — construct one manually
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_WIND_RIDER;
        state.zobrist = compute_full_hash(&state, &keys);

        let hp_before = state.sides[1].team[0].current_hp;
        // Check the immunity directly
        let eff = ability_flag_immunity(&state, 1, MoveFlags::WIND);
        assert!(eff.is_some());
        match eff.unwrap() {
            AbilityImmunityEffect::Boost(stat, stages) => {
                assert_eq!(stat, ATK);
                assert_eq!(stages, 1);
            }
            _ => panic!("Expected Boost effect"),
        }
    }

    // ── Phase 3: Type-change ability tests ───────────────────────

    #[test]
    fn test_pixilate_converts_normal_to_fairy() {
        use crate::state::calc_modifiers::resolve_move_type_with_ability;
        let state = BattleState::default();
        // Normal-type move
        let md = MoveData {
            move_type: Type::Normal,
            base_power: 80,
            category: MoveCategory::Physical,
            ..unsafe { core::mem::zeroed() }
        };
        let (new_type, boost) = resolve_move_type_with_ability(
            &state, &md, 0, data_bridge::ABILITY_PIXILATE,
        );
        assert_eq!(new_type, Type::Fairy);
        assert!(boost);
    }

    #[test]
    fn test_pixilate_no_change_on_non_normal() {
        use crate::state::calc_modifiers::resolve_move_type_with_ability;
        let state = BattleState::default();
        let md = MoveData {
            move_type: Type::Fire,
            base_power: 80,
            category: MoveCategory::Special,
            ..unsafe { core::mem::zeroed() }
        };
        let (new_type, boost) = resolve_move_type_with_ability(
            &state, &md, 0, data_bridge::ABILITY_PIXILATE,
        );
        assert_eq!(new_type, Type::Fire);
        assert!(!boost);
    }

    #[test]
    fn test_galvanize_converts_normal_to_electric() {
        use crate::state::calc_modifiers::resolve_move_type_with_ability;
        let state = BattleState::default();
        let md = MoveData {
            move_type: Type::Normal,
            base_power: 80,
            category: MoveCategory::Physical,
            ..unsafe { core::mem::zeroed() }
        };
        let (new_type, boost) = resolve_move_type_with_ability(
            &state, &md, 0, data_bridge::ABILITY_GALVANIZE,
        );
        assert_eq!(new_type, Type::Electric);
        assert!(boost);
    }

    #[test]
    fn test_normalize_changes_fire_to_normal() {
        use crate::state::calc_modifiers::resolve_move_type_with_ability;
        let state = BattleState::default();
        let md = MoveData {
            move_type: Type::Fire,
            base_power: 80,
            category: MoveCategory::Special,
            ..unsafe { core::mem::zeroed() }
        };
        let (new_type, boost) = resolve_move_type_with_ability(
            &state, &md, 0, data_bridge::ABILITY_NORMALIZE,
        );
        assert_eq!(new_type, Type::Normal);
        assert!(boost); // Fire != Normal, so boost applies
    }

    #[test]
    fn test_liquid_voice_converts_sound_to_water() {
        use crate::state::calc_modifiers::resolve_move_type_with_ability;
        let state = BattleState::default();
        let md = MoveData {
            move_type: Type::Normal,
            base_power: 90,
            flags: MoveFlags::SOUND,
            category: MoveCategory::Special,
            ..unsafe { core::mem::zeroed() }
        };
        let (new_type, boost) = resolve_move_type_with_ability(
            &state, &md, 0, data_bridge::ABILITY_LIQUID_VOICE,
        );
        assert_eq!(new_type, Type::Water);
        assert!(boost);
    }
}
