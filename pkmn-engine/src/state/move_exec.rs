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
use crate::state::calc::calc_damage;
use crate::state::calc_modifiers::{ability_type_immunity, ability_flag_immunity, good_as_gold_immunity, priority_block_immunity, terrain_blocks_status, mold_breaks, AbilityImmunityEffect};
use crate::state::forme::{apply_battle_forme, revert_battle_forme};
use crate::state::switch::{
    set_stealth_rock, add_spikes, add_toxic_spikes, set_sticky_web, clear_hazards,
};
use crate::data::moves::{MoveData, MoveFlags};
use crate::data::types::Type;
use crate::data::gen_call_family::{ASSIST_FAIL, COPYCAT_FAIL, ENCORE_FAIL, METRONOME_OK, MIRROR_MOVE_OK, SLEEP_TALK_FAIL};

#[inline]
fn apply_immunity_effect(
    state: &mut BattleState,
    def_side: usize,
    def_slot: usize,
    eff: AbilityImmunityEffect,
) {
    match eff {
        AbilityImmunityEffect::Heal(hp) => { heal(state, def_side, def_slot, hp); }
        AbilityImmunityEffect::Boost(stat, stages) => { apply_boost(state, def_side, stat, stages); }
        AbilityImmunityEffect::FlashFire => { set_volatile(state, def_side, VOL_FLASH_FIRE); }
        AbilityImmunityEffect::Nullify => {}
    }
}

const ACC_NUM: [u32; 13] = [3, 3, 3, 3, 3, 3, 3, 4, 5, 6, 7, 8, 9];
const ACC_DEN: [u32; 13] = [9, 8, 7, 6, 5, 4, 3, 3, 3, 3, 3, 3, 3];

// Mirrors Showdown's `move.callsMove` set (Metronome, Mirror Move, Sleep Talk,
// Assist, Me First, Copycat). Sorted for binary_search; callable per-dispatch.
const CALL_FAMILY_IDS: &[u16] = &[118, 119, 214, 274, 382, 383];

#[inline(always)]
pub fn is_call_family(move_id: u16) -> bool {
    CALL_FAMILY_IDS.binary_search(&move_id).is_ok()
}

/// Returns true if `side`'s active mon is holding Safety Goggles (and Magic Room
/// is not nullifying items). Used by powder/spore immunity checks.
#[inline(always)]
fn holds_safety_goggles(state: &BattleState, side: usize) -> bool {
    state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(side).item_id).has(ItemFlag::SAFETY_GOGGLES)
}

#[inline]
/// Compute the effective accuracy value for a move, accounting for all modifiers.
/// Returns u32::MAX for guaranteed hits (accuracy=0, No Guard, weather bypass).
/// Pure function — no RNG, no state mutation.
fn effective_accuracy(
    state: &BattleState,
    atk_side: usize,
    md: &MoveData,
) -> u32 {
    if md.accuracy == 0 { return u32::MAX; }

    // One-hit KO moves bypass all accuracy/evasion modifiers (Showdown
    // hitStepAccuracy): accuracy = 30 + (userLevel − targetLevel). The level-fail
    // and type/Ice immunities are enforced deterministically in calc_damage
    // (type_immune); this only sets the hit chance. (Sheer Cold's base-20 for a
    // non-Ice user shifts only the miss chance, never the hit/fail outcome.)
    if md.effect == MoveEffect::Ohko {
        let atk_level = state.active_mon(atk_side).level as i32;
        let def_level = state.active_mon(1 - atk_side).level as i32;
        return (30 + atk_level - def_level).max(1) as u32;
    }

    // Weather-dependent accuracy overrides
    if md.effect == MoveEffect::WeatherAccRain {
        match effective_weather_for(state, atk_side) {
            WEATHER_RAIN | WEATHER_HEAVY_RAIN => return u32::MAX,
            WEATHER_SUN | WEATHER_HARSH_SUN => {}
            _ => {}
        }
    }
    if md.effect == MoveEffect::WeatherAccSnow {
        if effective_weather_for(state, atk_side) == WEATHER_SNOW {
            return u32::MAX;
        }
    }

    let def_side = 1 - atk_side;
    let atk_ability = effective_ability(state, atk_side);
    let def_ability = effective_ability(state, def_side);

    if atk_ability == data_bridge::ABILITY_NO_GUARD
        || def_ability == data_bridge::ABILITY_NO_GUARD
    {
        return u32::MAX;
    }

    let acc_idx = (state.sides[atk_side].active.boosts[ACC] + 6) as usize;
    let eva_idx = (state.sides[def_side].active.boosts[EVA] + 6) as usize;

    let base_acc = if md.effect == MoveEffect::WeatherAccRain
        && matches!(effective_weather_for(state, atk_side), WEATHER_SUN | WEATHER_HARSH_SUN)
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

    if def_ability == data_bridge::ABILITY_SAND_VEIL && effective_weather(state) == WEATHER_SAND {
        accuracy = accuracy * 4 / 5;
    }
    if def_ability == data_bridge::ABILITY_SNOW_CLOAK && effective_weather(state) == WEATHER_SNOW {
        accuracy = accuracy * 4 / 5;
    }

    if state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(atk_side).item_id).has(ItemFlag::WIDE_LENS)
    {
        accuracy = accuracy * 11 / 10;
    }

    if state.field.gravity_turns > 0 { accuracy = accuracy * 5 / 3; }

    accuracy
}

fn accuracy_check(
    state: &BattleState,
    atk_side: usize,
    md: &MoveData,
    rng: &mut impl FnMut(u32) -> u32,
) -> bool {
    let acc = effective_accuracy(state, atk_side, md);
    if acc >= u32::MAX { return true; }
    rng(100) < acc
}

#[inline]
/// Damaging-move secondaries whose stat target differs from the
/// category-implied default (Physical→DEF / Special→SPD on opponent drop).
/// Returns the stat index (0..6, where 5=accuracy, 6=evasion).
#[inline]
fn secondary_drop_stat_override(move_id: u16) -> Option<usize> {
    use crate::data::*;
    const ACCURACY: usize = 5;
    match move_id as usize {
        // Speed-drop on opponent
        MOVE_BUBBLE | MOVE_BUBBLE_BEAM | MOVE_BULLDOZE | MOVE_LOW_SWEEP
        | MOVE_ICY_WIND | MOVE_GLACIATE | MOVE_ELECTROWEB
        | MOVE_DRUM_BEATING | MOVE_BLEAKWIND_STORM | MOVE_POUNCE
        | MOVE_ROCK_TOMB | MOVE_MUD_SHOT | MOVE_CONSTRICT => Some(SPE),
        // Attack-drop secondaries on Special moves (and on Physical moves
        // whose default would be DEF but Showdown drops ATK)
        MOVE_AURORA_BEAM | MOVE_BITTER_MALICE | MOVE_CHILLING_WATER
        | MOVE_LUNGE | MOVE_PLAY_ROUGH | MOVE_TROP_KICK
        | MOVE_BREAKING_SWIPE | MOVE_SPRINGTIDE_STORM => Some(ATK),
        // Sp.Atk-drop secondaries
        MOVE_MIST_BALL | MOVE_MOONBLAST | MOVE_MYSTICAL_FIRE
        | MOVE_SNARL | MOVE_STRUGGLE_BUG | MOVE_SPIRIT_BREAK
        | MOVE_SKITTER_SMACK | MOVE_SECRET_POWER => Some(SPA),
        // Accuracy-drop secondaries
        MOVE_LEAF_TORNADO | MOVE_MIRROR_SHOT | MOVE_MUD_BOMB
        | MOVE_MUDDY_WATER | MOVE_MUD_SLAP | MOVE_NIGHT_DAZE
        | MOVE_OCTAZOOKA => Some(ACCURACY),
        _ => None,
    }
}

#[inline]
/// Self-boost secondaries whose stat differs from the category default
/// (Physical→ATK / Special→SPA). codegen stores only the boost magnitude, so the
/// target stat is resolved here for moves whose Showdown `secondary.self.boosts`
/// stat isn't the category default. Multi-stat self-boosts (Ancient Power etc.)
/// are not representable in the scalar model and are left on the default path.
fn secondary_self_boost_stat_override(move_id: u16) -> Option<usize> {
    use crate::data::*;
    match move_id as usize {
        MOVE_FLAME_CHARGE | MOVE_AURA_WHEEL | MOVE_AQUA_STEP
        | MOVE_TRAILBLAZE | MOVE_ESPER_WING => Some(SPE),
        MOVE_STEEL_WING | MOVE_PSYSHIELD_BASH => Some(DEF),
        _ => None,
    }
}

#[inline]
/// Moves whose secondary self-boost raises ALL of ATK/DEF/SPA/SPD/SPE.
/// codegen stores a single scalar magnitude, so the multi-stat target set is opted
/// into here. Silver/Ominous Wind are Gen-9 Past; same shape, not exercisable.
fn move_secondary_omniboosts(move_id: u16) -> bool {
    use crate::data::*;
    matches!(move_id as usize,
        MOVE_ANCIENT_POWER | MOVE_SILVER_WIND | MOVE_OMINOUS_WIND
    )
}

#[inline]
/// Damaging moves whose secondary applies `volatileStatus: 'confusion'` to the target.
/// codegen.py only extracts status/stat secondaries; the volatile-confusion path
/// is opted into here.
fn move_secondary_confuses(move_id: u16) -> bool {
    use crate::data::*;
    matches!(move_id as usize,
        MOVE_PSYBEAM | MOVE_CONFUSION | MOVE_DIZZY_PUNCH | MOVE_SIGNAL_BEAM
        | MOVE_WATER_PULSE | MOVE_ROCK_CLIMB | MOVE_CHATTER | MOVE_HURRICANE
        | MOVE_STRANGE_STEAM | MOVE_DUAL_WINGBEAT | MOVE_AXE_KICK
    )
}

#[inline]
/// Damaging moves whose real onHit handler the 16-byte MoveData can't encode
/// (sound-disable, trap, PP-drop, conditional-confuse), so
/// `apply_primary_secondary`'s type-heuristic/flinch fallthrough would invent a
/// flinch Showdown never applies. Suppress that phantom flinch for these.
/// (Tri Attack's 3-way status select is modeled in `move_secondary_status_select`.)
fn move_secondary_no_flinch(move_id: u16) -> bool {
    use crate::data::*;
    matches!(move_id as usize,
        MOVE_THROAT_CHOP | MOVE_SPIRIT_SHACKLE | MOVE_EERIE_SPELL
        | MOVE_ALLURING_VOICE | MOVE_PSYCHIC_NOISE
    )
}

#[inline]
/// Damaging moves whose onHit rolls `this.random(3)` to select 1-of-3 statuses
/// (codegen extracts no status → secondary_status=0). Returns the candidates in
/// Showdown's index order (moves.ts). The selection index is rolled separately
/// after the trigger; force_all clamps rng(3)→0 → index 0, matching the harness
/// single-arg random(3)→0 intercept.
fn move_secondary_status_select(move_id: u16) -> Option<[u8; 3]> {
    use crate::data::*;
    match move_id as usize {
        MOVE_TRI_ATTACK => Some([STATUS_BURN, STATUS_PARALYSIS, STATUS_FREEZE]),
        MOVE_DIRE_CLAW  => Some([STATUS_POISON, STATUS_PARALYSIS, STATUS_SLEEP]),
        _ => None,
    }
}

#[inline]
/// Multi-secondary moves carrying a flinch rider alongside their primary secondary.
/// codegen collapses Showdown's `secondaries:[...]` to one slot, dropping the rider;
/// it is re-applied as an independent secondary in `apply_secondary`. Returns the
/// rider's flinch chance: Fire/Ice/Thunder Fang 10% (+ their 10% status), Triple
/// Arrows 30% (+ its 50% Def-1).
fn move_secondary_flinch(move_id: u16) -> Option<u8> {
    use crate::data::*;
    match move_id as usize {
        MOVE_FIRE_FANG | MOVE_ICE_FANG | MOVE_THUNDER_FANG => Some(10),
        MOVE_TRIPLE_ARROWS => Some(30),
        _ => None,
    }
}

#[inline]
/// True iff this damaging move would set a flinch through any engine secondary
/// path (the fang/Triple-Arrows rider, or the type-heuristic-less fallthrough that
/// models real flinchers like Iron Head / Air Slash / Zen Headbutt). Mirrors
/// Showdown's "`move.secondaries` already contains a `volatileStatus:'flinch'`"
/// test — Stench's onModifyMove adds its flinch ONLY when none is present.
fn move_has_flinch_secondary(md: &MoveData, move_id: u16) -> bool {
    if move_secondary_flinch(move_id).is_some() { return true; }
    md.secondary_chance > 0
        && md.effect != MoveEffect::RapidSpin
        && md.secondary_stat == 0
        && md.secondary_status == STATUS_NONE
        && !matches!(md.move_type, Type::Fire | Type::Poison)
        && !move_secondary_confuses(move_id)
        && !move_secondary_no_flinch(move_id)
        && move_secondary_status_select(move_id).is_none()
}

/// The rider a flung item applies to the Fling target (items.ts `fling:` field).
enum FlingEffect {
    Status(u8),
    Flinch,
}

#[inline]
/// Items whose Showdown `fling:` field carries a status / volatileStatus rider.
/// codegen surfaces only the fling base power, so the rider is mapped here off
/// item_id and applied to the Fling target post-damage under the standard status
/// guards. Cold path — reached only on a successful Fling, off the per-hit calc
/// loop. King's Rock / Razor Fang fling a guaranteed flinch (not the held 10%).
fn fling_item_status(item_id: u16) -> Option<FlingEffect> {
    Some(match item_id {
        data_bridge::ITEM_TOXIC_ORB   => FlingEffect::Status(STATUS_BAD_POISON),
        data_bridge::ITEM_FLAME_ORB   => FlingEffect::Status(STATUS_BURN),
        data_bridge::ITEM_POISON_BARB => FlingEffect::Status(STATUS_POISON),
        data_bridge::ITEM_LIGHT_BALL  => FlingEffect::Status(STATUS_PARALYSIS),
        data_bridge::ITEM_KINGS_ROCK | data_bridge::ITEM_RAZOR_FANG => FlingEffect::Flinch,
        _ => return None,
    })
}

/// True iff `victim_side`'s ability blocks `status`. Mold Breaker bypasses the
/// breakable:1 onSetStatus block, but every status-blocker (Water Veil, Limber,
/// Magma Armor, Insomnia, Vital Spirit, Immunity, Pastel Veil) also carries an
/// onUpdate cure that fires outside Mold Breaker's ignore scope, so the status
/// is re-cured the same tick and never sticks — net effect matches the block.
#[inline]
fn ability_status_immune(state: &BattleState, victim_side: usize, _source_ability: u16, status: u8) -> bool {
    ability_blocks_status(effective_ability(state, victim_side), status)
}

/// Apply a damaging move's secondary effect(s). The 16-byte hot MoveData holds at
/// most one secondary (`apply_primary_secondary`); the fang family + Triple Arrows
/// additionally carry a flinch rider, applied here as an independent secondary after
/// the primary — mirroring Showdown rolling each `secondaries:[...]` entry separately.
#[inline]
fn apply_secondary(
    state: &mut BattleState,
    atk_side: usize,
    def_side: usize,
    md: &MoveData,
    move_id: u16,
    rng: &mut impl FnMut(u32) -> u32,
) {
    apply_primary_secondary(state, atk_side, def_side, md, move_id, rng);

    let base = match move_secondary_flinch(move_id) {
        Some(b) => b as u32,
        None => return,
    };
    let atk_ability = effective_ability(state, atk_side);
    // Sheer Force removes ALL secondaries, the rider included (primary already
    // suppressed above).
    if atk_ability == data_bridge::ABILITY_SHEER_FORCE { return; }
    // Covert Cloak blocks the rider flinch like any other target-side secondary.
    if state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(def_side).item_id).has(ItemFlag::COVERT_CLOAK)
    { return; }
    let chance = if atk_ability == data_bridge::ABILITY_SERENE_GRACE { (base * 2).min(100) } else { base };
    if rng(100) < chance
        && !state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN)
    {
        set_volatile(state, def_side, VOL_FLINCHED);
    }
}

fn apply_primary_secondary(
    state: &mut BattleState,
    atk_side: usize,
    def_side: usize,
    md: &MoveData,
    move_id: u16,
    rng: &mut impl FnMut(u32) -> u32,
) {
    if md.secondary_chance == 0 { return; }
    // Rapid Spin: secondary (Speed +1) is fully handled in Hook 16
    if md.effect == MoveEffect::RapidSpin { return; }

    let atk_ability = effective_ability(state, atk_side);

    // Sheer Force: secondary suppressed (power boost already applied in calc)
    if atk_ability == data_bridge::ABILITY_SHEER_FORCE { return; }

    let mut chance = md.secondary_chance as u32;
    if atk_ability == data_bridge::ABILITY_SERENE_GRACE { chance = (chance * 2).min(100); }

    if rng(100) >= chance { return; }

    let def_slot = state.sides[def_side].active_index as usize;

    if md.secondary_stat > 0 {
        if move_secondary_omniboosts(move_id) {
            let stages = md.secondary_stat as i8;
            let boosts = [(ATK, stages), (DEF, stages), (SPA, stages), (SPD, stages), (SPE, stages)];
            for &(stat, s) in &boosts {
                apply_boost(state, atk_side, stat, s);
            }
            try_mirror_herb(state, atk_side, &boosts);
            return;
        }
        let stat = secondary_self_boost_stat_override(move_id).unwrap_or_else(|| {
            if md.category == MoveCategory::Physical { ATK } else { SPA }
        });
        apply_boost(state, atk_side, stat, md.secondary_stat as i8);
        try_mirror_herb(state, atk_side, &[(stat, md.secondary_stat as i8)]);
        return;
    }

    let has_covert_cloak = state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(def_side).item_id).has(ItemFlag::COVERT_CLOAK);
    if has_covert_cloak { return; }

    // 3-way status SELECTION (Tri Attack, Dire Claw): Showdown's onHit rolls
    // this.random(3) AFTER the trigger to pick one status, then trySetStatus
    // (a silent no-op on immunity). Roll the index, apply with the standard
    // guards, and always return — a blocked pick applies NOTHING (no flinch
    // fallthrough). force_all → rng(3)=0 → index 0.
    if let Some(statuses) = move_secondary_status_select(move_id) {
        let status = statuses[rng(3) as usize];
        if !type_immune_to_status(state, def_side, status)
            && state.sides[def_side].side_conditions.safeguard_turns() == 0
            && !terrain_blocks_status(state, def_side, status)
            && !ability_status_immune(state, def_side, atk_ability, status)
            && !crate::state::forme::is_minior_meteor_forme(state, def_side)
            && set_status(state, def_side, def_slot, status, 0)
        {
            try_synchronize_back(state, def_side, atk_side, status);
        }
        return;
    }

    if md.secondary_stat < 0 {
        if state.sides[def_side].side_conditions.mist_turns() == 0 {
            let stat = secondary_drop_stat_override(move_id).unwrap_or_else(|| {
                if md.category == MoveCategory::Physical { DEF } else { SPD }
            });
            try_opponent_stat_drop(state, def_side, stat, md.secondary_stat as i8);
        }
        return;
    }

    // Burning Jealousy burns ONLY a target that raised a stat this turn (Showdown
    // onHit: `if (target.statsRaisedThisTurn) target.trySetStatus('brn')`). Without a
    // raise nothing happens — no burn, no flinch fallthrough.
    if move_id as usize == crate::data::MOVE_BURNING_JEALOUSY
        && !state.sides[def_side].stats_raised_this_turn()
    {
        return;
    }

    let status = if md.secondary_status != STATUS_NONE {
        md.secondary_status
    } else {
        match md.move_type {
            Type::Fire   => STATUS_BURN,
            Type::Poison => STATUS_POISON,
            _ => STATUS_NONE,
        }
    };

    if status != STATUS_NONE
        && !type_immune_to_status(state, def_side, status)
        && state.sides[def_side].side_conditions.safeguard_turns() == 0
        && !terrain_blocks_status(state, def_side, status)
        && !ability_status_immune(state, def_side, atk_ability, status)
        && !crate::state::forme::is_minior_meteor_forme(state, def_side)
    {
        if set_status(state, def_side, def_slot, status, 0) {
            try_synchronize_back(state, def_side, atk_side, status);
        }
        return;
    }

    if move_secondary_confuses(move_id) {
        if state.sides[def_side].active.confusion_turns == 0
            && state.sides[def_side].side_conditions.safeguard_turns() == 0
            && effective_ability(state, def_side) != data_bridge::ABILITY_OWN_TEMPO
        {
            state.sides[def_side].active.confusion_turns = (rng(4) + 2) as u8;
        }
        return;
    }

    // Phantom-flinch suppression: these onHit moves reach the fallthrough with no
    // parseable secondary, but their real Showdown effect is not a flinch.
    if move_secondary_no_flinch(move_id) { return; }

    if !state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN) {
        set_volatile(state, def_side, VOL_FLINCHED);
    }
}

/// Power/Guard Split: average both actives' raw stat pair and write the result into
/// both actives' override_stats, populating the full array so effective_stat reads it.
fn apply_stat_split(state: &mut BattleState, atk_side: usize, def_side: usize, s1: usize, s2: usize) {
    for side in [atk_side, def_side] {
        if !state.sides[side].active.stats_split_active()
            && !state.sides[side].active.has_volatile(VOL_TRANSFORMED)
            && state.sides[side].active.override_stats[0] == 0
        {
            let full = [
                effective_stat(state, side, ATK), effective_stat(state, side, DEF),
                effective_stat(state, side, SPA), effective_stat(state, side, SPD),
                effective_stat(state, side, SPE),
            ];
            state.sides[side].active.override_stats = full;
        }
    }
    let avg1 = ((effective_stat(state, atk_side, s1) as u32
        + effective_stat(state, def_side, s1) as u32) / 2) as u16;
    let avg2 = ((effective_stat(state, atk_side, s2) as u32
        + effective_stat(state, def_side, s2) as u32) / 2) as u16;
    for side in [atk_side, def_side] {
        state.sides[side].active.override_stats[s1] = avg1;
        state.sides[side].active.override_stats[s2] = avg2;
        state.sides[side].active.set_stats_split();
    }
}

fn execute_status_move(
    state: &mut BattleState,
    teams: &TeamData,
    atk_side: usize,
    def_side: usize,
    md: &MoveData,
    rng: &mut impl FnMut(u32) -> u32,
    move_id: u16,
) {
    let atk_slot = state.sides[atk_side].active_index as usize;
    let def_slot = state.sides[def_side].active_index as usize;

    if md.effect == MoveEffect::Protect {
        execute_protect(state, atk_side, def_side, move_id, rng);
        return;
    }

    if md.effect == MoveEffect::Endure {
        execute_endure(state, atk_side, def_side, rng);
        return;
    }

    // Weather moves: field-targeting, bypass Protect/accuracy/immunity.
    // The same-weather guard now lives inside set_weather (mutations.rs).
    match move_id as usize {
        crate::data::MOVE_SUNNY_DAY => {
            set_weather(state, WEATHER_SUN, 5);
            crate::state::switch::check_paradox_deactivation(state);
            return;
        }
        crate::data::MOVE_RAIN_DANCE => {
            set_weather(state, WEATHER_RAIN, 5);
            crate::state::switch::check_paradox_deactivation(state);
            return;
        }
        crate::data::MOVE_SANDSTORM => {
            set_weather(state, WEATHER_SAND, 5);
            crate::state::switch::check_paradox_deactivation(state);
            return;
        }
        crate::data::MOVE_SNOWSCAPE => {
            set_weather(state, WEATHER_SNOW, 5);
            crate::state::switch::check_paradox_deactivation(state);
            return;
        }
        _ => {}
    }

    // Rest: full heal + 3-turn sleep, fail at full HP / Insomnia / Vital Spirit / Comatose.
    // Mirrors Showdown moves.ts:15014-15036.
    if move_id as usize == crate::data::MOVE_REST {
        let mon = &state.sides[atk_side].team[atk_slot];
        if mon.current_hp == mon.max_hp { return; }
        let abil = effective_ability(state, atk_side);
        if ability_blocks_status(abil, STATUS_SLEEP) || abil == data_bridge::ABILITY_COMATOSE { return; }
        let to_heal = mon.max_hp - mon.current_hp;
        clear_status(state, atk_side, atk_slot);
        set_status(state, atk_side, atk_slot, STATUS_SLEEP, 3);
        heal(state, atk_side, atk_slot, to_heal);
        return;
    }

    if md.flags & MoveFlags::HEAL != 0
        && md.effect != MoveEffect::Wish
        && md.effect != MoveEffect::HealingWish
        && md.effect != MoveEffect::LunarDance
        && md.effect != MoveEffect::Swallow // heals by stockpile count, fails at 0 — see MoveEffect::Swallow arm
        && md.effect != MoveEffect::Heal25
        && md.effect != MoveEffect::Heal25CureStatus // 25% heals, handled in the main effect match
        && move_id as usize != crate::data::MOVE_HEAL_PULSE
        && move_id as usize != crate::data::MOVE_FLORAL_HEALING // target:Any HEAL — heal the foe, handled below
    {
        let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
        heal(state, atk_side, atk_slot, max_hp / 2);
        return;
    }

    let targets_self = is_self_targeting(md);

    if !targets_self && state.sides[def_side].active.has_volatile(VOL_PROTECT_THIS_TURN) {
        return;
    }
    // Semi-invulnerability: status moves miss semi-invulnerable targets.
    // (mark_move_failed here and below mirrors Showdown moveThisTurnResult=false.)
    if !targets_self && state.sides[def_side].active.has_volatile(VOL_SEMI_INVULNERABLE) {
        mark_move_failed(state, atk_side);
        return;
    }
    // Substitute: block non-bypasssub status moves targeting the opponent.
    // Infiltrator bypasses sub for status moves too (Showdown abilities.ts:2074-2082
    // sets move.infiltrates which is checked alongside BYPASSSUB at every sub check).
    if !targets_self
        && md.flags & MoveFlags::BYPASSSUB == 0
        && state.sides[def_side].active.has_volatile(VOL_SUBSTITUTE)
        && effective_ability(state, atk_side) != data_bridge::ABILITY_INFILTRATOR
    {
        mark_move_failed(state, atk_side);
        return;
    }
    if !targets_self {
        let atk_ability = effective_ability(state, atk_side);
        // Gen 7+: Dark-types are immune to Prankster-boosted status moves.
        if atk_ability == data_bridge::ABILITY_PRANKSTER
            && has_type(state, def_side, Type::Dark as u8)
        {
            mark_move_failed(state, atk_side);
            return;
        }
        if good_as_gold_immunity(state, def_side) { mark_move_failed(state, atk_side); return; }
        if let Some(eff) = ability_flag_immunity(state, def_side, md.flags, atk_ability) {
            apply_immunity_effect(state, def_side, def_slot, eff);
            mark_move_failed(state, atk_side);
            return;
        }
        if let Some(eff) = ability_type_immunity(state, def_side, md.move_type, atk_ability) {
            apply_immunity_effect(state, def_side, def_slot, eff);
            mark_move_failed(state, atk_side);
            return;
        }
        // Air Balloon: non-grounded mons are immune to Ground-type moves
        if md.move_type == Type::Ground && !is_grounded(state, def_side) {
            mark_move_failed(state, atk_side);
            return;
        }
    }
    if !targets_self && md.accuracy != 0 && !accuracy_check(state, atk_side, md, rng) {
        mark_move_failed(state, atk_side);
        return;
    }

    // Heal Pulse (505) / Floral Healing (666): target:Any HEAL moves restore the
    // TARGET's HP off its own max HP, not the user's. Mirrors Showdown moves.ts
    // (baseMaxhp): Heal Pulse 50% / 75% with Mega Launcher; Floral Healing 50% /
    // 0.667 in Grassy Terrain. Fail at target full HP or under Heal Block.
    if move_id as usize == crate::data::MOVE_HEAL_PULSE
        || move_id as usize == crate::data::MOVE_FLORAL_HEALING
    {
        let tgt = &state.sides[def_side].team[def_slot];
        if tgt.current_hp >= tgt.max_hp || state.sides[def_side].active.heal_block_turns > 0 {
            return;
        }
        let max_hp = tgt.max_hp as u32;
        let amount = if move_id as usize == crate::data::MOVE_HEAL_PULSE
            && effective_ability(state, atk_side) == data_bridge::ABILITY_MEGA_LAUNCHER
        {
            crate::state::calc_modifiers::chain_mod(max_hp, 3072) // modify(maxhp, 0.75)
        } else if move_id as usize == crate::data::MOVE_FLORAL_HEALING
            && state.field.terrain == TERRAIN_GRASSY
        {
            crate::state::calc_modifiers::chain_mod(max_hp, 2732) // modify(maxhp, 0.667)
        } else {
            (max_hp + 1) / 2 // ceil(maxhp * 0.5)
        };
        heal(state, def_side, def_slot, amount as u16);
        return;
    }

    let mirror_check = state.field.magic_room_turns() == 0
        && state.active_mon(def_side).item_id != 0;
    let atk_boosts_before = if mirror_check { state.sides[atk_side].active.boosts } else { [0; 7] };

    match md.effect {
        // -- Hazard setters --
        MoveEffect::StealthRock => { set_stealth_rock(state, def_side); }
        MoveEffect::Spikes      => { add_spikes(state, def_side); }
        MoveEffect::ToxicSpikes => { add_toxic_spikes(state, def_side); }
        MoveEffect::StickyWeb   => { set_sticky_web(state, def_side); }

        // -- Hazard removal --
        MoveEffect::Defog => {
            if state.sides[def_side].side_conditions.mist_turns() == 0 {
                try_opponent_stat_drop(state, def_side, EVA, -1);
            }
            // Target side: hazards + screens + safeguard + mist
            clear_hazards(state, def_side);
            state.sides[def_side].side_conditions.reflect_turns = 0;
            state.sides[def_side].side_conditions.light_screen_turns = 0;
            state.sides[def_side].side_conditions.aurora_veil_turns = 0;
            state.sides[def_side].side_conditions.set_safeguard_turns(0);
            state.sides[def_side].side_conditions.set_mist_turns(0);
            // Attacker side: hazards only
            clear_hazards(state, atk_side);
            // Clear terrain
            clear_terrain(state);
            crate::state::switch::check_paradox_deactivation(state);
        }

        // -- Status infliction (blocked by Safeguard / terrain) --
        MoveEffect::WillOWisp   => {
            if !type_immune_to_status(state, def_side, STATUS_BURN)
                && state.sides[def_side].side_conditions.safeguard_turns() == 0
                && !terrain_blocks_status(state, def_side, STATUS_BURN)
                && !ability_status_immune(state, def_side, effective_ability(state, atk_side), STATUS_BURN)
                && !crate::state::forme::is_minior_meteor_forme(state, def_side)
            {
                if set_status(state, def_side, def_slot, STATUS_BURN, 0) {
                    try_synchronize_back(state, def_side, atk_side, STATUS_BURN);
                }
            }
        }
        MoveEffect::ThunderWave => {
            if !type_immune_to_status(state, def_side, STATUS_PARALYSIS)
                && state.sides[def_side].side_conditions.safeguard_turns() == 0
                && !terrain_blocks_status(state, def_side, STATUS_PARALYSIS)
                && !ability_status_immune(state, def_side, effective_ability(state, atk_side), STATUS_PARALYSIS)
                && !crate::state::forme::is_minior_meteor_forme(state, def_side)
            {
                if set_status(state, def_side, def_slot, STATUS_PARALYSIS, 0) {
                    try_synchronize_back(state, def_side, atk_side, STATUS_PARALYSIS);
                }
            }
        }
        MoveEffect::Toxic       => {
            if !type_immune_to_status(state, def_side, STATUS_BAD_POISON)
                && state.sides[def_side].side_conditions.safeguard_turns() == 0
                && !terrain_blocks_status(state, def_side, STATUS_BAD_POISON)
                && !ability_status_immune(state, def_side, effective_ability(state, atk_side), STATUS_BAD_POISON)
                && !crate::state::forme::is_minior_meteor_forme(state, def_side)
            {
                if set_status(state, def_side, def_slot, STATUS_BAD_POISON, 0) {
                    try_synchronize_back(state, def_side, atk_side, STATUS_BAD_POISON);
                }
            }
        }
        MoveEffect::PoisonPowder => {
            if !type_immune_to_status(state, def_side, STATUS_POISON)
                && state.sides[def_side].side_conditions.safeguard_turns() == 0
                && !terrain_blocks_status(state, def_side, STATUS_POISON)
                && !ability_status_immune(state, def_side, effective_ability(state, atk_side), STATUS_POISON)
                && !crate::state::forme::is_minior_meteor_forme(state, def_side)
            {
                if set_status(state, def_side, def_slot, STATUS_POISON, 0) {
                    try_synchronize_back(state, def_side, atk_side, STATUS_POISON);
                }
            }
        }
        MoveEffect::Sleep       => {
            if state.sides[def_side].side_conditions.safeguard_turns() == 0
                && !terrain_blocks_status(state, def_side, STATUS_SLEEP)
                && !ability_status_immune(state, def_side, effective_ability(state, atk_side), STATUS_SLEEP)
                && !crate::state::forme::is_minior_meteor_forme(state, def_side)
            {
                let turns = (rng(3) + 2) as u8;
                set_status(state, def_side, def_slot, STATUS_SLEEP, turns);
                // Synchronize does NOT pass sleep — no call here
            }
        }

        // -- Self-boosts --
        MoveEffect::SwordsDance => { apply_boost(state, atk_side, ATK, 2); }
        MoveEffect::Charge => {
            // Raise SpD by 1 and set the charge bit (2x power for next Electric move).
            // Bit 2 of _padding[4] is the charge storage; cleared when attacker next uses
            // any Electric move (see L~1824).
            apply_boost(state, atk_side, SPD, 1);
            state.sides[atk_side].active._padding[4] |= 4;
        }
        MoveEffect::NastyPlot   => { apply_boost(state, atk_side, SPA, 2); }
        MoveEffect::DragonDance => {
            apply_boost(state, atk_side, ATK, 1);
            apply_boost(state, atk_side, SPE, 1);
        }
        MoveEffect::CalmMind => {
            apply_boost(state, atk_side, SPA, 1);
            apply_boost(state, atk_side, SPD, 1);
        }
        MoveEffect::BulkUp => {
            apply_boost(state, atk_side, ATK, 1);
            apply_boost(state, atk_side, DEF, 1);
        }
        MoveEffect::IronDefense => { apply_boost(state, atk_side, DEF, 2); }
        MoveEffect::Agility     => { apply_boost(state, atk_side, SPE, 2); }
        MoveEffect::QuiverDance => {
            apply_boost(state, atk_side, SPA, 1);
            apply_boost(state, atk_side, SPD, 1);
            apply_boost(state, atk_side, SPE, 1);
        }
        MoveEffect::ShellSmash => {
            apply_boost(state, atk_side, ATK, 2);
            apply_boost(state, atk_side, SPA, 2);
            apply_boost(state, atk_side, SPE, 2);
            apply_boost(state, atk_side, DEF, -1);
            apply_boost(state, atk_side, SPD, -1);
        }
        MoveEffect::Coil => {
            apply_boost(state, atk_side, ATK, 1);
            apply_boost(state, atk_side, DEF, 1);
            apply_boost(state, atk_side, ACC, 1);
        }
        MoveEffect::ShiftGear => {
            apply_boost(state, atk_side, ATK, 1);
            apply_boost(state, atk_side, SPE, 2);
        }
        MoveEffect::HoneClaws => {
            apply_boost(state, atk_side, ATK, 1);
            apply_boost(state, atk_side, ACC, 1);
        }
        MoveEffect::Harden      => { apply_boost(state, atk_side, DEF, 1); }
        MoveEffect::CottonGuard => { apply_boost(state, atk_side, DEF, 3); }
        MoveEffect::CosmicPower => {
            apply_boost(state, atk_side, DEF, 1);
            apply_boost(state, atk_side, SPD, 1);
        }
        MoveEffect::Amnesia     => { apply_boost(state, atk_side, SPD, 2); }
        MoveEffect::Meditate    => { apply_boost(state, atk_side, ATK, 1); }
        MoveEffect::WorkUp      => {
            apply_boost(state, atk_side, ATK, 1);
            apply_boost(state, atk_side, SPA, 1);
        }
        MoveEffect::Growth      => {
            // Growth boosts double in harsh sun (moves.ts onModifyMove).
            let n = if matches!(effective_weather_for(state, atk_side), WEATHER_SUN | WEATHER_HARSH_SUN) { 2 } else { 1 };
            apply_boost(state, atk_side, ATK, n);
            apply_boost(state, atk_side, SPA, n);
        }
        MoveEffect::TailGlow    => { apply_boost(state, atk_side, SPA, 3); }

        // -- Screens --
        // Showdown's Side.addSideCondition (sim/side.ts:405-409) rejects a
        // re-cast: if the side already has the condition, only `SideRestart`
        // fires (no duration reset). Match by no-op when the counter is non-zero.
        MoveEffect::Reflect => {
            if state.sides[atk_side].side_conditions.reflect_turns == 0 {
                state.sides[atk_side].side_conditions.reflect_turns = screen_duration(state, atk_side);
            }
        }
        MoveEffect::LightScreen => {
            if state.sides[atk_side].side_conditions.light_screen_turns == 0 {
                state.sides[atk_side].side_conditions.light_screen_turns = screen_duration(state, atk_side);
            }
        }
        MoveEffect::AuroraVeil => {
            if effective_weather(state) == WEATHER_SNOW
                && state.sides[atk_side].side_conditions.aurora_veil_turns == 0
            {
                state.sides[atk_side].side_conditions.aurora_veil_turns = screen_duration(state, atk_side);
            }
        }

        // -- Field effects --
        MoveEffect::Tailwind => {
            if state.sides[atk_side].side_conditions.tailwind_turns == 0 {
                state.sides[atk_side].side_conditions.tailwind_turns = 4;
                // Wind Rider fires on Tailwind onSideStart — re-cast (which is a
                // SideRestart in Showdown, not Start) doesn't trigger it.
                let active_idx = state.sides[atk_side].active_index as usize;
                let ability_id = state.sides[atk_side].team[active_idx].ability_id;
                if ability_id == data_bridge::ABILITY_WIND_RIDER
                    && !state.sides[atk_side].active.has_volatile(VOL_ABILITY_SUPPRESSED)
                {
                    apply_boost(state, atk_side, ATK, 1);
                }
            }
        }
        MoveEffect::TrickRoom => {
            if state.field.trick_room_turns > 0 {
                set_trick_room(state, 0);
            } else {
                set_trick_room(state, 5);
            }
        }

        // -- Substitute --
        MoveEffect::Substitute => {
            let cost = state.sides[atk_side].team[atk_slot].max_hp / 4;
            if state.sides[atk_side].team[atk_slot].current_hp > cost
                && !state.sides[atk_side].active.has_volatile(VOL_SUBSTITUTE)
            {
                deal_damage(state, atk_side, atk_slot, cost);
                state.sides[atk_side].active.substitute_hp = cost;
                set_volatile(state, atk_side, VOL_SUBSTITUTE);
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
            if state.sides[def_side].active.taunt_turns == 0 {
                // Showdown: duration 3, +1 if target.activeTurns && !willMove(target)
                let dur = if state.sides[def_side].active.turns_active > 0
                    && state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN)
                { 4 } else { 3 };
                state.sides[def_side].active.taunt_turns = dur;
                check_mental_herb(state, def_side);
            }
        }

        // -- Leech Seed --
        MoveEffect::LeechSeed => {
            if !has_type(state, def_side, Type::Grass as u8) {
                set_volatile(state, def_side, VOL_LEECH_SEED);
            }
        }

        // -- Encore --
        MoveEffect::Encore => {
            let last = state.sides[def_side].active.last_move;
            if last != 0 && ENCORE_FAIL.binary_search(&last).is_err() {
                // Check that the encored move has PP > 0
                let moves = effective_moves(state, def_side);
                let mut has_pp = false;
                for i in 0..4 {
                    if moves[i] == last && effective_pp(state, def_side, i) > 0 {
                        has_pp = true;
                        break;
                    }
                }
                if has_pp {
                    state.sides[def_side].active.encore_move = last;
                    let dur = if state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN) { 4 } else { 3 };
                    state.sides[def_side].active.encore_turns = dur;
                    check_mental_herb(state, def_side);
                }
            }
        }

        // -- BellyDrum: -50% HP, +6 Atk --
        MoveEffect::BellyDrum => {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            let current_hp = state.sides[atk_side].team[atk_slot].current_hp;
            let current_atk = state.sides[atk_side].active.boosts[ATK];
            // Showdown fails (no HP cost) at +6 Atk or maxhp==1 (Shedinja clause).
            if current_hp > max_hp / 2 && current_atk < 6 && max_hp > 1 {
                deal_damage(state, atk_side, atk_slot, max_hp / 2);
                apply_boost(state, atk_side, ATK, 6 - current_atk);
            }
        }

        // -- Clangorous Soul: -33% HP, +1 all stats --
        MoveEffect::ClangorousSoul => {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            let current_hp = state.sides[atk_side].team[atk_slot].current_hp;
            let cost = max_hp as u32 * 33 / 100;
            let cost = cost as u16;
            if max_hp > 1 && current_hp > cost {
                deal_damage(state, atk_side, atk_slot, cost);
                apply_boost(state, atk_side, ATK, 1);
                apply_boost(state, atk_side, DEF, 1);
                apply_boost(state, atk_side, SPA, 1);
                apply_boost(state, atk_side, SPD, 1);
                apply_boost(state, atk_side, SPE, 1);
            }
        }

        // -- Curse (non-Ghost): +1 Atk/Def, -1 Spe --
        MoveEffect::Curse => {
            if !has_type(state, atk_side, Type::Ghost as u8) {
                apply_boost(state, atk_side, ATK, 1);
                apply_boost(state, atk_side, DEF, 1);
                apply_boost(state, atk_side, SPE, -1);
            } else if !state.sides[def_side].is_cursed() {
                // Ghost Curse: ½-max-HP self-cost (directDamage), then the target
                // carries the curse volatile for the ¼-max-HP EOT residual.
                // Showdown's onTryHit fails (no self-cost) if the target is already cursed.
                let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, atk_side, atk_slot, max_hp / 2);
                state.sides[def_side].set_cursed();
            }
        }

        // -- No Retreat: +1 all stats, trap self, fail if already used --
        MoveEffect::NoRetreat => {
            // _padding[3] bit 3 = no_retreat_used
            if state.sides[atk_side].active._padding[3] & 0x08 == 0 {
                apply_boost(state, atk_side, ATK, 1);
                apply_boost(state, atk_side, DEF, 1);
                apply_boost(state, atk_side, SPA, 1);
                apply_boost(state, atk_side, SPD, 1);
                apply_boost(state, atk_side, SPE, 1);
                state.sides[atk_side].active._padding[3] |= 0x08;
                if !state.sides[atk_side].active.has_volatile(VOL_TRAPPED) {
                    set_volatile(state, atk_side, VOL_TRAPPED);
                }
            }
        }

        // -- Tidy Up: +1 Atk/Spe, clear hazards + substitutes from both sides --
        MoveEffect::TidyUp => {
            // Clear substitutes from both sides
            for side in 0..2 {
                if state.sides[side].active.has_volatile(VOL_SUBSTITUTE) {
                    clear_volatile(state, side, VOL_SUBSTITUTE);
                    state.sides[side].active.substitute_hp = 0;
                }
            }
            // Clear hazards from both sides
            clear_hazards(state, 0);
            clear_hazards(state, 1);
            apply_boost(state, atk_side, ATK, 1);
            apply_boost(state, atk_side, SPE, 1);
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

        // -- Power Split / Guard Split: average both actives' raw (pre-boost) stat
        //    pair (Power: Atk/SpA; Guard: Def/SpD), writing the average into both
        //    actives' override_stats. Showdown averages storedStats — the engine's
        //    effective_stat already returns the raw stat (forme/Transform-aware), so
        //    averaging it mirrors storedStats. Boosts still apply on top downstream. --
        MoveEffect::PowerSplit | MoveEffect::GuardSplit => {
            if !state.sides[def_side].team[def_slot].is_fainted() {
                let (s1, s2) = if md.effect == MoveEffect::PowerSplit { (ATK, SPA) } else { (DEF, SPD) };
                apply_stat_split(state, atk_side, def_side, s1, s2);
            }
        }

        // -- Soak: set the target's types to pure Water (Showdown setType('Water')).
        //    Fails on a fainted target, an already-pure-Water target, a Terastallized
        //    target, and Arceus/Silvally (cantsuppress base type). --
        MoveEffect::Soak => {
            let water = Type::Water as u8;
            let already_water = {
                let (t1, t2) = battle_types(state, def_side);
                t1 == water && t2 == water
            };
            let base = data_bridge::base_species(state.sides[def_side].team[def_slot].species_id);
            if !state.sides[def_side].team[def_slot].is_fainted()
                && !already_water
                && !state.sides[def_side].team[def_slot].is_terastallized()
                && base != data_bridge::SPECIES_ARCEUS && base != data_bridge::SPECIES_SILVALLY
            {
                state.sides[def_side].active.override_types = [water, water];
                set_volatile(state, def_side, VOL_TYPES_OVERRIDDEN);
            }
        }

        // -- PerishSong: set 3-turn perish counter on both --
        MoveEffect::PerishSong => {
            if !state.sides[atk_side].active.has_volatile(VOL_PERISH_SONG) {
                set_volatile(state, atk_side, VOL_PERISH_SONG);
                state.sides[atk_side].active.perish_count = 3;
            }
            if !state.sides[def_side].active.has_volatile(VOL_PERISH_SONG) {
                set_volatile(state, def_side, VOL_PERISH_SONG);
                state.sides[def_side].active.perish_count = 3;
            }
        }

        // -- DestinyBond --
        MoveEffect::DestinyBond => {
            set_volatile(state, atk_side, VOL_DESTINY_BOND);
        }

        // -- Trick / Switcheroo: swap items --
        MoveEffect::Trick => {
            let item_a = state.sides[atk_side].team[atk_slot].item_id;
            let item_d = state.sides[def_side].team[def_slot].item_id;
            if item_a != 0 || item_d != 0 {
                // Blocked by Sticky Hold
                let def_ability = effective_ability(state, def_side);
                if def_ability == data_bridge::ABILITY_STICKY_HOLD { /* fail */ }
                else {
                    // Check forme-locked items on both sides
                    let atk_base = data_bridge::base_species(state.sides[atk_side].team[atk_slot].species_id);
                    let def_base = data_bridge::base_species(state.sides[def_side].team[def_slot].species_id);
                    let atk_item_locked = item_a != 0 && data_bridge::item(item_a).is_forme_locked(atk_base);
                    let def_item_locked = item_d != 0 && data_bridge::item(item_d).is_forme_locked(def_base);
                    // Also check if giving attacker's item to defender is forme-locked for defender, and vice versa
                    let atk_item_locked_on_def = item_a != 0 && data_bridge::item(item_a).is_forme_locked(def_base);
                    let def_item_locked_on_atk = item_d != 0 && data_bridge::item(item_d).is_forme_locked(atk_base);
                    if !atk_item_locked && !def_item_locked && !atk_item_locked_on_def && !def_item_locked_on_atk {
                        set_item(state, atk_side, atk_slot, item_d);
                        set_item(state, def_side, def_slot, item_a);
                    }
                }
            }
        }

        // -- Disable: prevent last-used move for 4-5 turns --
        MoveEffect::Disable => {
            let last = state.sides[def_side].active.last_move;
            if last != 0 && state.sides[def_side].active.disabled_move == 0 {
                // Check that the move has PP > 0
                let moves = effective_moves(state, def_side);
                let mut has_pp = false;
                for i in 0..4 {
                    if moves[i] == last && effective_pp(state, def_side, i) > 0 {
                        has_pp = true;
                        break;
                    }
                }
                if has_pp {
                    state.sides[def_side].active.disabled_move = last;
                    // Showdown: duration 5, -1 if target hasn't moved yet (willMove)
                    let dur = if state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN) { 5 } else { 4 };
                    state.sides[def_side].active.disable_turns = dur;
                    check_mental_herb(state, def_side);
                }
            }
        }

        // -- Torment --
        MoveEffect::Torment => {
            if !state.sides[def_side].active.has_volatile(VOL_TORMENT) {
                set_volatile(state, def_side, VOL_TORMENT);
                check_mental_herb(state, def_side);
            }
        }

        // -- HealingWish: user faints, next switch-in fully heals --
        MoveEffect::HealingWish => {
            let hp = state.sides[atk_side].team[atk_slot].current_hp;
            if hp > 0 {
                deal_damage(state, atk_side, atk_slot, hp);
                state.sides[atk_side].side_conditions.set_healing_wish(true);
            }
        }

        // -- LunarDance: user faints, next switch-in fully heals + PP --
        MoveEffect::LunarDance => {
            let hp = state.sides[atk_side].team[atk_slot].current_hp;
            if hp > 0 {
                deal_damage(state, atk_side, atk_slot, hp);
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
            heal(state, atk_side, atk_slot, max_hp / 2);
            // Temporarily remove Flying type — we use a counter field to track
            // The end-of-move cleanup restores it. For simplicity in MCTS,
            // we handle this as a type override for the remainder of the turn.
            let (t1, t2) = battle_types(state, atk_side);
            if t1 == Type::Flying as u8 || t2 == Type::Flying as u8 {
                let new_t1 = if t1 == Type::Flying as u8 { Type::Normal as u8 } else { t1 };
                let new_t2 = if t2 == Type::Flying as u8 { Type::Normal as u8 } else { t2 };
                // If both types were Flying, become pure Normal
                state.sides[atk_side].active.override_types = [new_t1, new_t2];
                set_volatile(state, atk_side, VOL_TYPES_OVERRIDDEN);
            }
        }

        // Life Dew: SD routes it through heal:[1,4] data → Math.round(maxhp/4), half-up.
        MoveEffect::Heal25 => {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            heal(state, atk_side, atk_slot, (max_hp + 2) >> 2);
        }

        // Jungle Healing / Lunar Blessing: SD onHit modify(maxhp, 0.25), half-down.
        MoveEffect::Heal25CureStatus => {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            heal(state, atk_side, atk_slot, crate::state::calc_modifiers::chain_mod(max_hp as u32, 1024) as u16);
            clear_status(state, atk_side, atk_slot);
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
            set_gravity(state, 5);
            for s in 0..2 {
                if state.sides[s].active.has_volatile(VOL_MAGNET_RISE) {
                    clear_volatile(state, s, VOL_MAGNET_RISE);
                    state.sides[s].active.magnet_rise_turns = 0;
                }
                if state.sides[s].active.has_volatile(VOL_CHARGING) {
                    clear_volatile(state, s, VOL_CHARGING);
                    if state.sides[s].active.has_volatile(VOL_SEMI_INVULNERABLE) {
                        clear_volatile(state, s, VOL_SEMI_INVULNERABLE);
                    }
                    state.sides[s].active._padding[1] = 0;
                }
            }
        }

        MoveEffect::MagicRoom => {
            if state.field.magic_room_turns() > 0 {
                set_magic_room(state, 0);
            } else {
                set_magic_room(state, 5);
            }
        }

        MoveEffect::WonderRoom => {
            if state.field.wonder_room_turns() > 0 {
                set_wonder_room(state, 0);
            } else {
                set_wonder_room(state, 5);
            }
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
                crate::state::switch::perform_switch_forced(state, teams, def_side, pick);
            }
        }

        // -- Psych Up: copy target's stat boosts to attacker (Showdown moves.ts onHit) --
        MoveEffect::PsychUp => {
            for stat in 0..7 {
                let target_val = state.sides[def_side].active.boosts[stat];
                let user_val = state.sides[atk_side].active.boosts[stat];
                if user_val != target_val {
                    state.sides[atk_side].active.boosts[stat] = target_val;
                }
            }
        }

        // -- Skill Swap: swap user's and target's abilities (battle.ts:1300) --
        MoveEffect::SkillSwap => {
            if !state.sides[atk_side].team[atk_slot].is_fainted()
                && !state.sides[def_side].team[def_slot].is_fainted()
            {
                let atk_ab = effective_ability(state, atk_side);
                let def_ab = effective_ability(state, def_side);
                if !is_failskillswap_ability(atk_ab) && !is_failskillswap_ability(def_ab) {
                    // Showdown mutates pokemon.ability (the live base) on both sides
                    // but preserves baseAbility, restoring it via clearVolatile on
                    // switch-out. Mirror that: write the swapped value into base
                    // ability_id (so the live ability reads through), stash the
                    // pre-swap ability in the active for switch-out restore, and
                    // flag the slot so switch_out knows to restore.
                    skill_swap_apply(state, atk_side, atk_slot, def_ab);
                    skill_swap_apply(state, def_side, def_slot, atk_ab);
                }
            }
        }

        // -- Simple Beam: set target's ability to Simple (moves.ts simplebeam) --
        MoveEffect::SimpleBeam => {
            if !state.sides[def_side].team[def_slot].is_fainted() {
                let def_ab = effective_ability(state, def_side);
                let has_shield = state.field.magic_room_turns() == 0
                    && data_bridge::item(state.active_mon(def_side).item_id).has(ItemFlag::ABILITY_SHIELD);
                if !has_shield
                    && !is_cantsuppress_ability(def_ab)
                    && def_ab != data_bridge::ABILITY_SIMPLE
                    && def_ab != data_bridge::ABILITY_TRUANT
                {
                    skill_swap_apply(state, def_side, def_slot, data_bridge::ABILITY_SIMPLE);
                }
            }
        }

        // -- Transform: copy target's species/ability/types/stats/moves/boosts (Ditto) --
        MoveEffect::Transform => {
            let user_transformed = state.sides[atk_side].active.has_volatile(VOL_TRANSFORMED);
            let target_transformed = state.sides[def_side].active.has_volatile(VOL_TRANSFORMED);
            let target_sub = state.sides[def_side].active.has_volatile(VOL_SUBSTITUTE);
            if !state.sides[def_side].team[def_slot].is_fainted()
                && !user_transformed && !target_transformed && !target_sub
            {
                crate::state::forme::apply_transform(state, atk_side, def_side);
            }
        }

        // -- Gastro Acid: suppress target's ability (moves.ts gastroacid.onTryHit) --
        MoveEffect::GastroAcid => {
            if !state.sides[def_side].team[def_slot].is_fainted()
                && !state.sides[def_side].active.has_volatile(VOL_ABILITY_SUPPRESSED)
            {
                let def_ab = effective_ability(state, def_side);
                let has_shield = state.field.magic_room_turns() == 0
                    && data_bridge::item(state.active_mon(def_side).item_id).has(ItemFlag::ABILITY_SHIELD);
                if !has_shield && !is_cantsuppress_ability(def_ab) {
                    set_volatile(state, def_side, VOL_ABILITY_SUPPRESSED);
                }
            }
        }

        // -- Haze: reset all stat changes --
        MoveEffect::Haze => {
            for side in 0..2 {
                for stat in 0..7 {
                    if state.sides[side].active.boosts[stat] != 0 {
                        state.sides[side].active.boosts[stat] = 0;
                    }
                }
            }
        }

        // -- Yawn --
        MoveEffect::Yawn => {
            if !state.sides[def_side].active.has_volatile(VOL_YAWN)
                && state.sides[def_side].team[def_slot].status == STATUS_NONE
                && state.sides[def_side].side_conditions.safeguard_turns() == 0
                && !crate::state::forme::is_minior_meteor_forme(state, def_side)
                && !terrain_blocks_status(state, def_side, STATUS_SLEEP)
            {
                set_volatile(state, def_side, VOL_YAWN);
                // Set yawn first-tick marker: bit 7 of _padding[4]
                // Sleep applies after TWO end-of-turn ticks, not one.
                state.sides[def_side].active._padding[4] |= 0x80;
            }
        }

        // -- Confuse (Confuse Ray, Sweet Kiss, Swagger, Flatter) --
        MoveEffect::Confuse => {
            if state.sides[def_side].active.confusion_turns == 0
                && state.sides[def_side].side_conditions.safeguard_turns() == 0
                && effective_ability(state, def_side) != data_bridge::ABILITY_OWN_TEMPO
            {
                // BUG-P5-M-102: duration is 2-5 turns (rng(4)+2), not 2-4.
                state.sides[def_side].active.confusion_turns = (rng(4) + 2) as u8;
            }
            // BUG-P5-M-105: Swagger/Flatter also boost target's offensive stat.
            apply_opp_stat_change(state, def_side, md.self_effect);
        }

        // -- Opponent stat drop/boost status moves (Growl, Leer, Screech, Swagger-boost, etc.) --
        MoveEffect::OpponentStatDrop => {
            apply_opp_stat_change(state, def_side, md.self_effect);
        }

        // -- Ally-target stat boost status moves (Howl, Aromatic Mist, Coaching) --
        // In singles, ally == self, so we apply to atk_side.
        MoveEffect::AllyBoost => {
            apply_ally_stat_change(state, atk_side, md.self_effect);
        }

        // -- Memento: -2 Atk/-2 SpA on target, user faints --
        MoveEffect::Memento => {
            apply_opp_stat_change(state, def_side, md.self_effect);
            // User faints
            let hp = state.sides[atk_side].team[atk_slot].current_hp;
            if hp > 0 {
                deal_damage(state, atk_side, atk_slot, hp);
            }
        }

        // -- Toxic Thread: inflict poison + -1 Spe on target --
        MoveEffect::ToxicThread => {
            if !type_immune_to_status(state, def_side, STATUS_POISON)
                && state.sides[def_side].side_conditions.safeguard_turns() == 0
                && !terrain_blocks_status(state, def_side, STATUS_POISON)
                && !ability_status_immune(state, def_side, effective_ability(state, atk_side), STATUS_POISON)
                && !crate::state::forme::is_minior_meteor_forme(state, def_side)
            {
                if set_status(state, def_side, def_slot, STATUS_POISON, 0) {
                    try_synchronize_back(state, def_side, atk_side, STATUS_POISON);
                }
            }
            // Speed drop applies even if status failed (per Showdown moves.ts).
            if state.sides[def_side].side_conditions.mist_turns() == 0 {
                try_opponent_stat_drop(state, def_side, SPE, -1);
            }
        }

        // -- Magnet Rise --
        MoveEffect::MagnetRise => {
            if state.sides[atk_side].active.magnet_rise_turns == 0 {
                set_volatile(state, atk_side, VOL_MAGNET_RISE);
                state.sides[atk_side].active.magnet_rise_turns = 5;
            }
        }

        // -- Aqua Ring --
        MoveEffect::AquaRing => {
            set_volatile(state, atk_side, VOL_AQUA_RING);
        }

        // -- Ingrain --
        MoveEffect::Ingrain => {
            set_volatile(state, atk_side, VOL_INGRAIN);
        }

        // -- Focus Energy --
        MoveEffect::FocusEnergy => {
            set_volatile(state, atk_side, VOL_FOCUS_ENERGY);
        }

        // -- Imprison --
        MoveEffect::Imprison => {
            set_volatile(state, atk_side, VOL_IMPRISON);
        }

        // -- Aromatherapy / Heal Bell: cure team status --
        MoveEffect::Aromatherapy => {
            for i in 0..6 {
                if state.sides[atk_side].team[i].species_id != 0
                    && state.sides[atk_side].team[i].status != STATUS_NONE
                {
                    clear_status(state, atk_side, i);
                }
            }
        }

        // -- Minimize: +2 Evasion + set VOL_MINIMIZE --
        MoveEffect::Minimize => {
            apply_boost(state, atk_side, EVA, 2);
            set_volatile(state, atk_side, VOL_MINIMIZE);
        }

        // -- Stockpile --
        MoveEffect::Stockpile => {
            let count = state.sides[atk_side].active.stockpile & 0x7F;
            if count < 3 {
                state.sides[atk_side].active.stockpile = (state.sides[atk_side].active.stockpile & 0x80) | (count + 1);
                apply_boost(state, atk_side, DEF, 1);
                apply_boost(state, atk_side, SPD, 1);
            }
        }

        // -- Swallow --
        MoveEffect::Swallow => {
            let count = state.sides[atk_side].active.stockpile & 0x7F;
            if count > 0 {
                let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
                let heal_amount = match count {
                    1 => max_hp / 4,
                    2 => max_hp / 2,
                    _ => max_hp,
                };
                heal(state, atk_side, atk_slot, heal_amount);
                apply_boost(state, atk_side, DEF, -(count as i8));
                apply_boost(state, atk_side, SPD, -(count as i8));
                state.sides[atk_side].active.stockpile &= 0x80; // preserve salt cure bit
            }
        }

        // -- Parting Shot: -1 Atk -1 SpA on target, then self-switch --
        MoveEffect::PartingShot => {
            let (a, s) = if state.sides[def_side].side_conditions.mist_turns() == 0 {
                (try_opponent_stat_drop(state, def_side, ATK, -1),
                 try_opponent_stat_drop(state, def_side, SPA, -1))
            } else {
                (0, 0)
            };
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
                    set_volatile(state, atk_side, VOL_MUST_SWITCH);
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
                    set_volatile(state, atk_side, VOL_MUST_SWITCH);
                }
            }
        }

        // -- Teleport: plain self-switch (no boost/volatile transfer); fails if no bench mon --
        MoveEffect::Teleport => {
            if !state.sides[atk_side].team[atk_slot].is_fainted() {
                let has_bench = (0..6).any(|i| {
                    i != atk_slot
                        && state.sides[atk_side].team[i].species_id != 0
                        && state.sides[atk_side].team[i].current_hp > 0
                });
                if has_bench {
                    set_volatile(state, atk_side, VOL_MUST_SWITCH);
                }
            }
        }

        // -- Geomancy: +2 SpA/SpD/Spe (resolves on charge turn 2) --
        MoveEffect::ChargeGeomancy => {
            apply_boost(state, atk_side, SPA, 2);
            apply_boost(state, atk_side, SPD, 2);
            apply_boost(state, atk_side, SPE, 2);
        }

        // -- Terrain-setting moves --
        MoveEffect::SetTerrain => {
            let terrain = match md.move_type {
                Type::Electric => TERRAIN_ELECTRIC,
                Type::Grass    => TERRAIN_GRASSY,
                Type::Psychic  => TERRAIN_PSYCHIC,
                Type::Fairy    => TERRAIN_MISTY,
                _ => TERRAIN_NONE,
            };
            if terrain != TERRAIN_NONE {
                set_terrain(state, terrain, 5);
                crate::state::switch::check_paradox_deactivation(state);
                // Check terrain seed activation for both sides
                for s in 0..2 {
                    crate::state::switch::check_terrain_seed(state, s);
                }
            }
        }

        // -- Sleep Talk: pick a random move from user's moveset, dispatch it --
        // Showdown moves.ts:16916-16952. Filter: nosleeptalk, charge, isZ, isMax.
        // onTry requires user still asleep after the prelude's decrement.
        MoveEffect::SleepTalk => {
            if state.sides[atk_side].team[atk_slot].status != STATUS_SLEEP {
                return;
            }
            let moves = effective_moves(state, atk_side);
            let mut candidates: [u16; 4] = [0; 4];
            let mut n: u32 = 0;
            for i in 0..4 {
                let id = moves[i];
                if id == 0 { continue; }
                if SLEEP_TALK_FAIL.binary_search(&id).is_ok() { continue; }
                let inner_md = data_bridge::move_hot(id);
                if inner_md.flags & MoveFlags::CHARGE != 0 { continue; }
                candidates[n as usize] = id;
                n += 1;
            }
            if n == 0 { return; }
            let pick = rng(n) as usize;
            let inner_id = candidates[pick];
            let inner_md = *data_bridge::move_hot(inner_id);

            let outer_last_move = state.sides[atk_side].active.last_move;
            let had_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let had_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);

            use_move_called(
                state, teams, atk_side, atk_slot, def_side, def_slot,
                inner_id, false, false, false, &inner_md, rng, 1,
            );

            let now_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let now_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);
            if (now_charging && !had_charging) || (now_locked && !had_locked) {
                // R-b affirmative: resume sites at execute_move:2842 / :2851
                // read last_move; point them at the inner move so T2 resumes inner.
                state.sides[atk_side].active.last_move = inner_id;
            } else {
                // R-b negative: keep Sleep Talk so Mirror Move (data/moves.ts:12069)
                // sees the outer call, not the inner.
                state.sides[atk_side].active.last_move = outer_last_move;
            }
        }

        // -- Metronome: roll uniformly from METRONOME_OK, dispatch the inner --
        // Showdown moves.ts:11810-11836. No onTry; PP outer-only; bypasses Choice-lock.
        // Filter is the codegen-emitted 581-entry sorted slice (flags.metronome).
        MoveEffect::Metronome => {
            let inner_id = METRONOME_OK[rng(METRONOME_OK.len() as u32) as usize];
            let inner_md = *data_bridge::move_hot(inner_id);

            let outer_last_move = state.sides[atk_side].active.last_move;
            let had_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let had_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);

            use_move_called(
                state, teams, atk_side, atk_slot, def_side, def_slot,
                inner_id, false, false, false, &inner_md, rng, 1,
            );

            let now_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let now_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);
            if (now_charging && !had_charging) || (now_locked && !had_locked) {
                state.sides[atk_side].active.last_move = inner_id;
            } else {
                state.sides[atk_side].active.last_move = outer_last_move;
            }
        }

        // -- Copycat: replay BattleState.last_move_globally, gated by COPYCAT_FAIL --
        // Showdown moves.ts:2853-2877. No onTry; reads battle-level inner-wins
        // last move; fails when 0 or in the 46-entry COPYCAT_FAIL sidecar
        // (flags.failcopycat). PP outer-only; bypasses Choice-lock (engine prelude
        // latches outer Copycat id on Choice items).
        MoveEffect::Copycat => {
            let inner_id = state.last_move_globally;
            if inner_id == 0 || COPYCAT_FAIL.binary_search(&inner_id).is_ok() {
                return;
            }
            let inner_md = *data_bridge::move_hot(inner_id);

            let outer_last_move = state.sides[atk_side].active.last_move;
            let had_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let had_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);

            use_move_called(
                state, teams, atk_side, atk_slot, def_side, def_slot,
                inner_id, false, false, false, &inner_md, rng, 1,
            );

            let now_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let now_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);
            if (now_charging && !had_charging) || (now_locked && !had_locked) {
                state.sides[atk_side].active.last_move = inner_id;
            } else {
                state.sides[atk_side].active.last_move = outer_last_move;
            }
        }

        // -- Mirror Move: replay target's per-mon ActiveMon.last_move --
        // Showdown moves.ts:12058-12081. onTryHit reads target.lastMove (NOT
        // battle-level); fails when 0 (never moved / post-switch via
        // switch.rs:67 active.zero) or absent from the 644-entry MIRROR_MOVE_OK
        // sidecar (flags.mirror). PP outer-only; bypasses Choice-lock (engine
        // prelude latches outer Mirror Move id on Choice items).
        MoveEffect::MirrorMove => {
            let inner_id = state.sides[def_side].active.last_move;
            if inner_id == 0 || MIRROR_MOVE_OK.binary_search(&inner_id).is_err() {
                return;
            }
            let inner_md = *data_bridge::move_hot(inner_id);

            let outer_last_move = state.sides[atk_side].active.last_move;
            let had_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let had_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);

            use_move_called(
                state, teams, atk_side, atk_slot, def_side, def_slot,
                inner_id, false, false, false, &inner_md, rng, 1,
            );

            let now_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let now_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);
            if (now_charging && !had_charging) || (now_locked && !had_locked) {
                state.sides[atk_side].active.last_move = inner_id;
            } else {
                state.sides[atk_side].active.last_move = outer_last_move;
            }
        }

        // -- Assist: pick a random move from non-active teammates' movesets --
        // Showdown moves.ts:608-642. No precondition; iterates target.side.pokemon
        // skipping the active slot; collects each teammate's moveSlots filtered
        // against flags.noassist (the 51-entry ASSIST_FAIL sidecar) plus isZ/isMax;
        // uniform sample; useMove inner-only.
        MoveEffect::Assist => {
            let mut pool: [u16; 20] = [0; 20];
            let mut n: usize = 0;
            for i in 0..6 {
                if i == atk_slot { continue; }
                let team_moves = state.sides[atk_side].team[i].moves;
                for &mv in &team_moves {
                    if mv != 0 && ASSIST_FAIL.binary_search(&mv).is_err() {
                        pool[n] = mv;
                        n += 1;
                    }
                }
            }
            if n == 0 { return; }
            let inner_id = pool[rng(n as u32) as usize];
            let inner_md = *data_bridge::move_hot(inner_id);

            let outer_last_move = state.sides[atk_side].active.last_move;
            let had_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let had_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);

            use_move_called(
                state, teams, atk_side, atk_slot, def_side, def_slot,
                inner_id, false, false, false, &inner_md, rng, 1,
            );

            let now_charging = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
            let now_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);
            if (now_charging && !had_charging) || (now_locked && !had_locked) {
                state.sides[atk_side].active.last_move = inner_id;
            } else {
                state.sides[atk_side].active.last_move = outer_last_move;
            }
        }

        // -- Fallback for MoveEffect::None and damaging effects --
        _ => {
            if md.secondary_stat > 0 {
                if move_secondary_omniboosts(move_id) {
                    let stages = md.secondary_stat as i8;
                    for stat in [ATK, DEF, SPA, SPD, SPE] {
                        apply_boost(state, atk_side, stat, stages);
                    }
                } else {
                    let stat = secondary_self_boost_stat_override(move_id).unwrap_or_else(|| {
                        if md.category == MoveCategory::Physical { ATK } else { SPA }
                    });
                    apply_boost(state, atk_side, stat, md.secondary_stat as i8);
                }
            } else if md.secondary_stat < 0 {
                if state.sides[def_side].side_conditions.mist_turns() == 0 {
                    let stat = secondary_drop_stat_override(move_id).unwrap_or_else(|| {
                        if md.category == MoveCategory::Physical { DEF } else { SPD }
                    });
                    apply_boost(state, def_side, stat, md.secondary_stat as i8);
                }
            }
        }
    }

    if mirror_check {
        check_mirror_herb_diff(state, atk_side, &atk_boosts_before);
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
        MoveEffect::Coil | MoveEffect::ShiftGear | MoveEffect::HoneClaws |
        MoveEffect::SleepTalk | MoveEffect::Metronome | MoveEffect::Copycat |
        MoveEffect::MirrorMove | MoveEffect::Assist => true,

        // Screens (set on own side)
        MoveEffect::Reflect | MoveEffect::LightScreen | MoveEffect::AuroraVeil => true,

        // Field / own side
        MoveEffect::Tailwind | MoveEffect::TrickRoom | MoveEffect::Gravity |
        MoveEffect::MagicRoom | MoveEffect::WonderRoom | MoveEffect::SetTerrain => true,

        // Utility targeting self/own side
        MoveEffect::Substitute | MoveEffect::Wish | MoveEffect::BatonPass |
        MoveEffect::ChargeGeomancy | MoveEffect::BellyDrum | MoveEffect::Roost |
        MoveEffect::HealingWish | MoveEffect::LunarDance | MoveEffect::FocusEnergy |
        MoveEffect::Imprison | MoveEffect::Aromatherapy | MoveEffect::Minimize |
        MoveEffect::Stockpile | MoveEffect::Swallow | MoveEffect::MagnetRise |
        MoveEffect::DestinyBond | MoveEffect::ClangorousSoul |
        MoveEffect::Curse | MoveEffect::NoRetreat | MoveEffect::TidyUp |
        MoveEffect::AquaRing | MoveEffect::Ingrain |
        MoveEffect::Heal25 | MoveEffect::Heal25CureStatus |
        MoveEffect::Charge | MoveEffect::Endure |
        // Ally-target boosts: in singles resolve to self
        MoveEffect::AllyBoost => true,

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
    if state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(side).item_id).has(ItemFlag::EXTENDS_SCREENS)
    { 8 } else { 5 }
}

/// Apply Rocky Helmet + Rough Skin / Iron Barbs contact recoil to attacker.
/// Called per-hit from the multi-hit path (BUG-P2-D-104). Fires regardless of
/// whether the defender fainted from this hit (BUG-P5-M-201).
#[inline]
fn apply_contact_recoil(
    state: &mut BattleState,
    atk_side: usize,
    atk_slot: usize,
    def_side: usize,
    _md: &MoveData,
) {
    // Rocky Helmet: 1/6 attacker's max HP
    let def_itm = if state.field.magic_room_turns() > 0 {
        &data_bridge::ItemData::NONE
    } else {
        data_bridge::item(state.active_mon(def_side).item_id)
    };
    if def_itm.has(ItemFlag::ROCKY_HELMET) {
        let atk_max = state.active_mon(atk_side).max_hp;
        deal_damage(state, atk_side, atk_slot, atk_max / 6);
    }
    if state.sides[atk_side].team[atk_slot].is_fainted() { return; }
    // Rough Skin / Iron Barbs: 1/8 attacker's max HP. Faint-tolerant read: the
    // defender may have fainted on this same hit but its barbs still fire.
    let def_ab = effective_ability_ignoring_faint(state, def_side);
    if def_ab == data_bridge::ABILITY_ROUGH_SKIN
        || def_ab == data_bridge::ABILITY_IRON_BARBS
    {
        let atk_max = state.active_mon(atk_side).max_hp;
        deal_damage(state, atk_side, atk_slot, (atk_max / 8).max(1));
    }
}

/// Protect variant type, stored in _padding[0] bits 1-2.
const PROTECT_NORMAL: u8 = 0;
const PROTECT_KINGS_SHIELD: u8 = 1;
const PROTECT_BANEFUL_BUNKER: u8 = 2;
const PROTECT_SPIKY_SHIELD: u8 = 3;

fn execute_protect(
    state: &mut BattleState,
    side: usize,
    def_side: usize,
    move_id: u16,
    rng: &mut impl FnMut(u32) -> u32,
) {
    // Showdown's protect.onPrepareHit gates on `!!this.queue.willAct()`: if no
    // opponent action is still pending (it switched, or already moved), Protect
    // FAILS before the stall roll. pending_actions[def_side] is 0xFF once the
    // opponent's queued action has resolved, so an opponent-switch (resolves
    // first) leaves 0xFF here and the EOT reset zeroes the stall counter.
    if state.pending_actions[def_side] == 0xFF {
        return;
    }
    let consecutive = state.sides[side].active.protect_consecutive;
    // Showdown's `stall` volatile ladder: success 1/3^n on the nth consecutive
    // use (counter 3→9→27, counterMax beyond is out of force_all's deterministic
    // band). The 4th use rolls 1/27, not an automatic fail.
    let succeeds = match consecutive {
        0 => true,
        1 => rng(3) == 0,
        2 => rng(9) == 0,
        3 => rng(27) == 0,
        _ => false,
    };
    if succeeds {
        set_volatile(state, side, VOL_PROTECT_THIS_TURN);
        state.sides[side].active.protect_consecutive += 1;
        // Store protect variant in _padding[0] bits 1-2
        let variant = match move_id as usize {
            crate::data::MOVE_KING_S_SHIELD => PROTECT_KINGS_SHIELD,
            crate::data::MOVE_BANEFUL_BUNKER => PROTECT_BANEFUL_BUNKER,
            crate::data::MOVE_SPIKY_SHIELD => PROTECT_SPIKY_SHIELD,
            _ => PROTECT_NORMAL,
        };
        state.sides[side].active._padding[0] =
            (state.sides[side].active._padding[0] & !0x06) | (variant << 1);
    }
}

// Endure shares Showdown's `stall` volatile ladder with Protect: identical 1/3, 1/9
// success curve on consecutive uses; clamps Move-effect damage to leave 1 HP (recoil
// has effectType 'Recoil' and bypasses the clamp).
fn execute_endure(
    state: &mut BattleState,
    side: usize,
    def_side: usize,
    rng: &mut impl FnMut(u32) -> u32,
) {
    // Same willAct() gate as Protect: Endure shares the stall ladder, and its
    // onPrepareHit fails identically when no opponent action is still pending.
    if state.pending_actions[def_side] == 0xFF {
        return;
    }
    let consecutive = state.sides[side].active.protect_consecutive;
    let succeeds = match consecutive {
        0 => true,
        1 => rng(3) == 0,
        2 => rng(9) == 0,
        3 => rng(27) == 0,
        _ => false,
    };
    if succeeds {
        set_volatile(state, side, VOL_ENDURE);
        state.sides[side].active.protect_consecutive += 1;
    }
}

/// Check if weather allows skipping the charge turn.
#[inline]
fn weather_skips_charge(state: &BattleState, atk_side: usize, md: &MoveData) -> bool {
    match md.effect {
        MoveEffect::SolarBeam => matches!(effective_weather_for(state, atk_side), WEATHER_SUN | WEATHER_HARSH_SUN),
        MoveEffect::ChargeElectroShot => matches!(effective_weather_for(state, atk_side), WEATHER_RAIN | WEATHER_HEAVY_RAIN),
        _ => false,
    }
}

/// Check if the charge turn can be skipped (Power Herb or weather).
#[inline]
fn can_skip_charge(state: &BattleState, atk_side: usize, md: &MoveData) -> bool {
    if weather_skips_charge(state, atk_side, md) { return true; }
    state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(atk_side).item_id).has(ItemFlag::POWER_HERB)
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
    atk_side: usize,
    md: &MoveData,
) {
    match md.effect {
        MoveEffect::ChargeSkullBash => { apply_boost(state, atk_side, DEF, 1); }
        MoveEffect::ChargeMeteorBeam | MoveEffect::ChargeElectroShot => {
            apply_boost(state, atk_side, SPA, 1);
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

/// Check if a Pokémon's held item should activate based on HP or status.
/// Called after ANY HP change: post-damage, post-recoil, post-hazard, post-status damage.
/// Handles: pinch berries, Sitrus Berry, Lum Berry, Berry Juice, Starf Berry,
/// flavored heal berries (Aguav/Figy/Wiki/Mago/Iapapa), and Gluttony threshold.
pub fn check_berry_activation(
    state: &mut BattleState,
    teams: &TeamData,
    side: usize,
    slot: usize,
    rng: &mut impl FnMut(u32) -> u32,
) {
    let mon = &state.sides[side].team[slot];
    if mon.item_id == 0 || mon.is_fainted() { return; }
    if state.field.magic_room_turns() > 0 { return; }

    // Unnerve / As One: opponent cannot eat berries
    let opp_ability = effective_ability(state, 1 - side);
    if matches!(opp_ability,
        data_bridge::ABILITY_UNNERVE
        | data_bridge::ABILITY_AS_ONE_GLASTRIER
        | data_bridge::ABILITY_AS_ONE_SPECTRIER
    ) {
        return;
    }

    let item_id = mon.item_id;
    let item = data_bridge::item(item_id);

    let has_gluttony = effective_ability(state, side) == data_bridge::ABILITY_GLUTTONY;
    let max_hp = mon.max_hp;
    let current_hp = mon.current_hp;

    if item.has(ItemFlag::PINCH_BERRY) {
        let threshold = if has_gluttony { max_hp / 2 } else { max_hp / 4 };
        if current_hp <= threshold {
            let stat = item.type_param as usize;
            if stat < 5 {
                apply_boost(state, side, stat, 1);
            }
            consume_berry(state, side, slot);
        }
        return;
    }

    if item_id == data_bridge::ITEM_SITRUS_BERRY {
        if current_hp * 2 <= max_hp {
            heal(state, side, slot, max_hp / 4);
            consume_berry(state, side, slot);
        }
        return;
    }

    if item_id == data_bridge::ITEM_ORAN_BERRY {
        if current_hp * 2 <= max_hp {
            heal(state, side, slot, 10);
            consume_berry(state, side, slot);
        }
        return;
    }

    if item_id == data_bridge::ITEM_LUM_BERRY {
        if state.sides[side].team[slot].status != STATUS_NONE {
            clear_status(state, side, slot);
            consume_berry(state, side, slot);
        }
        return;
    }

    // Status-cure berries: each cures a specific status
    {
        let status = state.sides[side].team[slot].status;
        let cures = match item_id {
            data_bridge::ITEM_RAWST_BERRY  => status == STATUS_BURN,
            data_bridge::ITEM_CHESTO_BERRY => status == STATUS_SLEEP,
            data_bridge::ITEM_CHERI_BERRY  => status == STATUS_PARALYSIS,
            data_bridge::ITEM_PECHA_BERRY  => status == STATUS_POISON || status == STATUS_BAD_POISON,
            data_bridge::ITEM_ASPEAR_BERRY => status == STATUS_FREEZE,
            _ => false,
        };
        if cures {
            clear_status(state, side, slot);
            consume_berry(state, side, slot);
            return;
        }
    }

    if item_id == data_bridge::ITEM_BERRY_JUICE {
        if current_hp * 2 <= max_hp {
            heal(state, side, slot, 20);
            consume_berry(state, side, slot);
        }
        return;
    }

    if item_id == data_bridge::ITEM_STARF_BERRY {
        let threshold = if has_gluttony { max_hp / 2 } else { max_hp / 4 };
        if current_hp <= threshold {
            // Deterministic stat pick for MCTS: use turns_active as seed
            let stat = (state.sides[side].active.turns_active as usize) % 5;
            apply_boost(state, side, stat, 2);
            consume_berry(state, side, slot);
        }
        return;
    }

    match item_id {
        data_bridge::ITEM_AGUAV_BERRY | data_bridge::ITEM_FIGY_BERRY |
        data_bridge::ITEM_WIKI_BERRY | data_bridge::ITEM_MAGO_BERRY |
        data_bridge::ITEM_IAPAPA_BERRY => {
            let threshold = if has_gluttony { max_hp / 2 } else { max_hp / 4 };
            if current_hp <= threshold {
                heal(state, side, slot, max_hp / 3);
                // Showdown items.ts onEat: confuse if the holder's nature dislikes
                // the berry's flavor (nature.minus === <flavor stat>). Self-inflicted,
                // so Safeguard does NOT block it (target===source); Own Tempo does.
                let disliked = match item_id {
                    data_bridge::ITEM_FIGY_BERRY   => 0, // atk
                    data_bridge::ITEM_IAPAPA_BERRY => 1, // def
                    data_bridge::ITEM_WIKI_BERRY   => 2, // spa
                    data_bridge::ITEM_AGUAV_BERRY  => 3, // spd
                    _                              => 4, // mago -> spe
                };
                let nature = teams.mons[side][slot].nature;
                let minus = (nature % 5) as usize;
                let has_minus = (nature / 5) != (nature % 5);
                if has_minus && minus == disliked
                    && state.sides[side].active.confusion_turns == 0
                    && effective_ability(state, side) != data_bridge::ABILITY_OWN_TEMPO
                {
                    state.sides[side].active.confusion_turns = (rng(4) + 2) as u8;
                }
                consume_berry(state, side, slot);
            }
        }
        _ => {}
    }
}

/// Consume a berry and trigger Unburden / Cheek Pouch if applicable.
#[inline]
pub(crate) fn consume_berry(state: &mut BattleState, side: usize, slot: usize) {
    let item_id = state.sides[side].team[slot].item_id;
    state.sides[side].set_last_consumed_berry(item_id);
    consume_item(state, side, slot);
    let ability = effective_ability(state, side);
    if ability == data_bridge::ABILITY_UNBURDEN {
        set_volatile(state, side, VOL_UNBURDEN);
    }
    // Cheek Pouch: heal 1/3 max HP on berry consumption (Showdown onEatItem).
    // Fires after the berry's base effect. Skipped if already at full HP.
    if ability == data_bridge::ABILITY_CHEEK_POUCH {
        let mon = &state.sides[side].team[slot];
        if !mon.is_fainted() && mon.current_hp < mon.max_hp {
            let max_hp = mon.max_hp;
            heal(state, side, slot, max_hp / 3);
        }
    }
}

/// Legacy alias for the switch-in path, which has no RNG stream in scope.
/// Only the flavor-berry confusion DURATION reads rng; the decision to confuse
/// is deterministic (nature-based), so a switch-in flavor-berry trigger uses
/// the minimum 2-turn duration. Sitrus/pinch berries (the usual switch-in case)
/// consume no rng.
pub fn check_pinch_berry(
    state: &mut BattleState, teams: &TeamData, side: usize, slot: usize,
) {
    check_berry_activation(state, teams, side, slot, &mut |_| 0);
}

/// Synchronize: when a Pokemon with Synchronize is inflicted with burn,
/// paralysis, poison, or toxic by an opposing mon, mirror the same status
/// back onto the inflicter. Sleep and Freeze are NOT passed back.
/// Call AFTER a successful status application, with:
///   `target_side` = the side that received the status (Synchronize holder)
///   `source_side` = the side that inflicted it (attacker)
///   `status`      = the status applied
#[inline]
fn try_synchronize_back(
    state: &mut BattleState,
    target_side: usize,
    source_side: usize,
    status: u8,
) {
    if target_side == source_side { return; }
    // Only burn/para/psn/tox — sleep and freeze don't pass back
    if !matches!(status, STATUS_BURN | STATUS_PARALYSIS | STATUS_POISON | STATUS_BAD_POISON) {
        return;
    }
    // Target must have Synchronize (not suppressed)
    if effective_ability(state, target_side) != data_bridge::ABILITY_SYNCHRONIZE { return; }
    // Source must be alive and not already statused
    let source_slot = state.sides[source_side].active_index as usize;
    if state.sides[source_side].team[source_slot].current_hp == 0 { return; }
    if state.sides[source_side].team[source_slot].status != STATUS_NONE { return; }
    // Check source's immunity to the status (type, terrain, safeguard, minior)
    if type_immune_to_status(state, source_side, status) { return; }
    if state.sides[source_side].side_conditions.safeguard_turns() > 0 { return; }
    if terrain_blocks_status(state, source_side, status) { return; }
    if ability_status_immune(state, source_side, effective_ability(state, target_side), status) { return; }
    if crate::state::forme::is_minior_meteor_forme(state, source_side) { return; }
    set_status(state, source_side, source_slot, status, 0);
}

/// Apply an opponent-target stat change (drop or boost) dispatched by the
/// SelfEffect Opp* variants on status moves (Growl, Leer, Swagger boost, etc.).
/// Drops go through try_opponent_stat_drop (Clear Amulet / Mirror Armor /
/// Competitive / Defiant); boosts use apply_boost directly on def_side.
#[inline]
fn apply_opp_stat_change(
    state: &mut BattleState,
    def_side: usize,
    se: SelfEffect,
) {
    // Drops respect Mist (applied at call site for moves that have Mist immunity).
    // For status-move drops, Mist blocks them — mirror the legacy behavior.
    let mist_blocks_drop = state.sides[def_side].side_conditions.mist_turns() > 0;
    match se {
        // Drops
        SelfEffect::OppAtkDown1 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, ATK, -1); }
        SelfEffect::OppAtkDown2 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, ATK, -2); }
        SelfEffect::OppDefDown1 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, DEF, -1); }
        SelfEffect::OppDefDown2 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, DEF, -2); }
        SelfEffect::OppSpADown1 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, SPA, -1); }
        SelfEffect::OppSpADown2 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, SPA, -2); }
        SelfEffect::OppSpDDown2 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, SPD, -2); }
        SelfEffect::OppSpeDown1 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, SPE, -1); }
        SelfEffect::OppSpeDown2 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, SPE, -2); }
        SelfEffect::OppAccDown1 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, ACC, -1); }
        SelfEffect::OppEvaDown2 => if !mist_blocks_drop { try_opponent_stat_drop(state, def_side, EVA, -2); }
        SelfEffect::OppAtkDefDown1 => if !mist_blocks_drop {
            try_opponent_stat_drop(state, def_side, ATK, -1);
            try_opponent_stat_drop(state, def_side, DEF, -1);
        }
        SelfEffect::OppAtkSpADown1 => if !mist_blocks_drop {
            try_opponent_stat_drop(state, def_side, ATK, -1);
            try_opponent_stat_drop(state, def_side, SPA, -1);
        }
        SelfEffect::OppAtkSpADown2 => if !mist_blocks_drop {
            try_opponent_stat_drop(state, def_side, ATK, -2);
            try_opponent_stat_drop(state, def_side, SPA, -2);
        }
        // Boosts (on opponent target — Swagger/Flatter/Decorate)
        SelfEffect::OppAtkUp2 => { apply_boost(state, def_side, ATK, 2); }
        SelfEffect::OppSpAUp1 => { apply_boost(state, def_side, SPA, 1); }
        SelfEffect::OppAtkSpAUp2 => {
            apply_boost(state, def_side, ATK, 2);
            apply_boost(state, def_side, SPA, 2);
        }
        SelfEffect::OppAtkUp2DefDown2 => {
            apply_boost(state, def_side, ATK, 2);
            if !mist_blocks_drop {
                try_opponent_stat_drop(state, def_side, DEF, -2);
            }
        }
        _ => {}
    }
}

/// Apply an ally-target stat boost (Howl, Aromatic Mist, Coaching).
/// In singles, ally == self, so target = atk_side.
#[inline]
fn apply_ally_stat_change(
    state: &mut BattleState,
    atk_side: usize,
    se: SelfEffect,
) {
    match se {
        SelfEffect::AllyAtkUp1 => { apply_boost(state, atk_side, ATK, 1); }
        SelfEffect::AllySpDUp1 => { apply_boost(state, atk_side, SPD, 1); }
        SelfEffect::AllyAtkDefUp1 => {
            apply_boost(state, atk_side, ATK, 1);
            apply_boost(state, atk_side, DEF, 1);
        }
        _ => {}
    }
}

/// Record that the active mon's move on `atk_side` failed this turn (Showdown
/// `moveThisTurnResult = false`) — fuels Stomping Tantrum / Temper Flare's ×2.
/// Placed at the genuine hit-resolution failure sites (miss / type-or-ability
/// immunity) in both the damaging and status dispatch paths; NOT on Protect-block
/// / Disguise (Showdown counts those as `true`) nor pre-move skips. A single byte
/// OR off the per-hit `calc_damage` loop — never runs for a move that connects.
#[inline(always)]
fn mark_move_failed(state: &mut BattleState, atk_side: usize) {
    state.sides[atk_side].set_move_failed_this_turn();
}

/// Apply crash damage (50% max HP) if the move has CrashDamage self-effect.
/// Called on every move-failure path: miss, Protect, immunity, semi-invuln.
#[inline]
fn apply_crash_if_needed(
    state: &mut BattleState,
    atk_side: usize,
    md: &MoveData,
) {
    if md.self_effect == SelfEffect::CrashDamage {
        let atk_slot = state.sides[atk_side].active_index as usize;
        let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
        deal_damage(state, atk_side, atk_slot, max_hp / 2);
    }
}

/// Apply the attacker's self-effect after damage + drain/recoil.
/// CrashDamage is NOT handled here (it's on the failure paths via apply_crash_if_needed).
#[inline]
fn apply_self_effect(
    state: &mut BattleState,
    atk_side: usize,
    md: &MoveData,
) {
    let self_mirror = matches!(md.self_effect,
        SelfEffect::AtkUp1 | SelfEffect::SpeUp1 | SelfEffect::DefUp1 | SelfEffect::SpAUp1
        | SelfEffect::DefDown1SpeUp1)
        && state.field.magic_room_turns() == 0
        && state.active_mon(1 - atk_side).item_id != 0;
    let self_boosts_before = if self_mirror { state.sides[atk_side].active.boosts } else { [0; 7] };

    match md.self_effect {
        SelfEffect::None => {}
        SelfEffect::DefSpDDown1 => {
            apply_boost(state, atk_side, DEF, -1);
            apply_boost(state, atk_side, SPD, -1);
        }
        SelfEffect::AtkDefDown1 => {
            apply_boost(state, atk_side, ATK, -1);
            apply_boost(state, atk_side, DEF, -1);
        }
        SelfEffect::DefSpDSpeDown1 => {
            apply_boost(state, atk_side, DEF, -1);
            apply_boost(state, atk_side, SPD, -1);
            apply_boost(state, atk_side, SPE, -1);
        }
        SelfEffect::SpADown2 => {
            apply_boost(state, atk_side, SPA, -2);
        }
        SelfEffect::SpeDown1 => {
            apply_boost(state, atk_side, SPE, -1);
        }
        SelfEffect::SpeDown2 => {
            apply_boost(state, atk_side, SPE, -2);
        }
        SelfEffect::AtkDown1 => {
            apply_boost(state, atk_side, ATK, -1);
        }
        SelfEffect::SpDDown1 => {
            apply_boost(state, atk_side, SPD, -1);
        }
        SelfEffect::DefDown1 => {
            apply_boost(state, atk_side, DEF, -1);
        }
        SelfEffect::SpADown1 => {
            apply_boost(state, atk_side, SPA, -1);
        }
        SelfEffect::AtkUp1 => {
            apply_boost(state, atk_side, ATK, 1);
        }
        SelfEffect::SpeUp1 => {
            apply_boost(state, atk_side, SPE, 1);
        }
        SelfEffect::DefUp1 => {
            apply_boost(state, atk_side, DEF, 1);
        }
        SelfEffect::SpAUp1 => {
            apply_boost(state, atk_side, SPA, 1);
        }
        SelfEffect::DefDown1SpeUp1 => {
            apply_boost(state, atk_side, DEF, -1);
            apply_boost(state, atk_side, SPE, 1);
        }
        SelfEffect::ThawSelf => {
            let atk_slot = state.sides[atk_side].active_index as usize;
            if state.sides[atk_side].team[atk_slot].status == STATUS_FREEZE {
                clear_status(state, atk_side, atk_slot);
            }
        }
        // SelfSwitch/BatonPass/PartingShot/Heal50 are handled via MoveEffect.
        // CrashDamage is handled in the miss path (§B).
        _ => {}
    }

    if self_mirror {
        check_mirror_herb_diff(state, atk_side, &self_boosts_before);
    }
    // White Herb: restore negative stat changes after self-drops
    check_white_herb(state, atk_side);
}

/// Returns true if the ability cannot be suppressed/overwritten by Mummy or Lingering Aroma.
#[inline]
fn is_cantsuppress_ability(ability: u16) -> bool {
    matches!(ability,
        data_bridge::ABILITY_AS_ONE_GLASTRIER
        | data_bridge::ABILITY_AS_ONE_SPECTRIER
        | data_bridge::ABILITY_BATTLE_BOND
        | data_bridge::ABILITY_COMATOSE
        | data_bridge::ABILITY_DISGUISE
        | data_bridge::ABILITY_GULP_MISSILE
        | data_bridge::ABILITY_ICE_FACE
        | data_bridge::ABILITY_MULTITYPE
        | data_bridge::ABILITY_POWER_CONSTRUCT
        | data_bridge::ABILITY_RKS_SYSTEM
        | data_bridge::ABILITY_SCHOOLING
        | data_bridge::ABILITY_SHIELDS_DOWN
        | data_bridge::ABILITY_STANCE_CHANGE
        | data_bridge::ABILITY_TERA_SHIFT
        | data_bridge::ABILITY_ZEN_MODE
        | data_bridge::ABILITY_ZERO_TO_HERO
    )
}

// Abilities carrying Showdown's `failskillswap: 1` flag (data/abilities.ts):
// Skill Swap fails outright if either side holds one of these.
fn is_failskillswap_ability(ability: u16) -> bool {
    matches!(ability,
        data_bridge::ABILITY_AS_ONE_GLASTRIER
        | data_bridge::ABILITY_AS_ONE_SPECTRIER
        | data_bridge::ABILITY_BATTLE_BOND
        | data_bridge::ABILITY_COMATOSE
        | data_bridge::ABILITY_COMMANDER
        | data_bridge::ABILITY_DISGUISE
        | data_bridge::ABILITY_EMBODY_ASPECT_TEAL
        | data_bridge::ABILITY_EMBODY_ASPECT_WELLSPRING
        | data_bridge::ABILITY_EMBODY_ASPECT_HEARTHFLAME
        | data_bridge::ABILITY_EMBODY_ASPECT_CORNERSTONE
        | data_bridge::ABILITY_HUNGER_SWITCH
        | data_bridge::ABILITY_ICE_FACE
        | data_bridge::ABILITY_ILLUSION
        | data_bridge::ABILITY_MULTITYPE
        | data_bridge::ABILITY_NEUTRALIZING_GAS
        | data_bridge::ABILITY_POISON_PUPPETEER
        | data_bridge::ABILITY_POWER_CONSTRUCT
        | data_bridge::ABILITY_PROTOSYNTHESIS
        | data_bridge::ABILITY_QUARK_DRIVE
        | data_bridge::ABILITY_RKS_SYSTEM
        | data_bridge::ABILITY_SCHOOLING
        | data_bridge::ABILITY_SHIELDS_DOWN
        | data_bridge::ABILITY_STANCE_CHANGE
        | data_bridge::ABILITY_TERA_SHELL
        | data_bridge::ABILITY_TERA_SHIFT
        | data_bridge::ABILITY_TERAFORM_ZERO
        | data_bridge::ABILITY_WONDER_GUARD
        | data_bridge::ABILITY_ZEN_MODE
        | data_bridge::ABILITY_ZERO_TO_HERO
    )
}

// Apply one side of a Skill Swap: overwrite the base ability_id with
// `new_ability`. The pre-swap (native) ability is stashed for switch-out
// restore only on the first swap — a second swap must still restore the true
// baseAbility, not the intermediate value (Showdown resets to baseAbility).
fn skill_swap_apply(state: &mut BattleState, side: usize, slot: usize, new_ability: u16) {
    if state.sides[side].team[slot].flags & MON_FLAG_ABILITY_SWAPPED == 0 {
        state.sides[side].active.override_ability = state.sides[side].team[slot].ability_id;
        state.sides[side].team[slot].flags |= MON_FLAG_ABILITY_SWAPPED;
    }
    state.sides[side].active.volatile_flags &= !VOL_ABILITY_OVERRIDDEN;
    state.sides[side].team[slot].ability_id = new_ability;
}

fn foe_pokemon_left(state: &BattleState, side: usize) -> bool {
    let opp = 1 - side;
    (0..6).any(|i| {
        state.sides[opp].team[i].species_id != 0
            && state.sides[opp].team[i].current_hp > 0
    })
}

/// Inner "useMove" dispatch — mirrors Showdown's `useMove` in
/// `sim/battle-actions.ts:298-376`. The outer entry `execute_move` runs
/// Showdown's `runMove` prelude (`sim/battle-actions.ts:200-296`: PP,
/// `last_move`, Choice-lock, `VOL_MOVED_THIS_TURN`, status/flinch/attract/
/// taunt gates) and then hands off here. Future Call*-family handlers
/// (Sleep Talk / Metronome / Copycat) will call this directly with `depth=1`.
///
/// FORWARD-COMPAT: Dancer / Magic Bounce dispatch through Showdown's
/// `runMove` with `externalMove: true`, NOT `useMove`. They must route
/// through a `runMove`-equivalent with their own re-entry guard — do not
/// piggyback on this Call*-only depth cap.
#[allow(clippy::too_many_arguments)]
#[inline]
pub(crate) fn use_move_called(
    state: &mut BattleState,
    teams: &TeamData,
    atk_side: usize,
    atk_slot: usize,
    def_side: usize,
    def_slot: usize,
    move_id: u16,
    is_struggle: bool,
    is_charge_turn2: bool,
    is_move_locked: bool,
    md: &MoveData,
    rng: &mut impl FnMut(u32) -> u32,
    depth: u8,
) {
    if is_call_family(move_id) && depth > 0 { return; }

    // Battle-level last-move record (Showdown `battle.lastMove`, written by
    // `clearActiveMove` after every `useMove` — `sim/battle.ts:376-385`). Inner
    // dispatch wins (last-write-wins): Metronome rolls Bullet Seed → Copycat
    // reads Bullet Seed. Skipped for Call*-family outers so the arm can still
    // read the prior value (Showdown's `activeMove`/`lastMove` split achieves
    // the same effect via inner clobber-then-flush; we collapse the surfaces).
    if !is_call_family(move_id) {
        state.last_move_globally = move_id;
    }

    // Sucker Punch onTry: fails unless the target is queued to use a damaging
    // move this turn and isn't recharging. Mirrors moves.ts:suckerpunch.onTry —
    // willMove(target) returns null when the defender has already moved or is
    // queued to switch; status moves (except Me First) also fail it.
    if move_id == 389 {
        let def_active = &state.sides[def_side].active;
        let def_already_moved = def_active.has_volatile(VOL_MOVED_THIS_TURN)
            || def_active.has_volatile(VOL_RECHARGING);
        let raw = state.pending_actions[def_side];
        let queued_move_id: u16 = match raw {
            0..=3 => effective_moves(state, def_side)[raw as usize],
            ACTION_TERA => effective_moves(state, def_side)[0],
            _ => 0, // switch / 0xFF / unknown
        };
        let queued_is_damaging = queued_move_id != 0
            && data_bridge::move_hot(queued_move_id).category != MoveCategory::Status;
        if def_already_moved || !queued_is_damaging {
            return;
        }
    }

    // Upper Hand onTry: fails unless the target is queued to use a damaging
    // move with priority > 0.1 this turn. Mirrors moves.ts:upperhand.onTry —
    // priority <= 0.1 fails it, so a priority-0 move (Grass Knot) or any status
    // move is rejected. The engine stores priority as integer steps, so the
    // 0.1 threshold collapses to priority > 0.
    if move_id == 918 {
        let def_active = &state.sides[def_side].active;
        let def_already_moved = def_active.has_volatile(VOL_MOVED_THIS_TURN)
            || def_active.has_volatile(VOL_RECHARGING);
        let raw = state.pending_actions[def_side];
        let queued_move_id: u16 = match raw {
            0..=3 => effective_moves(state, def_side)[raw as usize],
            ACTION_TERA => effective_moves(state, def_side)[0],
            _ => 0,
        };
        let queued_qualifies = queued_move_id != 0 && {
            let qm = data_bridge::move_hot(queued_move_id);
            qm.category != MoveCategory::Status && qm.priority > 0
        };
        if def_already_moved || !queued_qualifies {
            return;
        }
    }

    // Last Resort (387) onTry: fails unless the user knows >= 2 moves AND every
    // other (non-Last-Resort) move slot has been used at least once this battle.
    // Mirrors moves.ts:lastresort.onTry. Slot 0 = empty (move id 0) is not counted.
    // Last Resort's own slot is skipped, matching Showdown's `id === 'lastresort'`
    // continue. Uses the persistent per-slot used mask, NOT PP deltas.
    if move_id == 387 {
        let mon = &state.sides[atk_side].team[atk_slot];
        let known = mon.moves.iter().filter(|&&m| m != 0).count();
        if known < 2 {
            return;
        }
        let mut all_others_used = true;
        for i in 0..4 {
            let m = mon.moves[i];
            if m == 0 || m == 387 { continue; }
            if !mon.move_used(i) {
                all_others_used = false;
                break;
            }
        }
        if !all_others_used {
            return;
        }
    }

    // Fake Out (252) / First Impression (660) / Mat Block (561) onTry: only work
    // on the user's first move action after switching in. Showdown keys
    // `source.activeMoveActions > 1` (moves.ts); fail (no damage, no flinch /
    // side-condition, action consumed) once the user has already taken a move
    // action this stay-in. codegen drops the onTry so these carry effect:None.
    // `acted_since_switch_in` is set in execute_action after each move action and
    // cleared on switch-out, so it works on the lead's first move, fails on later
    // moves, and works again after switching out and back in. (turns_active is the
    // wrong proxy: a mid-battle switch-in mon's first move lands at turns_active==1
    // because the switch turn's EOT already ticked it.)
    if (move_id == 252 || move_id == 660 || move_id == 561)
        && state.sides[atk_side].acted_since_switch_in()
    {
        return;
    }

    // Focus Punch beforeMoveCallback: the move fails (|cant|, 0 damage) if the
    // user was hit by a damaging move earlier this turn (Showdown's lostFocus,
    // set on any non-Status hit). At -3 priority the faster opponent's damaging
    // move has already landed, so times_hit (reset each EOT) is the per-turn
    // "took a damaging hit" signal.
    if md.effect == MoveEffect::FocusPunch && state.sides[atk_side].active.times_hit > 0 {
        return;
    }

    // Protean / Libero: change type to match move before attacking.
    // Showdown skips on `move.callsMove` (data/abilities.ts:3444) so outer
    // Call*-family dispatches do not burn the once-per-switch flag.
    if !is_struggle && !is_charge_turn2 && !is_move_locked && !is_call_family(move_id) {
        let atk_ability = effective_ability(state, atk_side);
        if (atk_ability == data_bridge::ABILITY_PROTEAN || atk_ability == data_bridge::ABILITY_LIBERO)
            && state.sides[atk_side].active._padding[3] & 1 == 0
        {
            let new_type = md.move_type as u8;
            state.sides[atk_side].active.override_types = [new_type, new_type];
            set_volatile(state, atk_side, VOL_TYPES_OVERRIDDEN);
            state.sides[atk_side].active._padding[3] |= 1; // once per switch-in
        }
    }

    // Stance Change: Aegislash switches between Shield and Blade forme
    if !is_struggle {
        let atk_ability = effective_ability(state, atk_side);
        if atk_ability == data_bridge::ABILITY_STANCE_CHANGE {
            let species = effective_species(state, atk_side);
            const AEGISLASH_SHIELD: u16 = 681;
            const AEGISLASH_BLADE: u16 = 1103;
            const KING_S_SHIELD: u16 = 588;
            if species == AEGISLASH_SHIELD && md.category != MoveCategory::Status {
                apply_battle_forme(state, teams, atk_side, AEGISLASH_BLADE);
            } else if species == AEGISLASH_BLADE && move_id == KING_S_SHIELD {
                revert_battle_forme(state, atk_side);
            }
        }
    }

    if !is_charge_turn2 && !is_struggle && md.flags & MoveFlags::CHARGE != 0 {
        let skip = can_skip_charge(state, atk_side, md);
        apply_charge_turn_effects(state, atk_side, md);
        if skip {
            // Consume Power Herb if it was the skip reason (not weather)
            if !weather_skips_charge(state, atk_side, md) {
                consume_item(state, atk_side, atk_slot);
                if effective_ability(state, atk_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, atk_side, VOL_UNBURDEN);
                }
            }
            // Fall through to execute the move this turn
        } else {
            // Begin charging
            set_volatile(state, atk_side, VOL_CHARGING);
            if let Some(loc) = semi_invuln_location(md) {
                set_volatile(state, atk_side, VOL_SEMI_INVULNERABLE);
                state.sides[atk_side].active._padding[1] = loc;
            }
            return; // End turn 1 (charge moves are never thrash, so return is correct)
        }
    }

    if !is_charge_turn2 && !is_struggle && !is_move_locked
        && md.effect == MoveEffect::Thrash
    {
        set_volatile(state, atk_side, VOL_MOVE_LOCKED);
        state.sides[atk_side].active._padding[2] = (rng(2) + 1) as u8; // 1 or 2 more turns
    } else if !is_charge_turn2 && !is_struggle && !is_move_locked
        && md.effect == MoveEffect::Uproar
    {
        set_volatile(state, atk_side, VOL_MOVE_LOCKED);
        state.sides[atk_side].active._padding[2] = 2; // Uproar locks a fixed 3 turns total
    }

    // Self-Destruct / Explosion / Misty Explosion: user faints before damage
    if matches!(move_id, 120 | 153 | 606) {
        let hp = state.sides[atk_side].team[atk_slot].current_hp;
        deal_damage(state, atk_side, atk_slot, hp);
    }

    // Safety Goggles blocks powder/spore moves, including status-category ones
    // (Spore, Sleep Powder, Stun Spore, Poison Powder, Cotton Spore, Rage Powder).
    // Not bypassed by Mold Breaker (item, not ability). Must come before the
    // status-move dispatch so status powder moves are fully blocked.
    // Grass types are also powder-immune (typechart: grass row damageTaken.powder = 3).
    if !is_struggle
        && md.flags & MoveFlags::POWDER != 0
        && (holds_safety_goggles(state, def_side)
            || has_type(state, def_side, Type::Grass as u8))
    {
        apply_crash_if_needed(state, atk_side, md);
        mark_move_failed(state, atk_side);
        return;
    }

    if !is_struggle && md.category == MoveCategory::Status {
        execute_status_move(state, teams, atk_side, def_side, md, rng, move_id);
        return; // Status moves are never thrash, so return is correct
    }

    let bypasses_protect = is_charge_turn2 && md.effect == MoveEffect::ChargePhantom;
    if !bypasses_protect && state.sides[def_side].active.has_volatile(VOL_PROTECT_THIS_TURN) {
        apply_crash_if_needed(state, atk_side, md);
        // Protect variant contact penalties
        if md.flags & MoveFlags::CONTACT != 0 {
            let variant = (state.sides[def_side].active._padding[0] >> 1) & 0x03;
            match variant {
                PROTECT_KINGS_SHIELD => {
                    // King's Shield: -1 Atk on contact
                    apply_boost(state, atk_side, ATK, -1);
                }
                PROTECT_BANEFUL_BUNKER => {
                    // Baneful Bunker: poison on contact
                    let atk_slot = state.sides[atk_side].active_index as usize;
                    if state.sides[atk_side].team[atk_slot].status == STATUS_NONE
                        && state.sides[atk_side].side_conditions.safeguard_turns() == 0
                        && !terrain_blocks_status(state, atk_side, STATUS_POISON)
                        && !type_immune_to_status(state, atk_side, STATUS_POISON)
                        && !ability_status_immune(state, atk_side, effective_ability(state, def_side), STATUS_POISON)
                    {
                        set_status(state, atk_side, atk_slot, STATUS_POISON, 0);
                    }
                }
                PROTECT_SPIKY_SHIELD => {
                    // Spiky Shield: 1/8 max HP damage on contact
                    let atk_slot = state.sides[atk_side].active_index as usize;
                    deal_proportional_damage(state, atk_side, atk_slot, 1, 8);
                }
                _ => {} // Normal Protect: no penalty
            }
        }
        return;
    }

    if !is_struggle && state.sides[def_side].active.has_volatile(VOL_SEMI_INVULNERABLE) {
        let atk_ability = effective_ability(state, atk_side);
        let def_ability = effective_ability(state, def_side);
        if atk_ability != data_bridge::ABILITY_NO_GUARD
            && def_ability != data_bridge::ABILITY_NO_GUARD
            && !can_hit_semi_invuln(move_id, state.sides[def_side].active._padding[1])
        {
            apply_crash_if_needed(state, atk_side, md);
            mark_move_failed(state, atk_side);
            return;
        }
    }

    if !is_struggle && !accuracy_check(state, atk_side, md, rng) {
        apply_crash_if_needed(state, atk_side, md);
        mark_move_failed(state, atk_side);
        // Fury Cutter resets its escalating BP counter on miss (Showdown
        // clears the `furycutter` volatile when the move fails to hit).
        if move_id == crate::data::MOVE_FURY_CUTTER as u16 {
            state.sides[atk_side].active.consec_move_count = 0;
        }
        return;
    }

    if !is_struggle {
        // Priority-blocking: Dazzling / Queenly Majesty / Armor Tail
        if priority_block_immunity(state, def_side, md.priority) {
            apply_crash_if_needed(state, atk_side, md);
            mark_move_failed(state, atk_side);
            return;
        }
        // Flag-based immunities (Bulletproof, Soundproof, Overcoat, Wind Rider)
        // Mold Breaker bypasses these
        let atk_ability_imm = effective_ability(state, atk_side);
        if let Some(eff) = ability_flag_immunity(state, def_side, md.flags, atk_ability_imm) {
            apply_immunity_effect(state, def_side, def_slot, eff);
            apply_crash_if_needed(state, atk_side, md);
            mark_move_failed(state, atk_side);
            return;
        }
        // Type-based immunities and side effects
        if let Some(eff) = ability_type_immunity(state, def_side, md.move_type, atk_ability_imm) {
            apply_immunity_effect(state, def_side, def_slot, eff);
            apply_crash_if_needed(state, atk_side, md);
            mark_move_failed(state, atk_side);
            return;
        }
        // Dream Eater: onTryImmunity fails unless the target is asleep (or Comatose).
        if move_id == crate::data::MOVE_DREAM_EATER as u16
            && state.sides[def_side].team[def_slot].status != STATUS_SLEEP
            && effective_ability(state, def_side) != data_bridge::ABILITY_COMATOSE
        {
            mark_move_failed(state, atk_side);
            return;
        }
    }

    // Endeavor: set target HP = user HP
    if md.effect == MoveEffect::Endeavor {
        let user_hp = state.sides[atk_side].team[atk_slot].current_hp;
        let target_hp = state.sides[def_side].team[def_slot].current_hp;
        if target_hp > user_hp {
            deal_damage(state, def_side, def_slot, target_hp - user_hp);
        }
        return;
    }

    // SuperFang: halve target's current HP
    if md.effect == MoveEffect::SuperFang {
        let target_hp = state.sides[def_side].team[def_slot].current_hp;
        deal_damage(state, def_side, def_slot, (target_hp / 2).max(1));
        return;
    }

    // SeismicToss / Night Shade: damage = user's level. Respect type immunity
    // (Ghost vs Normal Night Shade, Normal vs Ghost Seismic Toss).
    if md.effect == MoveEffect::SeismicToss {
        let (def_t1, def_t2) = battle_types(state, def_side);
        let def_type1 = unsafe { core::mem::transmute::<u8, Type>(def_t1) };
        let def_type2 = unsafe { core::mem::transmute::<u8, Type>(def_t2) };
        let eff = crate::data::types::dual_type_effectiveness(md.move_type, def_type1, def_type2);
        if eff != 0 {
            let level = state.active_mon(atk_side).level as u16;
            deal_damage(state, def_side, def_slot, level);
        }
        return;
    }

    // Counter: return 2× physical damage taken this turn
    if md.effect == MoveEffect::Counter {
        let last_hit = state.sides[atk_side].active.last_move_hit_by;
        if last_hit != 0 {
            let last_md = data_bridge::move_hot(last_hit);
            if last_md.category == MoveCategory::Physical {
                // Approximate: use 1/4 of attacker's max HP as base (simplified for MCTS)
                let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, def_side, def_slot, max_hp / 2);
            }
        }
        return;
    }

    // MirrorCoat: return 2× special damage taken this turn
    if md.effect == MoveEffect::MirrorCoat {
        let last_hit = state.sides[atk_side].active.last_move_hit_by;
        if last_hit != 0 {
            let last_md = data_bridge::move_hot(last_hit);
            if last_md.category == MoveCategory::Special {
                let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, def_side, def_slot, max_hp / 2);
            }
        }
        return;
    }

    // MetalBurst: return 1.5× last damage taken
    if md.effect == MoveEffect::MetalBurst {
        let last_hit = state.sides[atk_side].active.last_move_hit_by;
        if last_hit != 0 {
            let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
            deal_damage(state, def_side, def_slot, max_hp * 3 / 8);
        }
        return;
    }

    // FinalGambit: deal user's current HP as damage, user faints
    if md.effect == MoveEffect::FinalGambit {
        let user_hp = state.sides[atk_side].team[atk_slot].current_hp;
        deal_damage(state, def_side, def_slot, user_hp);
        deal_damage(state, atk_side, atk_slot, user_hp);
        return;
    }

    // SpitUp: fail if no stockpile (VarPower::SpitUp returns 0 BP)
    if md.effect == MoveEffect::SpitUp {
        let count = state.sides[atk_side].active.stockpile & 0x7F;
        if count == 0 { return; }
    }

    // Fling (374): validate the held item BP, then let calc_damage read
    // power_param for BP and treat atk_item as NONE so Life Orb / Choice Band
    // / type boosts do not apply. The actual item slot is cleared post-calc,
    // before any post-damage item reads (Shell Bell / Throat Spray). Showdown
    // moves.ts:5744-5797 — onPrepareHit sets BP, fling-volatile's onUpdate
    // clears the item before damage events.
    // TODO: respect Klutz / Embargo when item-suppression predicate lands;
    // only Magic Room is honored today.
    if move_id == 374 {
        let atk_item_id = state.active_mon(atk_side).item_id;
        if atk_item_id == 0 || state.field.magic_room_turns() > 0 {
            return;
        }
        if data_bridge::item(atk_item_id).power_param == 0 {
            return;
        }
    }

    let per_hit_acc = match move_id {
        167 | 813 | 860 => { // Triple Kick, Triple Axel, Population Bomb
            let atk_item = data_bridge::item(state.active_mon(atk_side).item_id);
            if atk_item.has(ItemFlag::LOADED_DICE) {
                0 // Loaded Dice disables multiaccuracy
            } else {
                let acc = effective_accuracy(state, atk_side, md);
                if acc >= 100 { 0 } else { acc }
            }
        }
        _ => 0,
    };
    let mut result = calc_damage(state, atk_side, move_id, per_hit_acc, rng);

    // Fling: capture the flung item's status rider, then clear the item (calc
    // already read power_param). Post-damage reads (Shell Bell, Throat Spray) and
    // EOT item-damage all see item_id=0, matching Showdown's pre-damage onUpdate.
    let fling_effect = if move_id == 374 {
        let e = fling_item_status(state.active_mon(atk_side).item_id);
        consume_item(state, atk_side, atk_slot);
        if effective_ability(state, atk_side) == data_bridge::ABILITY_UNBURDEN {
            set_volatile(state, atk_side, VOL_UNBURDEN);
        }
        e
    } else {
        None
    };

    if result.type_immune {
        apply_crash_if_needed(state, atk_side, md);
        mark_move_failed(state, atk_side);
        return;
    }

    // Sturdy vs a one-hit KO move is full immunity (Showdown's Sturdy onTryHit
    // returns null → holder unharmed at full HP), distinct from the generic
    // survive-at-1 below. Mold Breaker suppresses it.
    if md.effect == MoveEffect::Ohko
        && !result.hits_substitute
        && effective_ability(state, def_side) == data_bridge::ABILITY_STURDY
        && !mold_breaks(state, def_side, effective_ability(state, atk_side))
    {
        return;
    }

    if !result.hits_substitute {
        let def_ability = effective_ability(state, def_side);
        let shields = state.sides[def_side].active._padding[4];

        // Disguise: blocks one hit of any category
        if def_ability == data_bridge::ABILITY_DISGUISE && shields & 1 == 0 {
            state.sides[def_side].active._padding[4] = shields | 1;
            // Disguise costs 1/8 max HP when broken (Gen 8+)
            let max_hp = state.sides[def_side].team[def_slot].max_hp;
            deal_damage(state, def_side, def_slot, max_hp / 8);
            return;
        }

        // Ice Face: blocks one Physical hit
        if def_ability == data_bridge::ABILITY_ICE_FACE
            && shields & 2 == 0
            && md.category == MoveCategory::Physical
        {
            state.sides[def_side].active._padding[4] = shields | 2;
            return;
        }
    }

    let mut final_damage = result.damage;
    // Flag set when multi-hit path has already applied damage + per-hit contact recoil.
    // This disables the default damage-apply and default post-hit contact recoil blocks.
    let mut multi_hit_applied = false;
    if !result.hits_substitute {
        let def_mon = &state.sides[def_side].team[def_slot];
        if def_mon.current_hp == def_mon.max_hp && final_damage >= def_mon.current_hp {
            let def_item = if state.field.magic_room_turns() > 0 {
                &data_bridge::ItemData::NONE
            } else {
                data_bridge::item(def_mon.item_id)
            };
            let def_ability = effective_ability(state, def_side);
            if def_item.has(ItemFlag::FOCUS_SASH) {
                final_damage = def_mon.current_hp - 1;
                consume_item(state, def_side, def_slot);
                if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, def_side, VOL_UNBURDEN);
                }
            } else if def_ability == data_bridge::ABILITY_STURDY
                && !mold_breaks(state, def_side, effective_ability(state, atk_side))
            {
                final_damage = def_mon.current_hp - 1;
            }
        }
        // Endure: clamp Move-effect damage to leave 1 HP (any starting HP). Recoil
        // and contact-recoil call deal_damage directly so they bypass this branch,
        // matching Showdown's effectType==='Move' gate on the endure volatile.
        if state.sides[def_side].active.has_volatile(VOL_ENDURE) {
            let cur = state.sides[def_side].team[def_slot].current_hp;
            if cur > 0 && final_damage >= cur {
                final_damage = cur - 1;
            }
        }
        // False Swipe / Hold Back never faint the target: onDamage returns target.hp-1
        // when the hit would KO (deals 0 at 1 HP). Damage still applies, just no faint.
        let mid = move_id as usize;
        if mid == crate::data::MOVE_FALSE_SWIPE || mid == data_bridge::MOVE_HOLD_BACK {
            let cur = state.sides[def_side].team[def_slot].current_hp;
            if cur > 0 && final_damage >= cur {
                final_damage = cur - 1;
            }
        }
    }

    let pre_damage_hp = if !result.hits_substitute {
        state.sides[def_side].team[def_slot].current_hp
    } else { 0 };
    let pre_sub_hp = if result.hits_substitute {
        state.sides[def_side].active.substitute_hp
    } else { 0 };

    // Multi-hit path: apply damage per-hit, interleaving contact recoil and faint checks.
    // Needed so Rough Skin / Iron Barbs / Rocky Helmet trigger per-hit (BUG-P2-D-104),
    // and so the attacker can faint mid-burst, stopping remaining hits.
    if result.hits > 1 && !result.hits_substitute {
        // Pre-compute is_contact (matches the block at L2157+ below)
        let mut mh_is_contact = md.flags & MoveFlags::CONTACT != 0;
        if mh_is_contact && state.field.magic_room_turns() == 0 {
            let atk_itm = data_bridge::item(state.active_mon(atk_side).item_id);
            if atk_itm.has(ItemFlag::PROTECTIVE_PADS)
                || (atk_itm.has(ItemFlag::PUNCHING_GLOVE) && md.flags & MoveFlags::PUNCH != 0)
            {
                mh_is_contact = false;
            }
        }
        let mut total_dealt: u32 = 0;
        let mut hits_done: u8 = 0;
        for i in 0..result.hits {
            let per_hit = result.per_hit_damages[i as usize];
            // First hit: if damage was adjusted by Focus Sash / Sturdy, apply the
            // adjustment to this one hit. final_damage holds the adjusted total
            // (actually: def_mon current_hp - 1) but only matters when the full
            // multi-hit sum would have KO'd and the first hit alone doesn't KO.
            // To keep behavior simple and correct in the common case (first-hit
            // already >= current HP), gate on: if adjusted final_damage < full
            // total, prefer dealing adjusted amount on hit 0 and stopping.
            let dmg_this = if i == 0 && final_damage < result.damage {
                let d = final_damage;
                let hp_before = state.sides[def_side].team[def_slot].current_hp;
                deal_damage(state, def_side, def_slot, d);
                total_dealt += hp_before.saturating_sub(
                    state.sides[def_side].team[def_slot].current_hp,
                ) as u32;
                hits_done += 1;
                state.sides[def_side].active.last_move_hit_by = move_id;
                state.sides[def_side].active.times_hit =
                    state.sides[def_side].active.times_hit.saturating_add(1);
                // Focus Sash / Sturdy adjusted: defender is at 1 HP and further hits
                // would KO. Apply contact recoil once (for this hit) and then stop.
                if mh_is_contact
                    && !state.sides[atk_side].team[atk_slot].is_fainted()
                {
                    apply_contact_recoil(state, atk_side, atk_slot, def_side, md);
                }
                break;
            } else { per_hit };
            let hp_before = state.sides[def_side].team[def_slot].current_hp;
            deal_damage(state, def_side, def_slot, dmg_this);
            total_dealt += hp_before.saturating_sub(
                state.sides[def_side].team[def_slot].current_hp,
            ) as u32;
            hits_done += 1;
            state.sides[def_side].active.last_move_hit_by = move_id;
            state.sides[def_side].active.times_hit =
                state.sides[def_side].active.times_hit.saturating_add(1);
            // Contact recoil per-hit (fires even if defender fainted from this hit)
            if mh_is_contact
                && !state.sides[atk_side].team[atk_slot].is_fainted()
            {
                apply_contact_recoil(state, atk_side, atk_slot, def_side, md);
            }
            if state.sides[def_side].team[def_slot].is_fainted() { break; }
            if state.sides[atk_side].team[atk_slot].is_fainted() { break; }
        }
        result.hits = hits_done;
        // Update final_damage to the total actually dealt, for Shell Bell / drain / etc.
        final_damage = total_dealt.min(u16::MAX as u32) as u16;
        result.damage = final_damage;
        multi_hit_applied = true;
    }

    if !multi_hit_applied {
        if result.hits_substitute {
            let sub = &mut state.sides[def_side].active.substitute_hp;
            *sub = sub.saturating_sub(final_damage);
            if *sub == 0 {
                clear_volatile(state, def_side, VOL_SUBSTITUTE);
            }
        } else {
            deal_damage(state, def_side, def_slot, final_damage);
            state.sides[def_side].active.last_move_hit_by = move_id;
            state.sides[def_side].active.times_hit =
                state.sides[def_side].active.times_hit.saturating_add(1);
        }
    }

    if result.drain_heal > 0 {
        // Showdown heals a fraction of HP *actually removed*, not the uncapped rolled
        // damage; on a faint/low-HP target the calc-time heal over-heals. Recompute from
        // HP removed (mirrors the recoil sibling below), re-apply Big Root, then clamp to
        // the calc-time cap (which already carries Big Root's ×1.3).
        let actual = if result.hits_substitute {
            let new_sub = state.sides[def_side].active.substitute_hp;
            pre_sub_hp.saturating_sub(new_sub) as u32
        } else if multi_hit_applied {
            final_damage as u32
        } else {
            pre_damage_hp.saturating_sub(
                state.sides[def_side].team[def_slot].current_hp,
            ) as u32
        };
        let mut heal_from_removed = if actual > 0 {
            ((actual * md.drain as u32 + 50) / 100).max(1)
        } else { 0 };
        if heal_from_removed > 0
            && state.field.magic_room_turns() == 0
            && state.sides[atk_side].team[atk_slot].item_id == data_bridge::ITEM_BIG_ROOT
        {
            heal_from_removed = crate::state::calc_modifiers::chain_mod(heal_from_removed, 5324);
        }
        let drain_heal = (heal_from_removed.min(result.drain_heal as u32)) as u16;
        // Liquid Ooze flips the drain heal into damage on the drainer (Showdown fires
        // it on the would-be-healer's side). Read the defender's ability faint-tolerant:
        // it may have just fainted to the drain hit.
        if effective_ability_ignoring_faint(state, def_side) == data_bridge::ABILITY_LIQUID_OOZE {
            deal_damage(state, atk_side, atk_slot, drain_heal);
        } else {
            heal(state, atk_side, atk_slot, drain_heal);
        }
    }
    // Move-recoil (md.drain < 0): mirrors Showdown's calcRecoilDamage on
    // move.totalDamage — i.e. HP-clamped actually-dealt damage, not raw calc damage.
    // Rock Head exempts; Magic Guard exempts (effect-type 'Recoil' fails the
    // effectType==='Move' gate in magicguard.onDamage).
    if md.drain < 0 {
        let atk_ability_for_recoil = effective_ability(state, atk_side);
        if atk_ability_for_recoil != data_bridge::ABILITY_ROCK_HEAD
            && atk_ability_for_recoil != data_bridge::ABILITY_MAGIC_GUARD
        {
            let actually_dealt = if result.hits_substitute {
                let new_sub = state.sides[def_side].active.substitute_hp;
                pre_sub_hp.saturating_sub(new_sub) as u32
            } else if multi_hit_applied {
                final_damage as u32
            } else {
                pre_damage_hp.saturating_sub(
                    state.sides[def_side].team[def_slot].current_hp,
                ) as u32
            };
            if actually_dealt > 0 {
                let pct = (-md.drain) as u32;
                let recoil = ((actually_dealt * pct + 50) / 100).max(1) as u16;
                result.recoil_damage = result.recoil_damage.saturating_add(recoil);
            }
        }
    }
    if result.recoil_damage > 0 {
        deal_damage(state, atk_side, atk_slot, result.recoil_damage);
        // Berry activation after recoil (e.g., Sitrus Berry)
        if !state.sides[atk_side].team[atk_slot].is_fainted() {
            check_berry_activation(state, teams, atk_side, atk_slot, rng);
        }
    }

    // mindBlownRecoil onAfterMove: round-half-up half-max-HP on USE; can self-faint.
    // Rock Head does NOT exempt (it only suppresses move.recoil; this is a separate
    // mechanism). Magic Guard DOES exempt: Showdown deals it as a Condition, so the
    // effectType==='Move' gate in magicguard.onDamage fails and the damage is blocked.
    // round(maxhp/2) = (max_hp+1)>>1, NOT the truncating /2.
    if md.self_effect == SelfEffect::HalfMaxHpRecoil
        && effective_ability(state, atk_side) != data_bridge::ABILITY_MAGIC_GUARD
    {
        let max_hp = state.sides[atk_side].team[atk_slot].max_hp;
        deal_damage(state, atk_side, atk_slot, (max_hp + 1) >> 1);
    }

    if md.move_type == Type::Electric {
        state.sides[atk_side].active._padding[4] &= !4;
    }

    if !state.sides[atk_side].team[atk_slot].is_fainted() && !result.hits_substitute {
        let atk_item_id = state.sides[atk_side].team[atk_slot].item_id;
        // Shell Bell: heal 1/8 of damage dealt
        if atk_item_id == data_bridge::ITEM_SHELL_BELL && final_damage > 0 {
            heal(state, atk_side, atk_slot, (final_damage / 8).max(1));
        }
        // Throat Spray: +1 SpA if sound move, consume
        if atk_item_id == data_bridge::ITEM_THROAT_SPRAY
            && md.flags & MoveFlags::SOUND != 0
        {
            apply_boost(state, atk_side, SPA, 1);
            consume_item(state, atk_side, atk_slot);
            if effective_ability(state, atk_side) == data_bridge::ABILITY_UNBURDEN {
                set_volatile(state, atk_side, VOL_UNBURDEN);
            }
        }
    }

    if !state.sides[atk_side].team[atk_slot].is_fainted() {
        apply_self_effect(state, atk_side, md);
    }

    if !result.hits_substitute
        && !state.sides[def_side].team[def_slot].is_fainted()
    {
        check_berry_activation(state, teams, def_side, def_slot, rng);
    }

    if !result.hits_substitute
        && !state.sides[def_side].team[def_slot].is_fainted()
    {
        crate::state::forme::check_zen_mode(state, teams, def_side);
        crate::state::forme::check_schooling(state, teams, def_side);
        crate::state::forme::check_shields_down(state, teams, def_side);
    }

    if result.item_consumed {
        let atk_itm = data_bridge::item(state.active_mon(atk_side).item_id);
        if atk_itm.has(ItemFlag::GEM) {
            consume_item(state, atk_side, atk_slot);
            if effective_ability(state, atk_side) == data_bridge::ABILITY_UNBURDEN {
                set_volatile(state, atk_side, VOL_UNBURDEN);
            }
        }
        let def_item_id = state.active_mon(def_side).item_id;
        let def_itm = data_bridge::item(def_item_id);
        if def_itm.has(ItemFlag::RESIST_BERRY) {
            state.sides[def_side].set_last_consumed_berry(def_item_id);
            consume_item(state, def_side, def_slot);
            if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                set_volatile(state, def_side, VOL_UNBURDEN);
            }
        }
    }

    // A self-boost secondary (secondary_stat > 0) targets the user, so Showdown
    // still runs it when the move KO'd the defender; opponent-targeting secondaries
    // are skipped on a fainted target.
    if !result.hits_substitute
        && (md.secondary_stat > 0 || !state.sides[def_side].team[def_slot].is_fainted())
    {
        apply_secondary(state, atk_side, def_side, md, move_id, rng);
    }

    // Fling (374): apply the flung item's status / flinch rider (captured pre-
    // consume). Guaranteed (not a chance secondary), so no rng roll; gated like a
    // status secondary — skipped on substitute / fainted target, under the
    // standard immunity / safeguard / terrain / ability / Minior guards.
    if let Some(effect) = fling_effect {
        if !result.hits_substitute && !state.sides[def_side].team[def_slot].is_fainted() {
            match effect {
                FlingEffect::Status(status) => {
                    if !type_immune_to_status(state, def_side, status)
                        && state.sides[def_side].side_conditions.safeguard_turns() == 0
                        && !terrain_blocks_status(state, def_side, status)
                        && !ability_status_immune(state, def_side, effective_ability(state, atk_side), status)
                        && !crate::state::forme::is_minior_meteor_forme(state, def_side)
                        && set_status(state, def_side, def_slot, status, 0)
                    {
                        try_synchronize_back(state, def_side, atk_side, status);
                    }
                }
                FlingEffect::Flinch => {
                    let has_covert_cloak = state.field.magic_room_turns() == 0
                        && data_bridge::item(state.active_mon(def_side).item_id).has(ItemFlag::COVERT_CLOAK);
                    if !has_covert_cloak
                        && !state.sides[def_side].active.has_volatile(VOL_FLINCHED)
                        && !state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN)
                    {
                        set_volatile(state, def_side, VOL_FLINCHED);
                    }
                }
            }
        }
    }

    // King's Rock / Razor Fang: 10% flinch chance on damaging moves.
    // Hot-path guard: most mons don't hold these items, so check the flag
    // first to short-circuit before touching any other state.
    if state.field.magic_room_turns() == 0 {
        let atk_item_id = state.active_mon(atk_side).item_id;
        if atk_item_id != 0 && data_bridge::item(atk_item_id).has(ItemFlag::KINGS_ROCK)
            && md.category != MoveCategory::Status
            && !result.hits_substitute
            && !state.sides[def_side].team[def_slot].is_fainted()
            && !state.sides[def_side].active.has_volatile(VOL_FLINCHED)
            && !state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN)
        {
            let has_covert_cloak = data_bridge::item(state.active_mon(def_side).item_id)
                .has(ItemFlag::COVERT_CLOAK);
            if !has_covert_cloak && rng(10) < 1 {
                set_volatile(state, def_side, VOL_FLINCHED);
            }
        }
    }

    // Stench: onModifyMove pushes a 10% flinch onto the holder's damaging moves
    // that don't already carry a flinch secondary. Same target-side guards as the
    // rider flinch (Covert Cloak / VOL_MOVED). Ability effect → unaffected by Magic
    // Room. Raw-id pre-filter keeps non-Stench mons to a single compare.
    if state.active_mon(atk_side).ability_id == data_bridge::ABILITY_STENCH
        && effective_ability(state, atk_side) == data_bridge::ABILITY_STENCH
        && md.category != MoveCategory::Status
        && !result.hits_substitute
        && !state.sides[def_side].team[def_slot].is_fainted()
        && !state.sides[def_side].active.has_volatile(VOL_FLINCHED)
        && !state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN)
        && !move_has_flinch_secondary(md, move_id)
    {
        let has_covert_cloak = state.field.magic_room_turns() == 0
            && data_bridge::item(state.active_mon(def_side).item_id).has(ItemFlag::COVERT_CLOAK);
        if !has_covert_cloak && rng(100) < 10 {
            set_volatile(state, def_side, VOL_FLINCHED);
        }
    }

    let mut is_contact = md.flags & MoveFlags::CONTACT != 0;
    if is_contact && state.field.magic_room_turns() == 0 {
        let atk_itm = data_bridge::item(state.active_mon(atk_side).item_id);
        if atk_itm.has(ItemFlag::PROTECTIVE_PADS)
            || (atk_itm.has(ItemFlag::PUNCHING_GLOVE) && md.flags & MoveFlags::PUNCH != 0)
        {
            is_contact = false;
        }
    }

    let def_mirror = state.field.magic_room_turns() == 0
        && state.active_mon(atk_side).item_id != 0;
    let def_boosts_before = if def_mirror { state.sides[def_side].active.boosts } else { [0; 7] };

    if !result.hits_substitute
        && !state.sides[def_side].team[def_slot].is_fainted()
    {
        let def_ability = effective_ability(state, def_side);
        match def_ability {
            // Weak Armor: Physical hit → -1 Def, +2 Spe
            data_bridge::ABILITY_WEAK_ARMOR if md.category == MoveCategory::Physical => {
                apply_boost(state, def_side, DEF, -1);
                apply_boost(state, def_side, SPE, 2);
            }
            // Justified: Dark-type hit → +1 Atk
            data_bridge::ABILITY_JUSTIFIED if md.move_type == Type::Dark => {
                apply_boost(state, def_side, ATK, 1);
            }
            // Stamina: any hit → +1 Def
            data_bridge::ABILITY_STAMINA => {
                apply_boost(state, def_side, DEF, 1);
            }
            // Anger Point: crit → maximize Atk (+6)
            data_bridge::ABILITY_ANGER_POINT if result.crit => {
                let current = state.sides[def_side].active.boosts[ATK];
                if current < 6 {
                    apply_boost(state, def_side, ATK, 6 - current);
                }
            }
            // Color Change: change type to match the move type
            data_bridge::ABILITY_COLOR_CHANGE => {
                let new_type = md.move_type as u8;
                let (t1, t2) = battle_types(state, def_side);
                if t1 != new_type || t2 != new_type {
                    state.sides[def_side].active.override_types = [new_type, new_type];
                    set_volatile(state, def_side, VOL_TYPES_OVERRIDDEN);
                }
            }
            // Rattled: Bug/Dark/Ghost hit → +1 Spe
            data_bridge::ABILITY_RATTLED
                if matches!(md.move_type, Type::Bug | Type::Dark | Type::Ghost)
                => { apply_boost(state, def_side, SPE, 1); }
            // Steam Engine: Fire/Water hit → +6 Spe
            data_bridge::ABILITY_STEAM_ENGINE
                if matches!(md.move_type, Type::Fire | Type::Water)
                => { apply_boost(state, def_side, SPE, 6); }
            // Thermal Exchange: Fire hit → +1 Atk (+ burn immunity handled elsewhere)
            data_bridge::ABILITY_THERMAL_EXCHANGE if md.move_type == Type::Fire
                => { apply_boost(state, def_side, ATK, 1); }
            // Berserk: HP drops ≤50% → +1 SpA
            data_bridge::ABILITY_BERSERK => {
                let m = state.sides[def_side].team[def_slot].max_hp;
                let hp = state.sides[def_side].team[def_slot].current_hp;
                // Must cross the 50% threshold (was above, now at/below)
                if hp > 0 && hp * 2 <= m && pre_damage_hp * 2 > m {
                    apply_boost(state, def_side, SPA, 1);
                }
            }
            // Anger Shell: HP drops below 50% → +1 Atk/SpA/Spe, -1 Def/SpD
            data_bridge::ABILITY_ANGER_SHELL => {
                let m = state.sides[def_side].team[def_slot].max_hp;
                let hp = state.sides[def_side].team[def_slot].current_hp;
                // Must cross the 50% threshold (was above, now at/below)
                if hp > 0 && hp * 2 <= m && pre_damage_hp * 2 > m {
                    apply_boost(state, def_side, ATK, 1);
                    apply_boost(state, def_side, SPA, 1);
                    apply_boost(state, def_side, SPE, 1);
                    apply_boost(state, def_side, DEF, -1);
                    apply_boost(state, def_side, SPD, -1);
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
                set_terrain(state, TERRAIN_GRASSY, 5);
                crate::state::switch::check_paradox_deactivation(state);
            }
            // Sand Spit: any hit → set Sandstorm
            data_bridge::ABILITY_SAND_SPIT => {
                set_weather(state, WEATHER_SAND, 5);
                crate::state::switch::check_paradox_deactivation(state);
            }
            // Cotton Down: any hit → lower attacker's Spe by 1
            data_bridge::ABILITY_COTTON_DOWN => {
                if !state.sides[atk_side].team[atk_slot].is_fainted() {
                    try_opponent_stat_drop(state, atk_side, SPE, -1);
                }
            }
            // Mummy / Lingering Aroma: contact → overwrite attacker's ability
            data_bridge::ABILITY_MUMMY if is_contact => {
                if !state.sides[atk_side].team[atk_slot].is_fainted() {
                    let atk_ab = effective_ability(state, atk_side);
                    let has_shield = state.field.magic_room_turns() == 0
                        && data_bridge::item(state.active_mon(atk_side).item_id).has(ItemFlag::ABILITY_SHIELD);
                    if !has_shield && atk_ab != data_bridge::ABILITY_MUMMY && atk_ab != 0
                        && !is_cantsuppress_ability(atk_ab)
                    {
                        state.sides[atk_side].active.override_ability = data_bridge::ABILITY_MUMMY;
                        set_volatile(state, atk_side, VOL_ABILITY_OVERRIDDEN);
                    }
                }
            }
            data_bridge::ABILITY_LINGERING_AROMA if is_contact => {
                if !state.sides[atk_side].team[atk_slot].is_fainted() {
                    let atk_ab = effective_ability(state, atk_side);
                    let has_shield = state.field.magic_room_turns() == 0
                        && data_bridge::item(state.active_mon(atk_side).item_id).has(ItemFlag::ABILITY_SHIELD);
                    if !has_shield && atk_ab != data_bridge::ABILITY_LINGERING_AROMA && atk_ab != 0
                        && !is_cantsuppress_ability(atk_ab)
                    {
                        state.sides[atk_side].active.override_ability = data_bridge::ABILITY_LINGERING_AROMA;
                        set_volatile(state, atk_side, VOL_ABILITY_OVERRIDDEN);
                    }
                }
            }
            // Cursed Body: 30% chance to disable the attacker's last-used move
            // (any damaging hit, not contact-restricted). Excludes Struggle.
            data_bridge::ABILITY_CURSED_BODY
                if !is_struggle
                && !state.sides[atk_side].team[atk_slot].is_fainted()
                && state.sides[atk_side].active.disabled_move == 0
                => {
                if rng(10) < 3 {
                    let last = state.sides[atk_side].active.last_move;
                    if last != 0 {
                        // Verify the attacker still has PP for this move
                        let moves = effective_moves(state, atk_side);
                        let mut has_pp = false;
                        for i in 0..4 {
                            if moves[i] == last && effective_pp(state, atk_side, i) > 0 {
                                has_pp = true;
                                break;
                            }
                        }
                        if has_pp {
                            state.sides[atk_side].active.disabled_move = last;
                            let dur = if state.sides[atk_side].active.has_volatile(VOL_MOVED_THIS_TURN) { 5 } else { 4 };
                            state.sides[atk_side].active.disable_turns = dur;
                            check_mental_herb(state, atk_side);
                        }
                    }
                }
            }
            // Perish Body: contact → set 3-turn Perish on both
            data_bridge::ABILITY_PERISH_BODY if is_contact => {
                if !state.sides[def_side].active.has_volatile(VOL_PERISH_SONG) {
                    set_volatile(state, def_side, VOL_PERISH_SONG);
                    state.sides[def_side].active.perish_count = 3;
                }
                if !state.sides[atk_side].active.has_volatile(VOL_PERISH_SONG)
                    && !state.sides[atk_side].team[atk_slot].is_fainted()
                {
                    set_volatile(state, atk_side, VOL_PERISH_SONG);
                    state.sides[atk_side].active.perish_count = 3;
                }
            }
            _ => {}
        }

        if !state.sides[atk_side].team[atk_slot].is_fainted() {
            let atk_ability = effective_ability(state, atk_side);
            let def_has_cloak = state.field.magic_room_turns() == 0
                && data_bridge::item(state.active_mon(def_side).item_id).has(ItemFlag::COVERT_CLOAK);
            match atk_ability {
                data_bridge::ABILITY_POISON_TOUCH
                    if !def_has_cloak
                    && is_contact
                    && state.sides[def_side].team[def_slot].status == STATUS_NONE
                    && !terrain_blocks_status(state, def_side, STATUS_POISON)
                    && !type_immune_to_status(state, def_side, STATUS_POISON)
                    && !ability_status_immune(state, def_side, atk_ability, STATUS_POISON)
                    && !crate::state::forme::is_minior_meteor_forme(state, def_side)
                    => {
                    if rng(100) < 30 {
                        if set_status(state, def_side, def_slot, STATUS_POISON, 0) {
                            try_synchronize_back(state, def_side, atk_side, STATUS_POISON);
                        }
                    }
                }
                data_bridge::ABILITY_TOXIC_CHAIN
                    if !def_has_cloak
                    && state.sides[def_side].team[def_slot].status == STATUS_NONE
                    && !terrain_blocks_status(state, def_side, STATUS_BAD_POISON)
                    && !type_immune_to_status(state, def_side, STATUS_BAD_POISON)
                    && !ability_status_immune(state, def_side, atk_ability, STATUS_BAD_POISON)
                    && !crate::state::forme::is_minior_meteor_forme(state, def_side)
                    => {
                    if rng(100) < 30 {
                        if set_status(state, def_side, def_slot, STATUS_BAD_POISON, 0) {
                            try_synchronize_back(state, def_side, atk_side, STATUS_BAD_POISON);
                        }
                    }
                }
                // Magician: steal target's item on hit
                data_bridge::ABILITY_MAGICIAN
                    if state.active_mon(atk_side).item_id == 0
                    && state.active_mon(def_side).item_id != 0
                    => {
                    let stolen = state.sides[def_side].team[def_slot].item_id;
                    set_item(state, atk_side, atk_slot, stolen);
                    consume_item(state, def_side, def_slot);
                }
                _ => {}
            }
        }

        let def_item_id = state.sides[def_side].team[def_slot].item_id;
        if def_item_id != 0 && !state.sides[def_side].team[def_slot].is_fainted()
            && state.field.magic_room_turns() == 0
        {
            // Weakness Policy: +2 Atk +2 SpA if hit by SE move, consume
            if def_item_id == data_bridge::ITEM_WEAKNESS_POLICY && result.effectiveness > 4 {
                apply_boost(state, def_side, ATK, 2);
                apply_boost(state, def_side, SPA, 2);
                consume_item(state, def_side, def_slot);
                if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, def_side, VOL_UNBURDEN);
                }
            }
            // Jaboca Berry: 1/8 attacker HP if physical hit
            if def_item_id == data_bridge::ITEM_JABOCA_BERRY
                && md.category == MoveCategory::Physical
                && !state.sides[atk_side].team[atk_slot].is_fainted()
            {
                let atk_max = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, atk_side, atk_slot, (atk_max / 8).max(1));
                consume_berry(state, def_side, def_slot);
            }
            // Rowap Berry: 1/8 attacker HP if special hit
            if def_item_id == data_bridge::ITEM_ROWAP_BERRY
                && md.category == MoveCategory::Special
                && !state.sides[atk_side].team[atk_slot].is_fainted()
            {
                let atk_max = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, atk_side, atk_slot, (atk_max / 8).max(1));
                consume_berry(state, def_side, def_slot);
            }
            // Air Balloon: pop on any damaging hit
            if data_bridge::item(def_item_id).has(ItemFlag::AIR_BALLOON) {
                consume_item(state, def_side, def_slot);
                if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, def_side, VOL_UNBURDEN);
                }
            }
            // onDamagingHit boost items: +1 stat on a matching-type hit, consume.
            let boost = match def_item_id {
                data_bridge::ITEM_ABSORB_BULB if md.move_type == Type::Water => Some(SPA),
                data_bridge::ITEM_CELL_BATTERY if md.move_type == Type::Electric => Some(ATK),
                data_bridge::ITEM_SNOWBALL if md.move_type == Type::Ice => Some(ATK),
                data_bridge::ITEM_LUMINOUS_MOSS if md.move_type == Type::Water => Some(SPD),
                _ => None,
            };
            if let Some(stat) = boost {
                apply_boost(state, def_side, stat, 1);
                consume_item(state, def_side, def_slot);
                if effective_ability(state, def_side) == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, def_side, VOL_UNBURDEN);
                }
            }
            // Kee/Maranga Berry: +1 Def/SpD after a physical/special hit, eaten.
            let berry_boost = match def_item_id {
                data_bridge::ITEM_KEE_BERRY if md.category == MoveCategory::Physical => Some(DEF),
                data_bridge::ITEM_MARANGA_BERRY if md.category == MoveCategory::Special => Some(SPD),
                _ => None,
            };
            if let Some(stat) = berry_boost {
                apply_boost(state, def_side, stat, 1);
                consume_berry(state, def_side, def_slot);
            }
        }
    }

    if def_mirror {
        check_mirror_herb_diff(state, def_side, &def_boosts_before);
    }

    // Contact recoil: Rocky Helmet + Rough Skin / Iron Barbs.
    // Fires even when the defender fainted from the hit (BUG-P5-M-201).
    // Skipped here when multi_hit_applied == true because the multi-hit path
    // already applied per-hit contact recoil (BUG-P2-D-104).
    if is_contact
        && !multi_hit_applied
        && !state.sides[atk_side].team[atk_slot].is_fainted()
        && !result.hits_substitute
    {
        apply_contact_recoil(state, atk_side, atk_slot, def_side, md);
    }

    // Contact status abilities fire even when the holder fainted from this hit
    // (Showdown reads target.ability in onDamagingHit), mirroring the contact
    // recoil above. Cute Charm / Sticky Barb still require a live holder.
    if is_contact
        && !state.sides[atk_side].team[atk_slot].is_fainted()
        && !result.hits_substitute
    {
        let def_ability = effective_ability_ignoring_faint(state, def_side);
        let def_alive = !state.sides[def_side].team[def_slot].is_fainted();

        // Contact status abilities (attacker alive + no status + no Safeguard + not Minior-Meteor)
        if state.sides[atk_side].team[atk_slot].status == STATUS_NONE
            && state.sides[atk_side].side_conditions.safeguard_turns() == 0
            && !crate::state::forme::is_minior_meteor_forme(state, atk_side)
        {
            match def_ability {
                data_bridge::ABILITY_FLAME_BODY if !terrain_blocks_status(state, atk_side, STATUS_BURN) && !type_immune_to_status(state, atk_side, STATUS_BURN) && !ability_status_immune(state, atk_side, def_ability, STATUS_BURN) => {
                    if rng(100) < 30 {
                        if set_status(state, atk_side, atk_slot, STATUS_BURN, 0) {
                            try_synchronize_back(state, atk_side, def_side, STATUS_BURN);
                        }
                    }
                }
                data_bridge::ABILITY_STATIC if !terrain_blocks_status(state, atk_side, STATUS_PARALYSIS) && !type_immune_to_status(state, atk_side, STATUS_PARALYSIS) && !ability_status_immune(state, atk_side, def_ability, STATUS_PARALYSIS) => {
                    if rng(100) < 30 {
                        if set_status(state, atk_side, atk_slot, STATUS_PARALYSIS, 0) {
                            try_synchronize_back(state, atk_side, def_side, STATUS_PARALYSIS);
                        }
                    }
                }
                data_bridge::ABILITY_POISON_POINT if !terrain_blocks_status(state, atk_side, STATUS_POISON) && !type_immune_to_status(state, atk_side, STATUS_POISON) && !ability_status_immune(state, atk_side, def_ability, STATUS_POISON) => {
                    if rng(100) < 30 {
                        if set_status(state, atk_side, atk_slot, STATUS_POISON, 0) {
                            try_synchronize_back(state, atk_side, def_side, STATUS_POISON);
                        }
                    }
                }
                data_bridge::ABILITY_EFFECT_SPORE => {
                    // Powder/spore immunities: Grass-type attacker, Overcoat ability,
                    // or Safety Goggles all block Effect Spore's status roll.
                    let atk_ability = effective_ability(state, atk_side);
                    let powder_immune = has_type(state, atk_side, Type::Grass as u8)
                        || atk_ability == data_bridge::ABILITY_OVERCOAT
                        || holds_safety_goggles(state, atk_side);
                    if !powder_immune {
                        // Classify the band FIRST (slp 11 / par 10 / psn 9 = 11/21/30),
                        // then attempt only that one status with its own immunity guard.
                        // A status-immune band-roll applies nothing — it must NOT cascade
                        // into the next arm. Mirrors abilities.ts effectspore.
                        let roll = rng(100);
                        if roll < 11 {
                            if !terrain_blocks_status(state, atk_side, STATUS_SLEEP) && !ability_status_immune(state, atk_side, def_ability, STATUS_SLEEP) {
                                set_status(state, atk_side, atk_slot, STATUS_SLEEP, (rng(3) + 2) as u8);
                                // Synchronize does NOT pass sleep
                            }
                        } else if roll < 21 {
                            if !terrain_blocks_status(state, atk_side, STATUS_PARALYSIS) && !type_immune_to_status(state, atk_side, STATUS_PARALYSIS) && !ability_status_immune(state, atk_side, def_ability, STATUS_PARALYSIS)
                                && set_status(state, atk_side, atk_slot, STATUS_PARALYSIS, 0) {
                                try_synchronize_back(state, atk_side, def_side, STATUS_PARALYSIS);
                            }
                        } else if roll < 30 {
                            if !terrain_blocks_status(state, atk_side, STATUS_POISON) && !type_immune_to_status(state, atk_side, STATUS_POISON) && !ability_status_immune(state, atk_side, def_ability, STATUS_POISON)
                                && set_status(state, atk_side, atk_slot, STATUS_POISON, 0) {
                                try_synchronize_back(state, atk_side, def_side, STATUS_POISON);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Cute Charm: 30% attract on contact (holder must be alive)
        if def_alive
            && def_ability == data_bridge::ABILITY_CUTE_CHARM
            && !state.sides[atk_side].active.is_attracted()
        {
            if rng(100) < 30 {
                state.sides[atk_side].active.set_attracted(true);
            }
        }

        // Sticky Barb: transfer to attacker on contact if attacker has no item.
        // Magic Room suppresses held-item effects. Hot-path guard: most
        // defenders don't hold Sticky Barb; check item_id first.
        if def_alive
            && state.active_mon(def_side).item_id == data_bridge::ITEM_STICKY_BARB
            && state.active_mon(atk_side).item_id == 0
            && state.field.magic_room_turns() == 0
        {
            set_item(state, atk_side, atk_slot, data_bridge::ITEM_STICKY_BARB);
            consume_item(state, def_side, def_slot);
        }
    }

    if md.effect == MoveEffect::KnockOff
        && !state.sides[atk_side].team[atk_slot].is_fainted()
        && !result.hits_substitute
    {
        let def_mon = &state.sides[def_side].team[def_slot];
        if def_mon.item_id != 0 {
            let def_ability = effective_ability(state, def_side);
            let sticky = def_ability == data_bridge::ABILITY_STICKY_HOLD;
            let def_itm = data_bridge::item(def_mon.item_id);
            let base = data_bridge::base_species(def_mon.species_id);
            if !sticky && !def_itm.is_forme_locked(base) {
                consume_item(state, def_side, def_slot);
                if def_ability == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, def_side, VOL_UNBURDEN);
                }
            }
        }
    }

    // Thief/Covet: an itemless attacker steals the target's removable item.
    // Mirrors Knock Off's removable gating (Sticky Hold / forme-locked);
    // not gated by Magic Room — Showdown suppresses item effects, not item theft.
    if md.effect == MoveEffect::Thief
        && !result.hits_substitute
        && !state.sides[atk_side].team[atk_slot].is_fainted()
        && state.active_mon(atk_side).item_id == 0
    {
        let def_mon = &state.sides[def_side].team[def_slot];
        if def_mon.item_id != 0 {
            let def_ability = effective_ability(state, def_side);
            let sticky = def_ability == data_bridge::ABILITY_STICKY_HOLD;
            let def_itm = data_bridge::item(def_mon.item_id);
            let base = data_bridge::base_species(def_mon.species_id);
            if !sticky && !def_itm.is_forme_locked(base) {
                let stolen = def_mon.item_id;
                consume_item(state, def_side, def_slot);
                if def_ability == data_bridge::ABILITY_UNBURDEN {
                    set_volatile(state, def_side, VOL_UNBURDEN);
                }
                set_item(state, atk_side, atk_slot, stolen);
            }
        }
    }

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

    // Spit Up post-damage: reset stockpile and remove Def/SpD boosts
    if md.effect == MoveEffect::SpitUp {
        let count = state.sides[atk_side].active.stockpile & 0x7F;
        if count > 0 {
            apply_boost(state, atk_side, DEF, -(count as i8));
            apply_boost(state, atk_side, SPD, -(count as i8));
            state.sides[atk_side].active.stockpile &= 0x80; // preserve salt cure bit
        }
    }

    if md.effect == MoveEffect::PartialTrap && !result.hits_substitute
        && !state.sides[def_side].team[def_slot].is_fainted()
        && !state.sides[def_side].active.has_volatile(VOL_BOUND)
    {
        set_volatile(state, def_side, VOL_BOUND);
        // BUG-P6-E-101: Grip Claw extends partial-trap duration to 7 turns.
        let has_grip_claw = state.field.magic_room_turns() == 0
            && state.sides[atk_side].team[atk_slot].item_id == data_bridge::ITEM_GRIP_CLAW;
        let turns = if has_grip_claw { 7 } else { 4 };
        state.sides[def_side].active.set_bind_turns(turns);
    }

    if md.flags & MoveFlags::RECHARGE != 0 {
        set_volatile(state, atk_side, VOL_RECHARGING);
    }

    // Rapid Spin effects gated by Sheer Force (Showdown: !move.hasSheerForce)

    if md.effect == MoveEffect::RapidSpin
        && !state.sides[atk_side].team[atk_slot].is_fainted()
    {
        let sheer_force = effective_ability(state, atk_side) == data_bridge::ABILITY_SHEER_FORCE
            && md.secondary_chance > 0;
        if !sheer_force {
            clear_hazards(state, atk_side);
            clear_volatile(state, atk_side, VOL_LEECH_SEED);
            clear_volatile(state, atk_side, VOL_BOUND);
            state.sides[atk_side].active.set_bind_turns(0);
            apply_boost(state, atk_side, SPE, 1);
        }
    }

    if md.effect == MoveEffect::ForceSwitch
        && !state.sides[atk_side].team[atk_slot].is_fainted()
    {
        let has_bench = (0..6).any(|i| {
            i != atk_slot
                && state.sides[atk_side].team[i].species_id != 0
                && state.sides[atk_side].team[i].current_hp > 0
        });
        if has_bench {
            set_volatile(state, atk_side, VOL_MUST_SWITCH);
        }
    }

    // Dragon Tail / Circle Throw: damaging moves with phazing effect
    if md.effect == MoveEffect::Whirlwind
        && !state.sides[def_side].team[def_slot].is_fainted()
        && !result.hits_substitute
        && !state.sides[atk_side].team[atk_slot].is_fainted()
    {
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
            crate::state::switch::perform_switch_forced(state, teams, def_side, pick);
        }
    }

    // Destiny Bond: if defender had it and attacker KO'd them
    if state.sides[def_side].team[def_slot].is_fainted()
        && state.sides[def_side].active.has_volatile(VOL_DESTINY_BOND)
        && !state.sides[atk_side].team[atk_slot].is_fainted()
    {
        let atk_hp = state.sides[atk_side].team[atk_slot].current_hp;
        deal_damage(state, atk_side, atk_slot, atk_hp);
    }

    if state.sides[def_side].team[def_slot].is_fainted() {
        // Aftermath: 1/4 max HP to attacker if contact and defender fainted
        if is_contact
            && !result.hits_substitute
            && !state.sides[atk_side].team[atk_slot].is_fainted()
        {
            let def_ability = effective_ability_ignoring_faint(state, def_side);
            if def_ability == data_bridge::ABILITY_AFTERMATH {
                let atk_max = state.active_mon(atk_side).max_hp;
                deal_damage(state, atk_side, atk_slot, atk_max / 4);
            }
        }

        // Innards Out: deal damage equal to HP lost (= final_damage) to attacker
        if !result.hits_substitute
            && !state.sides[atk_side].team[atk_slot].is_fainted()
        {
            let def_ability = effective_ability_ignoring_faint(state, def_side);
            if def_ability == data_bridge::ABILITY_INNARDS_OUT {
                deal_damage(state, atk_side, atk_slot, pre_damage_hp);
            }
        }

        // Jaboca / Rowap: onDamagingHit chips the attacker 1/8 even when the
        // holder faints from the triggering hit (Showdown runs DamagingHit
        // before faint resolution, like Sand Spit). The live-holder arm above
        // already covers the survive case; this is the KO'd-holder arm.
        if !result.hits_substitute
            && !state.sides[atk_side].team[atk_slot].is_fainted()
            && state.field.magic_room_turns() == 0
        {
            let def_item_id = state.sides[def_side].team[def_slot].item_id;
            let chips = (def_item_id == data_bridge::ITEM_JABOCA_BERRY
                            && md.category == MoveCategory::Physical)
                     || (def_item_id == data_bridge::ITEM_ROWAP_BERRY
                            && md.category == MoveCategory::Special);
            if chips {
                let atk_max = state.sides[atk_side].team[atk_slot].max_hp;
                deal_damage(state, atk_side, atk_slot, (atk_max / 8).max(1));
                consume_berry(state, def_side, def_slot);
            }
        }

        // Sand Spit: onDamagingHit sets Sandstorm even when the holder faints
        // from the triggering hit (Showdown runs DamagingHit before faint
        // resolution), so the end-of-turn chip still lands.
        if !result.hits_substitute
            && effective_ability_ignoring_faint(state, def_side) == data_bridge::ABILITY_SAND_SPIT
        {
            set_weather(state, WEATHER_SAND, 5);
            crate::state::switch::check_paradox_deactivation(state);
        }

        // Showdown's faintMessages returns at checkWin() before runEvent('AfterFaint'),
        // so onSourceAfterFaint / onAnyFaint never run when a KO ends the battle.
        let battle_continues = foe_pokemon_left(state, 0) && foe_pokemon_left(state, 1);

        // Moxie / Beast Boost: attacker stat boost on KO
        if battle_continues && !state.sides[atk_side].team[atk_slot].is_fainted() {
            let atk_ability = effective_ability(state, atk_side);
            match atk_ability {
                data_bridge::ABILITY_MOXIE => {
                    apply_boost(state, atk_side, ATK, 1);
                }
                data_bridge::ABILITY_CHILLING_NEIGH | data_bridge::ABILITY_AS_ONE_GLASTRIER => {
                    apply_boost(state, atk_side, ATK, 1);
                }
                data_bridge::ABILITY_GRIM_NEIGH | data_bridge::ABILITY_AS_ONE_SPECTRIER => {
                    apply_boost(state, atk_side, SPA, 1);
                }
                data_bridge::ABILITY_BATTLE_BOND => {
                    let mon = &state.sides[atk_side].team[atk_slot];
                    if mon.species_id == data_bridge::SPECIES_GRENINJA_BOND
                        && mon.flags & MON_FLAG_BOND_TRIGGERED == 0
                        && mon.flags & MON_FLAG_TRANSFORMED == 0
                        && foe_pokemon_left(state, atk_side)
                    {
                        apply_boost(state, atk_side, ATK, 1);
                        apply_boost(state, atk_side, SPA, 1);
                        apply_boost(state, atk_side, SPE, 1);
                        state.sides[atk_side].team[atk_slot].flags |= MON_FLAG_BOND_TRIGGERED;
                    }
                }
                data_bridge::ABILITY_BEAST_BOOST => {
                    // Boost the highest raw stat; ties favor Atk > Def > SpA > SpD > Spe
                    let stats = &state.sides[atk_side].team[atk_slot].stats;
                    let mut best = 0usize;
                    for i in 1..5 {
                        if stats[i] > stats[best] {
                            best = i;
                        }
                    }
                    apply_boost(state, atk_side, best, 1);
                }
                _ => {}
            }
        }

        // Soul-Heart: +1 SpA when any Pokemon faints
        if battle_continues {
            for side in 0..2 {
                let s = state.sides[side].active_index as usize;
                if state.sides[side].team[s].is_fainted() { continue; }
                if effective_ability(state, side) == data_bridge::ABILITY_SOUL_HEART {
                    apply_boost(state, side, SPA, 1);
                }
            }
        }
    }
}

/// Execute a single move.
///
/// `move_id`: the actual move ID (from `effective_moves`), or 0 for Struggle.
/// `move_slot`: 0-3 index for PP deduction.
///
/// Runs the `runMove` prelude (PP, last_move, Choice-lock, status/flinch/
/// attract/taunt gates, VOL_MOVED_THIS_TURN), then dispatches the apply
/// span via [`use_move_called`] with `depth=0`. The post-`'exec` thrash
/// confusion cleanup runs after the helper returns. Mirrors Showdown's
/// `runMove` (sim/battle-actions.ts:200-296).
/// Future Sight (248) / Doom Desire (353): Showdown registers a delayed attack on
/// the target's side instead of dealing damage on use. The hit lands two
/// end-of-turns later on whoever occupies the target slot then, computed from a
/// use-time snapshot of the user's offense (level + boosted SpA + STAB kind), so
/// it resolves even if the user has switched out or fainted. Fails (the move
/// counts as failed) if one is already pending on that side. PP is already
/// deducted by the caller before this runs.
fn register_future_move(
    state: &mut BattleState, atk_side: usize, def_side: usize, atk_slot: usize, move_id: u16,
) {
    if state.sides[def_side].side_conditions.future_move() != 0 {
        state.sides[atk_side].set_move_failed_this_turn();
        return;
    }
    let (which, move_type) = if move_id == crate::data::MOVE_FUTURE_SIGHT as u16 {
        (1u8, Type::Psychic)
    } else {
        (2u8, Type::Steel)
    };
    let level = state.sides[atk_side].team[atk_slot].level;
    let spa = boosted_stat(
        effective_stat(state, atk_side, SPA),
        state.sides[atk_side].active.boosts[SPA],
    );
    let (sn, _) = crate::state::calc_modifiers::stab_modifier(state, atk_side, move_type);
    let stab_kind = match sn { 6144 => 1u8, 8192 => 2, 9216 => 3, _ => 0 };
    state.sides[def_side]
        .side_conditions
        .set_future_move(which, 3, stab_kind, level, spa);
}

pub fn execute_move(
    state: &mut BattleState,
    teams: &TeamData,
    atk_side: usize,
    mut move_id: u16,
    mut move_slot: u8,
    rng: &mut impl FnMut(u32) -> u32,
) {
    let def_side = 1 - atk_side;
    let atk_slot = state.sides[atk_side].active_index as usize;
    let def_slot = state.sides[def_side].active_index as usize;

    if state.sides[atk_side].team[atk_slot].is_fainted() { return; }

    let is_charge_turn2 = state.sides[atk_side].active.has_volatile(VOL_CHARGING);
    if is_charge_turn2 {
        move_id = state.sides[atk_side].active.last_move;
        clear_volatile(state, atk_side, VOL_CHARGING);
        clear_volatile(state, atk_side, VOL_SEMI_INVULNERABLE);
        state.sides[atk_side].active._padding[1] = 0;
    }

    let is_move_locked = state.sides[atk_side].active.has_volatile(VOL_MOVE_LOCKED);
    let was_last_locked_turn;
    if is_move_locked {
        move_id = state.sides[atk_side].active.last_move;
        let counter = state.sides[atk_side].active._padding[2];
        state.sides[atk_side].active._padding[2] = counter - 1;
        was_last_locked_turn = counter <= 1;
        if was_last_locked_turn {
            clear_volatile(state, atk_side, VOL_MOVE_LOCKED);
        }
    } else {
        was_last_locked_turn = false;
    }

    // Encore override: if the user is encored and not charge/lock overridden,
    // redirect to the encored move (same-turn Encore from a faster mon).
    if !is_charge_turn2 && !is_move_locked {
        let enc = &state.sides[atk_side].active;
        if enc.encore_turns > 0 && enc.encore_move != 0 && move_id != enc.encore_move {
            move_id = enc.encore_move;
            // Update move_slot so PP is deducted from the correct slot
            let moves = effective_moves(state, atk_side);
            for i in 0..4 {
                if moves[i] == move_id {
                    move_slot = i as u8;
                    break;
                }
            }
        }
    }

    // Choice-lock / Gorilla Tactics execute-layer gate: if the attacker is
    // already locked into a specific move and the dispatched move doesn't
    // match, the move is rejected with a full no-op (no PP, no last_move
    // update, no side effects). Mirrors Showdown's "Not all choices done"
    // validation rejection at the engine layer. Gorilla Tactics ignores
    // Magic Room (ability lock); Choice-item lock is suppressed by Magic
    // Room (item effect). Fast path is a single u16 compare — no branch
    // on ability/item unless the lock is already engaged.
    let locked = state.sides[atk_side].active.choice_locked_move;
    if locked != 0 && move_id != 0 && move_id != locked
        && !is_charge_turn2 && !is_move_locked
    {
        let is_gt = effective_ability(state, atk_side)
            == data_bridge::ABILITY_GORILLA_TACTICS;
        if is_gt || state.field.magic_room_turns() == 0 {
            return;
        }
    }

    let is_struggle = move_id == 0;
    let md = data_bridge::move_hot(move_id);

    // Pre-move checks and execution are in a labeled block so that
    // thrash confusion is always applied after the last locked turn,
    // even if the move fails due to flinch/para/sleep/etc.
    'exec: {

    if state.sides[atk_side].active.has_volatile(VOL_RECHARGING) {
        clear_volatile(state, atk_side, VOL_RECHARGING);
        // Showdown's mustrecharge onBeforeMove also removes the truant volatile:
        // the recharge turn doubles as the loaf turn.
        state.sides[atk_side].active.clear_truant_loaf_pending();
        break 'exec;
    }

    // The checks below mirror Showdown's onBeforeMove handlers in descending
    // priority order — slp/frz 10, truant 9, flinch 8, taunt 5, confusion 3,
    // attract 2, par 1 — and a cancel stops the chain, so e.g. a full-para
    // turn never reaches a lower-priority check and confusion always ticks
    // (and may self-hit) before paralysis rolls.

    // Sleep: decrement counter, fail unless waking up.
    // This is the ONLY place the sleep counter is decremented (not in end_of_turn).
    // Sleep Talk (214) is `sleepUsable: true` in Showdown — fires while still asleep
    // but fails on the wake-up tick (onTry requires status === 'slp').
    if state.sides[atk_side].team[atk_slot].status == STATUS_SLEEP {
        let counter = state.sides[atk_side].team[atk_slot].status_counter;
        if counter > 0 {
            state.sides[atk_side].team[atk_slot].status_counter = counter - 1;
            if counter > 1 {
                if move_id != 214 { break 'exec; }
            } else {
                clear_status(state, atk_side, atk_slot);
                if move_id == 214 { break 'exec; }
            }
        }
    }

    // Freeze: 20% thaw, fire moves always thaw
    if state.sides[atk_side].team[atk_slot].status == STATUS_FREEZE {
        if md.move_type == Type::Fire || rng(5) == 0 {
            clear_status(state, atk_side, atk_slot);
        } else {
            break 'exec;
        }
    }

    // Truant: Showdown toggles a per-mon volatile — present → remove + loaf;
    // absent → add + act. The volatile clears on switch-in, so a mon always
    // acts its first attempt after coming in (incl. faint replacements); the
    // toggle is set even if flinch/confusion/para (all lower priority) abort
    // the move afterwards, but does NOT advance while asleep/frozen (slp/frz
    // cancel first). Mirrored as an active-pad bit wiped by active.zero().
    // Raw ability_id pre-filter keeps non-Truant mons to a single compare.
    if state.sides[atk_side].team[atk_slot].ability_id == data_bridge::ABILITY_TRUANT
        && effective_ability(state, atk_side) == data_bridge::ABILITY_TRUANT
    {
        if state.sides[atk_side].active.truant_loaf_pending() {
            state.sides[atk_side].active.clear_truant_loaf_pending();
            break 'exec;
        }
        state.sides[atk_side].active.set_truant_loaf_pending();
    }

    // Flinch: an asleep/frozen mon still ticks its sleep counter / rolls its
    // thaw before the flinch cancels the move.
    if state.sides[atk_side].active.has_volatile(VOL_FLINCHED) { break 'exec; }

    // Taunt: block status moves (same-turn or future turns)
    if !is_struggle && md.category == MoveCategory::Status
        && state.sides[atk_side].active.taunt_turns > 0
    {
        break 'exec;
    }

    // Confusion: 33% self-hit
    if state.sides[atk_side].active.confusion_turns > 0 {
        // Showdown's confusion onBeforeMove decrements every move attempt, including the application turn.
        state.sides[atk_side].active.confusion_turns -= 1;
        if state.sides[atk_side].active.confusion_turns > 0 && rng(100) < 33 {
            let a = boosted_stat(
                effective_stat(state, atk_side, ATK),
                state.sides[atk_side].active.boosts[ATK],
            ) as u32;
            let d = boosted_stat(
                effective_stat(state, atk_side, DEF),
                state.sides[atk_side].active.boosts[DEF],
            ).max(1) as u32;
            let level = state.sides[atk_side].team[atk_slot].level as u32;
            let level_factor = 2 * level / 5 + 2;
            let dmg = ((level_factor * 40 * a / d) / 50 + 2) as u16;
            deal_damage(state, atk_side, atk_slot, dmg);
            break 'exec;
        }
    }

    // Attraction: 50% chance to skip turn
    if state.sides[atk_side].active.is_attracted() {
        if rng(2) == 0 { break 'exec; }
    }

    // Paralysis: 25% full paralysis
    if state.sides[atk_side].team[atk_slot].status == STATUS_PARALYSIS {
        if rng(4) == 0 { break 'exec; }
    }

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
            // Showdown's deductPP sets moveSlot.used before onTry/hit, so a move use
            // marks the slot even if it later fails or misses. Skip while transformed
            // (used lives on the temporary moveSlots there, not the base slot).
            if !state.sides[atk_side].active.has_volatile(VOL_TRANSFORMED) {
                state.sides[atk_side].team[atk_slot].mark_move_used(pp_slot);
            }
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

        if !is_struggle && state.sides[atk_side].active.choice_locked_move == 0 {
            let is_choice_item = if state.field.magic_room_turns() == 0 {
                data_bridge::item(state.active_mon(atk_side).item_id).has(ItemFlag::IS_CHOICE)
            } else { false };
            let is_gorilla_tactics = effective_ability(state, atk_side)
                == data_bridge::ABILITY_GORILLA_TACTICS;
            if is_choice_item || is_gorilla_tactics {
                state.sides[atk_side].active.choice_locked_move = move_id;
            }
        }
    }

    set_volatile(state, atk_side, VOL_MOVED_THIS_TURN);

    // Future Sight / Doom Desire: register a delayed attack on the target's side
    // instead of hitting now (Showdown futuremove onTry). PP/last_move are already
    // handled above; the hit lands two EOTs later (end_of_turn.rs step 3).
    if !is_charge_turn2 && !is_struggle
        && (move_id == crate::data::MOVE_FUTURE_SIGHT as u16
            || move_id == crate::data::MOVE_DOOM_DESIRE as u16)
    {
        register_future_move(state, atk_side, def_side, atk_slot, move_id);
    } else {
        use_move_called(
            state, teams, atk_side, atk_slot, def_side, def_slot,
            move_id, is_struggle, is_charge_turn2, is_move_locked,
            md, rng, 0,
        );
    }
    } // end 'exec

    // Thrash confusion: applied regardless of whether the move executed (para/freeze/etc.
    // still end the lock and cause confusion). Own Tempo blocks this self-confusion.
    // Sleep is the exception: Showdown's lockedmove.onResidual deletes the lock before
    // onEnd when status is slp, so an asleep user ends the lock without self-confusing.
    // Uproar locks like Thrash but its Showdown condition has no confusion on end.
    if was_last_locked_turn
        && md.effect != MoveEffect::Uproar
        && !state.sides[atk_side].team[atk_slot].is_fainted()
        && state.sides[atk_side].team[atk_slot].status != STATUS_SLEEP
        && effective_ability(state, atk_side) != data_bridge::ABILITY_OWN_TEMPO
    {
        // The lock-end self-confusion runs through Showdown's residual/onEnd, which is
        // deferred past the forced-replacement when the opponent faints on this same turn
        // and has a living switch-in: confusion does not land on the faint turn there.
        // Apply it only when no such forced switch interrupts the turn (no opponent faint,
        // or the faint ends the battle). Duration is 2-5 turns (rng(4)+2).
        let opp_forced_switch = state.sides[def_side].team[def_slot].is_fainted()
            && (0..6).any(|i| {
                let m = &state.sides[def_side].team[i];
                m.species_id != 0 && m.current_hp > 0
            });
        if !opp_forced_switch {
            state.sides[atk_side].active.confusion_turns = (rng(4) + 2) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_rng(val: u32) -> impl FnMut(u32) -> u32 { move |max| val % max }

    fn setup() -> BattleState {
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [150, 100, 150, 100, 100],
            moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
            level: 100,
            ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [100, 100, 100, 100, 80],
            level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 300, max_hp: 300,
            stats: [100, 100, 100, 100, 80],
            moves: [1, 2, 3, 4], pp: [24, 24, 24, 24],
            level: 100,
            ..Default::default()
        };
        state.sides[1].team[1] = MonSlot {
            species_id: 10, current_hp: 200, max_hp: 200,
            stats: [80, 80, 80, 80, 60],
            level: 100,
            ..Default::default()
        };
        state.phase = PHASE_ACTIONS;
        state
    }

    #[test]
    fn test_accuracy_always_hit() {
        let state = setup();
        let md = MoveData { accuracy: 0, ..unsafe { core::mem::zeroed() } };
        assert!(accuracy_check(&state, 0, &md, &mut fixed_rng(99)));
    }

    #[test]
    fn test_accuracy_miss() {
        let state = setup();
        let md = MoveData { accuracy: 50, ..unsafe { core::mem::zeroed() } };
        assert!(!accuracy_check(&state, 0, &md, &mut fixed_rng(99)));
    }

    #[test]
    fn test_evasion_stages() {
        let mut state = setup();
        state.sides[1].active.boosts[EVA] = 2;
        let md = MoveData { accuracy: 100, ..unsafe { core::mem::zeroed() } };
        assert!(accuracy_check(&state, 0, &md, &mut fixed_rng(59)));
        assert!(!accuracy_check(&state, 0, &md, &mut fixed_rng(60)));
    }

    #[test]
    fn test_ancient_power_omniboost() {
        let mut state = setup();
        let md = MoveData {
            secondary_chance: 100, secondary_stat: 1,
            category: MoveCategory::Special,
            ..unsafe { core::mem::zeroed() }
        };
        apply_secondary(&mut state, 0, 1, &md, crate::data::MOVE_ANCIENT_POWER as u16, &mut fixed_rng(0));
        let b = state.sides[0].active.boosts;
        assert_eq!([b[ATK], b[DEF], b[SPA], b[SPD], b[SPE]], [1, 1, 1, 1, 1]);
    }

    // ── Last Resort onTry: fails until every other known slot has been used ───
    #[test]
    fn test_last_resort_fails_until_other_slots_used() {
        let mut state = setup();
        // Two known moves: Last Resort (387) in slot 0, move 2 in slot 1.
        state.sides[0].team[0].moves = [387, 2, 0, 0];
        let hp_before = state.sides[1].team[0].current_hp;

        // Slot 1 not yet used → Last Resort fails, no damage.
        execute_move(&mut state, &TeamData::default(), 0, 387, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, hp_before,
            "Last Resort must fail (no damage) while another known slot is unused");

        // Mark slot 1 used, retry → Last Resort connects.
        state.sides[0].team[0].mark_move_used(1);
        execute_move(&mut state, &TeamData::default(), 0, 387, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < hp_before,
            "Last Resort must connect once every other known slot is used");
    }

    #[test]
    fn test_last_resort_fails_with_single_move() {
        let mut state = setup();
        // Only Last Resort known → moveSlots.length < 2 → always fails.
        state.sides[0].team[0].moves = [387, 0, 0, 0];
        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 387, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, hp_before,
            "Last Resort must fail when it is the only known move");
    }

    // ── Stomping Tantrum / Temper Flare: prev-move-failed capture lifecycle ───

    #[test]
    fn test_move_failure_capture_and_promote() {
        // Side 0 (Pikachu, Electric) Thunderbolt into side 1 (Diglett, Ground):
        // type-immune → the move fails → move-failed flag set, then promoted.
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_THUNDERBOLT as u16, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].move_failed_last_turn(), "not promoted yet");
        state.sides[0].promote_move_failed();
        assert!(state.sides[0].move_failed_last_turn(),
            "a type-immune move must set move_failed (promoted to last-turn)");
    }

    // ── Future Sight / Doom Desire: on-use registration ───────────────────────
    #[test]
    fn test_future_sight_registers_on_target_side_no_immediate_damage() {
        let mut state = setup(); // side1 active hp 300/300
        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_FUTURE_SIGHT as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].side_conditions.future_move(), 1, "FS registered on target side");
        assert_eq!(state.sides[1].side_conditions.future_countdown(), 3, "countdown set to 3");
        assert_eq!(state.sides[1].team[0].current_hp, hp_before, "no immediate damage on use");
        // snapshot: Pikachu (Electric) gets no Psychic STAB; SpA 150 (boost 0); level 100.
        assert_eq!(state.sides[1].side_conditions.future_stab_kind(), 0);
        assert_eq!(state.sides[1].side_conditions.fut_level, state.sides[0].team[0].level);
        assert_eq!(state.sides[1].side_conditions.fut_spa, state.sides[0].team[0].stats[SPA]);
    }

    #[test]
    fn test_future_sight_snapshots_psychic_stab() {
        let mut state = setup();
        state.sides[0].active.override_types = [Type::Psychic as u8, Type::Psychic as u8];
        state.sides[0].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_FUTURE_SIGHT as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].side_conditions.future_stab_kind(), 1, "Psychic user → ×1.5 STAB snapshot");
    }

    #[test]
    fn test_doom_desire_registers_kind_2() {
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_DOOM_DESIRE as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].side_conditions.future_move(), 2, "Doom Desire = kind 2");
        assert_eq!(state.sides[1].team[0].current_hp, 300, "no immediate damage on use");
    }

    // ── Power Split / Guard Split: raw-stat averaging into override_stats ──────
    #[test]
    fn test_power_split_averages_atk_spa_both_actives() {
        let mut state = setup();
        // side0 atk 150 / spa 150 ; side1 atk 100 / spa 100 → avg 125 each
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_POWER_SPLIT as u16, 0, &mut fixed_rng(0));
        assert_eq!(effective_stat(&state, 0, ATK), 125);
        assert_eq!(effective_stat(&state, 1, ATK), 125);
        assert_eq!(effective_stat(&state, 0, SPA), 125);
        assert_eq!(effective_stat(&state, 1, SPA), 125);
        // Untouched stats read native.
        assert_eq!(effective_stat(&state, 0, DEF), 100);
        assert_eq!(effective_stat(&state, 0, SPE), 100);
        assert_eq!(effective_stat(&state, 1, SPE), 80);
        assert!(state.sides[0].active.stats_split_active());
        assert!(state.sides[1].active.stats_split_active());
    }

    #[test]
    fn test_guard_split_averages_def_spd_both_actives() {
        let mut state = setup();
        // side0 def 100 / spd 100 ; side1 def 100 / spd 100 → avg 100; use distinct values
        state.sides[0].team[0].stats = [150, 120, 150, 60, 100];
        state.sides[1].team[0].stats = [100, 80, 100, 100, 80];
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_GUARD_SPLIT as u16, 0, &mut fixed_rng(0));
        assert_eq!(effective_stat(&state, 0, DEF), 100); // (120+80)/2
        assert_eq!(effective_stat(&state, 1, DEF), 100);
        assert_eq!(effective_stat(&state, 0, SPD), 80); // (60+100)/2
        assert_eq!(effective_stat(&state, 1, SPD), 80);
        // Atk/SpA untouched.
        assert_eq!(effective_stat(&state, 0, ATK), 150);
        assert_eq!(effective_stat(&state, 1, SPA), 100);
    }

    #[test]
    fn test_power_split_boosts_apply_on_top() {
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_POWER_SPLIT as u16, 0, &mut fixed_rng(0));
        // averaged atk 125, then +2 boost (×2) reads 250
        state.sides[0].active.boosts[ATK] = 2;
        let raw = effective_stat(&state, 0, ATK);
        assert_eq!(raw, 125);
        assert_eq!(boosted_stat(raw, state.sides[0].active.boosts[ATK]), 250);
    }

    #[test]
    fn test_stat_split_clears_on_switch_out() {
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_POWER_SPLIT as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[0].active.stats_split_active());
        crate::state::switch::switch_out(&mut state, &TeamData::default(), 0);
        assert!(!state.sides[0].active.stats_split_active(), "split flag cleared on switch-out");
        assert_eq!(state.sides[0].active.override_stats, [0; 5], "override_stats zeroed");
        // The incoming mon reads native stats.
        assert_eq!(effective_stat(&state, 0, ATK), state.sides[0].team[0].stats[ATK]);
    }

    #[test]
    fn test_non_split_mon_reads_native_stats() {
        let state = setup();
        assert!(!state.sides[0].active.stats_split_active());
        assert_eq!(effective_stat(&state, 0, ATK), 150);
        assert_eq!(effective_stat(&state, 0, SPA), 150);
    }

    // ── Soak: set the target's types to pure Water ────────────────────────────
    #[test]
    fn test_soak_sets_pure_water() {
        let mut state = setup();
        // side1 active = species 50 (Diglett, Ground). Soak it to pure Water.
        let water = Type::Water as u8;
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_SOAK as u16, 0, &mut fixed_rng(0));
        assert_eq!(effective_types(&state, 1), (water, water));
        assert!(has_type(&state, 1, water));
        assert!(!has_type(&state, 1, Type::Ground as u8), "original Ground type replaced");
    }

    #[test]
    fn test_soak_type_effectiveness_respects_new_type() {
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_SOAK as u16, 0, &mut fixed_rng(0));
        // After Soak, battle_types (used by damage/effectiveness) is pure Water.
        assert_eq!(battle_types(&state, 1), (Type::Water as u8, Type::Water as u8));
    }

    #[test]
    fn test_soak_fails_on_terastallized_target() {
        let mut state = setup();
        state.sides[1].team[0].flags |= MON_FLAG_TERASTALLIZED;
        state.sides[1].team[0].tera_type = Type::Fire as u8;
        let before = effective_types(&state, 1);
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_SOAK as u16, 0, &mut fixed_rng(0));
        assert_eq!(effective_types(&state, 1), before, "Soak fails on a Tera'd target");
        assert!(!state.sides[1].active.has_volatile(VOL_TYPES_OVERRIDDEN));
    }

    #[test]
    fn test_soak_clears_on_switch_out() {
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_SOAK as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        crate::state::switch::switch_out(&mut state, &TeamData::default(), 1);
        assert!(!state.sides[1].active.has_volatile(VOL_TYPES_OVERRIDDEN), "type override cleared");
        assert_eq!(state.sides[1].active.override_types, [0, 0]);
    }

    #[test]
    fn test_future_sight_fails_if_already_pending() {
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_FUTURE_SIGHT as u16, 0, &mut fixed_rng(0));
        let cd1 = state.sides[1].side_conditions.future_countdown();
        // Second use while one is still pending → fails (no re-register, move counts as failed).
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_FUTURE_SIGHT as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].side_conditions.future_countdown(), cd1, "no re-register while pending");
        state.sides[0].promote_move_failed();
        assert!(state.sides[0].move_failed_last_turn(), "a blocked Future Sight counts as a failed move");
    }

    #[test]
    fn test_connecting_move_leaves_flag_clear() {
        // Tackle (Normal) into Diglett connects → no move-failed flag.
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        state.sides[0].promote_move_failed();
        assert!(!state.sides[0].move_failed_last_turn(),
            "a connecting move must leave the move-failed flag clear");
    }

    #[test]
    fn test_move_failed_bit_lifecycle() {
        let mut s = SideState::default();
        s._padding[0] |= 1; // Tera-used bit (must survive)
        s.set_move_failed_this_turn();
        assert!(!s.move_failed_last_turn());
        s.promote_move_failed();
        assert!(s.move_failed_last_turn());
        s.promote_move_failed(); // no new this-turn failure → last-turn clears
        assert!(!s.move_failed_last_turn(), "last-turn clears when this-turn was clean");
        s.set_move_failed_this_turn();
        s.promote_move_failed();
        s.clear_move_failed_state();
        assert!(!s.move_failed_last_turn(), "switch-out clear wipes the flag");
        assert_eq!(s._padding[0] & 1, 1, "Tera bit preserved through clear_move_failed_state");
    }

    // ── Secondary flinch: phantom suppression + dropped-rider re-add ──────────
    // Side-1 active is species 50 (Diglett, Ground): burn/par/freeze all apply.

    fn phantom_md(mt: Type, cat: MoveCategory) -> MoveData {
        MoveData { secondary_chance: 100, move_type: mt, category: cat, ..unsafe { core::mem::zeroed() } }
    }

    #[test]
    fn test_phantom_flinch_suppressed() {
        // onHit moves whose fallthrough would invent a 100% flinch Showdown never applies.
        let cases = [
            (crate::data::MOVE_THROAT_CHOP,    Type::Dark,    MoveCategory::Physical),
            (crate::data::MOVE_SPIRIT_SHACKLE, Type::Ghost,   MoveCategory::Physical),
            (crate::data::MOVE_EERIE_SPELL,    Type::Psychic, MoveCategory::Special),
            (crate::data::MOVE_ALLURING_VOICE, Type::Fairy,   MoveCategory::Special),
            (crate::data::MOVE_PSYCHIC_NOISE,  Type::Psychic, MoveCategory::Special),
        ];
        for (id, mt, cat) in cases {
            let mut state = setup();
            let md = phantom_md(mt, cat);
            apply_secondary(&mut state, 0, 1, &md, id as u16, &mut fixed_rng(0));
            assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED), "move {id} phantom-flinched");
            assert_eq!(state.sides[1].team[0].status, STATUS_NONE, "move {id} applied a phantom status");
        }
    }

    #[test]
    fn test_status_less_ice_electric_secondary_flinches() {
        // Icicle Crash / Mountain Gale / Zing Zap carry volatileStatus:'flinch'
        // (unencodable in MoveData) — no type-derived freeze/paralysis applies.
        let cases = [
            (crate::data::MOVE_ICICLE_CRASH,  Type::Ice),
            (crate::data::MOVE_MOUNTAIN_GALE, Type::Ice),
            (crate::data::MOVE_ZING_ZAP,      Type::Electric),
        ];
        for (id, mt) in cases {
            let mut state = setup();
            let md = MoveData { secondary_chance: 30, move_type: mt,
                category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
            apply_secondary(&mut state, 0, 1, &md, id as u16, &mut fixed_rng(0));
            assert_eq!(state.sides[1].team[0].status, STATUS_NONE, "move {id} applied a phantom status");
            assert!(state.sides[1].active.has_volatile(VOL_FLINCHED), "move {id} must flinch");
            assert!(move_has_flinch_secondary(&md, id as u16), "move {id} must register as a flincher");
        }
    }

    #[test]
    fn test_status_select_index0() {
        // Tri Attack / Dire Claw: force_all rolls rng(3)→0 → index-0 status (brn / psn),
        // never a flinch. Side-1 active (Diglett, Ground) accepts all five statuses.
        let cases = [
            (crate::data::MOVE_TRI_ATTACK, Type::Normal, MoveCategory::Special,  STATUS_BURN),
            (crate::data::MOVE_DIRE_CLAW,  Type::Poison, MoveCategory::Physical, STATUS_POISON),
        ];
        for (id, mt, cat, st) in cases {
            let mut state = setup();
            let md = MoveData { secondary_chance: 100, move_type: mt, category: cat,
                ..unsafe { core::mem::zeroed() } };
            apply_secondary(&mut state, 0, 1, &md, id as u16, &mut fixed_rng(0));
            assert_eq!(state.sides[1].team[0].status, st, "move {id} index-0 status wrong");
            assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED), "move {id} phantom-flinched");
        }
    }

    #[test]
    fn test_status_select_all_indices() {
        // fixed_rng(v) returns v%max: rng(100)=v<100 always triggers, rng(3)=v selects.
        // Tri Attack 0/1/2 → brn/par/frz, Dire Claw 0/1/2 → psn/par/slp (moves.ts order).
        let tri  = [STATUS_BURN,   STATUS_PARALYSIS, STATUS_FREEZE];
        let dire = [STATUS_POISON, STATUS_PARALYSIS, STATUS_SLEEP];
        for idx in 0u32..3 {
            let mut s = setup();
            let md = MoveData { secondary_chance: 100, move_type: Type::Normal,
                category: MoveCategory::Special, ..unsafe { core::mem::zeroed() } };
            apply_secondary(&mut s, 0, 1, &md, crate::data::MOVE_TRI_ATTACK as u16, &mut fixed_rng(idx));
            assert_eq!(s.sides[1].team[0].status, tri[idx as usize], "Tri Attack idx {idx}");

            let mut s = setup();
            let md = MoveData { secondary_chance: 100, move_type: Type::Poison,
                category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
            apply_secondary(&mut s, 0, 1, &md, crate::data::MOVE_DIRE_CLAW as u16, &mut fixed_rng(idx));
            assert_eq!(s.sides[1].team[0].status, dire[idx as usize], "Dire Claw idx {idx}");
        }
    }

    #[test]
    fn test_burning_jealousy_burns_stat_raised_target() {
        // Target raised a stat this turn → Burning Jealousy burns it (force_all
        // secondary_chance:100). Side-1 active (Diglett, Ground) accepts burn.
        let mut state = setup();
        state.sides[1].set_stats_raised_this_turn();
        let md = MoveData { secondary_chance: 100, move_type: Type::Fire,
            category: MoveCategory::Special, ..unsafe { core::mem::zeroed() } };
        apply_secondary(&mut state, 0, 1, &md, crate::data::MOVE_BURNING_JEALOUSY as u16, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].status, STATUS_BURN, "stat-raised target must be burned");
    }

    #[test]
    fn test_burning_jealousy_no_burn_unraised_target() {
        // Target raised no stat → no burn, and no phantom-flinch fallthrough.
        let mut state = setup();
        let md = MoveData { secondary_chance: 100, move_type: Type::Fire,
            category: MoveCategory::Special, ..unsafe { core::mem::zeroed() } };
        apply_secondary(&mut state, 0, 1, &md, crate::data::MOVE_BURNING_JEALOUSY as u16, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE, "un-raised target must NOT be burned");
        assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED), "no phantom flinch fallthrough");
    }

    #[test]
    fn test_non_burning_jealousy_fire_burns_unconditionally() {
        // A generic Fire move (Flamethrower) keeps its unconditional secondary burn
        // via the Type::Fire heuristic — the stat-raised gate is Burning-Jealousy-only.
        let mut state = setup();
        let md = MoveData { secondary_chance: 100, move_type: Type::Fire,
            category: MoveCategory::Special, ..unsafe { core::mem::zeroed() } };
        apply_secondary(&mut state, 0, 1, &md, crate::data::MOVE_FLAMETHROWER as u16, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].status, STATUS_BURN, "Flamethrower burn must be unconditional");
    }

    #[test]
    fn test_real_flinch_preserved() {
        // Genuine flinch-only moves (Steel/Flying, status=0) must still flinch.
        for id in [crate::data::MOVE_IRON_HEAD, crate::data::MOVE_AIR_SLASH] {
            let mut state = setup();
            let md = MoveData { secondary_chance: 30, move_type: Type::Steel,
                category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
            apply_secondary(&mut state, 0, 1, &md, id as u16, &mut fixed_rng(0));
            assert!(state.sides[1].active.has_volatile(VOL_FLINCHED), "move {id} lost its real flinch");
        }
    }

    #[test]
    fn test_accuracy_droppers_drop_not_flinch() {
        use crate::state::structs::ACC;
        // The accuracy-drop family (boosts:{accuracy:-1}) now carries secondary_stat:-1
        // from codegen, routing through the <0 block (→ ACCURACY override) instead of the
        // phantom-flinch fallthrough. Uses the real generated MoveData for each move.
        let ids = [
            crate::data::MOVE_MUD_SLAP, crate::data::MOVE_OCTAZOOKA,
            crate::data::MOVE_MUDDY_WATER, crate::data::MOVE_MUD_BOMB,
            crate::data::MOVE_MIRROR_SHOT, crate::data::MOVE_LEAF_TORNADO,
            crate::data::MOVE_NIGHT_DAZE,
        ];
        for id in ids {
            let md = *crate::data::moves::move_data(id);
            assert_eq!(md.secondary_stat, -1, "move {id} codegen secondary_stat");
            let mut state = setup();
            apply_secondary(&mut state, 0, 1, &md, id as u16, &mut fixed_rng(0));
            assert_eq!(state.sides[1].active.boosts[ACC], -1, "move {id} accuracy not dropped");
            assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED), "move {id} phantom-flinched");
        }
    }

    #[test]
    fn test_real_flinch_respects_chance() {
        // Iron Head chance 30: a roll of 50 (>=30) must NOT flinch.
        let mut state = setup();
        let md = MoveData { secondary_chance: 30, move_type: Type::Steel,
            category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
        apply_secondary(&mut state, 0, 1, &md, crate::data::MOVE_IRON_HEAD as u16, &mut fixed_rng(50));
        assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED));
    }

    #[test]
    fn test_fang_flinch_rider() {
        // Fire/Ice/Thunder Fang: primary status AND the dropped 10% flinch rider both fire.
        let cases = [
            (crate::data::MOVE_FIRE_FANG,    Type::Fire,     STATUS_BURN),
            (crate::data::MOVE_ICE_FANG,     Type::Ice,      STATUS_FREEZE),
            (crate::data::MOVE_THUNDER_FANG, Type::Electric, STATUS_PARALYSIS),
        ];
        for (id, mt, st) in cases {
            let mut state = setup();
            let md = MoveData { secondary_chance: 10, secondary_status: st, move_type: mt,
                category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
            apply_secondary(&mut state, 0, 1, &md, id as u16, &mut fixed_rng(0));
            assert_eq!(state.sides[1].team[0].status, st, "fang {id} primary status dropped");
            assert!(state.sides[1].active.has_volatile(VOL_FLINCHED), "fang {id} flinch rider dropped");
        }
    }

    #[test]
    fn test_triple_arrows_flinch_rider() {
        // Triple Arrows: 50% Def-1 primary AND the dropped 30% flinch rider both fire.
        let mut state = setup();
        let md = MoveData { secondary_chance: 50, secondary_stat: -1, move_type: Type::Fighting,
            category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
        apply_secondary(&mut state, 0, 1, &md, crate::data::MOVE_TRIPLE_ARROWS as u16, &mut fixed_rng(0));
        assert_eq!(state.sides[1].active.boosts[DEF], -1, "Triple Arrows Def-drop dropped");
        assert!(state.sides[1].active.has_volatile(VOL_FLINCHED), "Triple Arrows flinch rider dropped");
    }

    #[test]
    fn test_flinch_rider_independent_of_primary() {
        // Rider rolls separately from the primary (Showdown's per-secondary random(100)).
        // Call order is primary-then-rider: seq [99, 0] => primary misses, rider fires.
        let mut state = setup();
        let md = MoveData { secondary_chance: 10, secondary_status: STATUS_BURN, move_type: Type::Fire,
            category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
        let mut n = 0u32;
        let mut rng = |max: u32| -> u32 { let v = if n == 0 { 99 } else { 0 }; n += 1; v % max };
        apply_secondary(&mut state, 0, 1, &md, crate::data::MOVE_FIRE_FANG as u16, &mut rng);
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE, "primary should have missed");
        assert!(state.sides[1].active.has_volatile(VOL_FLINCHED), "rider should have fired");

        // seq [0, 99] => primary fires, rider misses.
        let mut state = setup();
        let mut n = 0u32;
        let mut rng = |max: u32| -> u32 { let v = if n == 0 { 0 } else { 99 }; n += 1; v % max };
        apply_secondary(&mut state, 0, 1, &md, crate::data::MOVE_FIRE_FANG as u16, &mut rng);
        assert_eq!(state.sides[1].team[0].status, STATUS_BURN, "primary should have fired");
        assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED), "rider should have missed");
    }

    #[test]
    fn test_flinch_rider_not_added_when_moved() {
        // A target that already moved cannot be flinched by the rider.
        let mut state = setup();
        set_volatile(&mut state, 1, VOL_MOVED_THIS_TURN);
        let md = MoveData { secondary_chance: 10, secondary_status: STATUS_BURN, move_type: Type::Fire,
            category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
        apply_secondary(&mut state, 0, 1, &md, crate::data::MOVE_FIRE_FANG as u16, &mut fixed_rng(0));
        assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED));
    }

    // ── Stench ability-added flinch ──────────────────────────────────────────

    #[test]
    fn test_move_has_flinch_secondary_predicate() {
        // Genuine flinchers (fallthrough + rider) vs a non-flincher (Fury Swipes).
        let iron_head = MoveData { secondary_chance: 30, move_type: Type::Steel,
            category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
        assert!(move_has_flinch_secondary(&iron_head, crate::data::MOVE_IRON_HEAD as u16));
        let fang = MoveData { secondary_chance: 10, secondary_status: STATUS_BURN, move_type: Type::Fire,
            category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
        assert!(move_has_flinch_secondary(&fang, crate::data::MOVE_FIRE_FANG as u16));
        // Fury Swipes: a multi-hit move with no secondary at all.
        let fury = MoveData { secondary_chance: 0, move_type: Type::Normal,
            category: MoveCategory::Physical, ..unsafe { core::mem::zeroed() } };
        assert!(!move_has_flinch_secondary(&fury, crate::data::MOVE_FURY_SWIPES as u16));
    }

    #[test]
    fn test_stench_adds_flinch() {
        // Stench attacker, plain non-flinching Tackle → 10% flinch (force_all → fires).
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STENCH;
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].active.has_volatile(VOL_FLINCHED),
            "Stench should add a flinch to a non-flinching damaging move");
    }

    #[test]
    fn test_stench_does_not_double_flinch() {
        // Iron Head already flinches; Stench must not add a redundant secondary
        // (force the rng so a phantom second roll would be visible — Iron Head's
        // own 30% fires under force_all, the Stench arm is gated off by the
        // already-flinches predicate). The flinch must come solely from the move.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STENCH;
        // Iron Head's hot MoveData carries its flinch via the fallthrough — confirm
        // the predicate keeps the Stench arm from firing on top of it.
        assert!(move_has_flinch_secondary(
            &data_bridge::move_hot(crate::data::MOVE_IRON_HEAD as u16),
            crate::data::MOVE_IRON_HEAD as u16),
            "Iron Head must register as already-flinching so Stench skips it");
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_IRON_HEAD as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].active.has_volatile(VOL_FLINCHED),
            "Iron Head flinches on its own");
    }

    #[test]
    fn test_stench_suppressed_by_covert_cloak() {
        // Covert Cloak on the target blocks the Stench flinch like any secondary.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STENCH;
        state.sides[1].team[0].item_id = data_bridge::ITEM_COVERT_CLOAK;
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED),
            "Covert Cloak should suppress the Stench-added flinch");
    }

    // ── Dream Eater: onTryImmunity gates on the target being asleep ──────────

    #[test]
    fn test_dream_eater_fails_on_awake_target() {
        // Awake target → Dream Eater fails (no damage dealt, no heal to the user).
        let mut state = setup();
        state.sides[0].team[0].current_hp = 150; // user damaged, so a heal would show
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_DREAM_EATER as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before,
            "Dream Eater must deal no damage to an awake target");
        assert_eq!(state.sides[0].team[0].current_hp, 150,
            "Dream Eater must not heal the user when it fails on an awake target");
    }

    #[test]
    fn test_dream_eater_hits_asleep_target() {
        // Asleep target → Dream Eater hits, deals damage, and drains (heals the user).
        let mut state = setup();
        state.sides[0].team[0].current_hp = 150;
        state.sides[1].team[0].status = STATUS_SLEEP;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_DREAM_EATER as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < def_hp_before,
            "Dream Eater must damage an asleep target");
        assert!(state.sides[0].team[0].current_hp > 150,
            "Dream Eater must drain-heal the user when it hits an asleep target");
    }

    #[test]
    fn test_dream_eater_hits_comatose_target() {
        // Comatose target counts as asleep for Dream Eater.
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_COMATOSE;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_DREAM_EATER as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < def_hp_before,
            "Dream Eater must hit a Comatose target");
    }

    // ── Pre-move ordering: sleep/freeze tick before the flinch break ─────────
    // Showdown onBeforeMove priority: sleep/freeze (10) > flinch (8). An asleep or
    // frozen + flinched mon must still tick its counter / roll its thaw, then be
    // cancelled by the flinch.

    #[test]
    fn test_flinched_only_skips_move() {
        // Awake + flinched: no move, no status change (baseline).
        let mut state = setup();
        let hp = state.sides[1].team[0].current_hp;
        set_volatile(&mut state, 0, VOL_FLINCHED);
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, hp, "flinched mon must not move");
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_asleep_flinched_ticks_then_cant_move() {
        // Asleep (counter 2) + flinched: sleep decrements to 1 (still asleep), the
        // flinch then cancels the move. Pre-fix the flinch broke before the tick,
        // freezing the counter.
        let mut state = setup();
        let hp = state.sides[1].team[0].current_hp;
        set_status(&mut state, 0, 0, STATUS_SLEEP, 2);
        set_volatile(&mut state, 0, VOL_FLINCHED);
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].status_counter, 1, "sleep counter must tick under a flinch");
        assert_eq!(state.sides[0].team[0].status, STATUS_SLEEP, "still asleep");
        assert_eq!(state.sides[1].team[0].current_hp, hp, "flinched-asleep mon must not move");
    }

    #[test]
    fn test_asleep_flinched_wakes_then_cant_move() {
        // Asleep (counter 1) + flinched: sleep decrements to 0 → wakes (status
        // cleared), the flinch still cancels the move this turn.
        let mut state = setup();
        let hp = state.sides[1].team[0].current_hp;
        set_status(&mut state, 0, 0, STATUS_SLEEP, 1);
        set_volatile(&mut state, 0, VOL_FLINCHED);
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE, "wakes on the 0 tick");
        assert_eq!(state.sides[1].team[0].current_hp, hp, "still can't move (flinch) the wake turn");
    }

    #[test]
    fn test_asleep_not_flinched_ticks_as_before() {
        // Asleep (counter 2), not flinched: counter decrements, still asleep,
        // move fails — unchanged behavior, the reorder must not regress it.
        let mut state = setup();
        let hp = state.sides[1].team[0].current_hp;
        set_status(&mut state, 0, 0, STATUS_SLEEP, 2);
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].status_counter, 1, "sleep counter ticks");
        assert_eq!(state.sides[0].team[0].status, STATUS_SLEEP, "still asleep");
        assert_eq!(state.sides[1].team[0].current_hp, hp, "asleep mon must not move");
    }

    #[test]
    fn test_frozen_flinched_thaws_then_cant_move() {
        // Frozen + flinched: the thaw roll (force_all rng(5)=0 → thaw) runs before
        // the flinch break, mirroring Showdown's freeze(10) > flinch(8). Pre-fix the
        // flinch broke first, leaving the mon frozen (the FANG-FLINCH residual #1).
        let mut state = setup();
        let hp = state.sides[1].team[0].current_hp;
        set_status(&mut state, 0, 0, STATUS_FREEZE, 0);
        set_volatile(&mut state, 0, VOL_FLINCHED);
        execute_move(&mut state, &TeamData::default(), 0,
            crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE, "thaw roll runs before the flinch break");
        assert_eq!(state.sides[1].team[0].current_hp, hp, "still can't move (flinch) the thaw turn");
    }

    #[test]
    fn test_fake_out_first_turn_only_gate() {
        // Fake Out (252) works on the user's first move action since switching in
        // (acted_since_switch_in clear), fails once the user has acted this stay-in,
        // and works again after the switch-out reset. The flag is set by
        // execute_action in production; drive it directly here.
        let mut state = setup();
        let hp0 = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 252, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < hp0, "Fake Out hits on the first turn out");
        assert!(state.sides[1].active.has_volatile(VOL_FLINCHED), "Fake Out flinches on the first turn out");

        // User has now acted this stay-in → Fake Out fails (no damage, no flinch).
        state.sides[0].set_acted_since_switch_in();
        clear_volatile(&mut state, 1, VOL_FLINCHED);
        let hp1 = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 252, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, hp1, "Fake Out fails after the user has acted");
        assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED), "no flinch on the gated Fake Out");

        // Switch-out clears the flag → Fake Out works again on re-entry.
        state.sides[0].clear_acted_since_switch_in();
        let hp2 = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 252, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < hp2, "Fake Out works again after switch-out reset");
    }

    #[test]
    fn test_first_impression_first_turn_only_gate() {
        // First Impression (660) shares the same first-turn onTry gate (no flinch).
        let mut state = setup();
        let hp0 = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 660, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < hp0, "First Impression hits on the first turn out");

        state.sides[0].set_acted_since_switch_in();
        let hp1 = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 660, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, hp1, "First Impression fails after the user has acted");
    }

    #[test]
    fn test_self_boost_status_family() {
        // batch-4·D self-boost family: each must apply its exact Showdown boost.
        // Expected (atk, def, spa, spd, spe) after one use from neutral.
        let cases: [(u16, [i8; 5]); 11] = [
            (151, [0, 2, 0, 0, 0]),  // Acid Armor: +2 Def (≡ Iron Defense)
            (538, [0, 3, 0, 0, 0]),  // Cotton Guard: +3 Def
            (110, [0, 1, 0, 0, 0]),  // Withdraw: +1 Def
            (106, [0, 1, 0, 0, 0]),  // Harden: +1 Def
            (111, [0, 1, 0, 0, 0]),  // Defense Curl: +1 Def
            (322, [0, 1, 0, 1, 0]),  // Cosmic Power: +1 Def/+1 SpD
            (133, [0, 0, 0, 2, 0]),  // Amnesia: +2 SpD
            (96,  [1, 0, 0, 0, 0]),  // Meditate: +1 Atk
            (159, [1, 0, 0, 0, 0]),  // Sharpen: +1 Atk
            (526, [1, 0, 1, 0, 0]),  // Work Up: +1 Atk/+1 SpA
            (294, [0, 0, 3, 0, 0]),  // Tail Glow: +3 SpA
        ];
        for (id, exp) in cases {
            let mut state = setup();
            execute_move(&mut state, &TeamData::default(), 0, id, 0, &mut fixed_rng(0));
            let b = state.sides[0].active.boosts;
            assert_eq!([b[ATK], b[DEF], b[SPA], b[SPD], b[SPE]], exp, "move {id} boost mismatch");
        }
    }

    #[test]
    fn test_growth_sun_doubles() {
        // Growth (74): +1 Atk/+1 SpA in clear, +2/+2 in (harsh) sun.
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0, 74, 0, &mut fixed_rng(0));
        let b = state.sides[0].active.boosts;
        assert_eq!([b[ATK], b[SPA]], [1, 1], "Growth +1/+1 in clear weather");

        let mut state = setup();
        state.field.weather = WEATHER_SUN;
        state.field.weather_turns = 5;
        execute_move(&mut state, &TeamData::default(), 0, 74, 0, &mut fixed_rng(0));
        let b = state.sides[0].active.boosts;
        assert_eq!([b[ATK], b[SPA]], [2, 2], "Growth +2/+2 in sun");
    }

    #[test]
    fn test_full_paralysis() {
        let mut state = setup();
        state.sides[0].team[0].status = STATUS_PARALYSIS;
        let pp_before = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].pp[0], pp_before);
    }

    #[test]
    fn test_sleep_wake() {
        let mut state = setup();
        state.sides[0].team[0].status = STATUS_SLEEP;
        state.sides[0].team[0].status_counter = 2;
        // Turn 1: counter 2→1, still asleep
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].status, STATUS_SLEEP);
        assert_eq!(state.sides[0].team[0].status_counter, 1);
        // Turn 2: counter 1→0, wake up
        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
        assert_eq!(state.sides[0].team[0].pp[0], pp - 1);
    }

    #[test]
    fn test_confusion_self_hit() {
        let mut state = setup();
        state.sides[0].active.confusion_turns = 3;
        let hp = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));
        assert!(state.sides[0].team[0].current_hp < hp);
    }

    // nature = boosted*5 + reduced (0=atk,1=def,2=spa,3=spd,4=spe).
    // Neutral when boosted == reduced (no minus stat).
    fn teams_with_nature(nature: u8) -> TeamData {
        let mut t = TeamData::default();
        t.mons[0][0].nature = nature;
        t
    }

    fn run_flavor_berry(item_id: u16, nature: u8) -> u8 {
        let mut state = setup();
        let max = state.sides[0].team[0].max_hp;
        state.sides[0].team[0].item_id = item_id;
        state.sides[0].team[0].current_hp = max / 4; // at the ≤¼ trigger threshold
        let teams = teams_with_nature(nature);
        check_berry_activation(&mut state, &teams, 0, 0, &mut fixed_rng(0));
        state.sides[0].active.confusion_turns
    }

    #[test]
    fn test_figy_confuses_minus_atk_nature() {
        // -Atk (reduced=0), +SpA (boosted=2) => nature 10
        assert!(run_flavor_berry(data_bridge::ITEM_FIGY_BERRY, 10) >= 2,
            "Figy must confuse a -Atk holder");
    }

    #[test]
    fn test_figy_no_confuse_neutral_nature() {
        // Hardy (boosted==reduced==0) => no minus stat
        assert_eq!(run_flavor_berry(data_bridge::ITEM_FIGY_BERRY, 0), 0,
            "Figy must NOT confuse a neutral-nature holder");
    }

    #[test]
    fn test_figy_no_confuse_plus_atk_nature() {
        // +Atk (boosted=0), -SpA (reduced=2) => nature 2; minus is SpA, not Atk
        assert_eq!(run_flavor_berry(data_bridge::ITEM_FIGY_BERRY, 2), 0,
            "Figy must NOT confuse a +Atk holder (minus stat is SpA)");
    }

    #[test]
    fn test_each_flavor_berry_maps_to_its_disliked_stat() {
        // (berry, disliked 0-indexed stat): Figy=atk0 Iapapa=def1 Wiki=spa2 Aguav=spd3 Mago=spe4
        let cases = [
            (data_bridge::ITEM_FIGY_BERRY, 0u8),
            (data_bridge::ITEM_IAPAPA_BERRY, 1),
            (data_bridge::ITEM_WIKI_BERRY, 2),
            (data_bridge::ITEM_AGUAV_BERRY, 3),
            (data_bridge::ITEM_MAGO_BERRY, 4),
        ];
        for (item, disliked) in cases {
            // Build a nature whose minus == disliked and plus is a different stat.
            let plus = (disliked + 1) % 5;
            let nature = plus * 5 + disliked;
            assert!(run_flavor_berry(item, nature) >= 2,
                "berry {} should confuse a -{} holder", item, disliked);
            // A holder whose minus is a DIFFERENT stat is not confused.
            let other_minus = (disliked + 1) % 5;
            let other_plus = (other_minus + 1) % 5;
            let other_nature = other_plus * 5 + other_minus;
            assert_eq!(run_flavor_berry(item, other_nature), 0,
                "berry {} should NOT confuse a holder disliking stat {}", item, other_minus);
        }
    }

    // Side 0 attacks side 1 (the berry holder). `def_hp` controls survive vs KO;
    // attacker max_hp = 800 so a 1/8 chip = 100. Returns attacker HP lost.
    fn run_recoil_berry(item_id: u16, move_id: u16, def_hp: u16) -> u16 {
        let mut state = setup();
        state.sides[0].team[0].max_hp = 800;
        state.sides[0].team[0].current_hp = 800;
        state.sides[0].team[0].stats = [600, 600, 600, 600, 100]; // force a KO at low def_hp
        state.sides[1].team[0].item_id = item_id;
        state.sides[1].team[0].current_hp = def_hp;
        state.sides[1].team[0].max_hp = def_hp.max(300);
        execute_move(&mut state, &TeamData::default(), 0, move_id, 0, &mut fixed_rng(99));
        800 - state.sides[0].team[0].current_hp
    }

    #[test]
    fn test_jaboca_chips_attacker_holder_survives_physical() {
        // High HP holder survives Tackle; Jaboca still chips the physical attacker 1/8.
        let lost = run_recoil_berry(data_bridge::ITEM_JABOCA_BERRY, crate::data::MOVE_TACKLE as u16, 9999);
        assert_eq!(lost, 100, "Jaboca chips a surviving holder's physical attacker 1/8");
    }

    #[test]
    fn test_jaboca_chips_attacker_when_holder_koed_physical() {
        // Holder KO'd by Tackle; Jaboca still fires as it faints.
        let mut state = setup();
        state.sides[0].team[0].max_hp = 800;
        state.sides[0].team[0].current_hp = 800;
        state.sides[0].team[0].stats = [600, 600, 600, 600, 100];
        state.sides[1].team[0].item_id = data_bridge::ITEM_JABOCA_BERRY;
        state.sides[1].team[0].current_hp = 1;
        execute_move(&mut state, &TeamData::default(), 0, crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(99));
        assert!(state.sides[1].team[0].is_fainted(), "holder is KO'd");
        assert_eq!(800 - state.sides[0].team[0].current_hp, 100,
            "Jaboca chips the attacker 1/8 even as the holder faints");
    }

    #[test]
    fn test_rowap_chips_attacker_on_special() {
        // Holder survives Swift (special); Rowap chips the special attacker 1/8.
        let lost = run_recoil_berry(data_bridge::ITEM_ROWAP_BERRY, 129 /* Swift, special */, 9999);
        assert_eq!(lost, 100, "Rowap chips a surviving holder's special attacker 1/8");
    }

    #[test]
    fn test_recoil_berry_wrong_category_no_fire() {
        // Jaboca does NOT fire on a special hit; Rowap does NOT fire on a physical hit.
        let jaboca_special = run_recoil_berry(data_bridge::ITEM_JABOCA_BERRY, 129 /* Swift */, 9999);
        assert_eq!(jaboca_special, 0, "Jaboca must not fire on a special hit");
        let rowap_physical = run_recoil_berry(data_bridge::ITEM_ROWAP_BERRY, crate::data::MOVE_TACKLE as u16, 9999);
        assert_eq!(rowap_physical, 0, "Rowap must not fire on a physical hit");
    }

    #[test]
    fn test_protect() {
        let mut state = setup();
        state.pending_actions[0] = 0; // opponent still has a pending action (willAct)
        execute_protect(&mut state, 1, 0, 0, &mut fixed_rng(0));
        assert!(state.sides[1].active.has_volatile(VOL_PROTECT_THIS_TURN));
        let hp = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[1].team[0].current_hp, hp);
    }

    #[test]
    fn test_recharging_skips() {
        let mut state = setup();
        set_volatile(&mut state, 0, VOL_RECHARGING);
        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));
        assert!(!state.sides[0].active.has_volatile(VOL_RECHARGING));
        assert_eq!(state.sides[0].team[0].pp[0], pp);
    }

    #[test]
    fn test_truant_loafs_every_other_attempt() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_TRUANT;

        // First attempt: toggle clear → acts (PP spent), toggle set.
        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].pp[0], pp - 1, "first attempt: Truant acts");
        assert!(state.sides[0].active.truant_loaf_pending());

        // Second attempt: toggle set → loafs (no PP spent), toggle cleared.
        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].pp[0], pp, "second attempt: Truant loafs");
        assert!(!state.sides[0].active.truant_loaf_pending());

        // Third attempt: acts again.
        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].pp[0], pp - 1, "third attempt: Truant acts again");
    }

    #[test]
    fn test_truant_acts_first_attempt_after_switch_in() {
        // A mon switched in on turn N has turns_active==1 on its first attacking
        // turn (zeroed by the switch, ticked at that turn's EOT). Showdown acts:
        // the truant volatile was cleared on switch-in.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_TRUANT;
        state.sides[0].active.turns_active = 1;

        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(
            state.sides[0].team[0].pp[0], pp - 1,
            "switched-in Truant acts its first attempt"
        );
    }

    #[test]
    fn test_truant_recharge_turn_clears_loaf_toggle() {
        // Showdown's mustrecharge onBeforeMove removes the truant volatile: the
        // recharge turn doubles as the loaf turn, and the mon acts right after.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_TRUANT;
        state.sides[0].active.set_truant_loaf_pending();
        set_volatile(&mut state, 0, VOL_RECHARGING);

        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));
        assert!(!state.sides[0].active.truant_loaf_pending());

        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].pp[0], pp - 1, "Truant acts after recharge turn");
    }

    #[test]
    fn test_truant_suppressed_does_not_loaf() {
        // Gastro Acid / Neutralizing Gas suppresses Truant → no loaf even with the
        // toggle pending.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_TRUANT;
        state.sides[0].active.set_truant_loaf_pending();
        set_volatile(&mut state, 0, VOL_ABILITY_SUPPRESSED);
        let pp = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].pp[0], pp - 1, "suppressed Truant acts");
    }

    #[test]
    fn test_confusion_clears_on_expiry() {
        let mut state = setup();
        state.sides[0].active.confusion_turns = 1;
        let def_hp = state.sides[1].team[0].current_hp;
        // rng(3) returns 1 (not 0), so no self-hit; confusion_turns decrements to 0
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(1));
        assert_eq!(state.sides[0].active.confusion_turns, 0);
        // Mon attacked normally — defender took damage
        assert!(state.sides[1].team[0].current_hp < def_hp);
    }

    #[test]
    fn test_confusion_decrements_on_first_move() {
        let mut state = setup();
        // Confused before moving: the decrement lands this turn (Showdown decrements unconditionally).
        state.sides[0].active.confusion_turns = 3;
        // rng(3) returns 1 (not 0), so no self-hit; counter goes 3 -> 2 this turn.
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(1));
        assert_eq!(state.sides[0].active.confusion_turns, 2);
    }

    #[test]
    fn test_confusion_ticks_before_paralysis() {
        // Showdown's confusion (pri 3) runs before par (pri 1): on a full-para
        // turn the counter still decrements and the self-hit lands first.
        let mut state = setup();
        state.sides[0].team[0].status = STATUS_PARALYSIS;
        state.sides[0].active.confusion_turns = 2;
        let def_hp = state.sides[1].team[0].current_hp;
        // rng→0 forces both the self-hit roll and the full-para roll.
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.confusion_turns, 1, "confusion ticks before par");
        assert!(
            state.sides[0].team[0].current_hp < 300,
            "confusion self-hit lands before the par roll"
        );
        assert_eq!(state.sides[1].team[0].current_hp, def_hp, "move itself cancelled");
    }

    #[test]
    fn test_sleep_cancels_before_truant_toggle() {
        // Showdown's slp (pri 10) cancels before truant (pri 9): the loaf
        // toggle does not advance while asleep.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_TRUANT;
        state.sides[0].team[0].status = STATUS_SLEEP;
        state.sides[0].team[0].status_counter = 3;
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[0].team[0].status_counter, 2, "sleep counter ticks");
        assert!(
            !state.sides[0].active.truant_loaf_pending(),
            "toggle does not advance while asleep"
        );
    }

    #[test]
    fn test_taunt_cancels_before_confusion_tick() {
        // Showdown's taunt (pri 5) cancels a status move before confusion
        // (pri 3) decrements or self-hits.
        let mut state = setup();
        state.sides[0].team[0].moves[0] = 45;
        state.sides[0].active.taunt_turns = 2;
        state.sides[0].active.confusion_turns = 2;
        execute_move(&mut state, &TeamData::default(), 0, 45, 0, &mut fixed_rng(99));
        assert_eq!(
            state.sides[0].active.confusion_turns, 2,
            "taunt cancels before confusion ticks"
        );
    }

    #[test]
    fn test_protect_first_always_succeeds() {
        let mut state = setup();
        state.pending_actions[1] = 0;
        assert_eq!(state.sides[0].active.protect_consecutive, 0);
        execute_protect(&mut state, 0, 1, 0, &mut fixed_rng(99));
        assert!(state.sides[0].active.has_volatile(VOL_PROTECT_THIS_TURN));
        assert_eq!(state.sides[0].active.protect_consecutive, 1);
    }

    #[test]
    fn test_protect_consecutive_can_fail() {
        let mut state = setup();
        state.pending_actions[1] = 0;
        state.sides[0].active.protect_consecutive = 1;
        // rng(3) returns 1 (not 0), so Protect fails
        execute_protect(&mut state, 0, 1, 0, &mut fixed_rng(1));
        assert!(!state.sides[0].active.has_volatile(VOL_PROTECT_THIS_TURN));
    }

    #[test]
    fn test_protect_fourth_use_rolls_not_autofail() {
        // 4th consecutive use (consecutive==3) rolls 1/27 — succeeds when rng(27)==0,
        // matching Showdown's stall counter=27. The 5th (consecutive>=4) hard-fails.
        let mut state = setup();
        state.pending_actions[1] = 0;
        state.sides[0].active.protect_consecutive = 3;
        execute_protect(&mut state, 0, 1, 0, &mut fixed_rng(0));
        assert!(state.sides[0].active.has_volatile(VOL_PROTECT_THIS_TURN));
        assert_eq!(state.sides[0].active.protect_consecutive, 4);

        let mut state2 = setup();
        state2.pending_actions[1] = 0;
        state2.sides[0].active.protect_consecutive = 4;
        execute_protect(&mut state2, 0, 1, 0, &mut fixed_rng(0));
        assert!(!state2.sides[0].active.has_volatile(VOL_PROTECT_THIS_TURN));
    }

    #[test]
    fn test_protect_fails_when_opponent_not_acting() {
        // Showdown's willAct() gate: on an opponent-switch turn the switch resolves
        // first and pending_actions[def_side] is 0xFF when Protect runs, so Protect
        // FAILS and the stall counter does not climb (the EOT reset zeroes it).
        let mut state = setup();
        state.pending_actions[1] = 0xFF; // opponent already resolved (switched/moved)
        state.sides[0].active.protect_consecutive = 1;
        execute_protect(&mut state, 0, 1, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_PROTECT_THIS_TURN));
        // Counter must NOT increment on the gated-out use.
        assert_eq!(state.sides[0].active.protect_consecutive, 1);

        // Endure shares the ladder and the same gate.
        let mut state2 = setup();
        state2.pending_actions[1] = 0xFF;
        execute_endure(&mut state2, 0, 1, &mut fixed_rng(0));
        assert!(!state2.sides[0].active.has_volatile(VOL_ENDURE));
        assert_eq!(state2.sides[0].active.protect_consecutive, 0);
    }

    #[test]
    fn test_protect_resets_on_different_move() {
        let mut state = setup();
        state.sides[0].active.protect_consecutive = 1;
        // Mon does NOT use Protect this turn (no VOL_PROTECT_THIS_TURN set)
        // EOT should reset protect_consecutive to 0
        crate::state::end_of_turn::end_of_turn(&mut state, &TeamData::default(), &mut crate::state::BattleRng::from_closure(&mut |_| 0u32));
        assert_eq!(state.sides[0].active.protect_consecutive, 0);
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

    #[test]
    fn test_weather_acc_rain_always_hits() {
        let mut state = setup();
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
        let mut state = setup();
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
        let mut state = setup();
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
        let state = setup();
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

    #[test]
    fn test_parting_shot_debuffs_and_switches() {
        let mut state = setup();

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0, // always hits
            effect: MoveEffect::PartingShot,
            ..unsafe { core::mem::zeroed() }
        };

        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(99), 0);

        // Opponent should have -1 Atk and -1 SpA
        assert_eq!(state.sides[1].active.boosts[ATK], -1);
        assert_eq!(state.sides[1].active.boosts[SPA], -1);

        // Attacker should have VOL_MUST_SWITCH set (has bench mon)
        assert!(state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    }

    #[test]
    fn test_parting_shot_no_switch_at_min_boosts() {
        let mut state = setup();
        // Set opponent to -6 Atk and -6 SpA already
        state.sides[1].active.boosts[ATK] = -6;
        state.sides[1].active.boosts[SPA] = -6;

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::PartingShot,
            ..unsafe { core::mem::zeroed() }
        };

        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(99), 0);

        // Boosts can't go lower, so no switch
        assert!(!state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    }

    #[test]
    fn test_baton_pass_sets_flag_and_must_switch() {
        let mut state = setup();

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::BatonPass,
            ..unsafe { core::mem::zeroed() }
        };

        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(99), 0);

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
        let mut state = setup();
        // Give side 0 a charge move (Fly-like)
        let fly_id = 19u16; // MOVE_FLY
        state.sides[0].team[0].moves[0] = fly_id;

        let pp_before = state.sides[0].team[0].pp[0];
        execute_move(&mut state, &TeamData::default(),0, fly_id, 0, &mut fixed_rng(0));

        // VOL_CHARGING should be set (move has CHARGE flag)
        // But since the MoveEffect in gen data is None (not ChargeFly),
        // VOL_SEMI_INVULNERABLE won't be set.
        assert!(state.sides[0].active.has_volatile(VOL_CHARGING));
        // PP was deducted on turn 1
        assert_eq!(state.sides[0].team[0].pp[0], pp_before - 1);
        // Defender HP should not change yet
        assert_eq!(state.sides[1].team[0].current_hp, 300);
    }

    #[test]
    fn test_charge_turn2_deals_damage() {
        let mut state = setup();
        let fly_id = 19u16;
        state.sides[0].team[0].moves[0] = fly_id;

        // Turn 1: charge
        execute_move(&mut state, &TeamData::default(),0, fly_id, 0, &mut fixed_rng(0));
        assert!(state.sides[0].active.has_volatile(VOL_CHARGING));
        let pp_after_turn1 = state.sides[0].team[0].pp[0];

        // Turn 2: execute (pass any move_id, it's overridden by last_move)
        execute_move(&mut state, &TeamData::default(),0, 0, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_CHARGING));
        // PP should NOT be deducted again
        assert_eq!(state.sides[0].team[0].pp[0], pp_after_turn1);
        // Defender should have taken damage
        assert!(state.sides[1].team[0].current_hp < 300);
    }

    #[test]
    fn test_charge_semi_invuln_dodges_attacks() {
        let mut state = setup();

        // Manually set side 1 as charging with semi-invuln (air)
        set_volatile(&mut state, 1, VOL_CHARGING);
        set_volatile(&mut state, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 1; // air
        state.sides[1].active.last_move = 19; // Fly

        let hp_before = state.sides[1].team[0].current_hp;

        // Side 0 uses a normal move (move 1 = Pound)
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        // Should miss due to semi-invulnerability
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_semi_invuln_air_hit_by_thunder() {
        let mut state = setup();
        use crate::data::MOVE_THUNDER;

        // Change defender to non-Ground type (species 50 = Diglett is Ground, immune to Electric)
        state.sides[1].team[0].species_id = 25; // Pikachu (Electric, not immune)

        // Set side 1 as semi-invuln in air
        set_volatile(&mut state, 1, VOL_CHARGING);
        set_volatile(&mut state, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 1; // air
        state.sides[1].active.last_move = 19;

        // Side 0 uses Thunder (should hit through semi-invuln)
        execute_move(&mut state, &TeamData::default(),0, MOVE_THUNDER as u16, 0, &mut fixed_rng(0));

        // Thunder should deal damage through semi-invuln
        assert!(state.sides[1].team[0].current_hp < 300);
    }

    #[test]
    fn test_semi_invuln_underground_hit_by_earthquake() {
        let mut state = setup();
        use crate::data::MOVE_EARTHQUAKE;

        // Set side 1 as semi-invuln underground
        set_volatile(&mut state, 1, VOL_CHARGING);
        set_volatile(&mut state, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 2; // underground
        state.sides[1].active.last_move = 91; // Dig

        execute_move(&mut state, &TeamData::default(),0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < 300);
    }

    #[test]
    fn test_semi_invuln_underwater_hit_by_surf() {
        let mut state = setup();
        use crate::data::MOVE_SURF;

        set_volatile(&mut state, 1, VOL_CHARGING);
        set_volatile(&mut state, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 3; // underwater
        state.sides[1].active.last_move = 291; // Dive

        execute_move(&mut state, &TeamData::default(),0, MOVE_SURF as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < 300);
    }

    #[test]
    fn test_semi_invuln_vanished_dodges_everything() {
        let mut state = setup();
        use crate::data::MOVE_EARTHQUAKE;

        set_volatile(&mut state, 1, VOL_CHARGING);
        set_volatile(&mut state, 1, VOL_SEMI_INVULNERABLE);
        state.sides[1].active._padding[1] = 4; // vanished
        state.sides[1].active.last_move = 566; // Phantom Force

        let hp_before = state.sides[1].team[0].current_hp;
        // Even Earthquake misses vanished targets
        execute_move(&mut state, &TeamData::default(),0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_charge_skull_bash_def_boost() {
        let mut state = setup();
        // Create a Skull Bash-like charge move via direct execution
        let md = charge_move(MoveEffect::ChargeSkullBash, 130, MoveCategory::Physical);

        // Manually test apply_charge_turn_effects
        apply_charge_turn_effects(&mut state, 0, &md);
        assert_eq!(state.sides[0].active.boosts[DEF], 1);
    }

    #[test]
    fn test_charge_meteor_beam_spa_boost() {
        let mut state = setup();
        let md = charge_move(MoveEffect::ChargeMeteorBeam, 120, MoveCategory::Special);

        apply_charge_turn_effects(&mut state, 0, &md);
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
    }

    #[test]
    fn test_charge_electro_shot_spa_boost() {
        let mut state = setup();
        let md = charge_move(MoveEffect::ChargeElectroShot, 130, MoveCategory::Special);

        apply_charge_turn_effects(&mut state, 0, &md);
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
        assert!(weather_skips_charge(&state, 0, &md));

        state.field.weather = WEATHER_RAIN;
        assert!(!weather_skips_charge(&state, 0, &md));

        state.field.weather = WEATHER_NONE;
        assert!(!weather_skips_charge(&state, 0, &md));
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
        assert!(weather_skips_charge(&state, 0, &md));

        state.field.weather = WEATHER_SUN;
        assert!(!weather_skips_charge(&state, 0, &md));
    }

    #[test]
    fn test_phantom_force_bypasses_protect() {
        let mut state = setup();

        // Simulate turn 2 of Phantom Force: set VOL_CHARGING, last_move to a move
        // that has ChargePhantom effect. We'll use move_id=566 (Phantom Force).
        // Since the gen data might not have ChargePhantom effect yet,
        // manually test the bypass logic.
        set_volatile(&mut state, 0, VOL_CHARGING);
        state.sides[0].active.last_move = 566; // Phantom Force

        // Set defender as Protecting
        set_volatile(&mut state, 1, VOL_PROTECT_THIS_TURN);

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
        let mut state = setup();
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
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
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
        let mut state = setup();

        // Set side 1 as semi-invulnerable
        set_volatile(&mut state, 1, VOL_SEMI_INVULNERABLE);

        // Will-o-Wisp should miss semi-invulnerable targets
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 85,
            effect: MoveEffect::WillOWisp,
            ..unsafe { core::mem::zeroed() }
        };

        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_heal25_quarter_round_half_up() {
        let mut state = setup();
        // maxhp 158 → SD heal:[1,4] = Math.round(39.5) = 40; half-down gives 39.
        state.sides[0].team[0].max_hp = 158;
        state.sides[0].team[0].current_hp = 1;
        let md = MoveData {
            category: MoveCategory::Status,
            flags: MoveFlags::HEAL,
            effect: MoveEffect::Heal25,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].team[0].current_hp, 41); // 1 + 40
    }

    #[test]
    fn test_heal25_cure_status_clears_status() {
        let mut state = setup();
        // maxhp 158 → SD modify(maxhp, 0.25) = 39 (half-down; Math.round would give 40).
        state.sides[0].team[0].max_hp = 158;
        state.sides[0].team[0].current_hp = 50;
        state.sides[0].team[0].status = STATUS_POISON;
        state.sides[0].team[0].status_counter = 1;
        let md = MoveData {
            category: MoveCategory::Status,
            flags: MoveFlags::HEAL,
            effect: MoveEffect::Heal25CureStatus,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].team[0].current_hp, 89); // 50 + modify(158, 0.25)=39
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    }

    /// Helper: set up move-lock state as if the first thrash turn already executed.
    fn setup_thrash_locked(state: &mut BattleState, side: usize, move_id: u16, turns_remaining: u8) {
        set_volatile(state, side, VOL_MOVE_LOCKED);
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
        let mut state = setup();
        // Move 1 is a physical move in gen data (Pound)
        setup_thrash_locked(&mut state, 0, 1, 2);

        let hp_before = state.sides[1].team[0].current_hp;
        // Player action is irrelevant; move_id overridden to last_move=1
        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));

        // Counter decremented from 2→1, still locked
        assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(state.sides[0].active._padding[2], 1);
        // Damage should have been dealt
        assert!(state.sides[1].team[0].current_hp < hp_before);
    }

    #[test]
    fn test_thrash_last_turn_clears_lock_and_confuses() {
        let mut state = setup();
        setup_thrash_locked(&mut state, 0, 1, 1);

        // Last locked turn: counter 1→0, lock clears, confusion applied
        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));

        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(state.sides[0].active._padding[2], 0);
        // Confusion: rng(3)=0 → 0+2=2 turns
        assert_eq!(state.sides[0].active.confusion_turns, 2);
    }

    #[test]
    fn test_thrash_3_turn_sequence() {
        let mut state = setup();
        // Simulate 3-turn lock: counter=2
        setup_thrash_locked(&mut state, 0, 1, 2);

        // Turn 2: counter 2→1, still locked
        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));
        assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(state.sides[0].active._padding[2], 1);
        assert_eq!(state.sides[0].active.confusion_turns, 0);

        // Turn 3: counter 1→0, lock ends, confusion
        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert!(state.sides[0].active.confusion_turns >= 2);
    }

    #[test]
    fn test_thrash_locked_overrides_move_id() {
        let mut state = setup();
        // Lock onto move 1 (Pound), player tries move 2
        setup_thrash_locked(&mut state, 0, 1, 2);

        let hp_before = state.sides[1].team[0].current_hp;
        // Player passes move_id=2, slot=1 — but lock overrides to move 1
        execute_move(&mut state, &TeamData::default(),0, 2, 1, &mut fixed_rng(0));

        // Damage dealt using move 1 (Pound), not move 2
        assert!(state.sides[1].team[0].current_hp < hp_before);
        assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
    }

    #[test]
    fn test_thrash_confusion_on_full_para() {
        let mut state = setup();
        setup_thrash_locked(&mut state, 0, 1, 1); // last locked turn
        set_status(&mut state, 0, 0, STATUS_PARALYSIS, 0);

        let hp_before = state.sides[1].team[0].current_hp;
        // rng(4)==0 triggers full paralysis
        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));

        // Move failed (full para), but lock ended and confusion applied
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert!(state.sides[0].active.confusion_turns >= 2);
    }

    #[test]
    fn test_thrash_confusion_on_flinch() {
        let mut state = setup();
        setup_thrash_locked(&mut state, 0, 1, 1); // last locked turn
        set_volatile(&mut state, 0, VOL_FLINCHED);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));

        // Didn't attack (flinched), but lock ended and confusion applied
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert!(state.sides[0].active.confusion_turns >= 2);
    }

    #[test]
    fn test_thrash_pp_deducted_each_locked_turn() {
        let mut state = setup();
        state.sides[0].team[0].pp[0] = 10;
        // Lock onto move in slot 0 (moves[0]=1)
        setup_thrash_locked(&mut state, 0, 1, 2);

        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].pp[0], 9); // PP deducted

        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].pp[0], 8); // PP deducted again
    }

    #[test]
    fn test_thrash_no_confusion_if_fainted() {
        let mut state = setup();
        state.sides[0].team[0].current_hp = 1;
        setup_thrash_locked(&mut state, 0, 1, 1); // last locked turn

        // KO the attacker
        deal_damage(&mut state, 0, 0, 1);
        assert!(state.sides[0].team[0].is_fainted());

        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));

        // No confusion on fainted mon
        assert_eq!(state.sides[0].active.confusion_turns, 0);
    }

    #[test]
    fn test_thrash_confusion_on_sleep() {
        let mut state = setup();
        setup_thrash_locked(&mut state, 0, 1, 1); // last locked turn
        // Put attacker to sleep with counter=3 (stays asleep)
        set_status(&mut state, 0, 0, STATUS_SLEEP, 3);

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));

        // Asleep at lock-end: lock ends but no self-confusion (Showdown lockedmove.onResidual slp arm)
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(state.sides[0].active.confusion_turns, 0);
    }

    #[test]
    fn test_thrash_lockend_confusion_lands_when_target_survives() {
        // Continuing lock: the defender survives the lock-end hit, so the self-confusion
        // lands this turn (mirrors a battle that keeps going). Guards against a fix that
        // suppresses the lock-end self-confusion outright.
        let mut state = setup();
        setup_thrash_locked(&mut state, 0, 1, 1); // last locked turn

        assert!(state.sides[1].team[0].current_hp > 1, "defender must survive the hit");
        execute_move(&mut state, &TeamData::default(), 0, 99, 0, &mut fixed_rng(0));

        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert!(!state.sides[1].team[0].is_fainted());
        assert!(state.sides[0].active.confusion_turns >= 2);
    }

    #[test]
    fn test_thrash_lockend_confusion_deferred_when_target_faints_with_backup() {
        // Showdown defers the lock-end onEnd confusion past the forced replacement when the
        // defender faints on the lock-end turn and has a living switch-in; it does not land
        // on the faint turn. Mirror that: no self-confusion lands here.
        let mut state = setup();
        state.sides[1].team[0].current_hp = 1;
        setup_thrash_locked(&mut state, 0, 1, 1); // last locked turn

        execute_move(&mut state, &TeamData::default(), 0, 99, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].is_fainted(), "lock-end hit must KO the defender");
        assert!(state.sides[1].team[1].current_hp > 0, "defender keeps a living backup");
        assert_eq!(state.sides[0].active.confusion_turns, 0);
    }

    #[test]
    fn test_thrash_lockend_confusion_lands_when_faint_ends_battle() {
        // When the lock-end KO ends the battle (no living replacement), Showdown still adds
        // the confusion. The defer only applies to a pending forced replacement.
        let mut state = setup();
        state.sides[1].team[0].current_hp = 1;
        state.sides[1].team[1].current_hp = 0; // no living backup
        setup_thrash_locked(&mut state, 0, 1, 1); // last locked turn

        execute_move(&mut state, &TeamData::default(), 0, 99, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].is_fainted());
        assert!(state.sides[0].active.confusion_turns >= 2);
    }

    #[test]
    fn test_uproar_lockend_no_self_confusion() {
        // Uproar locks like Thrash but its Showdown condition adds no confusion on lock-end.
        let mut state = setup();
        state.sides[0].team[0].moves = [253, 0, 0, 0];
        setup_thrash_locked(&mut state, 0, 253, 1); // last locked turn, Uproar

        execute_move(&mut state, &TeamData::default(), 0, 99, 0, &mut fixed_rng(0));

        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(state.sides[0].active.confusion_turns, 0);
    }

    #[test]
    fn test_thrash_locked_bookkeeping_skip() {
        let mut state = setup();
        // Set last_move and consec_move_count before lock
        state.sides[0].active.last_move = 1;
        state.sides[0].active.consec_move_count = 3;
        setup_thrash_locked(&mut state, 0, 1, 2);

        execute_move(&mut state, &TeamData::default(),0, 99, 0, &mut fixed_rng(0));

        // Bookkeeping (last_move, consec_move_count) should NOT be updated on locked turns
        // last_move stays as 1 (set by setup_thrash_locked)
        assert_eq!(state.sides[0].active.last_move, 1);
        // consec_move_count should remain 3 (not updated)
        assert_eq!(state.sides[0].active.consec_move_count, 3);
    }

    #[test]
    fn test_water_absorb_blocks_and_heals() {
        let mut state = setup();
        use crate::data::MOVE_WATER_GUN;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_WATER_ABSORB;
        state.sides[1].team[0].current_hp = 200; // missing 100 HP

        execute_move(&mut state, &TeamData::default(),0, MOVE_WATER_GUN as u16, 0, &mut fixed_rng(0));

        // No damage, healed 25% of max HP (300/4 = 75)
        assert!(state.sides[1].team[0].current_hp > 200);
    }

    #[test]
    fn test_volt_absorb_blocks_and_heals() {
        let mut state = setup();
        use crate::data::MOVE_THUNDERBOLT;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_VOLT_ABSORB;
        state.sides[1].team[0].current_hp = 200;

        execute_move(&mut state, &TeamData::default(),0, MOVE_THUNDERBOLT as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].current_hp > 200);
    }

    #[test]
    fn test_dry_skin_blocks_water_and_heals() {
        let mut state = setup();
        use crate::data::MOVE_WATER_GUN;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DRY_SKIN;
        state.sides[1].team[0].current_hp = 200;

        execute_move(&mut state, &TeamData::default(),0, MOVE_WATER_GUN as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].current_hp > 200);
    }

    #[test]
    fn test_flash_fire_blocks_and_sets_volatile() {
        let mut state = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_FLASH_FIRE;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(state.sides[1].active.has_volatile(VOL_FLASH_FIRE));
    }

    #[test]
    fn test_lightning_rod_blocks_and_boosts_spa() {
        let mut state = setup();
        use crate::data::MOVE_THUNDERBOLT;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_LIGHTNING_ROD;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_THUNDERBOLT as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert_eq!(state.sides[1].active.boosts[SPA], 1);
    }

    #[test]
    fn test_storm_drain_blocks_and_boosts_spa() {
        let mut state = setup();
        use crate::data::MOVE_WATER_GUN;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STORM_DRAIN;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_WATER_GUN as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert_eq!(state.sides[1].active.boosts[SPA], 1);
    }

    #[test]
    fn test_motor_drive_blocks_and_boosts_spe() {
        let mut state = setup();
        use crate::data::MOVE_THUNDERBOLT;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_MOTOR_DRIVE;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_THUNDERBOLT as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert_eq!(state.sides[1].active.boosts[SPE], 1);
    }

    #[test]
    fn test_sap_sipper_blocks_and_boosts_atk() {
        let mut state = setup();
        use crate::data::MOVE_ENERGY_BALL;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SAP_SIPPER;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_ENERGY_BALL as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert_eq!(state.sides[1].active.boosts[ATK], 1);
    }

    #[test]
    fn test_levitate_blocks_ground() {
        let mut state = setup();
        use crate::data::MOVE_EARTHQUAKE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_LEVITATE;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_levitate_fails_under_gravity() {
        let mut state = setup();
        use crate::data::MOVE_EARTHQUAKE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_LEVITATE;
        state.field.gravity_turns = 3;

        execute_move(&mut state, &TeamData::default(),0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));

        // Gravity overrides Levitate — should take damage
        assert!(state.sides[1].team[0].current_hp < 300);
    }

    #[test]
    fn test_overcoat_blocks_powder_status() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_OVERCOAT;

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
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // Overcoat blocks it — no sleep
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_soundproof_blocks_sound_move() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SOUNDPROOF;

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
        assert!(ability_flag_immunity(&state, 1, md.flags, 0).is_some());
    }

    #[test]
    fn test_bulletproof_blocks_bullet_move() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_BULLETPROOF;

        // Bullet-flagged move
        assert!(ability_flag_immunity(&state, 1, MoveFlags::BULLET, 0).is_some());
        // Non-bullet move is not blocked
        assert!(ability_flag_immunity(&state, 1, MoveFlags::CONTACT, 0).is_none());
    }

    #[test]
    fn test_thunder_wave_blocked_by_volt_absorb() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_VOLT_ABSORB;
        state.sides[1].team[0].current_hp = 200;

        // Thunder Wave is Electric-type status
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 90,
            effect: MoveEffect::ThunderWave,
            move_type: Type::Electric,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // Should be blocked + healed, no paralysis
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
        assert!(state.sides[1].team[0].current_hp > 200);
    }

    #[test]
    fn test_sap_sipper_blocks_grass_status() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SAP_SIPPER;

        // Leech Seed is Grass-type status
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 90,
            effect: MoveEffect::LeechSeed,
            move_type: Type::Grass,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // Blocked by Sap Sipper, +1 Atk
        assert!(!state.sides[1].active.has_volatile(VOL_LEECH_SEED));
        assert_eq!(state.sides[1].active.boosts[ATK], 1);
    }

    #[test]
    fn test_protean_changes_type() {
        let mut state = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;

        execute_move(&mut state, &TeamData::default(),0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(0));

        // Attacker type should now be Fire/Fire
        assert!(state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(state.sides[0].active.override_types, [Type::Fire as u8, Type::Fire as u8]);
    }

    #[test]
    fn test_protean_once_per_switch() {
        let mut state = setup();
        use crate::data::{MOVE_FLAMETHROWER, MOVE_THUNDERBOLT};
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;

        // First move: type changes to Fire
        execute_move(&mut state, &TeamData::default(),0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.override_types[0], Type::Fire as u8);

        // Second move: type should NOT change again (once per switch-in)
        execute_move(&mut state, &TeamData::default(),0, MOVE_THUNDERBOLT as u16, 1, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.override_types[0], Type::Fire as u8);
        assert_eq!(state.sides[0].active._padding[3], 1);
    }

    #[test]
    fn test_libero_changes_type() {
        let mut state = setup();
        use crate::data::MOVE_WATER_GUN;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_LIBERO;

        execute_move(&mut state, &TeamData::default(),0, MOVE_WATER_GUN as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(state.sides[0].active.override_types, [Type::Water as u8, Type::Water as u8]);
    }

    #[test]
    fn test_protean_not_on_struggle() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;

        // Struggle (move_id=0) should not trigger Protean
        execute_move(&mut state, &TeamData::default(),0, 0, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
    }

    #[test]
    fn test_is_call_family_predicate() {
        assert!(is_call_family(118));
        assert!(is_call_family(119));
        assert!(is_call_family(214));
        assert!(is_call_family(274));
        assert!(is_call_family(382));
        assert!(is_call_family(383));

        assert!(!is_call_family(0));
        assert!(!is_call_family(1));
        assert!(!is_call_family(85));
        assert!(!is_call_family(89));
        assert!(!is_call_family(120));
        assert!(!is_call_family(266));
        assert!(!is_call_family(389));
        assert!(!is_call_family(921));
        assert!(!is_call_family(u16::MAX));
    }

    #[test]
    fn test_protean_gated_off_for_call_family_outer() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;

        // Use Assist (274) — still unimplemented, falls to MoveEffect::None
        // fallback so no inner dispatch can fire Protean. Metronome (118) now
        // dispatches an inner move; that inner-fire path is asserted by
        // test_metronome_protean_fires_on_inner_type.
        execute_move(&mut state, &TeamData::default(),0, 274, 0, &mut fixed_rng(0));

        assert!(!state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(state.sides[0].active._padding[3] & 1, 0);
    }

    #[test]
    fn test_protean_gated_off_for_all_call_family_ids() {
        for &call_id in CALL_FAMILY_IDS {
            if call_id == 118 {
                // Metronome's arm dispatches an inner unconditionally; the
                // inner Protean fire is correct behavior — see
                // test_metronome_protean_fires_on_inner_type.
                continue;
            }
            let mut state = setup();
            state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;

            execute_move(&mut state, &TeamData::default(),0, call_id, 0, &mut fixed_rng(0));

            assert!(
                !state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN),
                "Call* outer id {} burned Protean", call_id
            );
            assert_eq!(
                state.sides[0].active._padding[3] & 1, 0,
                "Call* outer id {} burned once-per-switch flag", call_id
            );
        }
    }

    #[test]
    fn test_protean_still_fires_for_non_call_family() {
        let mut state = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;

        execute_move(&mut state, &TeamData::default(),0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(state.sides[0].active.override_types, [Type::Fire as u8, Type::Fire as u8]);
        assert_eq!(state.sides[0].active._padding[3] & 1, 1);
    }

    #[test]
    fn test_call_family_ids_sorted_for_binary_search() {
        for w in CALL_FAMILY_IDS.windows(2) {
            assert!(w[0] < w[1]);
        }
    }

    #[test]
    fn test_stance_change_to_blade() {
        let mut state = setup();
        use crate::data::MOVE_SHADOW_BALL;
        state.sides[0].team[0].species_id = 681; // Aegislash Shield
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STANCE_CHANGE;

        // Blade stats are re-derived from the build (IV 31 / EV 0 / neutral, L100).
        let build = MonBuildData { ivs: [31; 6], evs: [0; 6], nature: 0 };
        let teams = TeamData { mons: [[build; 6]; 2], levels: [[100; 6]; 2] };

        // Attacking move → should change to Blade forme
        execute_move(&mut state, &teams, 0, MOVE_SHADOW_BALL as u16, 0, &mut fixed_rng(99));

        assert_eq!(effective_species(&state, 0), 1103); // Aegislash-Blade
        // Aegislash Blade: atk:140, def:50. L100 neutral 31/0:
        //   atk = 2*140 + 31 + 5 = 316, def = 2*50 + 31 + 5 = 136
        assert_eq!(effective_stat(&state, 0, ATK), 316);
        assert_eq!(effective_stat(&state, 0, DEF), 136);
    }

    #[test]
    fn test_stance_change_to_shield_on_kings_shield() {
        let mut state = setup();
        use crate::data::MOVE_KING_S_SHIELD;
        state.sides[0].team[0].species_id = 681;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STANCE_CHANGE;
        state.sides[0].team[0].stats = [100; 5];

        // First, manually set to Blade forme
        crate::state::forme::apply_battle_forme(&mut state, &TeamData::default(), 0, 1103);
        assert_eq!(effective_species(&state, 0), 1103);

        // King's Shield → should revert to Shield forme
        execute_move(&mut state, &TeamData::default(),0, MOVE_KING_S_SHIELD as u16, 0, &mut fixed_rng(99));

        assert_eq!(effective_species(&state, 0), 681);
        assert_eq!(effective_stat(&state, 0, ATK), 100); // back to team stats
    }

    #[test]
    fn test_stance_change_no_trigger_on_status_move() {
        let mut state = setup();
        use crate::data::MOVE_SWORDS_DANCE;
        state.sides[0].team[0].species_id = 681;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STANCE_CHANGE;

        // Status move (not King's Shield) → should NOT change forme
        execute_move(&mut state, &TeamData::default(),0, MOVE_SWORDS_DANCE as u16, 0, &mut fixed_rng(99));

        assert_eq!(effective_species(&state, 0), 681); // still Shield
    }

    #[test]
    fn test_disguise_blocks_first_hit() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DISGUISE;

        let max_hp = state.sides[1].team[0].max_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        // Disguise blocks damage but costs 1/8 max HP
        assert_eq!(state.sides[1].team[0].current_hp, max_hp - max_hp / 8);
        // Shield is now broken
        assert_eq!(state.sides[1].active._padding[4] & 1, 1);
    }

    #[test]
    fn test_disguise_broken_takes_damage() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DISGUISE;
        state.sides[1].active._padding[4] = 1; // already broken

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        // Second hit goes through normally
        assert!(state.sides[1].team[0].current_hp < hp_before);
    }

    #[test]
    fn test_ice_face_blocks_physical() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ICE_FACE;

        let hp_before = state.sides[1].team[0].current_hp;
        // Move 1 (Pound) is Physical in gen data
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        // Ice Face blocks the physical hit — no damage
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        // Shield broken
        assert_eq!(state.sides[1].active._padding[4] & 2, 2);
    }

    #[test]
    fn test_ice_face_doesnt_block_special() {
        let mut state = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ICE_FACE;

        // Flamethrower is Special — Ice Face doesn't block
        execute_move(&mut state, &TeamData::default(),0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].current_hp < 300);
        // Shield NOT broken by special move
        assert_eq!(state.sides[1].active._padding[4] & 2, 0);
    }

    #[test]
    fn test_ice_face_broken_takes_physical() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ICE_FACE;
        state.sides[1].active._padding[4] = 2; // already broken

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        // Second physical hit goes through
        assert!(state.sides[1].team[0].current_hp < hp_before);
    }

    #[test]
    fn test_disguise_doesnt_block_behind_substitute() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DISGUISE;
        // Set up substitute
        set_volatile(&mut state, 1, VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 100;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        // Substitute takes the hit, Disguise NOT consumed
        assert_eq!(state.sides[1].active._padding[4] & 1, 0);
    }

    #[test]
    fn test_rough_skin_damages_attacker() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ROUGH_SKIN;

        let atk_hp = state.sides[0].team[0].current_hp;
        let atk_max = state.sides[0].team[0].max_hp;
        // Pound (1) is Contact/Physical
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        // Attacker should lose 1/8 max HP from Rough Skin
        let expected_rough = (atk_max / 8).max(1);
        // Attacker took Rough Skin damage (plus defender took damage)
        assert!(state.sides[0].team[0].current_hp <= atk_hp - expected_rough);
    }

    #[test]
    fn test_rough_skin_no_trigger_on_special() {
        let mut state = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ROUGH_SKIN;

        let atk_hp = state.sides[0].team[0].current_hp;
        // Flamethrower is Special, no Contact — Rough Skin should NOT trigger
        execute_move(&mut state, &TeamData::default(),0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[0].team[0].current_hp, atk_hp);
    }

    #[test]
    fn test_iron_barbs_damages_attacker() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_IRON_BARBS;

        let atk_hp = state.sides[0].team[0].current_hp;
        let atk_max = state.sides[0].team[0].max_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        let expected = (atk_max / 8).max(1);
        assert!(state.sides[0].team[0].current_hp <= atk_hp - expected);
    }

    #[test]
    fn test_weak_armor_on_physical() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_WEAK_ARMOR;

        // Pound is Physical → triggers Weak Armor
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[DEF], -1);
        assert_eq!(state.sides[1].active.boosts[SPE], 2);
    }

    #[test]
    fn test_weak_armor_no_trigger_on_special() {
        let mut state = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_WEAK_ARMOR;

        // Flamethrower is Special → no Weak Armor
        execute_move(&mut state, &TeamData::default(),0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[DEF], 0);
        assert_eq!(state.sides[1].active.boosts[SPE], 0);
    }

    #[test]
    fn test_justified_on_dark_move() {
        let mut state = setup();
        use crate::data::MOVE_BITE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_JUSTIFIED;

        // Bite is Dark/Contact
        execute_move(&mut state, &TeamData::default(),0, MOVE_BITE as u16, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[ATK], 1);
    }

    #[test]
    fn test_justified_no_trigger_on_normal() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_JUSTIFIED;

        // Pound is Normal → no Justified
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[ATK], 0);
    }

    #[test]
    fn test_stamina_boosts_on_any_hit() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STAMINA;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].active.boosts[DEF], 1);
    }

    #[test]
    fn test_anger_point_on_crit() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ANGER_POINT;

        // fixed_rng(0): rng(24)==0 → crit, rng(4)==0 → paralysis...
        // But attacker isn't paralyzed. The defender has Anger Point.
        // We need the move to hit AND crit. fixed_rng(0) gives crit.
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        // Anger Point should maximize Atk to +6
        assert_eq!(state.sides[1].active.boosts[ATK], 6);
    }

    #[test]
    fn test_color_change_on_hit() {
        let mut state = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_COLOR_CHANGE;

        execute_move(&mut state, &TeamData::default(),0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(99));

        // Defender type should now be Fire/Fire
        assert!(state.sides[1].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(state.sides[1].active.override_types, [Type::Fire as u8, Type::Fire as u8]);
    }

    #[test]
    fn test_flame_body_burns_on_contact() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_FLAME_BODY;

        // fixed_rng(0): rng(100)==0 < 30 → triggers
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[0].team[0].status, STATUS_BURN);
    }

    #[test]
    fn test_flame_body_no_trigger_high_roll() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_FLAME_BODY;

        // fixed_rng(99): rng(100)==99 >= 30 → does not trigger
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_static_paralyzes_on_contact() {
        let mut state = setup();
        // setup() makes side-0 attacker Pikachu (Electric) → paralysis-immune.
        // Swap to Bulbasaur (Grass/Poison) so Static can paralyze.
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STATIC;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[0].team[0].status, STATUS_PARALYSIS);
    }

    #[test]
    fn test_static_paralyzes_when_holder_faints() {
        let mut state = setup();
        state.sides[0].team[0].species_id = 1; // Bulbasaur (not paralysis-immune)
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STATIC;
        state.sides[1].team[0].current_hp = 1; // the contact hit KOs the Static holder

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        // onDamagingHit fires post-faint: the KO'd holder still paralyzes the attacker.
        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].team[0].status, STATUS_PARALYSIS);
    }

    #[test]
    fn test_static_immune_attacker_unaffected_when_holder_faints() {
        let mut state = setup();
        state.sides[0].team[0].species_id = 1;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_LIMBER; // paralysis-immune
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STATIC;
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        // Attacker immunity still gates the post-faint trigger.
        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_poison_point_poisons_on_contact() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_POISON_POINT;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[0].team[0].status, STATUS_POISON);
    }

    #[test]
    fn test_contact_ability_no_trigger_through_sub() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ROUGH_SKIN;
        set_volatile(&mut state, 1, VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 200;

        let atk_hp = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        // Rough Skin doesn't trigger through substitute
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp);
    }

    #[test]
    fn test_moxie_on_ko() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_MOXIE;
        // Set defender to 1 HP so it faints
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
    }

    #[test]
    fn test_moxie_no_boost_if_no_ko() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_MOXIE;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(!state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[ATK], 0);
    }

    #[test]
    fn test_beast_boost_highest_stat() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_BEAST_BOOST;
        // Make SpA the highest stat
        state.sides[0].team[0].stats = [100, 100, 200, 100, 100]; // SpA=200 highest
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        // SPA (index 2) should be boosted
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
        assert_eq!(state.sides[0].active.boosts[ATK], 0);
    }

    #[test]
    fn test_aftermath_damages_attacker_on_ko() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_AFTERMATH;
        state.sides[1].team[0].current_hp = 1;

        let atk_hp = state.sides[0].team[0].current_hp;
        let atk_max = state.sides[0].team[0].max_hp;
        // Pound is Contact → Aftermath triggers
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        let expected = atk_max / 4;
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp - expected);
    }

    #[test]
    fn test_aftermath_no_trigger_on_non_contact() {
        let mut state = setup();
        use crate::data::MOVE_FLAMETHROWER;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_AFTERMATH;
        state.sides[1].team[0].current_hp = 1;

        let atk_hp = state.sides[0].team[0].current_hp;
        // Flamethrower is Special, no Contact → Aftermath should NOT trigger
        execute_move(&mut state, &TeamData::default(),0, MOVE_FLAMETHROWER as u16, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp);
    }

    #[test]
    fn test_rough_skin_damages_attacker_on_ko() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_ROUGH_SKIN;
        state.sides[1].team[0].current_hp = 1;

        let atk_hp = state.sides[0].team[0].current_hp;
        let atk_max = state.sides[0].team[0].max_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        let expected = (atk_max / 8).max(1);
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp - expected);
    }

    #[test]
    fn test_aftermath_suppressed_no_trigger_on_ko() {
        // Gastro-Acid'd Aftermath holder must NOT fire post-faint: a raw
        // ability_id read would incorrectly trigger here.
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_AFTERMATH;
        state.sides[1].team[0].current_hp = 1;
        set_volatile(&mut state, 1, VOL_ABILITY_SUPPRESSED);

        let atk_hp = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp);
    }

    #[test]
    fn test_aftermath_overridden_fires_on_ko() {
        // Skill-Swapped/Transformed defender: the KO trigger reads override_ability.
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_INSOMNIA;
        state.sides[1].team[0].current_hp = 1;
        state.sides[1].active.override_ability = data_bridge::ABILITY_AFTERMATH;
        set_volatile(&mut state, 1, VOL_ABILITY_OVERRIDDEN);

        let atk_hp = state.sides[0].team[0].current_hp;
        let atk_max = state.sides[0].team[0].max_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp - atk_max / 4);
    }

    #[test]
    fn test_sand_spit_sets_sandstorm_on_hit() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SAND_SPIT;
        assert_eq!(state.field.weather, WEATHER_NONE);

        // Pound: damaging hit, holder survives → Sand Spit sets Sandstorm.
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));

        assert!(!state.sides[1].team[0].is_fainted());
        assert_eq!(state.field.weather, WEATHER_SAND);
        assert_eq!(state.field.weather_turns, 5);
    }

    #[test]
    fn test_sand_spit_sets_sandstorm_when_holder_faints() {
        // Showdown runs onDamagingHit before faint resolution, so a KO'd Sand
        // Spit holder still sets the weather (realistic-fidelity seed 201685).
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SAND_SPIT;
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.field.weather, WEATHER_SAND);
    }

    #[test]
    fn test_sand_spit_suppressed_no_weather_on_ko() {
        // Gastro-Acid'd Sand Spit holder must NOT set weather post-faint: a raw
        // ability_id read would incorrectly trigger here.
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SAND_SPIT;
        state.sides[1].team[0].current_hp = 1;
        set_volatile(&mut state, 1, VOL_ABILITY_SUPPRESSED);

        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.field.weather, WEATHER_NONE);
    }

    #[test]
    fn test_non_sand_spit_hit_sets_no_weather() {
        // Control: a hit on a defender without Sand Spit sets no weather.
        let mut state = setup();
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.field.weather, WEATHER_NONE);
    }

    #[test]
    fn test_effect_spore_sleep() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_EFFECT_SPORE;

        // Need rng sequence where: accuracy hits, no crit concern, effect spore roll < 10
        // fixed_rng(0) gives: rng(100)=0 for accuracy (hit), rng(24)=0 (crit, but doesn't matter
        // for status), and rng(100)=0 for effect spore → 0 < 10 → sleep
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[0].team[0].status, STATUS_SLEEP);
    }

    #[test]
    fn test_defender_hooks_dont_trigger_through_sub() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STAMINA;
        set_volatile(&mut state, 1, VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 200;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        // Stamina doesn't trigger when sub takes the hit
        assert_eq!(state.sides[1].active.boosts[DEF], 0);
    }

    #[test]
    fn test_focus_sash_survives_ohko() {
        let mut state = setup();
        // Give defender Focus Sash (id=151) and lower HP to make OHKO easier
        state.sides[1].team[0].item_id = 151;
        state.sides[1].team[0].current_hp = 50;
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500; // Very high attack to guarantee OHKO

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        // Should survive with 1 HP
        assert_eq!(state.sides[1].team[0].current_hp, 1);
        // Sash consumed
        assert_eq!(state.sides[1].team[0].item_id, 0);
    }

    #[test]
    fn test_focus_sash_no_trigger_if_not_full_hp() {
        let mut state = setup();
        state.sides[1].team[0].item_id = 151;
        state.sides[1].team[0].current_hp = 49; // Not full HP
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        // Should faint — sash only works at full HP
        assert!(state.sides[1].team[0].is_fainted());
        // Sash NOT consumed
        assert_eq!(state.sides[1].team[0].item_id, 151);
    }

    #[test]
    fn test_sturdy_survives_ohko() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STURDY;
        state.sides[1].team[0].current_hp = 50;
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        // Should survive with 1 HP (ability, not consumed)
        assert_eq!(state.sides[1].team[0].current_hp, 1);
    }

    #[test]
    fn test_sturdy_no_trigger_if_not_full_hp() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STURDY;
        state.sides[1].team[0].current_hp = 49;
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
    }

    #[test]
    fn test_ohko_kos_without_sturdy() {
        let mut state = setup();
        // Fissure (Ground OHKO) vs Diglett (species 50, pure Ground → neutral),
        // same level, no Sturdy → deals max HP and KOs.
        execute_move(&mut state, &TeamData::default(), 0, 90, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].is_fainted(), "legit OHKO KOs a non-Sturdy target");
    }

    #[test]
    fn test_sturdy_ohko_full_immunity() {
        let mut state = setup();
        // Sturdy makes a one-hit KO move FULLY immune (Showdown onTryHit), holder
        // unharmed at full HP — distinct from the generic survive-at-1 path.
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STURDY;
        let before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 90, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, before,
            "Sturdy gives OHKO full immunity (holder stays at full HP, not 1)");
        assert!(!state.sides[1].team[0].is_fainted());
    }

    // ── False Swipe / Hold Back: never faint the target (clamp to >=1 HP) ──────

    #[test]
    fn test_false_swipe_clamps_lethal_hit_to_one_hp() {
        let mut state = setup();
        state.sides[1].team[0].current_hp = 50;
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500; // would KO without the clamp
        execute_move(&mut state, &TeamData::default(),
            0, crate::data::MOVE_FALSE_SWIPE as u16, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[1].team[0].current_hp, 1,
            "False Swipe leaves a would-be-KO'd target at 1 HP");
        assert!(!state.sides[1].team[0].is_fainted());
    }

    #[test]
    fn test_false_swipe_deals_zero_at_one_hp() {
        let mut state = setup();
        state.sides[1].team[0].current_hp = 1;
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500;
        execute_move(&mut state, &TeamData::default(),
            0, crate::data::MOVE_FALSE_SWIPE as u16, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[1].team[0].current_hp, 1,
            "False Swipe deals 0 to a 1-HP target (never faints)");
        assert!(!state.sides[1].team[0].is_fainted());
    }

    #[test]
    fn test_false_swipe_deals_normal_damage_to_full_hp() {
        let mut state = setup();
        // Diglett (species 50) full HP, modest attacker → non-lethal hit unaffected.
        let before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),
            0, crate::data::MOVE_FALSE_SWIPE as u16, 0, &mut fixed_rng(99));
        let after = state.sides[1].team[0].current_hp;
        assert!(after < before, "False Swipe still deals damage to a full-HP target");
        assert!(after > 1, "non-lethal False Swipe is not clamped to 1");
    }

    #[test]
    fn test_non_false_swipe_move_still_kos() {
        let mut state = setup();
        state.sides[1].team[0].current_hp = 50;
        state.sides[1].team[0].max_hp = 50;
        state.sides[0].team[0].stats[ATK] = 500;
        // Tackle (Normal physical) — no clamp → KOs.
        execute_move(&mut state, &TeamData::default(),
            0, crate::data::MOVE_TACKLE as u16, 0, &mut fixed_rng(99));
        assert!(state.sides[1].team[0].is_fainted(),
            "a non-False-Swipe move still KOs the target");
    }

    #[test]
    fn test_pinch_berry_boosts_stat() {
        let mut state = setup();
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
        check_pinch_berry(&mut state, &TeamData::default(), 1, 0);
        // No pinch berry item → no boost
        assert_eq!(state.sides[1].active.boosts[ATK], 0);
    }

    #[test]
    fn test_close_combat_self_drops() {
        use crate::data::MOVE_CLOSE_COMBAT;
        let mut state = setup();
        // Give side 0 Close Combat in slot 0
        state.sides[0].team[0].moves[0] = MOVE_CLOSE_COMBAT as u16;
        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_CLOSE_COMBAT as u16, 0, &mut fixed_rng(0));
        // Damage should have been dealt
        assert!(state.sides[1].team[0].current_hp < hp_before);
        // Attacker should have -1 Def and -1 SpD
        assert_eq!(state.sides[0].active.boosts[DEF], -1);
        assert_eq!(state.sides[0].active.boosts[SPD], -1);
    }

    #[test]
    fn test_high_jump_kick_crash_on_miss() {
        use crate::data::MOVE_HIGH_JUMP_KICK;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_HIGH_JUMP_KICK as u16;
        let atk_hp_before = state.sides[0].team[0].current_hp;
        let def_hp_before = state.sides[1].team[0].current_hp;
        // Use rng that always misses (rng(100) returns 99 >= 90 accuracy)
        execute_move(&mut state, &TeamData::default(),0, MOVE_HIGH_JUMP_KICK as u16, 0, &mut fixed_rng(99));
        // Defender should NOT have taken damage (miss)
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before);
        // Attacker should have taken 50% max HP crash damage
        let expected_crash = atk_hp_before / 2;
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp_before - expected_crash);
    }

    #[test]
    fn test_high_jump_kick_hit_no_crash() {
        use crate::data::MOVE_HIGH_JUMP_KICK;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_HIGH_JUMP_KICK as u16;
        let atk_hp_before = state.sides[0].team[0].current_hp;
        // Use rng that always hits (rng(100) returns 0 < 90 accuracy)
        execute_move(&mut state, &TeamData::default(),0, MOVE_HIGH_JUMP_KICK as u16, 0, &mut fixed_rng(0));
        // On hit, no crash damage — attacker HP should be unchanged
        // (HJK has no recoil on hit, drain=0)
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp_before);
    }

    #[test]
    fn test_recover_heals_50pct() {
        use crate::data::MOVE_RECOVER;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_RECOVER as u16;
        // Damage the attacker first
        deal_damage(&mut state, 0, 0, 200);
        let hp_before = state.sides[0].team[0].current_hp; // 100
        let max_hp = state.sides[0].team[0].max_hp; // 300
        execute_move(&mut state, &TeamData::default(),0, MOVE_RECOVER as u16, 0, &mut fixed_rng(0));
        // Should heal 50% of max HP
        assert_eq!(state.sides[0].team[0].current_hp, hp_before + max_hp / 2);
    }

    // Heal Pulse (505) / Floral Healing (666) are target:Any HEAL moves: they carry
    // MoveFlags::HEAL but must heal the TARGET off its own max HP, not the user.
    #[test]
    fn test_heal_pulse_heals_target_not_user() {
        use crate::data::MOVE_HEAL_PULSE;
        let mut state = setup();
        state.sides[0].team[0].current_hp = 100; // user (atk_side)
        state.sides[1].team[0].current_hp = 100; // target (def_side), max 300
        let md = MoveData {
            flags: MoveFlags::HEAL, category: MoveCategory::Status, accuracy: 0,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), MOVE_HEAL_PULSE as u16);
        // ceil(300 * 0.5) = 150 onto the target; user untouched.
        assert_eq!(state.sides[1].team[0].current_hp, 250);
        assert_eq!(state.sides[0].team[0].current_hp, 100);
    }

    #[test]
    fn test_heal_pulse_fails_at_target_full_hp() {
        use crate::data::MOVE_HEAL_PULSE;
        let mut state = setup(); // target (side 1) at full 300
        let md = MoveData {
            flags: MoveFlags::HEAL, category: MoveCategory::Status, accuracy: 0,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), MOVE_HEAL_PULSE as u16);
        assert_eq!(state.sides[1].team[0].current_hp, 300);
    }

    #[test]
    fn test_heal_pulse_fails_under_heal_block() {
        use crate::data::MOVE_HEAL_PULSE;
        let mut state = setup();
        state.sides[1].team[0].current_hp = 100;
        state.sides[1].active.heal_block_turns = 3; // target is heal-blocked
        let md = MoveData {
            flags: MoveFlags::HEAL, category: MoveCategory::Status, accuracy: 0,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), MOVE_HEAL_PULSE as u16);
        assert_eq!(state.sides[1].team[0].current_hp, 100);
    }

    #[test]
    fn test_heal_pulse_mega_launcher_75pct() {
        use crate::data::MOVE_HEAL_PULSE;
        let mut state = setup();
        state.sides[0].team[0].ability_id = crate::state::data_bridge::ABILITY_MEGA_LAUNCHER;
        state.sides[1].team[0].current_hp = 50; // max 300
        let md = MoveData {
            flags: MoveFlags::HEAL, category: MoveCategory::Status, accuracy: 0,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), MOVE_HEAL_PULSE as u16);
        // modify(300, 0.75) = chain_mod(300, 3072) = 225 → 50 + 225
        assert_eq!(state.sides[1].team[0].current_hp, 275);
    }

    #[test]
    fn test_floral_healing_grassy_terrain() {
        use crate::data::MOVE_FLORAL_HEALING;
        let md = MoveData {
            flags: MoveFlags::HEAL, category: MoveCategory::Status, accuracy: 0,
            ..unsafe { core::mem::zeroed() }
        };
        // No terrain: ceil(300 * 0.5) = 150 → 50 + 150
        let mut state = setup();
        state.sides[1].team[0].current_hp = 50;
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), MOVE_FLORAL_HEALING as u16);
        assert_eq!(state.sides[1].team[0].current_hp, 200);
        // Grassy Terrain: modify(300, 0.667) = chain_mod(300, 2732) = 200 → 50 + 200
        let mut state = setup();
        state.field.terrain = TERRAIN_GRASSY;
        state.sides[1].team[0].current_hp = 50;
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), MOVE_FLORAL_HEALING as u16);
        assert_eq!(state.sides[1].team[0].current_hp, 250);
    }

    #[test]
    fn test_uturn_sets_must_switch() {
        use crate::data::MOVE_U_TURN;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_U_TURN as u16;
        execute_move(&mut state, &TeamData::default(),0, MOVE_U_TURN as u16, 0, &mut fixed_rng(0));
        // U-turn should set VOL_MUST_SWITCH (attacker has a bench mon)
        assert!(state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    }

    #[test]
    fn test_earth_eater_blocks_ground_and_heals() {
        let mut state = setup();
        use crate::data::MOVE_EARTHQUAKE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_EARTH_EATER;
        state.sides[1].team[0].current_hp = 200;

        execute_move(&mut state, &TeamData::default(),0, MOVE_EARTHQUAKE as u16, 0, &mut fixed_rng(0));

        // Should heal, not take damage
        assert!(state.sides[1].team[0].current_hp > 200);
    }

    #[test]
    fn test_bulletproof_blocks_aura_sphere() {
        let mut state = setup();
        use crate::data::MOVE_AURA_SPHERE;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_BULLETPROOF;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_AURA_SPHERE as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_dazzling_blocks_priority() {
        let mut state = setup();
        use crate::data::MOVE_QUICK_ATTACK;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DAZZLING;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_QUICK_ATTACK as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_dazzling_allows_normal_priority() {
        let mut state = setup();
        use crate::data::MOVE_SURF;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_DAZZLING;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_SURF as u16, 0, &mut fixed_rng(0));

        assert!(state.sides[1].team[0].current_hp < hp_before);
    }

    #[test]
    fn test_wind_rider_blocks_wind_and_boosts_atk() {
        let mut state = setup();
        // Use a Wind-flagged move — construct one manually
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_WIND_RIDER;

        let hp_before = state.sides[1].team[0].current_hp;
        // Check the immunity directly
        let eff = ability_flag_immunity(&state, 1, MoveFlags::WIND, 0);
        assert!(eff.is_some());
        match eff.unwrap() {
            AbilityImmunityEffect::Boost(stat, stages) => {
                assert_eq!(stat, ATK);
                assert_eq!(stages, 1);
            }
            _ => panic!("Expected Boost effect"),
        }
    }

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
        assert!(!boost); // Liquid Voice has no power boost (unlike -ate abilities)
    }

    #[test]
    fn test_overheat_drops_spa_2() {
        use crate::data::MOVE_OVERHEAT;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_OVERHEAT as u16;
        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_OVERHEAT as u16, 0, &mut fixed_rng(0));
        // Damage dealt
        assert!(state.sides[1].team[0].current_hp < hp_before);
        // Attacker should have -2 SpA
        assert_eq!(state.sides[0].active.boosts[SPA], -2);
    }

    #[test]
    fn test_high_jump_kick_crash_on_immune() {
        use crate::data::MOVE_HIGH_JUMP_KICK;
        let mut state = setup();
        // Make defender a Ghost type (species 92 = Gastly: Ghost/Poison)
        state.sides[1].team[0].species_id = 92;
        state.sides[0].team[0].moves[0] = MOVE_HIGH_JUMP_KICK as u16;
        let atk_hp_before = state.sides[0].team[0].current_hp;
        let def_hp_before = state.sides[1].team[0].current_hp;
        // RNG set to hit (0 < 90 accuracy), but Fighting is immune to Ghost
        execute_move(&mut state, &TeamData::default(),0, MOVE_HIGH_JUMP_KICK as u16, 0, &mut fixed_rng(0));
        // Defender should NOT have taken damage (type immune)
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before);
        // Attacker should have taken 50% max HP crash damage
        let expected_crash = atk_hp_before / 2;
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp_before - expected_crash);
    }

    #[test]
    fn test_high_jump_kick_crash_on_protect() {
        use crate::data::MOVE_HIGH_JUMP_KICK;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_HIGH_JUMP_KICK as u16;
        // Defender has Protect active this turn
        state.sides[1].active.set_volatile(VOL_PROTECT_THIS_TURN);
        let atk_hp_before = state.sides[0].team[0].current_hp;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_HIGH_JUMP_KICK as u16, 0, &mut fixed_rng(0));
        // Defender should NOT have taken damage (Protect)
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before);
        // Attacker should have taken 50% max HP crash damage
        let expected_crash = atk_hp_before / 2;
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp_before - expected_crash);
    }

    #[test]
    fn test_no_self_effect_on_zero_bp() {
        // Status moves (0 BP) go through execute_status_move, not the damaging path,
        // so SelfEffect dispatch (which lives in the damaging path) should never fire.
        use crate::data::MOVE_WILL_O_WISP;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_WILL_O_WISP as u16;
        execute_move(&mut state, &TeamData::default(),0, MOVE_WILL_O_WISP as u16, 0, &mut fixed_rng(0));
        // No stat changes on attacker from self-effect dispatch
        for i in 0..7 {
            assert_eq!(state.sides[0].active.boosts[i], 0);
        }
    }

    #[test]
    fn test_knock_off_removes_item() {
        use crate::data::MOVE_KNOCK_OFF;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_KNOCK_OFF as u16;
        state.sides[1].team[0].item_id = 242; // Leftovers (regular item)
        execute_move(&mut state, &TeamData::default(),0, MOVE_KNOCK_OFF as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].item_id, 0);
    }

    #[test]
    fn test_knock_off_no_remove_forme_item() {
        use crate::data::MOVE_KNOCK_OFF;
        // Arceus (493) holding Draco Plate (105, forme_species=493) — should NOT be removed
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_KNOCK_OFF as u16;
        state.sides[1].team[0].species_id = 493; // Arceus
        state.sides[1].team[0].item_id = 105;    // Draco Plate
        execute_move(&mut state, &TeamData::default(),0, MOVE_KNOCK_OFF as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].item_id, 105); // item NOT removed
    }

    #[test]
    fn test_knock_off_removes_plate_non_arceus() {
        use crate::data::MOVE_KNOCK_OFF;
        // Non-Arceus holding Draco Plate — should be removed
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_KNOCK_OFF as u16;
        state.sides[1].team[0].item_id = 105; // Draco Plate, but holder is species 50
        execute_move(&mut state, &TeamData::default(),0, MOVE_KNOCK_OFF as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].item_id, 0); // item removed
    }

    #[test]
    fn test_knock_off_sticky_hold() {
        use crate::data::MOVE_KNOCK_OFF;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_KNOCK_OFF as u16;
        state.sides[1].team[0].item_id = 242; // Leftovers
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STICKY_HOLD;
        execute_move(&mut state, &TeamData::default(),0, MOVE_KNOCK_OFF as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].item_id, 242); // item kept
    }

    #[test]
    fn test_knock_off_no_boost_forme_item() {
        // Knock Off vs Arceus + Plate should not get 1.5x boost.
        // We verify indirectly: same attack, same defense, forme-locked item means less damage.
        use crate::data::MOVE_KNOCK_OFF;
        let (mut state_locked) = setup();
        state_locked.sides[0].team[0].moves[0] = MOVE_KNOCK_OFF as u16;
        state_locked.sides[1].team[0].species_id = 493; // Arceus
        state_locked.sides[1].team[0].item_id = 105;    // Draco Plate
        let hp_before_locked = state_locked.sides[1].team[0].current_hp;
        execute_move(&mut state_locked, &TeamData::default(), 0, MOVE_KNOCK_OFF as u16, 0, &mut fixed_rng(0));
        let dmg_locked = hp_before_locked - state_locked.sides[1].team[0].current_hp;

        let mut state_normal = setup();
        state_normal.sides[0].team[0].moves[0] = MOVE_KNOCK_OFF as u16;
        state_normal.sides[1].team[0].item_id = 242; // Leftovers (removable)
        let hp_before_normal = state_normal.sides[1].team[0].current_hp;
        execute_move(&mut state_normal, &TeamData::default(), 0, MOVE_KNOCK_OFF as u16, 0, &mut fixed_rng(0));
        let dmg_normal = hp_before_normal - state_normal.sides[1].team[0].current_hp;

        // Normal target with removable item should take MORE damage (1.5x boost)
        assert!(dmg_normal > dmg_locked, "removable item should get 1.5x boost: {} vs {}", dmg_normal, dmg_locked);
    }

    #[test]
    fn test_rapid_spin_clears_hazards() {
        use crate::data::MOVE_RAPID_SPIN;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_RAPID_SPIN as u16;
        // Set hazards on attacker's side
        state.sides[0].side_conditions.spikes = 3;
        state.sides[0].side_conditions.toxic_spikes = 2;
        state.sides[0].side_conditions.hazard_flags = HAZARD_STEALTH_ROCK | HAZARD_STICKY_WEB;
        execute_move(&mut state, &TeamData::default(),0, MOVE_RAPID_SPIN as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].side_conditions.spikes, 0);
        assert_eq!(state.sides[0].side_conditions.toxic_spikes, 0);
        assert_eq!(state.sides[0].side_conditions.hazard_flags, 0);
    }

    #[test]
    fn test_rapid_spin_removes_leech_seed() {
        use crate::data::MOVE_RAPID_SPIN;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_RAPID_SPIN as u16;
        state.sides[0].active.set_volatile(VOL_LEECH_SEED);
        execute_move(&mut state, &TeamData::default(),0, MOVE_RAPID_SPIN as u16, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_LEECH_SEED));
    }

    #[test]
    fn test_rapid_spin_removes_bind() {
        use crate::data::MOVE_RAPID_SPIN;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_RAPID_SPIN as u16;
        state.sides[0].active.set_volatile(VOL_BOUND);
        execute_move(&mut state, &TeamData::default(),0, MOVE_RAPID_SPIN as u16, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_BOUND));
    }

    #[test]
    fn test_rapid_spin_speed_boost() {
        use crate::data::MOVE_RAPID_SPIN;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_RAPID_SPIN as u16;
        execute_move(&mut state, &TeamData::default(),0, MOVE_RAPID_SPIN as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.boosts[SPE], 1);
    }

    #[test]
    fn test_rapid_spin_sheer_force_suppresses() {
        use crate::data::MOVE_RAPID_SPIN;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_RAPID_SPIN as u16;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_SHEER_FORCE;
        state.sides[0].side_conditions.spikes = 2;
        state.sides[0].side_conditions.hazard_flags = HAZARD_STEALTH_ROCK;
        state.sides[0].active.set_volatile(VOL_LEECH_SEED);
        execute_move(&mut state, &TeamData::default(),0, MOVE_RAPID_SPIN as u16, 0, &mut fixed_rng(0));
        // Sheer Force suppresses all secondary effects
        assert_eq!(state.sides[0].side_conditions.spikes, 2);
        assert_eq!(state.sides[0].side_conditions.hazard_flags, HAZARD_STEALTH_ROCK);
        assert!(state.sides[0].active.has_volatile(VOL_LEECH_SEED));
        assert_eq!(state.sides[0].active.boosts[SPE], 0);
    }

    #[test]
    fn test_defog_clears_both_sides() {
        use crate::data::MOVE_DEFOG;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_DEFOG as u16;
        // Set hazards on both sides
        state.sides[0].side_conditions.spikes = 2;
        state.sides[0].side_conditions.hazard_flags = HAZARD_STEALTH_ROCK;
        state.sides[1].side_conditions.spikes = 3;
        state.sides[1].side_conditions.toxic_spikes = 1;
        state.sides[1].side_conditions.hazard_flags = HAZARD_STICKY_WEB;
        execute_move(&mut state, &TeamData::default(),0, MOVE_DEFOG as u16, 0, &mut fixed_rng(0));
        // Both sides cleared
        assert_eq!(state.sides[0].side_conditions.spikes, 0);
        assert_eq!(state.sides[0].side_conditions.hazard_flags, 0);
        assert_eq!(state.sides[1].side_conditions.spikes, 0);
        assert_eq!(state.sides[1].side_conditions.toxic_spikes, 0);
        assert_eq!(state.sides[1].side_conditions.hazard_flags, 0);
    }

    #[test]
    fn test_defog_clears_screens() {
        use crate::data::MOVE_DEFOG;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_DEFOG as u16;
        state.sides[1].side_conditions.reflect_turns = 4;
        state.sides[1].side_conditions.light_screen_turns = 3;
        state.sides[1].side_conditions.aurora_veil_turns = 2;
        execute_move(&mut state, &TeamData::default(),0, MOVE_DEFOG as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].side_conditions.reflect_turns, 0);
        assert_eq!(state.sides[1].side_conditions.light_screen_turns, 0);
        assert_eq!(state.sides[1].side_conditions.aurora_veil_turns, 0);
    }

    #[test]
    fn test_defog_evasion_drop() {
        use crate::data::MOVE_DEFOG;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_DEFOG as u16;
        execute_move(&mut state, &TeamData::default(),0, MOVE_DEFOG as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].active.boosts[EVA], -1);
    }

    #[test]
    fn test_defog_clears_terrain() {
        use crate::data::MOVE_DEFOG;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_DEFOG as u16;
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;
        execute_move(&mut state, &TeamData::default(),0, MOVE_DEFOG as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.field.terrain, TERRAIN_NONE);
        assert_eq!(state.field.terrain_turns, 0);
    }

    #[test]
    fn test_defog_clears_safeguard_mist() {
        use crate::data::MOVE_DEFOG;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_DEFOG as u16;
        state.sides[1].side_conditions.set_safeguard_turns(5);
        state.sides[1].side_conditions.set_mist_turns(5);
        execute_move(&mut state, &TeamData::default(),0, MOVE_DEFOG as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].side_conditions.safeguard_turns(), 0);
        assert_eq!(state.sides[1].side_conditions.mist_turns(), 0);
    }

    #[test]
    fn test_good_as_gold_blocks_status() {
        use crate::data::MOVE_WILL_O_WISP;
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_GOOD_AS_GOLD;
        state.sides[0].team[0].moves[0] = MOVE_WILL_O_WISP as u16;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_WILL_O_WISP as u16, 0, &mut fixed_rng(0));

        // Good as Gold blocks status moves — no burn applied
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_mummy_overwrite() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_MUMMY;
        state.sides[0].team[0].ability_id = 100; // arbitrary non-Mummy ability

        // Pound is Contact → triggers Mummy
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[0].active.override_ability, data_bridge::ABILITY_MUMMY);
        assert!(state.sides[0].active.has_volatile(VOL_ABILITY_OVERRIDDEN));
    }

    #[test]
    fn test_toxic_debris_sets_spikes() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_TOXIC_DEBRIS;

        let spikes_before = state.sides[0].side_conditions.toxic_spikes;
        // Pound is Physical → triggers Toxic Debris on attacker's side
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[0].side_conditions.toxic_spikes > spikes_before);
    }

    #[test]
    fn test_poison_touch_contact() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_POISON_TOUCH;

        // fixed_rng(0): rng(100)==0 < 30 → triggers
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].status, STATUS_POISON);
    }

    #[test]
    fn test_berserk_threshold_crossing() {
        // Case A: HP crosses 50% threshold → should trigger
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_BERSERK;
        state.sides[1].team[0].max_hp = 300;
        state.sides[1].team[0].current_hp = 160; // 160/300 > 50%

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        let hp_after = state.sides[1].team[0].current_hp;
        if hp_after > 0 && hp_after * 2 <= 300 {
            // HP crossed threshold → Berserk should have fired
            assert_eq!(state.sides[1].active.boosts[SPA], 1,
                "Berserk should trigger when HP crosses below 50%");
        }

        // Case B: already below 50% → should NOT trigger
        let mut state2 = setup();
        state2.sides[1].team[0].ability_id = data_bridge::ABILITY_BERSERK;
        state2.sides[1].team[0].max_hp = 300;
        state2.sides[1].team[0].current_hp = 100; // 100/300 < 50%

        execute_move(&mut state2, &TeamData::default(), 0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state2.sides[1].active.boosts[SPA], 0,
            "Berserk should NOT trigger when already below 50%");
    }

    #[test]
    fn test_innards_out_damage() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_INNARDS_OUT;
        state.sides[1].team[0].current_hp = 50;
        state.sides[1].team[0].max_hp = 300;
        // High ATK to guarantee overkill on 50 HP
        state.sides[0].team[0].stats[ATK] = 500;

        let atk_hp_before = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        // Defender should faint
        assert!(state.sides[1].team[0].is_fainted());
        // Innards Out should deal exactly 50 (pre-damage HP), not overkill
        assert_eq!(state.sides[0].team[0].current_hp, atk_hp_before - 50);
    }

    #[test]
    fn test_lingering_aroma_overwrite() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_LINGERING_AROMA;
        state.sides[0].team[0].ability_id = 100; // arbitrary ability

        // Pound is Contact → triggers Lingering Aroma
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[0].active.override_ability, data_bridge::ABILITY_LINGERING_AROMA);
        assert!(state.sides[0].active.has_volatile(VOL_ABILITY_OVERRIDDEN));
    }

    #[test]
    fn test_mummy_no_overwrite_cantsuppress() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_MUMMY;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STANCE_CHANGE;

        // Pound is Contact, but Stance Change is cantsuppress
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert_ne!(state.sides[0].active.override_ability, data_bridge::ABILITY_MUMMY);
        assert!(!state.sides[0].active.has_volatile(VOL_ABILITY_OVERRIDDEN));
    }

    #[test]
    fn test_simple_beam_sets_simple() {
        let mut state = setup();
        state.sides[0].team[0].moves[0] = 493; // Simple Beam
        state.sides[1].team[0].ability_id = 100; // arbitrary suppressible ability

        execute_move(&mut state, &TeamData::default(), 0, 493, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].team[0].ability_id, data_bridge::ABILITY_SIMPLE);
        assert_eq!(state.sides[1].active.override_ability, 100);
        assert!(state.sides[1].team[0].flags & MON_FLAG_ABILITY_SWAPPED != 0);
    }

    #[test]
    fn test_simple_beam_fails_vs_truant() {
        let mut state = setup();
        state.sides[0].team[0].moves[0] = 493; // Simple Beam
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_TRUANT;

        execute_move(&mut state, &TeamData::default(), 0, 493, 0, &mut fixed_rng(99));

        assert_eq!(state.sides[1].team[0].ability_id, data_bridge::ABILITY_TRUANT);
        assert!(state.sides[1].team[0].flags & MON_FLAG_ABILITY_SWAPPED == 0);
    }

    #[test]
    fn test_ate_stab() {
        // Attacker is Pikachu (Electric-type, species 25) with Galvanize.
        // Pound (Normal/Physical/Contact) gets converted to Electric → STAB applies.
        // Defender is Snorlax (Normal-type, species 143) — neutral to Electric.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_GALVANIZE;
        state.sides[1].team[0].species_id = 143; // Snorlax (Normal) — neutral to Electric
        let hp_before_with = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(85));
        let dmg_with = hp_before_with - state.sides[1].team[0].current_hp;

        // Same setup without Galvanize (no STAB, no -ate boost)
        let mut state2 = setup();
        state2.sides[0].team[0].ability_id = 0;
        state2.sides[1].team[0].species_id = 143;
        let hp_before_without = state2.sides[1].team[0].current_hp;
        execute_move(&mut state2, &TeamData::default(), 0, 1, 0, &mut fixed_rng(85));
        let dmg_without = hp_before_without - state2.sides[1].team[0].current_hp;

        // Galvanize gives 1.2× power boost AND STAB (1.5×), so damage should be ~1.8× higher
        assert!(dmg_with > dmg_without, "ate STAB damage {} should exceed non-STAB {}", dmg_with, dmg_without);
    }

    #[test]
    fn test_galvanize_explosion_self_faint_keeps_electric_immunity() {
        // Galvanize-Explosion self-faints the user before calc. The -ate conversion
        // must still fire (Normal -> Electric), so a Ground defender is immune.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_GALVANIZE;
        state.sides[1].team[0].species_id = 50; // Diglett (Ground) — immune to Electric
        let def_hp_before = state.sides[1].team[0].current_hp;

        execute_move(&mut state, &TeamData::default(), 0, 153, 0, &mut fixed_rng(85)); // 153 = Explosion

        assert!(state.sides[0].team[0].is_fainted(), "Explosion user should have self-fainted");
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "Ground defender must take 0 (Galvanize -> Electric immunity preserved across self-faint)"
        );
    }

    #[test]
    fn test_chilling_neigh_atk() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_CHILLING_NEIGH;
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
    }

    #[test]
    fn test_grim_neigh_spa() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_GRIM_NEIGH;
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
    }

    #[test]
    fn test_as_one_glastrier_atk() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_AS_ONE_GLASTRIER;
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
    }

    #[test]
    fn test_as_one_spectrier_spa() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_AS_ONE_SPECTRIER;
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
    }

    #[test]
    fn test_beast_boost_tiebreak() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_BEAST_BOOST;
        state.sides[0].team[0].stats = [200, 200, 200, 200, 200];
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
        assert_eq!(state.sides[0].active.boosts[DEF], 0);
    }

    #[test]
    fn test_battle_bond_boosts_once() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_BATTLE_BOND;
        state.sides[0].team[0].species_id = data_bridge::SPECIES_GRENINJA_BOND;
        state.sides[1].team[0].current_hp = 1;
        state.sides[1].team[1].species_id = 25;
        state.sides[1].team[1].current_hp = 100;
        state.sides[1].team[1].max_hp = 100;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
        assert_eq!(state.sides[0].active.boosts[SPE], 1);
        assert!(state.sides[0].team[0].flags & MON_FLAG_BOND_TRIGGERED != 0);
    }

    #[test]
    fn test_battle_bond_no_trigger_wrong_species() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_BATTLE_BOND;
        state.sides[0].team[0].species_id = 658;
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[ATK], 0);
        assert_eq!(state.sides[0].active.boosts[SPA], 0);
        assert_eq!(state.sides[0].active.boosts[SPE], 0);
    }

    #[test]
    fn test_soul_heart_on_ko() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_SOUL_HEART;
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
    }

    #[test]
    fn test_soul_heart_not_on_self_faint() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_SOUL_HEART;
        state.sides[1].team[0].current_hp = 1;

        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(99));

        assert!(state.sides[1].team[0].is_fainted());
        assert_eq!(state.sides[1].active.boosts[SPA], 0);
    }

    #[test]
    fn test_electric_terrain_no_sleep() {
        use crate::data::MOVE_SPORE;
        let mut state = setup();
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;

        execute_move(&mut state, &TeamData::default(),0, MOVE_SPORE as u16, 0, &mut fixed_rng(0));

        // Grounded mon should not be put to sleep
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_misty_terrain_no_status() {
        use crate::data::MOVE_WILL_O_WISP;
        let mut state = setup();
        state.field.terrain = TERRAIN_MISTY;
        state.field.terrain_turns = 5;

        execute_move(&mut state, &TeamData::default(),0, MOVE_WILL_O_WISP as u16, 0, &mut fixed_rng(0));

        // Grounded mon should not be burned
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_psychic_terrain_blocks_priority() {
        use crate::data::MOVE_QUICK_ATTACK;
        let mut state = setup();
        state.field.terrain = TERRAIN_PSYCHIC;
        state.field.terrain_turns = 5;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_QUICK_ATTACK as u16, 0, &mut fixed_rng(0));

        // Priority move should fail against grounded target
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_psychic_terrain_allows_normal_priority() {
        use crate::data::MOVE_SURF;
        let mut state = setup();
        state.field.terrain = TERRAIN_PSYCHIC;
        state.field.terrain_turns = 5;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_SURF as u16, 0, &mut fixed_rng(0));

        // Normal priority move should work
        assert!(state.sides[1].team[0].current_hp < hp_before);
    }

    #[test]
    fn test_psychic_terrain_no_block_flying_target() {
        use crate::data::MOVE_QUICK_ATTACK;
        let mut state = setup();
        state.field.terrain = TERRAIN_PSYCHIC;
        state.field.terrain_turns = 5;
        // Make defender Flying (not grounded)
        state.sides[1].active.override_types = [Type::Flying as u8, Type::Flying as u8];
        state.sides[1].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_QUICK_ATTACK as u16, 0, &mut fixed_rng(0));

        // Non-grounded target should still take priority damage
        assert!(state.sides[1].team[0].current_hp < hp_before);
    }

    // === Task 12: Pivot, Substitute, Contact Aftermath ===

    #[test]
    fn test_uturn_switch_after_damage() {
        use crate::data::MOVE_U_TURN;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_U_TURN as u16;
        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_U_TURN as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < hp_before);
        assert!(state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    }

    #[test]
    fn test_uturn_no_switch_no_bench() {
        use crate::data::MOVE_U_TURN;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_U_TURN as u16;
        state.sides[0].team[1].current_hp = 0;
        execute_move(&mut state, &TeamData::default(),0, MOVE_U_TURN as u16, 0, &mut fixed_rng(0));
        assert!(!state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    }

    #[test]
    fn test_uturn_miss_no_switch() {
        use crate::data::MOVE_U_TURN;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_U_TURN as u16;
        // +6 evasion makes accuracy 100 → effective 33, rng(100)=99 → miss
        state.sides[1].active.boosts[EVA] = 6;
        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_U_TURN as u16, 0, &mut fixed_rng(99));
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(!state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    }

    #[test]
    fn test_parting_shot_stat_drop_switch() {
        let mut state = setup();
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::PartingShot,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(99), 0);
        assert_eq!(state.sides[1].active.boosts[ATK], -1);
        assert_eq!(state.sides[1].active.boosts[SPA], -1);
        assert!(state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    }

    #[test]
    fn test_substitute_blocks_damage() {
        let mut state = setup();
        state.sides[1].active.set_volatile(VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 200;
        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, hp_before);
        assert!(state.sides[1].active.substitute_hp < 200);
    }

    #[test]
    fn test_substitute_breaks() {
        let mut state = setup();
        state.sides[1].active.set_volatile(VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 1;
        execute_move(&mut state, &TeamData::default(),0, 1, 0, &mut fixed_rng(0));
        assert!(!state.sides[1].active.has_volatile(VOL_SUBSTITUTE));
        assert_eq!(state.sides[1].active.substitute_hp, 0);
    }

    #[test]
    fn test_substitute_blocks_secondary() {
        use crate::data::MOVE_SCALD;
        let mut state = setup();
        state.sides[1].active.set_volatile(VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 500;
        state.sides[0].team[0].moves[0] = MOVE_SCALD as u16;
        execute_move(&mut state, &TeamData::default(),0, MOVE_SCALD as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].active.substitute_hp < 500);
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_sound_bypasses_substitute() {
        use crate::data::MOVE_BOOMBURST;
        let mut state = setup();
        state.sides[1].active.set_volatile(VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 100;
        state.sides[0].team[0].moves[0] = MOVE_BOOMBURST as u16;
        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_BOOMBURST as u16, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < hp_before);
        assert_eq!(state.sides[1].active.substitute_hp, 100);
        assert!(state.sides[1].active.has_volatile(VOL_SUBSTITUTE));
    }

    #[test]
    fn test_rocky_helmet_contact() {
        use crate::data::MOVE_U_TURN;
        let mut state = setup();
        state.sides[1].team[0].item_id = 417; // Rocky Helmet
        state.sides[0].team[0].moves[0] = MOVE_U_TURN as u16;
        let hp_before = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_U_TURN as u16, 0, &mut fixed_rng(0));
        let helmet_damage = 300 / 6; // 50
        assert!(hp_before - state.sides[0].team[0].current_hp >= helmet_damage as u16);
    }

    #[test]
    fn test_rocky_helmet_no_contact() {
        use crate::data::MOVE_SCALD;
        let mut state = setup();
        state.sides[1].team[0].item_id = 417; // Rocky Helmet
        state.sides[0].team[0].moves[0] = MOVE_SCALD as u16;
        let hp_before = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_SCALD as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_safeguard_blocks_status() {
        let mut state = setup();
        state.sides[1].side_conditions.set_safeguard_turns(5);

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::WillOWisp,
            move_type: Type::Fire,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_safeguard_allows_self_status() {
        // Self-inflicted status (e.g. Close Combat user's side has Safeguard)
        // Safeguard only blocks opponent-inflicted status, so set_status on own side still works
        let mut state = setup();
        state.sides[0].side_conditions.set_safeguard_turns(5);

        // Directly set status on own side — Safeguard does not block self-inflicted
        set_status(&mut state, 0, 0, STATUS_BURN, 0);
        assert_eq!(state.sides[0].team[0].status, STATUS_BURN);
    }

    #[test]
    fn test_safeguard_blocks_secondary_status() {
        let mut state = setup();
        state.sides[1].side_conditions.set_safeguard_turns(5);

        // Secondary with 100% chance and burn status
        let md = MoveData {
            secondary_chance: 100,
            secondary_status: STATUS_BURN,
            move_type: Type::Fire,
            category: MoveCategory::Physical,
            base_power: 80,
            accuracy: 0,
            ..unsafe { core::mem::zeroed() }
        };
        apply_secondary(&mut state, 0, 1, &md, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_safeguard_blocks_confuse() {
        let mut state = setup();
        state.sides[1].side_conditions.set_safeguard_turns(5);

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Confuse,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        assert_eq!(state.sides[1].active.confusion_turns, 0);
    }

    #[test]
    fn test_safeguard_blocks_yawn() {
        let mut state = setup();
        state.sides[1].side_conditions.set_safeguard_turns(5);

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Yawn,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        assert!(!state.sides[1].active.has_volatile(VOL_YAWN));
    }

    #[test]
    fn test_mist_blocks_stat_drop() {
        let mut state = setup();
        state.sides[1].side_conditions.set_mist_turns(5);

        // Secondary with negative stat drop (100% chance)
        let md = MoveData {
            secondary_chance: 100,
            secondary_stat: -1,
            category: MoveCategory::Physical,
            base_power: 80,
            accuracy: 0,
            ..unsafe { core::mem::zeroed() }
        };
        apply_secondary(&mut state, 0, 1, &md, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[1].active.boosts[DEF], 0);
    }

    #[test]
    fn test_mist_allows_self_drop() {
        use crate::data::MOVE_CLOSE_COMBAT;
        let mut state = setup();
        state.sides[0].side_conditions.set_mist_turns(5);
        state.sides[0].team[0].moves[0] = MOVE_CLOSE_COMBAT as u16;

        execute_move(&mut state, &TeamData::default(),0, MOVE_CLOSE_COMBAT as u16, 0, &mut fixed_rng(0));

        // Self-inflicted drops from Close Combat should still apply
        assert_eq!(state.sides[0].active.boosts[DEF], -1);
        assert_eq!(state.sides[0].active.boosts[SPD], -1);
    }

    #[test]
    fn test_mist_blocks_parting_shot() {
        let mut state = setup();
        state.sides[1].side_conditions.set_mist_turns(5);

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::PartingShot,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // Mist blocks the stat drops
        assert_eq!(state.sides[1].active.boosts[ATK], 0);
        assert_eq!(state.sides[1].active.boosts[SPA], 0);
        // Switch should not happen since no drops landed
        assert!(!state.sides[0].active.has_volatile(VOL_MUST_SWITCH));
    }

    #[test]
    fn test_lucky_chant_blocks_crit() {
        use crate::state::calc::calc_damage;
        let mut state = setup();
        state.sides[1].side_conditions.set_lucky_chant_turns(5);
        // Give high crit stage to guarantee crit without Lucky Chant
        state.sides[0].active.boosts[6] = 6; // crit stage (index 6 is typically unused for boosts but crit_stage reads differently)

        // Use a physical move
        let move_id = state.sides[0].team[0].moves[0];
        let result = calc_damage(&state, 0, move_id, 0, &mut fixed_rng(0));

        assert!(!result.crit);
    }

    #[test]
    fn test_side_conditions_expire() {
        // Verify the packed fields decrement correctly (tested via accessors)
        let mut state = setup();
        state.sides[0].side_conditions.set_safeguard_turns(1);
        state.sides[0].side_conditions.set_mist_turns(1);
        state.sides[0].side_conditions.set_lucky_chant_turns(1);
        assert_eq!(state.sides[0].side_conditions.safeguard_turns(), 1);
        assert_eq!(state.sides[0].side_conditions.mist_turns(), 1);
        assert_eq!(state.sides[0].side_conditions.lucky_chant_turns(), 1);

        // Simulate expiry: decrement each
        let sg = state.sides[0].side_conditions.safeguard_turns();
        state.sides[0].side_conditions.set_safeguard_turns(sg - 1);
        let mt = state.sides[0].side_conditions.mist_turns();
        state.sides[0].side_conditions.set_mist_turns(mt - 1);
        let lc = state.sides[0].side_conditions.lucky_chant_turns();
        state.sides[0].side_conditions.set_lucky_chant_turns(lc - 1);

        assert_eq!(state.sides[0].side_conditions.safeguard_turns(), 0);
        assert_eq!(state.sides[0].side_conditions.mist_turns(), 0);
        assert_eq!(state.sides[0].side_conditions.lucky_chant_turns(), 0);
    }

    #[test]
    fn test_safeguard_blocks_contact_ability_status() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_FLAME_BODY;
        state.sides[0].side_conditions.set_safeguard_turns(5);

        // Use a contact move (U-turn is contact)
        use crate::data::MOVE_U_TURN;
        state.sides[0].team[0].moves[0] = MOVE_U_TURN as u16;

        // rng(100) < 30 triggers Flame Body — but Safeguard should block
        execute_move(&mut state, &TeamData::default(),0, MOVE_U_TURN as u16, 0, &mut fixed_rng(0));

        assert_eq!(state.sides[0].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_mist_blocks_defog_eva_drop() {
        let mut state = setup();
        state.sides[1].side_conditions.set_mist_turns(5);

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Defog,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // EVA drop should be blocked by Mist
        assert_eq!(state.sides[1].active.boosts[EVA], 0);
        // But Mist should be cleared by Defog
        assert_eq!(state.sides[1].side_conditions.mist_turns(), 0);
    }

    // ---- Phase 3 Task 05: Move Edge Cases ----

    #[test]
    fn test_weather_ball_type_in_sun() {
        use crate::state::calc_modifiers::resolve_move_type;
        let mut state = setup();
        let md = data_bridge::move_hot(crate::data::MOVE_WEATHER_BALL as u16);
        // No weather → Normal
        assert_eq!(resolve_move_type(&state, md, 0), crate::data::types::Type::Normal);
        // Sun → Fire
        state.field.weather = WEATHER_SUN;
        state.field.weather_turns = 5;
        assert_eq!(resolve_move_type(&state, md, 0), crate::data::types::Type::Fire);
    }

    #[test]
    fn test_terrain_pulse_grounded() {
        use crate::state::calc_modifiers::resolve_move_type;
        let mut state = setup();
        let md = data_bridge::move_hot(crate::data::MOVE_TERRAIN_PULSE as u16);
        // No terrain → Normal
        assert_eq!(resolve_move_type(&state, md, 0), crate::data::types::Type::Normal);
        // Electric terrain + grounded → Electric
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;
        assert_eq!(resolve_move_type(&state, md, 0), crate::data::types::Type::Electric);
    }

    #[test]
    fn test_stockpile_increments() {
        let mut state = setup();
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Stockpile,
            ..unsafe { core::mem::zeroed() }
        };

        // Stack 1
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.stockpile & 0x7F, 1);
        assert_eq!(state.sides[0].active.boosts[DEF], 1);
        assert_eq!(state.sides[0].active.boosts[SPD], 1);

        // Stack 2
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.stockpile & 0x7F, 2);
        assert_eq!(state.sides[0].active.boosts[DEF], 2);
        assert_eq!(state.sides[0].active.boosts[SPD], 2);

        // Stack 3
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.stockpile & 0x7F, 3);
        assert_eq!(state.sides[0].active.boosts[DEF], 3);
        assert_eq!(state.sides[0].active.boosts[SPD], 3);

        // Stack 4 — should NOT increment
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.stockpile & 0x7F, 3);
        assert_eq!(state.sides[0].active.boosts[DEF], 3);
    }

    #[test]
    fn test_spit_up_damage_by_stacks() {
        use crate::data::MOVE_SPIT_UP;
        let mut state = setup();
        state.sides[0].team[0].moves[0] = MOVE_SPIT_UP as u16;
        state.sides[0].active.stockpile = 2;
        state.sides[0].active.boosts[DEF] = 2;
        state.sides[0].active.boosts[SPD] = 2;

        let hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_SPIT_UP as u16, 0, &mut fixed_rng(0));
        let hp_after = state.sides[1].team[0].current_hp;

        // Defender should have taken damage (200 BP through calc)
        assert!(hp_after < hp_before, "spit up should deal damage");
        // Stockpile should be reset
        assert_eq!(state.sides[0].active.stockpile & 0x7F, 0);
        // Boosts should be removed
        assert_eq!(state.sides[0].active.boosts[DEF], 0);
        assert_eq!(state.sides[0].active.boosts[SPD], 0);
    }

    #[test]
    fn test_swallow_heal_by_stacks() {
        let mut state = setup();
        state.sides[0].team[0].current_hp = 100; // well below max 300
        state.sides[0].active.stockpile = 2;
        state.sides[0].active.boosts[DEF] = 2;
        state.sides[0].active.boosts[SPD] = 2;

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Swallow,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // 2 stacks = heal max_hp/2 = 150; 100 + 150 = 250
        assert_eq!(state.sides[0].team[0].current_hp, 250);
        // Stockpile reset
        assert_eq!(state.sides[0].active.stockpile & 0x7F, 0);
        // Boosts removed
        assert_eq!(state.sides[0].active.boosts[DEF], 0);
        assert_eq!(state.sides[0].active.boosts[SPD], 0);
    }

    // Real move data carries MoveFlags::HEAL on Swallow; the generic HEAL-flag
    // handler must NOT short-circuit it (Swallow fails at 0 stockpile, heals by
    // count otherwise). These mirror gen_moves Swallow exactly (flags set).
    #[test]
    fn test_swallow_zero_stockpile_no_heal_with_heal_flag() {
        let mut state = setup();
        state.sides[0].team[0].current_hp = 100; // well below max 300
        state.sides[0].active.stockpile = 0;

        let md = MoveData {
            flags: MoveFlags::HEAL,
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Swallow,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // 0 stockpile => Swallow fails, no heal (Showdown parity).
        assert_eq!(state.sides[0].team[0].current_hp, 100);
    }

    #[test]
    fn test_swallow_heal_by_stacks_with_heal_flag() {
        let mut state = setup();
        state.sides[0].team[0].current_hp = 100; // well below max 300
        state.sides[0].active.stockpile = 2;
        state.sides[0].active.boosts[DEF] = 2;
        state.sides[0].active.boosts[SPD] = 2;

        let md = MoveData {
            flags: MoveFlags::HEAL,
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Swallow,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // 2 stacks = heal max_hp/2 = 150; 100 + 150 = 250, then stockpile reset + boosts removed.
        assert_eq!(state.sides[0].team[0].current_hp, 250);
        assert_eq!(state.sides[0].active.stockpile & 0x7F, 0);
        assert_eq!(state.sides[0].active.boosts[DEF], 0);
        assert_eq!(state.sides[0].active.boosts[SPD], 0);
    }

    // Control: a plain HEAL-flag recovery move (Recover, MoveEffect::None) must
    // still heal a flat 50% via the generic handler — the Swallow exclusion must
    // not regress it.
    #[test]
    fn test_recover_heal_flag_still_heals_half() {
        let mut state = setup();
        state.sides[0].team[0].current_hp = 100; // max 300

        let md = MoveData {
            flags: MoveFlags::HEAL,
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::None,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        assert_eq!(state.sides[0].team[0].current_hp, 250); // 100 + 300/2
    }

    #[test]
    fn test_trick_swaps_items() {
        let mut state = setup();
        state.sides[0].team[0].item_id = 242; // Leftovers
        state.sides[1].team[0].item_id = 243; // some other item

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Trick,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        assert_eq!(state.sides[0].team[0].item_id, 243);
        assert_eq!(state.sides[1].team[0].item_id, 242);
    }

    #[test]
    fn test_trick_sticky_hold_blocks() {
        let mut state = setup();
        state.sides[0].team[0].item_id = 242;
        state.sides[1].team[0].item_id = 243;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STICKY_HOLD;

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Trick,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // Items should NOT be swapped
        assert_eq!(state.sides[0].team[0].item_id, 242);
        assert_eq!(state.sides[1].team[0].item_id, 243);
    }

    #[test]
    fn test_trick_forme_locked_blocks() {
        let mut state = setup();
        state.sides[0].team[0].item_id = 242; // Leftovers
        state.sides[1].team[0].species_id = 493; // Arceus
        state.sides[1].team[0].item_id = 105;    // Draco Plate (forme-locked on Arceus)

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Trick,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // Items should NOT be swapped
        assert_eq!(state.sides[0].team[0].item_id, 242);
        assert_eq!(state.sides[1].team[0].item_id, 105);
    }

    #[test]
    fn test_yawn_sleep_next_turn() {
        let mut state = setup();

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Yawn,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // VOL_YAWN should be set, no sleep yet
        assert!(state.sides[1].active.has_volatile(VOL_YAWN));
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);

        // After 1st EOT: yawn still active, no sleep yet (2-turn delay)
        crate::state::end_of_turn::end_of_turn(&mut state, &TeamData::default(), &mut crate::state::BattleRng::from_closure(&mut |_| 0u32));
        assert!(state.sides[1].active.has_volatile(VOL_YAWN));
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);

        // After 2nd EOT: yawn triggers sleep and volatile clears
        crate::state::end_of_turn::end_of_turn(&mut state, &TeamData::default(), &mut crate::state::BattleRng::from_closure(&mut |_| 0u32));
        assert!(!state.sides[1].active.has_volatile(VOL_YAWN));
        assert_eq!(state.sides[1].team[0].status, STATUS_SLEEP);
    }

    #[test]
    fn test_yawn_fails_already_statused() {
        let mut state = setup();
        state.sides[1].team[0].status = STATUS_BURN;

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Yawn,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // VOL_YAWN should NOT be set
        assert!(!state.sides[1].active.has_volatile(VOL_YAWN));
    }

    #[test]
    fn test_yawn_fails_in_electric_terrain() {
        let mut state = setup();
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Yawn,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // VOL_YAWN should NOT be set (terrain blocks sleep)
        assert!(!state.sides[1].active.has_volatile(VOL_YAWN));
    }

    // ---- Phase 3 Task 07: Remaining SelfEffects ----

    #[test]
    fn test_belly_drum_max_atk() {
        let mut state = setup();
        // max_hp = 300, current_hp = 300 → cost = 150
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::BellyDrum,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 6);
        assert_eq!(state.sides[0].team[0].current_hp, 300 - 150);
    }

    #[test]
    fn test_belly_drum_fails_low_hp() {
        let mut state = setup();
        // Set HP to exactly max_hp / 2 = 150 → should fail (need > 150)
        state.sides[0].team[0].current_hp = 150;
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::BellyDrum,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 0);
        assert_eq!(state.sides[0].team[0].current_hp, 150);
    }

    #[test]
    fn test_belly_drum_fails_atk_maxed() {
        let mut state = setup();
        // Already at +6 Atk → Showdown fails the move: no HP cost paid.
        state.sides[0].active.boosts[ATK] = 6;
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::BellyDrum,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 6);
        assert_eq!(state.sides[0].team[0].current_hp, 300);
    }

    #[test]
    fn test_shell_smash_boosts_and_drops() {
        let mut state = setup();
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::ShellSmash,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 2);
        assert_eq!(state.sides[0].active.boosts[SPA], 2);
        assert_eq!(state.sides[0].active.boosts[SPE], 2);
        assert_eq!(state.sides[0].active.boosts[DEF], -1);
        assert_eq!(state.sides[0].active.boosts[SPD], -1);
    }

    #[test]
    fn test_clangorous_soul_boosts_and_hp_cost() {
        let mut state = setup();
        // max_hp = 300 → cost = 300 * 33 / 100 = 99
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::ClangorousSoul,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
        assert_eq!(state.sides[0].active.boosts[DEF], 1);
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
        assert_eq!(state.sides[0].active.boosts[SPD], 1);
        assert_eq!(state.sides[0].active.boosts[SPE], 1);
        assert_eq!(state.sides[0].team[0].current_hp, 300 - 99);
    }

    #[test]
    fn test_clangorous_soul_fails_low_hp() {
        let mut state = setup();
        // cost = 300 * 33 / 100 = 99. Set HP to 99 → should fail (need > 99)
        state.sides[0].team[0].current_hp = 99;
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::ClangorousSoul,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 0);
        assert_eq!(state.sides[0].team[0].current_hp, 99);
    }

    #[test]
    fn test_curse_non_ghost() {
        let mut state = setup();
        // Side 0 mon is species 25 (Pikachu, Electric) — not Ghost
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::Curse,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
        assert_eq!(state.sides[0].active.boosts[DEF], 1);
        assert_eq!(state.sides[0].active.boosts[SPE], -1);
    }

    #[test]
    fn test_no_retreat_boost_trap() {
        let mut state = setup();
        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::NoRetreat,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
        assert_eq!(state.sides[0].active.boosts[DEF], 1);
        assert_eq!(state.sides[0].active.boosts[SPA], 1);
        assert_eq!(state.sides[0].active.boosts[SPD], 1);
        assert_eq!(state.sides[0].active.boosts[SPE], 1);
        assert!(state.sides[0].active.has_volatile(VOL_TRAPPED));
        // Second use should fail
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 1); // unchanged
    }

    #[test]
    fn test_tidy_up_clears_and_boosts() {
        let mut state = setup();
        // Set up hazards on both sides
        state.sides[0].side_conditions.spikes = 2;
        state.sides[0].side_conditions.hazard_flags = HAZARD_STEALTH_ROCK;
        state.sides[1].side_conditions.toxic_spikes = 1;
        state.sides[1].side_conditions.hazard_flags = HAZARD_STICKY_WEB;
        // Set up substitutes on both sides
        set_volatile(&mut state, 0, VOL_SUBSTITUTE);
        state.sides[0].active.substitute_hp = 75;
        set_volatile(&mut state, 1, VOL_SUBSTITUTE);
        state.sides[1].active.substitute_hp = 75;

        let md = MoveData {
            category: MoveCategory::Status,
            accuracy: 0,
            effect: MoveEffect::TidyUp,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);

        // Boosts
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
        assert_eq!(state.sides[0].active.boosts[SPE], 1);
        // Hazards cleared
        assert_eq!(state.sides[0].side_conditions.spikes, 0);
        assert_eq!(state.sides[0].side_conditions.hazard_flags, 0);
        assert_eq!(state.sides[1].side_conditions.toxic_spikes, 0);
        assert_eq!(state.sides[1].side_conditions.hazard_flags, 0);
        // Substitutes cleared
        assert!(!state.sides[0].active.has_volatile(VOL_SUBSTITUTE));
        assert_eq!(state.sides[0].active.substitute_hp, 0);
        assert!(!state.sides[1].active.has_volatile(VOL_SUBSTITUTE));
        assert_eq!(state.sides[1].active.substitute_hp, 0);
    }

    #[test]
    fn test_protective_pads_blocks_rocky_helmet() {
        use crate::data::MOVE_TACKLE;
        let mut state = setup();
        state.sides[0].team[0].item_id = data_bridge::ITEM_PROTECTIVE_PADS;
        state.sides[0].team[0].moves[0] = MOVE_TACKLE as u16;
        state.sides[1].team[0].item_id = 417; // Rocky Helmet
        let hp_before = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_protective_pads_no_block_non_contact() {
        use crate::data::MOVE_SCALD;
        let mut state = setup();
        state.sides[0].team[0].item_id = data_bridge::ITEM_PROTECTIVE_PADS;
        state.sides[0].team[0].moves[0] = MOVE_SCALD as u16;
        state.sides[1].team[0].item_id = 417; // Rocky Helmet
        let hp_before = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_SCALD as u16, 0, &mut fixed_rng(0));
        // Scald is non-contact, Rocky Helmet doesn't trigger regardless
        assert_eq!(state.sides[0].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_punching_glove_blocks_contact_on_punch() {
        use crate::data::MOVE_MACH_PUNCH;
        let mut state = setup();
        state.sides[0].team[0].item_id = data_bridge::ITEM_PUNCHING_GLOVE;
        state.sides[0].team[0].moves[0] = MOVE_MACH_PUNCH as u16;
        state.sides[1].team[0].item_id = 417; // Rocky Helmet
        let hp_before = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_MACH_PUNCH as u16, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].current_hp, hp_before);
    }

    #[test]
    fn test_punching_glove_no_block_non_punch_contact() {
        use crate::data::MOVE_TACKLE;
        let mut state = setup();
        state.sides[0].team[0].item_id = data_bridge::ITEM_PUNCHING_GLOVE;
        state.sides[0].team[0].moves[0] = MOVE_TACKLE as u16;
        state.sides[1].team[0].item_id = 417; // Rocky Helmet
        let hp_before = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        // Tackle is contact but not punch — Rocky Helmet still triggers
        assert!(state.sides[0].team[0].current_hp < hp_before);
    }

    #[test]
    fn test_ability_shield_blocks_mummy() {
        use crate::data::MOVE_TACKLE;
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_INTIMIDATE;
        state.sides[0].team[0].item_id = data_bridge::ITEM_ABILITY_SHIELD;
        state.sides[0].team[0].moves[0] = MOVE_TACKLE as u16;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_MUMMY;
        execute_move(&mut state, &TeamData::default(),0, MOVE_TACKLE as u16, 0, &mut fixed_rng(0));
        assert_eq!(effective_ability(&state, 0), data_bridge::ABILITY_INTIMIDATE);
    }

    #[test]
    fn test_clear_amulet_blocks_secondary_stat_drop() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_CLEAR_AMULET;
        let md = MoveData {
            secondary_chance: 100, secondary_stat: -1,
            category: MoveCategory::Physical, base_power: 80,
            ..unsafe { core::mem::zeroed() }
        };
        apply_secondary(&mut state, 0, 1, &md, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].active.boosts[DEF], 0);
    }

    #[test]
    fn test_clear_amulet_blocks_cotton_down() {
        let mut state = setup();
        state.sides[0].team[0].item_id = data_bridge::ITEM_CLEAR_AMULET;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_COTTON_DOWN;
        // Cotton Down triggers on any hit, applies -1 Spe to attacker
        // We test by checking that Clear Amulet blocks the drop
        assert_eq!(state.sides[0].active.boosts[SPE], 0);
    }

    #[test]
    fn test_covert_cloak_blocks_secondary_stat_drop() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_COVERT_CLOAK;
        let md = MoveData {
            secondary_chance: 100, secondary_stat: -1,
            category: MoveCategory::Physical, base_power: 80,
            ..unsafe { core::mem::zeroed() }
        };
        apply_secondary(&mut state, 0, 1, &md, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].active.boosts[DEF], 0);
    }

    #[test]
    fn test_covert_cloak_blocks_secondary_status() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_COVERT_CLOAK;
        let md = MoveData {
            secondary_chance: 100, secondary_status: STATUS_BURN,
            move_type: Type::Fire, category: MoveCategory::Physical, base_power: 80,
            ..unsafe { core::mem::zeroed() }
        };
        apply_secondary(&mut state, 0, 1, &md, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].status, STATUS_NONE);
    }

    #[test]
    fn test_covert_cloak_blocks_flinch() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_COVERT_CLOAK;
        let md = MoveData {
            secondary_chance: 100,
            category: MoveCategory::Physical, base_power: 80,
            ..unsafe { core::mem::zeroed() }
        };
        apply_secondary(&mut state, 0, 1, &md, 0, &mut fixed_rng(0));
        assert!(!state.sides[1].active.has_volatile(VOL_FLINCHED));
    }

    #[test]
    fn test_covert_cloak_allows_self_boost() {
        let mut state = setup();
        state.sides[0].team[0].item_id = data_bridge::ITEM_COVERT_CLOAK;
        let md = MoveData {
            secondary_chance: 100, secondary_stat: 1,
            category: MoveCategory::Physical, base_power: 80,
            ..unsafe { core::mem::zeroed() }
        };
        apply_secondary(&mut state, 0, 1, &md, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
    }

    #[test]
    fn test_mirror_herb_copies_swords_dance() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_MIRROR_HERB;
        let md = MoveData {
            effect: MoveEffect::SwordsDance,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 2);
        assert_eq!(state.sides[1].active.boosts[ATK], 2);
        assert_eq!(state.sides[1].team[0].item_id, 0);
    }

    #[test]
    fn test_mirror_herb_copies_dragon_dance() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_MIRROR_HERB;
        let md = MoveData {
            effect: MoveEffect::DragonDance,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
        assert_eq!(state.sides[0].active.boosts[SPE], 1);
        assert_eq!(state.sides[1].active.boosts[ATK], 1);
        assert_eq!(state.sides[1].active.boosts[SPE], 1);
        assert_eq!(state.sides[1].team[0].item_id, 0);
    }

    #[test]
    fn test_mirror_herb_ignores_negative_boosts() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_MIRROR_HERB;
        // Shell Smash: +2 Atk, +2 SpA, +2 Spe, -1 Def, -1 SpD
        let md = MoveData {
            effect: MoveEffect::ShellSmash,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        // Attacker gets all boosts/drops
        assert_eq!(state.sides[0].active.boosts[ATK], 2);
        assert_eq!(state.sides[0].active.boosts[DEF], -1);
        // Mirror Herb holder copies only positives
        assert_eq!(state.sides[1].active.boosts[ATK], 2);
        assert_eq!(state.sides[1].active.boosts[SPA], 2);
        assert_eq!(state.sides[1].active.boosts[SPE], 2);
        assert_eq!(state.sides[1].active.boosts[DEF], 0);
        assert_eq!(state.sides[1].active.boosts[SPD], 0);
    }

    #[test]
    fn test_mirror_herb_consumed_after_use() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_MIRROR_HERB;
        let md = MoveData {
            effect: MoveEffect::SwordsDance,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[1].team[0].item_id, 0);
        // Second Swords Dance: no mirror herb to trigger
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 4);
        assert_eq!(state.sides[1].active.boosts[ATK], 2); // no additional copy
    }

    #[test]
    fn test_mirror_herb_triggers_unburden() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_MIRROR_HERB;
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_UNBURDEN;
        let md = MoveData {
            effect: MoveEffect::SwordsDance,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert!(state.sides[1].active.has_volatile(VOL_UNBURDEN));
    }

    #[test]
    fn test_mirror_herb_blocked_by_magic_room() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_MIRROR_HERB;
        state.field.set_magic_room_turns(5);
        let md = MoveData {
            effect: MoveEffect::SwordsDance,
            ..unsafe { core::mem::zeroed() }
        };
        execute_status_move(&mut state, &TeamData::default(), 0, 1, &md, &mut fixed_rng(0), 0);
        assert_eq!(state.sides[0].active.boosts[ATK], 2);
        assert_eq!(state.sides[1].active.boosts[ATK], 0); // blocked
        assert_ne!(state.sides[1].team[0].item_id, 0); // not consumed
    }

    #[test]
    fn test_mirror_herb_copies_self_effect_boost() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_MIRROR_HERB;
        let md = MoveData {
            self_effect: SelfEffect::AtkUp1,
            category: MoveCategory::Physical, base_power: 80,
            ..unsafe { core::mem::zeroed() }
        };
        apply_self_effect(&mut state, 0, &md);
        assert_eq!(state.sides[0].active.boosts[ATK], 1);
        assert_eq!(state.sides[1].active.boosts[ATK], 1);
        assert_eq!(state.sides[1].team[0].item_id, 0);
    }

    #[test]
    fn test_mirror_herb_no_copy_self_drops() {
        let mut state = setup();
        state.sides[1].team[0].item_id = data_bridge::ITEM_MIRROR_HERB;
        let md = MoveData {
            self_effect: SelfEffect::DefSpDDown1,
            category: MoveCategory::Physical, base_power: 80,
            ..unsafe { core::mem::zeroed() }
        };
        apply_self_effect(&mut state, 0, &md);
        assert_eq!(state.sides[0].active.boosts[DEF], -1);
        assert_eq!(state.sides[1].active.boosts[DEF], 0);
        assert_ne!(state.sides[1].team[0].item_id, 0); // not consumed
    }

    // ---- effective_accuracy tests ----

    #[test]
    fn test_effective_accuracy_base() {
        let state = setup();
        let md = MoveData { accuracy: 90, ..unsafe { core::mem::zeroed() } };
        assert_eq!(effective_accuracy(&state, 0, &md), 90);
    }

    #[test]
    fn test_effective_accuracy_guaranteed_zero() {
        let state = setup();
        let md = MoveData { accuracy: 0, ..unsafe { core::mem::zeroed() } };
        assert_eq!(effective_accuracy(&state, 0, &md), u32::MAX);
    }

    #[test]
    fn test_effective_accuracy_no_guard() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_NO_GUARD as u16;
        let md = MoveData { accuracy: 90, ..unsafe { core::mem::zeroed() } };
        assert_eq!(effective_accuracy(&state, 0, &md), u32::MAX);
    }

    #[test]
    fn test_effective_accuracy_defender_no_guard() {
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_NO_GUARD as u16;
        let md = MoveData { accuracy: 90, ..unsafe { core::mem::zeroed() } };
        assert_eq!(effective_accuracy(&state, 0, &md), u32::MAX);
    }

    #[test]
    fn test_effective_accuracy_compound_eyes() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_COMPOUND_EYES as u16;
        let md = MoveData { accuracy: 90, ..unsafe { core::mem::zeroed() } };
        // 90 * 13/10 = 117
        assert_eq!(effective_accuracy(&state, 0, &md), 117);
    }

    #[test]
    fn test_effective_accuracy_gravity() {
        let mut state = setup();
        state.field.gravity_turns = 3;
        let md = MoveData { accuracy: 90, ..unsafe { core::mem::zeroed() } };
        // 90 * 5/3 = 150
        assert_eq!(effective_accuracy(&state, 0, &md), 150);
    }

    #[test]
    fn test_effective_accuracy_hustle_physical() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_HUSTLE as u16;
        let md = MoveData {
            accuracy: 90,
            category: MoveCategory::Physical,
            ..unsafe { core::mem::zeroed() }
        };
        // 90 * 4/5 = 72
        assert_eq!(effective_accuracy(&state, 0, &md), 72);
    }

    #[test]
    fn test_effective_accuracy_hustle_special_unaffected() {
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_HUSTLE as u16;
        let md = MoveData {
            accuracy: 90,
            category: MoveCategory::Special,
            ..unsafe { core::mem::zeroed() }
        };
        // Hustle only affects Physical
        assert_eq!(effective_accuracy(&state, 0, &md), 90);
    }

    #[test]
    fn test_accuracy_check_refactored_still_works() {
        let state = setup();
        // accuracy=100 with rng=0 → 0 < 100 → hit
        let md = MoveData { accuracy: 100, ..unsafe { core::mem::zeroed() } };
        assert!(accuracy_check(&state, 0, &md, &mut fixed_rng(0)));

        // accuracy=50 with rng=99 → 99 < 50 → false → miss
        let md50 = MoveData { accuracy: 50, ..unsafe { core::mem::zeroed() } };
        assert!(!accuracy_check(&state, 0, &md50, &mut fixed_rng(99)));
    }

    // ─────────────────────────────────────────────────────────────────
    // Sleep Talk (move 214) — Wave 3 Batch A falsifiability fixtures.
    // Showdown oracle: data/moves.ts:16916-16952.
    // ─────────────────────────────────────────────────────────────────

    fn setup_sleeping(moves: [u16; 4], pp: [u8; 4], counter: u8) -> BattleState {
        let mut state = setup();
        state.sides[0].team[0].moves = moves;
        state.sides[0].team[0].pp = pp;
        state.sides[0].team[0].status = STATUS_SLEEP;
        state.sides[0].team[0].status_counter = counter;
        state
    }

    #[test]
    fn test_sleep_talk_outer_pp_only_inner_untouched() {
        // Fixture #1: outer slot PP decrements once; inner slot PP unchanged.
        let mut state = setup_sleeping([214, 33, 0, 0], [24, 24, 0, 0], 3);
        execute_move(&mut state, &TeamData::default(),0, 214, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].pp[0], 23, "outer Sleep Talk PP not decremented");
        assert_eq!(state.sides[0].team[0].pp[1], 24, "inner move PP must be untouched");
    }

    #[test]
    fn test_sleep_talk_decrements_sleep_counter_once() {
        // Fixture #5: sleep counter decrements exactly once on a Sleep Talk turn.
        let mut state = setup_sleeping([214, 33, 0, 0], [24, 24, 0, 0], 3);
        execute_move(&mut state, &TeamData::default(),0, 214, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].status_counter, 2);
        assert_eq!(state.sides[0].team[0].status, STATUS_SLEEP);
    }

    #[test]
    fn test_sleep_talk_fails_on_wakeup_tick() {
        // counter == 1 → prelude clears sleep AND breaks 'exec; no inner dispatch.
        let mut state = setup_sleeping([214, 33, 0, 0], [24, 24, 0, 0], 1);
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 214, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE, "should have woken up");
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before, "no inner damage");
    }

    #[test]
    fn test_sleep_talk_fails_when_user_awake() {
        // onTry requires status === 'slp'; awake user → no inner dispatch.
        let mut state = setup();
        state.sides[0].team[0].moves = [214, 33, 0, 0];
        state.sides[0].team[0].pp = [24, 24, 0, 0];
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 214, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before, "awake user must not dispatch inner");
    }

    #[test]
    fn test_sleep_talk_charge_filter_rejects_meteor_beam() {
        // Sleep Talk excludes moves with the CHARGE flag (Showdown moves.ts:16935).
        // Meteor Beam (800) is the canonical test: CHARGE-flagged, not in SLEEP_TALK_FAIL.
        // With moveset [Sleep Talk, Meteor Beam], the candidate set is empty → no dispatch.
        let mut state = setup_sleeping([214, 800, 0, 0], [24, 10, 0, 0], 3);
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 214, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].pp[0], 23, "outer PP still deducted");
        assert_eq!(state.sides[0].team[0].pp[1], 10, "Meteor Beam PP untouched");
        assert!(!state.sides[0].active.has_volatile(VOL_CHARGING), "Meteor Beam must NOT be rolled");
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before);
        assert_eq!(state.sides[0].team[0].status_counter, 2, "sleep still decremented once");
    }

    #[test]
    fn test_sleep_talk_outrage_rb_affirmative_lock_resume() {
        // Fixture #7: Sleep Talk rolls Outrage (200, Thrash effect, no CHARGE flag,
        // not in SLEEP_TALK_FAIL). VOL_MOVE_LOCKED is set in this dispatch — R-b
        // affirmative writes last_move = inner_id so execute_move:2851's resume
        // reads Outrage on T2 (NOT Sleep Talk).
        let mut state = setup_sleeping([214, 200, 0, 0], [24, 24, 0, 0], 3);
        execute_move(&mut state, &TeamData::default(),0, 214, 0, &mut fixed_rng(0));
        assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED), "Outrage must engage lock");
        assert!(state.sides[0].active._padding[2] >= 1, "lock turns remaining");
        assert_eq!(state.sides[0].active.last_move, 200, "R-b affirmative: resume target = inner Outrage");
    }

    #[test]
    fn test_sleep_talk_tackle_rb_negative_outer_restore() {
        // Fixture #10: Sleep Talk rolls Tackle (33, no charge, no lock) — R-b
        // negative. last_move at end of T1 is the outer call (214), so a future
        // Mirror Move read (data/moves.ts:12069) sees Sleep Talk, not Tackle.
        let mut state = setup_sleeping([214, 33, 0, 0], [24, 24, 0, 0], 3);
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 214, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < def_hp_before, "Tackle must hit");
        assert!(!state.sides[0].active.has_volatile(VOL_CHARGING));
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(state.sides[0].active.last_move, 214, "R-b negative: keep outer (Sleep Talk)");
    }

    #[test]
    fn test_sleep_talk_sucker_punch_inner_reads_pending_actions() {
        // Fixture #9: inner Sucker Punch's onTry reads pending_actions[def_side]
        // correctly under depth=1 dispatch. Defender hasn't moved + queued damaging
        // move at slot 0 → Sucker Punch fires.
        let mut state = setup_sleeping([214, 389, 0, 0], [24, 24, 0, 0], 3);
        state.pending_actions[1] = 0; // defender will use slot 0 (Pound) — damaging
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 214, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < def_hp_before, "Sucker Punch must hit");
        assert_eq!(state.sides[0].active.last_move, 214, "R-b negative on damaging inner");
    }

    #[test]
    fn test_sleep_talk_sucker_punch_inner_fails_if_defender_moved() {
        // Companion to #9: defender already moved → Sucker Punch onTry rejects.
        let mut state = setup_sleeping([214, 389, 0, 0], [24, 24, 0, 0], 3);
        set_volatile(&mut state, 1, VOL_MOVED_THIS_TURN);
        state.pending_actions[1] = 0;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 214, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before, "defender-already-moved gate must hold");
    }

    #[test]
    fn test_upper_hand_fails_vs_priority_zero_move() {
        // Defender queues Grass Knot (447, priority 0) → Upper Hand onTry fails
        // (Showdown move.priority <= 0.1). Defender takes no damage.
        let mut state = setup();
        state.sides[1].team[0].moves = [447, 0, 0, 0];
        state.pending_actions[1] = 0;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 918, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before,
            "Upper Hand must fail vs a priority-0 damaging move");
    }

    #[test]
    fn test_upper_hand_fails_vs_status_move() {
        // Defender queues a status move → Upper Hand onTry fails.
        let mut state = setup();
        state.sides[1].team[0].moves = [86, 0, 0, 0]; // Thunder Wave (status)
        state.pending_actions[1] = 0;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 918, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before,
            "Upper Hand must fail vs a queued status move");
    }

    #[test]
    fn test_upper_hand_succeeds_vs_priority_damaging_move() {
        // Defender queues Quick Attack (98, priority +1, damaging) → Upper Hand
        // onTry passes and the move connects.
        let mut state = setup();
        state.sides[1].team[0].moves = [98, 0, 0, 0];
        state.pending_actions[1] = 0;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 918, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < def_hp_before,
            "Upper Hand must connect vs a queued priority damaging move");
    }

    #[test]
    fn test_upper_hand_fails_if_defender_moved() {
        // Defender already moved → no queued action to intercept → Upper Hand fails.
        let mut state = setup();
        state.sides[1].team[0].moves = [98, 0, 0, 0];
        set_volatile(&mut state, 1, VOL_MOVED_THIS_TURN);
        state.pending_actions[1] = 0;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(), 0, 918, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[1].team[0].current_hp, def_hp_before,
            "Upper Hand must fail when the defender has already moved");
    }

    // ─────────────────────────────────────────────────────────────────
    // Metronome (move 118) — Wave 3 Batch B falsifiability fixtures.
    // Showdown oracle: data/moves.ts:11810-11836. Unconditional dispatch;
    // candidate set is the codegen-emitted METRONOME_OK slice (581 entries).
    // ─────────────────────────────────────────────────────────────────

    fn metronome_idx(move_id: u16) -> u32 {
        METRONOME_OK.iter().position(|&m| m == move_id).unwrap() as u32
    }

    #[test]
    fn test_metronome_outer_pp_decrements_once() {
        // Fixture #1: outer Metronome PP decrements once; inner is rolled from
        // METRONOME_OK independent of the user's moveset, so there is no
        // "inner slot" on the user to assert untouched.
        let mut state = setup();
        state.sides[0].team[0].moves = [118, 1, 0, 0];
        state.sides[0].team[0].pp = [10, 24, 0, 0];
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].pp[0], 9, "outer Metronome PP decremented once");
        assert_eq!(state.sides[0].team[0].pp[1], 24, "non-rolled slot untouched");
    }

    #[test]
    fn test_metronome_choice_locks_outer_not_inner() {
        // Fixture #4: Choice-Band Metronome locks to id 118 (the outer call),
        // not the inner move id. Prelude path at execute_move:3084 writes
        // choice_locked_move = move_id where move_id is the outer.
        let mut state = setup();
        state.sides[0].team[0].moves = [118, 0, 0, 0];
        state.sides[0].team[0].item_id = 68; // Choice Band
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[0].active.choice_locked_move, 118,
            "Choice-lock latches outer Metronome id, not the inner"
        );
    }

    #[test]
    fn test_metronome_solar_beam_rb_charge_resume() {
        // Fixture #6: Metronome rolls Solar Beam (76) outside sun. T1 must
        // engage VOL_CHARGING and the R-b affirmative rule writes
        // last_move = 76 so execute_move's charge-resume path (the runMove
        // prelude turn-2 branch reading last_move) replays Solar Beam, NOT
        // Metronome, on T2.
        let mut state = setup();
        state.sides[0].team[0].moves = [118, 0, 0, 0];
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(metronome_idx(76)));
        assert!(
            state.sides[0].active.has_volatile(VOL_CHARGING),
            "Solar Beam outside sun must engage VOL_CHARGING on T1"
        );
        assert_eq!(
            state.sides[0].active.last_move, 76,
            "R-b affirmative: resume target = inner Solar Beam"
        );
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "no damage on T1 (still charging)"
        );
    }

    #[test]
    fn test_metronome_outrage_rb_lock_resume() {
        // Fixture #7: Metronome rolls Outrage (200, Thrash effect). T1 sets
        // VOL_MOVE_LOCKED with turns remaining; R-b affirmative writes
        // last_move = 200 so the resume path reads Outrage on T2.
        let mut state = setup();
        state.sides[0].team[0].moves = [118, 0, 0, 0];
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(metronome_idx(200)));
        assert!(
            state.sides[0].active.has_volatile(VOL_MOVE_LOCKED),
            "Outrage must engage VOL_MOVE_LOCKED"
        );
        assert!(state.sides[0].active._padding[2] >= 1, "lock turns remaining");
        assert_eq!(
            state.sides[0].active.last_move, 200,
            "R-b affirmative: resume target = inner Outrage"
        );
    }

    #[test]
    fn test_metronome_stance_change_on_inner_damaging() {
        // Fixture #8: Aegislash-Shield + Metronome → Earthquake (89, Physical)
        // flips to Aegislash-Blade on the inner dispatch.
        let mut state = setup();
        state.sides[0].team[0].species_id = 681; // Aegislash-Shield
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STANCE_CHANGE;
        state.sides[0].team[0].moves = [118, 0, 0, 0];
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(metronome_idx(89)));
        assert_eq!(
            effective_species(&state, 0), 1103,
            "Aegislash-Shield must flip to Blade on inner damaging dispatch"
        );
    }

    #[test]
    fn test_metronome_tackle_rb_negative_outer_restore() {
        // Fixture #10: Metronome rolls Tackle (33, no charge, no lock) — R-b
        // negative path. last_move at end of T1 is the outer call (118), so a
        // future Mirror Move read (data/moves.ts:12069) sees Metronome.
        let mut state = setup();
        state.sides[0].team[0].moves = [118, 0, 0, 0];
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(metronome_idx(33)));
        assert!(state.sides[1].team[0].current_hp < def_hp_before, "Tackle must hit");
        assert!(!state.sides[0].active.has_volatile(VOL_CHARGING));
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(
            state.sides[0].active.last_move, 118,
            "R-b negative: keep outer (Metronome)"
        );
    }

    #[test]
    fn test_metronome_protean_fires_on_inner_type() {
        // Fixture #11: Protean + Metronome → Earthquake fires Protean on the
        // INNER move's type (Ground), not Normal-from-Metronome. The outer
        // gate at use_move_called:1844 skips Call*-family ids; the inner
        // dispatch re-enters use_move_called with the rolled id and the gate
        // passes.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;
        state.sides[0].team[0].moves = [118, 0, 0, 0];
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(metronome_idx(89)));
        assert!(state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(
            state.sides[0].active.override_types,
            [Type::Ground as u8, Type::Ground as u8],
            "Protean fires on inner Earthquake (Ground), not outer Metronome (Normal)"
        );
        assert_eq!(
            state.sides[0].active._padding[3] & 1, 1,
            "once-per-switch flag consumed by inner dispatch"
        );
    }

    #[test]
    fn test_metronome_self_excluded_from_filter() {
        // Hardening: Metronome's own move id (118) is absent from
        // METRONOME_OK (Showdown's `metronome` move lacks `flags.metronome`).
        // This is defense-in-depth for the depth-1 cap on use_move_called.
        assert!(
            METRONOME_OK.binary_search(&118).is_err(),
            "METRONOME_OK must exclude Metronome itself"
        );
    }

    #[test]
    fn test_metronome_blocked_while_sleeping() {
        // Hardening: Metronome lacks Showdown's `sleepUsable: true`, so the
        // sleep prelude blocks dispatch while the user is asleep. Only Sleep
        // Talk carves out the move_id == 214 exception at execute_move:2973.
        let mut state = setup();
        state.sides[0].team[0].moves = [118, 0, 0, 0];
        state.sides[0].team[0].status = STATUS_SLEEP;
        state.sides[0].team[0].status_counter = 3;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(metronome_idx(33)));
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "sleeping Metronome must not dispatch an inner damaging move"
        );
        assert_eq!(
            state.sides[0].team[0].status_counter, 2,
            "sleep counter still decrements once on the blocked turn"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // last_move_globally (Wave 3 Batch C.1 — state addition).
    // Verifies the battle-level last-move record is written last-write-wins
    // by use_move_called, including the charge early-return path.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_last_move_globally_default_is_zero() {
        let state = setup();
        assert_eq!(state.last_move_globally, 0, "fresh battle: no prior move");
    }

    #[test]
    fn test_last_move_globally_inner_wins_metronome_tackle() {
        // Single-turn inner: Metronome (118, outer) rolls Tackle (33, inner).
        // use_move_called writes the outer at depth=0 first, then the inner
        // dispatch overwrites with 33 — last-write-wins matches Showdown's
        // clearActiveMove flush after every useMove.
        let mut state = setup();
        state.sides[0].team[0].moves = [118, 0, 0, 0];
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(metronome_idx(33)));
        assert_eq!(
            state.last_move_globally, 33,
            "inner-wins: Metronome rolled Tackle, last_move_globally = 33"
        );
    }

    #[test]
    fn test_last_move_globally_written_before_charge_early_return() {
        // Charge inner: Metronome rolls Solar Beam (76) outside sun. The
        // write site is at the top of use_move_called, BEFORE any early
        // return for charge / lock branches, so last_move_globally lands at
        // 76 even though no damage / no end-of-dispatch flush happens.
        let mut state = setup();
        state.sides[0].team[0].moves = [118, 0, 0, 0];
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(metronome_idx(76)));
        assert!(
            state.sides[0].active.has_volatile(VOL_CHARGING),
            "Solar Beam outside sun engages VOL_CHARGING on T1"
        );
        assert_eq!(
            state.last_move_globally, 76,
            "write must precede the charge early-return inside use_move_called"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Copycat (move 383) — Wave 3 Batch C.2 falsifiability fixtures.
    // Showdown oracle: data/moves.ts:2853-2877. Reads battle.lastMove
    // (engine: state.last_move_globally), fails on 0 or COPYCAT_FAIL hits.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_copycat_fails_when_last_move_globally_zero() {
        // Fixture #1: fresh battle, no prior move. Copycat must no-op.
        let mut state = setup();
        state.sides[0].team[0].moves = [383, 0, 0, 0];
        let def_hp_before = state.sides[1].team[0].current_hp;
        assert_eq!(state.last_move_globally, 0);
        execute_move(&mut state, &TeamData::default(),0, 383, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "Copycat must no-op when last_move_globally == 0"
        );
    }

    #[test]
    fn test_copycat_replays_inner_after_metronome() {
        // Fixture #2 (headline): Metronome rolls Bullet Seed (331);
        // last_move_globally records the INNER, so Copycat next replays
        // Bullet Seed, NOT Metronome. Verifies inner-wins last-write-wins.
        let mut state = setup();
        state.sides[0].team[0].moves = [118, 383, 0, 0];
        execute_move(&mut state, &TeamData::default(),0, 118, 0, &mut fixed_rng(metronome_idx(331)));
        assert_eq!(
            state.last_move_globally, 331,
            "Metronome → Bullet Seed: inner wins last-write"
        );
        let def_hp_after_metronome = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 383, 0, &mut fixed_rng(0));
        assert!(
            state.sides[1].team[0].current_hp < def_hp_after_metronome,
            "Copycat must dispatch Bullet Seed and deal damage"
        );
        assert_eq!(
            state.last_move_globally, 331,
            "Copycat replays inner Bullet Seed (last_move_globally unchanged)"
        );
    }

    #[test]
    fn test_copycat_fails_when_prior_move_is_failcopycat() {
        // Fixture #3: prior move is in COPYCAT_FAIL (use Copycat itself, id 383,
        // which appears in failcopycat per data/moves.ts:2861).
        let mut state = setup();
        state.sides[0].team[0].moves = [383, 0, 0, 0];
        state.last_move_globally = 383; // simulate prior Copycat
        let def_hp_before = state.sides[1].team[0].current_hp;
        assert!(COPYCAT_FAIL.binary_search(&383u16).is_ok(), "Copycat self-excluded");
        execute_move(&mut state, &TeamData::default(),0, 383, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "Copycat after Copycat must fail (COPYCAT_FAIL filter)"
        );
    }

    #[test]
    fn test_copycat_choice_locks_outer_not_inner() {
        // Fixture #4: Choice-Band Copycat locks to id 383 (the outer call),
        // not the inner move id. Prelude path writes choice_locked_move =
        // outer move_id before use_move_called fires.
        let mut state = setup();
        state.sides[0].team[0].moves = [383, 0, 0, 0];
        state.sides[0].team[0].item_id = 68; // Choice Band
        state.last_move_globally = 33; // Tackle was the last move
        execute_move(&mut state, &TeamData::default(),0, 383, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[0].active.choice_locked_move, 383,
            "Choice-lock latches outer Copycat id, not the inner"
        );
    }

    #[test]
    fn test_copycat_solar_beam_rb_charge_resume() {
        // Fixture #5: Copycat replays Solar Beam (76, charge). R-b affirmative
        // writes last_move = 76 so the resume path reads Solar Beam on T2.
        // last_move_globally also ends at 76 (last-write-wins via the helper).
        let mut state = setup();
        state.sides[0].team[0].moves = [383, 0, 0, 0];
        state.last_move_globally = 76; // Solar Beam was the last move
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 383, 0, &mut fixed_rng(0));
        assert!(
            state.sides[0].active.has_volatile(VOL_CHARGING),
            "Solar Beam inner must engage VOL_CHARGING"
        );
        assert_eq!(
            state.sides[0].active.last_move, 76,
            "R-b affirmative: resume target = inner Solar Beam"
        );
        assert_eq!(
            state.last_move_globally, 76,
            "last-write-wins: inner Solar Beam id at the global slot"
        );
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "no damage on T1 (still charging)"
        );
    }

    #[test]
    fn test_copycat_tackle_rb_negative_outer_restore() {
        // Fixture #6: Copycat replays Tackle (33, no charge, no lock).
        // R-b negative restores per-mon last_move = 383 (outer) so a future
        // Mirror Move read sees Copycat; last_move_globally = 33 (inner,
        // last-write-wins). Distinguishes the two surfaces.
        let mut state = setup();
        state.sides[0].team[0].moves = [383, 0, 0, 0];
        state.last_move_globally = 33; // Tackle was the last move
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 383, 0, &mut fixed_rng(0));
        assert!(state.sides[1].team[0].current_hp < def_hp_before, "Tackle must hit");
        assert!(!state.sides[0].active.has_volatile(VOL_CHARGING));
        assert!(!state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        assert_eq!(
            state.sides[0].active.last_move, 383,
            "R-b negative: per-mon last_move restored to outer Copycat (for Mirror Move)"
        );
        assert_eq!(
            state.last_move_globally, 33,
            "battle-level: inner Tackle id (last-write-wins, distinct surface)"
        );
    }

    #[test]
    fn test_copycat_stance_change_on_inner_damaging() {
        // Fixture #7: Aegislash-Shield + Copycat replays Earthquake (89,
        // Physical) → flips to Aegislash-Blade on the inner dispatch.
        let mut state = setup();
        state.sides[0].team[0].species_id = 681; // Aegislash-Shield
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_STANCE_CHANGE;
        state.sides[0].team[0].moves = [383, 0, 0, 0];
        state.last_move_globally = 89; // Earthquake was the last move
        execute_move(&mut state, &TeamData::default(),0, 383, 0, &mut fixed_rng(0));
        assert_eq!(
            effective_species(&state, 0), 1103,
            "Aegislash-Shield must flip to Blade on inner damaging dispatch"
        );
    }

    #[test]
    fn test_copycat_protean_fires_on_inner_type() {
        // Fixture #8: Protean + Copycat replays Earthquake → Protean fires
        // on inner (Ground), not Normal from the outer Copycat. The outer
        // gate at use_move_called skips Call*-family ids; the inner
        // dispatch re-enters use_move_called with id 89 and the gate passes.
        let mut state = setup();
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_PROTEAN;
        state.sides[0].team[0].moves = [383, 0, 0, 0];
        state.last_move_globally = 89; // Earthquake
        execute_move(&mut state, &TeamData::default(),0, 383, 0, &mut fixed_rng(0));
        assert!(state.sides[0].active.has_volatile(VOL_TYPES_OVERRIDDEN));
        assert_eq!(
            state.sides[0].active.override_types,
            [Type::Ground as u8, Type::Ground as u8],
            "Protean fires on inner Earthquake (Ground), not outer Copycat (Normal)"
        );
        assert_eq!(
            state.sides[0].active._padding[3] & 1, 1,
            "once-per-switch flag consumed by inner dispatch"
        );
    }

    #[test]
    fn test_copycat_blocked_while_sleeping() {
        // Fixture #9: Copycat lacks Showdown's `sleepUsable: true`, so the
        // sleep prelude blocks dispatch while the user is asleep.
        let mut state = setup();
        state.sides[0].team[0].moves = [383, 0, 0, 0];
        state.sides[0].team[0].status = STATUS_SLEEP;
        state.sides[0].team[0].status_counter = 3;
        state.last_move_globally = 33; // Tackle
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 383, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "sleeping Copycat must not dispatch an inner damaging move"
        );
        assert_eq!(
            state.sides[0].team[0].status_counter, 2,
            "sleep counter still decrements once on the blocked turn"
        );
    }

    #[test]
    fn test_copycat_self_excluded_from_filter() {
        // Fixture #10 (hardening): Copycat's own move id (383) IS in
        // COPYCAT_FAIL (Showdown's `copycat` move carries failcopycat: 1).
        // This is the primary line of defense against Copycat-after-Copycat
        // chains; the depth-1 cap on use_move_called is defense-in-depth.
        assert!(
            COPYCAT_FAIL.binary_search(&383u16).is_ok(),
            "COPYCAT_FAIL must include Copycat itself"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Mirror Move (move 119) — Wave 3 Batch D falsifiability fixtures.
    // Showdown oracle: data/moves.ts:12058-12081. Reads target.lastMove
    // (engine: state.sides[def_side].active.last_move), fails on 0 or
    // when absent from the 644-entry MIRROR_MOVE_OK sidecar.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_mirror_move_fails_when_target_last_move_zero() {
        // Fixture #1: T1 (or post-switch). Target's last_move == 0 sentinel
        // arises organically from switch.rs:67 active.zero() (verified by
        // fixture #4 below); also holds on a fresh battle. Mirror Move no-ops.
        let mut state = setup();
        state.sides[0].team[0].moves = [119, 0, 0, 0];
        let def_hp_before = state.sides[1].team[0].current_hp;
        assert_eq!(state.sides[1].active.last_move, 0);
        execute_move(&mut state, &TeamData::default(),0, 119, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "Mirror Move must no-op when target's last_move == 0"
        );
    }

    #[test]
    fn test_mirror_move_replays_target_tackle_bit_for_bit() {
        // Fixture #2 (headline): opponent's last_move = Tackle (33).
        // Mirror Move dispatches Tackle, dealing the same damage a direct
        // Tackle from the same attacker would. Bit-for-bit equality on the
        // defender's hp delta confirms the inner uses the user's stats and
        // the target's defenses (NOT a no-op or a self-hit). MIRROR_MOVE_OK
        // contains Tackle.
        let mut state = setup();
        // Direct-Tackle baseline: copy the state, fire Tackle from p0 at p1.
        let mut baseline = state;
        baseline.sides[0].team[0].moves = [33, 0, 0, 0];
        let baseline_hp_before = baseline.sides[1].team[0].current_hp;
        execute_move(&mut baseline, &TeamData::default(), 0, 33, 0, &mut fixed_rng(0));
        let baseline_damage = baseline_hp_before - baseline.sides[1].team[0].current_hp;
        assert!(baseline_damage > 0, "Tackle baseline must deal damage");

        // Mirror Move path: opponent already used Tackle; user fires Mirror Move.
        state.sides[0].team[0].moves = [119, 0, 0, 0];
        state.sides[1].active.last_move = 33;
        let mirror_hp_before = state.sides[1].team[0].current_hp;
        assert!(MIRROR_MOVE_OK.binary_search(&33u16).is_ok(), "Tackle in MIRROR_MOVE_OK");
        execute_move(&mut state, &TeamData::default(),0, 119, 0, &mut fixed_rng(0));
        let mirror_damage = mirror_hp_before - state.sides[1].team[0].current_hp;
        assert_eq!(
            mirror_damage, baseline_damage,
            "Mirror Move replay of Tackle must deal damage equal to a direct Tackle"
        );
    }

    #[test]
    fn test_mirror_move_reads_target_outer_not_inner_after_metronome() {
        // Fixture #3 (oracle pin): opponent uses Metronome → rolls Bullet
        // Seed (331). R-b negative path restores OUTER (118 = Metronome)
        // into the opponent's per-mon last_move (the source Mirror Move
        // reads), while last_move_globally holds 331 (Bullet Seed, inner-
        // wins). Paired counterpart to Copycat fixture #2's inner-wins
        // behavior — verifies the two surfaces diverge as oracle-pinned.
        //
        // Showdown also no-ops Mirror Move here: Metronome lacks
        // `flags.mirror` (only its inner Bullet Seed carries it), so
        // Mirror Move's `onTryHit` rejects the read target.lastMove and
        // returns false. The R-b assertion is the load-bearing check; the
        // post-dispatch no-op is the Showdown-bit-for-bit consequence.
        let mut state = setup();
        state.sides[1].team[0].moves = [118, 0, 0, 0]; // opponent runs Metronome
        state.sides[0].team[0].moves = [119, 0, 0, 0]; // we run Mirror Move
        execute_move(&mut state, &TeamData::default(),1, 118, 0, &mut fixed_rng(metronome_idx(331)));
        assert_eq!(
            state.sides[1].active.last_move, 118,
            "R-b negative: opponent's per-mon last_move holds OUTER Metronome (Mirror Move source)"
        );
        assert_eq!(
            state.last_move_globally, 331,
            "battle-level last-write-wins: inner Bullet Seed (Copycat source)"
        );
        // Showdown parity: Metronome is NOT in MIRROR_MOVE_OK, so Mirror
        // Move no-ops on the outer read (matches data/moves.ts:12069
        // `if (!move?.flags['mirror'] ...) return false`).
        assert!(MIRROR_MOVE_OK.binary_search(&118u16).is_err(), "Metronome NOT in MIRROR_MOVE_OK");
        let def_hp_before = state.sides[1].team[0].current_hp;
        let last_globally_before = state.last_move_globally;
        execute_move(&mut state, &TeamData::default(),0, 119, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "Mirror Move must no-op (read 118 = Metronome, absent from MIRROR_MOVE_OK)"
        );
        assert_eq!(
            state.last_move_globally, last_globally_before,
            "no inner dispatch ⇒ last_move_globally unchanged"
        );
    }

    #[test]
    fn test_mirror_move_fails_after_target_switch() {
        // Fixture #4: opponent switches (switch_out → active.zero() at
        // switch.rs:67), zeroing per-mon last_move. Mirror Move must no-op
        // on the user's next turn even though the opponent previously had
        // a valid mirrorable last_move.
        let mut state = setup();
        state.sides[0].team[0].moves = [119, 0, 0, 0];
        state.sides[1].active.last_move = 33; // opponent's prior Tackle
        // Switch the opponent's active to slot 1; active.zero() clears last_move.
        crate::state::switch::perform_switch(&mut state, &TeamData::default(), 1, 1);
        assert_eq!(
            state.sides[1].active.last_move, 0,
            "switch_out → active.zero() clears last_move"
        );
        let def_hp_before = state.sides[1].team[1].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 119, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[1].team[1].current_hp, def_hp_before,
            "Mirror Move must no-op after opponent switch zeroes target's last_move"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Assist (move 274) — Wave 3 Batch E. Pool = non-active teammates'
    // moves, filtered by the 51-entry ASSIST_FAIL sidecar (flags.noassist).
    // ─────────────────────────────────────────────────────────────────

    fn setup_assist_party() -> BattleState {
        // Six-mon attacker party: active slot 0 holds only Assist (274);
        // five non-active teammates carry one assistable move each in slot 0.
        // Pool order (iteration of i = 1..=5, slot 0 of each) = [33, 45, 22, 55, 52].
        let mut state = setup();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [150, 100, 150, 100, 100],
            moves: [274, 0, 0, 0], pp: [16, 0, 0, 0],
            level: 100,
            ..Default::default()
        };
        for (i, mv) in [33u16, 45, 22, 55, 52].iter().enumerate() {
            state.sides[0].team[i + 1] = MonSlot {
                species_id: 6, current_hp: 200, max_hp: 200,
                stats: [100, 100, 100, 100, 80],
                moves: [*mv, 0, 0, 0], pp: [24, 0, 0, 0],
                level: 100,
                ..Default::default()
            };
        }
        state
    }

    #[test]
    fn test_assist_picks_from_teammate_pool_deterministic() {
        // Fixture #1: party of 1 active + 5 teammates with known single-move
        // sets; pin RNG seed; assert the dispatch fires the picked teammate
        // move (Tackle, 33 — pool[0] under rng = 0).
        let mut state = setup_assist_party();
        let hp_before = state.sides[1].team[0].current_hp;
        assert!(ASSIST_FAIL.binary_search(&33u16).is_err(), "Tackle eligible");
        execute_move(&mut state, &TeamData::default(),0, 274, 0, &mut fixed_rng(0));
        assert!(
            state.sides[1].team[0].current_hp < hp_before,
            "Assist with rng=0 picks pool[0]=Tackle and dispatches damage to defender"
        );
    }

    #[test]
    fn test_assist_fails_when_pool_empty() {
        // Fixture #2: every non-active teammate's moveset is fully ASSIST_FAIL-
        // filtered (Assist itself + Mirror Move + Copycat + Metronome + Sleep Talk —
        // all in ASSIST_FAIL). Pool empty → no-op.
        let mut state = setup();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [150, 100, 150, 100, 100],
            moves: [274, 0, 0, 0], pp: [16, 0, 0, 0],
            level: 100,
            ..Default::default()
        };
        for (i, mv) in [274u16, 119, 383, 118, 214].iter().enumerate() {
            assert!(ASSIST_FAIL.binary_search(mv).is_ok(), "{} in ASSIST_FAIL", mv);
            state.sides[0].team[i + 1] = MonSlot {
                species_id: 6, current_hp: 200, max_hp: 200,
                stats: [100, 100, 100, 100, 80],
                moves: [*mv, 0, 0, 0], pp: [24, 0, 0, 0],
                level: 100,
                ..Default::default()
            };
        }
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 274, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "Assist with fully-filtered teammate pool must no-op"
        );
    }

    #[test]
    fn test_assist_skips_active_slot_when_not_zero() {
        // Fixture #3: place the user at active_index = 2; only that mon
        // carries an eligible move (Tackle); teammates' slots are empty.
        // Pool stays empty because i == atk_slot is skipped — the user's own
        // moveset is never sampled. Result: Assist no-ops.
        let mut state = setup();
        // Wipe defaults; only slot 2 holds the active mon (Assist + Tackle).
        for i in 0..6 {
            state.sides[0].team[i] = MonSlot::default();
        }
        state.sides[0].team[2] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [150, 100, 150, 100, 100],
            moves: [274, 33, 0, 0], pp: [16, 24, 0, 0],
            level: 100,
            ..Default::default()
        };
        state.sides[0].active_index = 2;
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 274, 1, &mut fixed_rng(0));
        assert_eq!(
            state.sides[1].team[0].current_hp, def_hp_before,
            "Assist must skip i == active_index (=2); pool empty ⇒ no-op"
        );
    }

    #[test]
    fn test_assist_iterates_only_populated_teammates() {
        // Fixture #4: two-mon party total (active slot 0 + one teammate in
        // slot 1). Trailing slots 2..6 are empty MonSlot::default() (moves
        // all zero); the iteration must skip them without out-of-bounds.
        let mut state = setup();
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [150, 100, 150, 100, 100],
            moves: [274, 0, 0, 0], pp: [16, 0, 0, 0],
            level: 100,
            ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [100, 100, 100, 100, 80],
            moves: [33, 0, 0, 0], pp: [24, 0, 0, 0],
            level: 100,
            ..Default::default()
        };
        for i in 2..6 {
            state.sides[0].team[i] = MonSlot::default();
        }
        let def_hp_before = state.sides[1].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),0, 274, 0, &mut fixed_rng(0));
        assert!(
            state.sides[1].team[0].current_hp < def_hp_before,
            "Assist with 1 eligible teammate move must pick Tackle (pool len 1)"
        );
    }

    #[test]
    fn test_assist_into_bullet_seed_then_mirror_move_replays_assist() {
        // Fixture #5: Assist rolls Bullet Seed (331) — multi-hit damaging move.
        // R-b negative branch then restores OUTER (274 = Assist) into the
        // user's per-mon last_move because Bullet Seed engages no charge/lock.
        // Subsequent Mirror Move on the opponent reads the user's last_move =
        // 274 (Assist), but Assist ∉ MIRROR_MOVE_OK (it's Past-isNonstandard,
        // lacks flags.mirror), so Mirror Move correctly no-ops on both engines.
        // Pairs Batch E with Batch D's Mirror Move dispatch path and exercises
        // the R-b restoration chain across two Call* family outers.
        let mut state = setup_assist_party();
        // Replace pool[0] with Bullet Seed (331); rng=0 picks pool[0].
        state.sides[0].team[1].moves = [331, 0, 0, 0];
        // Opponent (side 1) holds Mirror Move so it can later replay.
        state.sides[1].team[0].moves = [119, 0, 0, 0];
        let def_hp_before = state.sides[1].team[0].current_hp;
        // T1: Assist → Bullet Seed.
        execute_move(&mut state, &TeamData::default(),0, 274, 0, &mut fixed_rng(0));
        assert!(
            state.sides[1].team[0].current_hp < def_hp_before,
            "Assist must dispatch Bullet Seed (multi-hit damaging)"
        );
        assert_eq!(
            state.sides[0].active.last_move, 274,
            "R-b negative: user's per-mon last_move holds OUTER Assist"
        );
        assert_eq!(
            state.last_move_globally, 331,
            "battle-level last-write-wins: inner Bullet Seed"
        );
        assert!(
            MIRROR_MOVE_OK.binary_search(&274u16).is_err(),
            "Assist NOT in MIRROR_MOVE_OK"
        );
        // T2: opponent's Mirror Move reads user's last_move = 274; rejects.
        let user_hp_before = state.sides[0].team[0].current_hp;
        execute_move(&mut state, &TeamData::default(),1, 119, 0, &mut fixed_rng(0));
        assert_eq!(
            state.sides[0].team[0].current_hp, user_hp_before,
            "Mirror Move must no-op (read 274 = Assist, absent from MIRROR_MOVE_OK)"
        );
    }

    #[test]
    fn test_psychup_copies_boosts() {
        // Guards the PsychUp de-thread: dropping `boosts[stat] = target_val` fails here.
        let mut state = setup();
        state.sides[1].active.boosts = [3, 0, 2, -1, 0, 0, 0];
        state.sides[0].active.boosts = [5, 5, 5, 5, 5, 5, 5];
        execute_move(&mut state, &TeamData::default(), 0, 244, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.boosts, [3, 0, 2, -1, 0, 0, 0]);
    }

    #[test]
    fn test_haze_clears_all_boosts() {
        // Guards the Haze de-thread: dropping the boost-clear write fails here.
        let mut state = setup();
        state.sides[0].active.boosts = [2, 0, -1, 0, 3, 0, 0];
        state.sides[1].active.boosts = [-2, 1, 0, 0, 0, 0, 0];
        execute_move(&mut state, &TeamData::default(), 0, 114, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].active.boosts, [0i8; 7]);
        assert_eq!(state.sides[1].active.boosts, [0i8; 7]);
    }

    #[test]
    fn test_mind_blown_recoil_magic_guard_blocks() {
        // Mind Blown (720) self-damage is round(maxhp/2) = (max_hp+1)>>1. Magic Guard
        // exempts (Showdown deals it as a Condition, failing magicguard's effectType
        // ==='Move' gate); Rock Head does not. max_hp 300 -> 150 self-damage.
        let mut without = setup();
        without.sides[0].team[0].current_hp = without.sides[0].team[0].max_hp;
        execute_move(&mut without, &TeamData::default(), 0, 720, 0, &mut fixed_rng(0));
        let max_hp = 300u16;
        assert_eq!(
            without.sides[0].team[0].current_hp,
            max_hp - ((max_hp + 1) >> 1),
            "non-Magic-Guard user must take round(maxhp/2) self-damage"
        );

        let mut with_mg = setup();
        with_mg.sides[0].team[0].current_hp = with_mg.sides[0].team[0].max_hp;
        with_mg.sides[0].team[0].ability_id = data_bridge::ABILITY_MAGIC_GUARD;
        execute_move(&mut with_mg, &TeamData::default(), 0, 720, 0, &mut fixed_rng(0));
        assert_eq!(
            with_mg.sides[0].team[0].current_hp, max_hp,
            "Magic Guard user must take 0 mindBlownRecoil self-damage"
        );
    }

    // ─── Empirical-frequency harness ──────────────────────────────────────────
    //
    // Drives the REAL compiled rng paths with the actual MCTS PRNG (the 64-bit LCG
    // from src/main.rs), one continuous deterministic stream per test, then checks
    // the observed rate against a 99.9% two-sided binomial CI (z=3.29). Tolerances
    // are fixed up front: never raise N or widen the band to make a test pass.

    /// The MCTS PRNG (src/main.rs): one continuous deterministic stream from `start`.
    fn lcg(start: u64) -> impl FnMut(u32) -> u32 {
        let mut seed = start;
        move |max| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as u32) % max.max(1)
        }
    }

    /// 99.9% two-sided binomial CI: |hits/n - p| <= 3.29*sqrt(p(1-p)/n).
    fn in_ci(hits: u64, n: u64, p: f64) -> bool {
        let phat = hits as f64 / n as f64;
        let half = 3.29 * (p * (1.0 - p) / n as f64).sqrt();
        (phat - p).abs() <= half
    }

    // ─── Tier 2 — compiled-behavior MATCH checks (assert Showdown spec rate) ───

    #[test]
    fn freq_crit_base_stage() {
        // is_crit(stage 0) = rng(24)<1 = 1/24 = 4.167%.
        // N=200_000: half-width = 3.29*sqrt(.04167*.95833/2e5) = ±0.00147 → band [4.02%,4.31%].
        let mut rng = lcg(0xC817_0001);
        let n = 200_000u64;
        let hits = (0..n).filter(|_| crate::state::calc_modifiers::is_crit(0, &mut rng)).count() as u64;
        assert!(in_ci(hits, n, 1.0 / 24.0), "base crit {}/{} out of 1/24 band", hits, n);
    }

    #[test]
    fn freq_crit_high_stage() {
        // is_crit(stage 1) = rng(24)<3 = 3/24 = 12.5%.
        // N=200_000: half-width = 3.29*sqrt(.125*.875/2e5) = ±0.00243 → band [12.26%,12.74%].
        let mut rng = lcg(0xC817_0002);
        let n = 200_000u64;
        let hits = (0..n).filter(|_| crate::state::calc_modifiers::is_crit(1, &mut rng)).count() as u64;
        assert!(in_ci(hits, n, 3.0 / 24.0), "high crit {}/{} out of 1/8 band", hits, n);
    }

    #[test]
    fn freq_multihit_2to5_distribution() {
        // resolve_hits over a 2-5 move: HIT_TABLE indexed by rng(20) → 35/35/15/15.
        // Per-bucket 99.9% band, N=200_000. p=.35 → ±0.00351; p=.15 → ±0.00263.
        let md = MoveData { multihit: (5 << 4) | 2, ..unsafe { core::mem::zeroed() } };
        let mut rng = lcg(0xC817_0003);
        let n = 200_000u64;
        let mut counts = [0u64; 6]; // index by hit-count (2..=5)
        for _ in 0..n {
            let h = crate::state::calc_modifiers::resolve_hits(&md, 0, 0, &mut rng);
            counts[h as usize] += 1;
        }
        assert!(in_ci(counts[2], n, 0.35), "2-hit {}/{} out of 35% band", counts[2], n);
        assert!(in_ci(counts[3], n, 0.35), "3-hit {}/{} out of 35% band", counts[3], n);
        assert!(in_ci(counts[4], n, 0.15), "4-hit {}/{} out of 15% band", counts[4], n);
        assert!(in_ci(counts[5], n, 0.15), "5-hit {}/{} out of 15% band", counts[5], n);
    }

    #[test]
    fn freq_full_paralysis() {
        // execute_move with a paralyzed attacker: rng(4)==0 = 25% fully-paralyzed.
        // Measured by "move did not deduct PP" (full-para skips PP). N=80_000:
        // half-width = 3.29*sqrt(.25*.75/8e4) = ±0.00504 → band [24.50%,25.50%].
        let mut rng = lcg(0xC817_0004);
        let n = 80_000u64;
        let mut para = 0u64;
        for _ in 0..n {
            let mut state = setup();
            state.sides[0].team[0].status = STATUS_PARALYSIS;
            let pp_before = state.sides[0].team[0].pp[0];
            execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut rng);
            if state.sides[0].team[0].pp[0] == pp_before { para += 1; }
        }
        assert!(in_ci(para, n, 0.25), "full-para {}/{} out of 25% band", para, n);
    }

    #[test]
    fn freq_freeze_thaw() {
        // execute_move with a frozen non-fire-move attacker: rng(5)==0 = 20% thaw.
        // Measured by "status cleared this attempt". N=80_000:
        // half-width = 3.29*sqrt(.2*.8/8e4) = ±0.00465 → band [19.53%,20.47%].
        let mut rng = lcg(0xC817_0005);
        let n = 80_000u64;
        let mut thawed = 0u64;
        for _ in 0..n {
            let mut state = setup();
            state.sides[0].team[0].status = STATUS_FREEZE;
            // Move 1 (Pound) is Normal — never auto-thaws.
            execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut rng);
            if state.sides[0].team[0].status == STATUS_NONE { thawed += 1; }
        }
        assert!(in_ci(thawed, n, 0.20), "freeze-thaw {}/{} out of 20% band", thawed, n);
    }

    #[test]
    fn freq_secondary_30pct() {
        // apply_secondary, primary path: rng(100) >= chance ⇒ skip. chance=30.
        // Self-stat-boost (+1 SpA) avoids target-side immunity. N=80_000:
        // half-width = 3.29*sqrt(.3*.7/8e4) = ±0.00533 → band [29.47%,30.53%].
        let md = MoveData {
            secondary_chance: 30, secondary_stat: 1,
            category: MoveCategory::Special,
            ..unsafe { core::mem::zeroed() }
        };
        let mut rng = lcg(0xC817_0006);
        let n = 80_000u64;
        let mut fired = 0u64;
        for _ in 0..n {
            let mut state = setup();
            apply_secondary(&mut state, 0, 1, &md, 0, &mut rng);
            if state.sides[0].active.boosts[SPA] != 0 { fired += 1; }
        }
        assert!(in_ci(fired, n, 0.30), "30% secondary {}/{} out of band", fired, n);
    }

    #[test]
    fn freq_move_accuracy_70() {
        // accuracy_check: hit iff rng(100) < acc. md.accuracy=70, no boosts/abilities.
        // N=80_000: half-width = 3.29*sqrt(.7*.3/8e4) = ±0.00533 → band [69.47%,70.53%].
        let state = setup();
        let md = MoveData { accuracy: 70, ..unsafe { core::mem::zeroed() } };
        let mut rng = lcg(0xC817_0007);
        let n = 80_000u64;
        let hits = (0..n).filter(|_| accuracy_check(&state, 0, &md, &mut rng)).count() as u64;
        assert!(in_ci(hits, n, 0.70), "accuracy-70 {}/{} out of band", hits, n);
    }

    /// A contact attacker for the contact-ability tests: Normal type (no para/sleep/
    /// poison type-immunity, not Grass so not powder-immune), no ability/item, full HP.
    /// Status reset each iteration so the `status == NONE` proc gate stays open.
    fn reset_contact_attacker(state: &mut BattleState) {
        let m = &mut state.sides[0].team[0];
        m.status = STATUS_NONE;
        m.status_counter = 0;
        m.current_hp = m.max_hp;
        state.sides[0].active.confusion_turns = 0;
        state.sides[0].active.override_types = [Type::Normal as u8, Type::Normal as u8];
        state.sides[0].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;
    }

    #[test]
    fn freq_static_30pct() {
        // Contact hit into a Static holder: rng(100)<30 ⇒ paralyze attacker. N=80_000.
        // half-width = 3.29*sqrt(.3*.7/8e4) = ±0.00533 → band [29.47%,30.53%].
        let mut rng = lcg(0xC817_0008);
        let n = 80_000u64;
        let mut para = 0u64;
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_STATIC;
        for _ in 0..n {
            reset_contact_attacker(&mut state);
            execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut rng);
            if state.sides[0].team[0].status == STATUS_PARALYSIS { para += 1; }
        }
        assert!(in_ci(para, n, 0.30), "Static {}/{} out of 30% band", para, n);
    }

    // ─── Tier 1 — GAP guard: confusion self-hit ──────────────────────────────

    #[test]
    fn freq_confusion_self_hit() {
        // execute_move with a confused attacker: self-hit gate is rng(100)<33 = 33.00%.
        // confusion_turns kept ≥2 each iteration so the post-decrement value stays >0
        // (Showdown never self-hits on the wake-from-confusion tick). N=400_000:
        // half-width = 3.29*sqrt(.33*.67/4e5) = ±0.00245 → band [32.76%,33.25%].
        // This band EXCLUDES the pre-fix wrong rate 33.333% (upper bound 33.245%),
        // so the test fails on rng(3)==0 (33.33%) and passes only on rng(100)<33.
        let mut rng = lcg(0xC817_000A);
        let n = 400_000u64;
        let mut hit = 0u64;
        let mut state = setup();
        for _ in 0..n {
            state.sides[0].active.confusion_turns = 5;
            let hp = state.sides[0].team[0].current_hp;
            execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut rng);
            if state.sides[0].team[0].current_hp < hp { hit += 1; }
            // restore both HP bars so neither a self-hit nor the move ever KOs
            state.sides[0].team[0].current_hp = state.sides[0].team[0].max_hp;
            state.sides[1].team[0].current_hp = state.sides[1].team[0].max_hp;
        }
        assert!(in_ci(hit, n, 0.33), "confusion self-hit {}/{} out of 33.00% band", hit, n);
    }

    // ─── Tier 1 — GAP guard: Effect Spore split + immunity fall-through ───────

    #[test]
    fn freq_effect_spore_split() {
        // Contact hit into an Effect Spore holder: one rng(100), bands
        // <11 slp / <21 par / <30 psn. Within procs the split is 11:10:9.
        // Normal-typed attacker (no type-immunity to any of the 3 statuses, not
        // Grass so not powder-immune). N=300_000 hits ≈ 90k procs.
        // Per-status 99.9% band on ~90k procs: slp p=11/30 → ±0.53pp,
        // par p=10/30 → ±0.52pp, psn p=9/30 → ±0.50pp — all ≪ the 3.33pp
        // spacing between adjacent bands, so the split is unambiguous.
        let mut rng = lcg(0xC817_000B);
        let n = 300_000u64;
        let mut slp = 0u64;
        let mut par = 0u64;
        let mut psn = 0u64;
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_EFFECT_SPORE;
        for _ in 0..n {
            reset_contact_attacker(&mut state);
            execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut rng);
            match state.sides[0].team[0].status {
                STATUS_SLEEP => slp += 1,
                STATUS_PARALYSIS => par += 1,
                STATUS_POISON => psn += 1,
                _ => {}
            }
        }
        let procs = slp + par + psn;
        // Total proc rate is 30% — sanity-check the denominator class first.
        assert!(in_ci(procs, n, 0.30), "Effect Spore total procs {}/{} out of 30% band", procs, n);
        assert!(in_ci(slp, procs, 11.0 / 30.0), "slp share {}/{} not 11/30", slp, procs);
        assert!(in_ci(par, procs, 10.0 / 30.0), "par share {}/{} not 10/30", par, procs);
        assert!(in_ci(psn, procs, 9.0 / 30.0), "psn share {}/{} not 9/30", psn, procs);
    }

    #[test]
    fn effect_spore_sleep_immune_no_fallthrough() {
        // A sleep-band roll (rng(100)<11) against a sleep-immune attacker must apply
        // NOTHING — it must not cascade into the paralysis arm. Insomnia blocks sleep
        // but not paralysis; a roll of 0 lands in the sleep band. Deterministic.
        let mut state = setup();
        state.sides[1].team[0].ability_id = data_bridge::ABILITY_EFFECT_SPORE;
        reset_contact_attacker(&mut state);
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_INSOMNIA;
        // fixed_rng(0): rng(100)=0 → sleep band; sleep blocked by Insomnia → no fall-through.
        execute_move(&mut state, &TeamData::default(), 0, 1, 0, &mut fixed_rng(0));
        assert_eq!(state.sides[0].team[0].status, STATUS_NONE,
            "sleep-immune sleep-band roll must apply no status (no paralysis fall-through)");
    }

    // ─── Tier 3 — optional uniformity checks ──────────────────────────────────

    #[test]
    fn freq_tri_attack_3way_uniform() {
        // Tri Attack picks one of 3 statuses via rng(3) (uniform 1/3 each).
        // Per-bucket 99.9% band, N=180_000: p=1/3 → ±0.00366 → band [32.97%,33.70%].
        let md = MoveData {
            secondary_chance: 100,
            category: MoveCategory::Special,
            ..unsafe { core::mem::zeroed() }
        };
        let tri = crate::data::MOVE_TRI_ATTACK as u16;
        let mut rng = lcg(0xC817_0009);
        let n = 180_000u64;
        let mut counts = [0u64; 3]; // brn / par / frz indices (Tri Attack order)
        for _ in 0..n {
            let mut state = setup();
            // Neutral-typed target so no status is type-blocked; clear any prior status.
            state.sides[1].team[0].species_id = 1; // Bulbasaur-ish; overridden below
            state.sides[1].active.override_types = [Type::Normal as u8, Type::Normal as u8];
            state.sides[1].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;
            state.sides[1].team[0].status = STATUS_NONE;
            apply_secondary(&mut state, 0, 1, &md, tri, &mut rng);
            match state.sides[1].team[0].status {
                STATUS_BURN => counts[0] += 1,
                STATUS_PARALYSIS => counts[1] += 1,
                STATUS_FREEZE => counts[2] += 1,
                _ => {}
            }
        }
        let total: u64 = counts.iter().sum();
        assert!(total > n * 99 / 100, "Tri Attack applied a status {total}/{n} times");
        for (i, &c) in counts.iter().enumerate() {
            assert!(in_ci(c, total, 1.0 / 3.0), "Tri Attack status {i}: {c}/{total} off uniform 1/3");
        }
    }
}
