//! Damage calculator modifier helpers.
//!
//! All modifier functions return (numerator, denominator) pairs in
//! 4096-scale, or modify values in-place.  Zero heap allocations.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemData, ItemFlag, MoveCategory};
use crate::state::accessors::*;
use crate::data::moves::{MoveData, MoveFlags, MoveEffect, SelfEffect, VarPower};
use crate::data::types::Type;
use crate::data::{MOVE_EARTHQUAKE, MOVE_BULLDOZE, MOVE_MAGNITUDE};

/// Apply a 4096-scale modifier using Showdown's `modify()` rounding.
/// Formula: floor((value * num + 2047) / 4096).
/// Rounds 0.5 DOWN (towards zero), matching Showdown exactly.
#[inline(always)]
pub fn chain_mod(value: u32, num: u32) -> u32 {
    if num == 4096 { return value; }
    (value * num + 2047) >> 12
}

/// Check if attacker's ability suppresses the target's ability during damage calc.
/// Mold Breaker / Turboblaze / Teravolt bypass defensive abilities.
#[inline(always)]
pub fn is_mold_breaker(atk_ability: u16) -> bool {
    matches!(atk_ability,
        data_bridge::ABILITY_MOLD_BREAKER |
        data_bridge::ABILITY_TURBOBLAZE |
        data_bridge::ABILITY_TERAVOLT
    )
}

/// True iff the given side has an *effective* Heatproof ability (not suppressed
/// by Neutralizing Gas or other ability-suppression).  Used both in damage calc
/// (0.5× Fire-move Atk/SpA) and in burn residual halving.
#[inline(always)]
pub fn is_heatproof_effective(state: &BattleState, side: usize) -> bool {
    effective_ability(state, side) == data_bridge::ABILITY_HEATPROOF
}

/// True iff attacker's Mold Breaker (or equivalent) effectively bypasses the
/// defender's ability — i.e., attacker has MB *and* defender is NOT shielded
/// by Ability Shield. Inline branch, no allocations.
#[inline(always)]
pub fn mold_breaks(state: &BattleState, def_side: usize, atk_ability: u16) -> bool {
    if !is_mold_breaker(atk_ability) { return false; }
    let def_item_id = state.active_mon(def_side).item_id;
    if def_item_id == 0 { return true; }
    !data_bridge::item(def_item_id).has(ItemFlag::ABILITY_SHIELD)
}

/// Returns (num, den) in 4096-scale for weather's effect on move damage.
/// Returns (0, 4096) if the move is completely nullified (Harsh Sun vs Water).
#[inline]
pub fn weather_modifier(weather: u8, move_type: Type) -> (u32, u32) {
    match (weather, move_type) {
        (WEATHER_SUN, Type::Fire)          => (6144, 4096),  // 1.5×
        (WEATHER_SUN, Type::Water)         => (2048, 4096),  // 0.5×
        (WEATHER_RAIN, Type::Water)        => (6144, 4096),
        (WEATHER_RAIN, Type::Fire)         => (2048, 4096),
        (WEATHER_HARSH_SUN, Type::Water)   => (0, 4096),     // nullified
        (WEATHER_HEAVY_RAIN, Type::Fire)   => (0, 4096),
        _ => (4096, 4096),
    }
}

/// Returns (num, den) in 4096-scale for terrain's effect on move damage.
/// Checks attacker grounding for type boosts, defender grounding for
/// Misty Dragon-type weakening and Grassy Earthquake/Bulldoze/Magnitude weakening.
#[inline]
pub fn terrain_modifier(
    state: &BattleState, atk_side: usize, def_side: usize,
    move_type: Type, move_id: u16,
) -> (u32, u32) {
    match state.field.terrain {
        TERRAIN_ELECTRIC => {
            if move_type == Type::Electric && is_grounded(state, atk_side) {
                (5325, 4096) // 1.3×
            } else {
                (4096, 4096)
            }
        }
        TERRAIN_GRASSY => {
            let mid = move_id as usize;
            if (mid == MOVE_EARTHQUAKE || mid == MOVE_BULLDOZE || mid == MOVE_MAGNITUDE)
                && is_grounded(state, def_side)
            {
                (2048, 4096) // 0.5×
            } else if move_type == Type::Grass && is_grounded(state, atk_side) {
                (5325, 4096) // 1.3×
            } else {
                (4096, 4096)
            }
        }
        TERRAIN_PSYCHIC => {
            if move_type == Type::Psychic && is_grounded(state, atk_side) {
                (5325, 4096) // 1.3×
            } else {
                (4096, 4096)
            }
        }
        TERRAIN_MISTY => {
            if move_type == Type::Dragon && is_grounded(state, def_side) {
                (2048, 4096) // 0.5×
            } else {
                (4096, 4096)
            }
        }
        _ => (4096, 4096),
    }
}

/// Returns (num, den) for Reflect / Light Screen / Aurora Veil.
/// Crits ignore screens.
#[inline]
pub fn screen_modifier(
    state: &BattleState, def_side: usize, category: MoveCategory, is_crit: bool,
) -> (u32, u32) {
    if is_crit { return (4096, 4096); }
    let sc = &state.sides[def_side].side_conditions;
    if sc.aurora_veil_turns > 0 { return (2048, 4096); }
    match category {
        MoveCategory::Physical if sc.reflect_turns > 0 => (2048, 4096),
        MoveCategory::Special  if sc.light_screen_turns > 0 => (2048, 4096),
        _ => (4096, 4096),
    }
}

/// Returns (num, den) for STAB.
/// Handles Tera STAB rules:
/// - Tera type matches original type: 2× STAB on tera-type moves
/// - Tera type doesn't match: 1.5× on tera-type moves, 1.5× on original-type moves
#[inline]
pub fn stab_modifier(state: &BattleState, atk_side: usize, move_type: Type) -> (u32, u32) {
    let mon = state.active_mon(atk_side);
    let ability = effective_ability(state, atk_side);
    let mt = move_type as u8;

    if mon.is_terastallized() {
        let tera_type = mon.tera_type;
        let sp = data_bridge::species(mon.species_id);
        let orig_t1 = sp.type1 as u8;
        let orig_t2 = sp.type2 as u8;
        let matches_tera = mt == tera_type;
        let matches_original = mt == orig_t1 || mt == orig_t2;

        if matches_tera && matches_original {
            // Tera type == original type: 2× (or 2.25× with Adaptability)
            if ability == data_bridge::ABILITY_ADAPTABILITY {
                (9216, 4096) // 2.25×
            } else {
                (8192, 4096) // 2.0×
            }
        } else if matches_tera || matches_original {
            // Tera type XOR original type: 1.5×
            if ability == data_bridge::ABILITY_ADAPTABILITY {
                (8192, 4096) // 2.0×
            } else {
                (6144, 4096) // 1.5×
            }
        } else {
            (4096, 4096) // No STAB
        }
    } else {
        let has_stab = has_type(state, atk_side, mt);
        if !has_stab {
            return (4096, 4096);
        }
        if ability == data_bridge::ABILITY_ADAPTABILITY {
            (8192, 4096) // 2.0×
        } else {
            (6144, 4096) // 1.5×
        }
    }
}

/// Compute the effective crit stage (0-4+).
#[inline]
pub fn crit_stage(state: &BattleState, atk_side: usize, md: &MoveData) -> u8 {
    let active = &state.sides[atk_side].active;
    let mon = state.active_mon(atk_side);

    // Laser Focus = guaranteed crit
    if active.has_volatile(VOL_LASER_FOCUS) { return 4; }

    let mut stage = md.crit_ratio;

    if active.has_volatile(VOL_FOCUS_ENERGY) { stage += 2; }

    let ability = effective_ability(state, atk_side);
    if ability == data_bridge::ABILITY_SUPER_LUCK { stage += 1; }

    if state.field.magic_room_turns() == 0 {
        let item = data_bridge::item(mon.item_id);
        if item.has(ItemFlag::CRIT_BOOST) { stage += 1; }
    }

    stage
}

/// Determine if this hit is a crit.
/// Always calls rng(24) for the crit check, matching Showdown's
/// `randomChance(1, critMult[stage])` which varies the threshold
/// but uses a consistent RNG range.
#[inline]
pub fn is_crit(stage: u8, rng: &mut impl FnMut(u32) -> u32) -> bool {
    // crit_threshold: how many of 24 values result in a crit
    // stage 0: 1/24, stage 1: 3/24=1/8, stage 2: 12/24=1/2, stage 3+: guaranteed
    let crit_threshold = match stage {
        0 => 1,    // 1/24
        1 => 3,    // 3/24 = 1/8
        2 => 12,   // 12/24 = 1/2
        _ => 24,   // guaranteed
    };
    rng(24) < crit_threshold
}

/// Crit damage multiplier. Always 1.5x; Sniper's extra 1.5x is applied
/// separately via `sniper_final_mod` in the final damage chain (matching Showdown's
/// two-step: 1.5x crit in formula, then 1.5x Sniper via onModifyDamage).
#[inline]
pub fn crit_multiplier(_atk_ability: u16) -> (u32, u32) {
    (6144, 4096) // 1.5×
}

/// Sniper: additional 1.5x final damage modifier on crits.
/// Applied via chain_mod in the per-hit loop after other final mods.
#[inline]
pub fn sniper_final_mod(atk_ability: u16, is_crit: bool) -> u32 {
    if is_crit && atk_ability == data_bridge::ABILITY_SNIPER {
        6144 // 1.5×
    } else {
        4096 // 1.0×
    }
}

/// Returns (num, den) for the burn penalty on physical moves.
/// Facade bypasses burn's Atk halving in Gen 9.
#[inline]
pub fn burn_modifier(
    atk_status: u8, category: MoveCategory, atk_ability: u16, is_facade: bool,
) -> (u32, u32) {
    if atk_status != STATUS_BURN { return (4096, 4096); }
    if category != MoveCategory::Physical { return (4096, 4096); }
    if atk_ability == data_bridge::ABILITY_GUTS { return (4096, 4096); } // Guts ignores burn
    if is_facade { return (4096, 4096); } // Facade ignores burn Atk halving
    (2048, 4096) // 0.5×
}

/// Resolve variable base power moves.  Returns the effective base power.
/// Returns u16 because Spit Up at 3 stacks reaches 300 BP (overflows u8).
#[inline]
pub fn resolve_power(
    state: &BattleState, md: &MoveData, atk_side: usize, def_side: usize,
) -> u16 {
    if md.var_power == VarPower::None {
        return md.base_power as u16;
    }

    let atk_mon = state.active_mon(atk_side);
    let def_mon = state.active_mon(def_side);
    let atk_species = data_bridge::species(atk_mon.species_id);
    let def_species = data_bridge::species(def_mon.species_id);

    // For speed-based moves, apply boost to raw stat
    let atk_spe = boosted_stat(
        effective_stat(state, atk_side, SPE),
        state.sides[atk_side].active.boosts[SPE],
    );
    let def_spe = boosted_stat(
        effective_stat(state, def_side, SPE),
        state.sides[def_side].active.boosts[SPE],
    );

    match md.var_power {
        VarPower::Weight => crate::data::moves::weight_based_bp(def_species.weight) as u16,
        VarPower::HeavySlam => crate::data::moves::heavy_slam_bp(atk_species.weight, def_species.weight) as u16,
        VarPower::GyroBall => crate::data::moves::gyro_ball_bp(atk_spe, def_spe) as u16,
        VarPower::Eruption => crate::data::moves::eruption_bp(atk_mon.current_hp, atk_mon.max_hp) as u16,
        VarPower::Flail => crate::data::moves::flail_bp(atk_mon.current_hp, atk_mon.max_hp) as u16,
        VarPower::ElectroBall => crate::data::moves::electro_ball_bp(atk_spe, def_spe) as u16,
        VarPower::StoredPower => {
            let pos: u8 = state.sides[atk_side].active.boosts.iter()
                .filter(|&&b| b > 0).map(|&b| b as u8).sum();
            crate::data::moves::stored_power_bp(pos) as u16
        }
        VarPower::Punishment => {
            let pos: u8 = state.sides[def_side].active.boosts.iter()
                .filter(|&&b| b > 0).map(|&b| b as u8).sum();
            crate::data::moves::punishment_bp(pos) as u16
        }
        VarPower::Facade => {
            if atk_mon.status != STATUS_NONE { 140 } else { 70 }
        }
        VarPower::Hex => {
            if def_mon.status != STATUS_NONE { 130 } else { 65 }
        }
        VarPower::Acrobatics => {
            if atk_mon.item_id == 0 || state.field.magic_room_turns() > 0 { 110 } else { 55 }
        }
        VarPower::RisingVoltage => {
            // 2× if Electric Terrain and target is grounded
            if state.field.terrain == TERRAIN_ELECTRIC
                && is_grounded(state, def_side)
            {
                140
            } else {
                70
            }
        }
        VarPower::SpitUp => {
            // Up to 300 BP at 3 stockpile stacks — needs u16 arithmetic.
            let count = (state.sides[atk_side].active.stockpile & 0x7F) as u16;
            if count == 0 { 0 } else { count * 100 }
        }
        VarPower::Brine => {
            // 2× if target's current HP ≤ 50% of max
            if def_mon.current_hp * 2 <= def_mon.max_hp { 130 } else { 65 }
        }
        VarPower::Payback => {
            // 2× if user moves after target (target already moved this turn)
            if state.sides[def_side].active.has_volatile(VOL_MOVED_THIS_TURN) { 100 } else { 50 }
        }
        VarPower::Avalanche => {
            // 2× if user was hit this turn (Avalanche, Revenge)
            if state.sides[atk_side].active.times_hit > 0 { 120 } else { 60 }
        }
        VarPower::FuryCutter => {
            // 40 BP base, doubles on each consecutive successful hit.
            // consec_move_count tracks consecutive uses of the same move
            // (incremented per selection, reset on switch-out and move
            // change; engine additionally resets on miss — see move_exec.rs).
            // Count=1 → 40, 2 → 80, 3+ → 160.
            let count = state.sides[atk_side].active.consec_move_count;
            match count {
                0 | 1 => 40,
                2 => 80,
                _ => 160,
            }
        }
        _ => md.base_power as u16,
    }
}

/// Returns (num, den) in 4096-scale for attacker's ability effect on power.
#[inline]
pub fn ability_power_mod(
    state: &BattleState, md: &MoveData, atk_side: usize, power: u16,
) -> (u32, u32) {
    let ability = effective_ability(state, atk_side);
    let atk_mon = state.active_mon(atk_side);

    match ability {
        data_bridge::ABILITY_TECHNICIAN if power <= 60 => (6144, 4096), // 1.5×
        data_bridge::ABILITY_RECKLESS if md.drain < 0 || md.self_effect == SelfEffect::CrashDamage => (4915, 4096), // 1.2×
        data_bridge::ABILITY_IRON_FIST if md.flags & MoveFlags::PUNCH != 0 => (4915, 4096),
        data_bridge::ABILITY_MEGA_LAUNCHER if md.flags & MoveFlags::PULSE != 0 => (6144, 4096),
        data_bridge::ABILITY_STRONG_JAW if md.flags & MoveFlags::BITE != 0 => (6144, 4096),
        data_bridge::ABILITY_TOUGH_CLAWS if md.flags & MoveFlags::CONTACT != 0 => (5325, 4096), // 1.3×
        data_bridge::ABILITY_SHEER_FORCE if md.secondary_chance > 0 => (5325, 4096),
        data_bridge::ABILITY_SHARPNESS if md.flags & MoveFlags::SLICE != 0 => (6144, 4096), // 1.5×
        data_bridge::ABILITY_PUNK_ROCK if md.flags & MoveFlags::SOUND != 0 => (5325, 4096), // 1.3×
        data_bridge::ABILITY_SAND_FORCE
            if matches!(md.move_type, Type::Rock | Type::Ground | Type::Steel)
            && effective_weather(state) == WEATHER_SAND => (5325, 4096), // 1.3×
        // Analytic: 1.3× if no other active mon still has a pending move this
        // turn. Mirrors abilities.ts:analytic — boost is gated by
        // `this.queue.willMove(target)`, which is false both when the target
        // already moved and when the target switched in this turn (newlySwitched
        // also has no pending move). Engine signal: pending_actions[def] is set
        // to 0xFF the moment the defender's queued action resolves (move or
        // switch — see turn.rs:execute_turn). Requiring atk's slot to still be
        // populated keeps calc-only mode (both slots default 0xFF) from
        // spuriously activating Analytic.
        data_bridge::ABILITY_ANALYTIC
            if state.pending_actions[1 - atk_side] == 0xFF
                && state.pending_actions[atk_side] != 0xFF
            => (5325, 4096),

        // Rivalry: 1.25× same gender, 0.75× opposite. Showdown short-circuits to
        // 1.0× when either side is genderless.
        data_bridge::ABILITY_RIVALRY => {
            let def_mon = state.active_mon(1 - atk_side);
            if (atk_mon.flags | def_mon.flags) & MON_FLAG_GENDERLESS != 0 {
                (4096, 4096)
            } else {
                let same = (atk_mon.flags & MON_FLAG_FEMALE) == (def_mon.flags & MON_FLAG_FEMALE);
                if same { (5120, 4096) } else { (3072, 4096) }
            }
        }

        // Pinch abilities (Overgrow/Blaze/Torrent/Swarm) moved to ability_atk_stat_mod
        // because Showdown applies them via onModifyAtk/SpA. Keeping them here would
        // round at the wrong stage and produce off-by-one damage when STAB also applies.

        // Flash Fire: 1.5× Fire when activated
        data_bridge::ABILITY_FLASH_FIRE if md.move_type == Type::Fire
            && state.sides[atk_side].active.has_volatile(VOL_FLASH_FIRE) => (6144, 4096),

        // Supreme Overlord: 1.1× per fainted ally (max 5)
        // Exact Showdown lookup table (abilities.ts:4649) — values are NOT evenly spaced
        data_bridge::ABILITY_SUPREME_OVERLORD => {
            let slot = state.sides[atk_side].active_index as usize;
            let fainted = (0..6).filter(|&i| {
                i != slot
                    && state.sides[atk_side].team[i].species_id != 0
                    && state.sides[atk_side].team[i].current_hp == 0
            }).count().min(5);
            const POW_MOD: [u32; 6] = [4096, 4506, 4915, 5325, 5734, 6144];
            (POW_MOD[fainted], 4096)
        }

        _ => (4096, 4096),
    }
}

/// Returns 4096-scale numerator for field-wide Aura abilities (Dark Aura / Fairy Aura).
/// If Aura Break is present on either side, the boost becomes 0.75x instead of 1.33x.
/// `a0` and `a1` are the effective abilities of side 0 and side 1 (pre-computed by caller).
#[inline(always)]
pub fn aura_power_mod(a0: u16, a1: u16, move_type: Type) -> u32 {
    // Quick reject: these abilities are rare, so most calls will exit here
    let dark_aura = a0 == data_bridge::ABILITY_DARK_AURA || a1 == data_bridge::ABILITY_DARK_AURA;
    let fairy_aura = a0 == data_bridge::ABILITY_FAIRY_AURA || a1 == data_bridge::ABILITY_FAIRY_AURA;
    if !dark_aura && !fairy_aura { return 4096; }

    let has_aura_break = a0 == data_bridge::ABILITY_AURA_BREAK || a1 == data_bridge::ABILITY_AURA_BREAK;

    // Dark Aura: boost Dark-type moves (5448/4096 = 1.33x, or 3072/4096 = 0.75x with Aura Break)
    if move_type == Type::Dark && dark_aura {
        return if has_aura_break { 3072 } else { 5448 };
    }
    // Fairy Aura: boost Fairy-type moves
    if move_type == Type::Fairy && fairy_aura {
        return if has_aura_break { 3072 } else { 5448 };
    }
    4096
}

/// Modify the offensive stat A based on attacker's ability.
/// `weather`, `hp`, `max_hp`, `turns_active`, `def_turns_active`, `field_turn` are passed
/// to avoid needing the full state reference.
#[inline]
pub fn ability_atk_stat_mod(
    a: u16, ability: u16, category: MoveCategory, status: u8,
    move_type: Type, weather: u8, hp: u16, max_hp: u16, turns_active: u8,
    def_turns_active: u8, field_turn: u16, terrain: u8,
) -> u16 {
    match ability {
        data_bridge::ABILITY_HUGE_POWER | data_bridge::ABILITY_PURE_POWER
            if category == MoveCategory::Physical => a * 2,
        data_bridge::ABILITY_HUSTLE
            if category == MoveCategory::Physical => (a as u32 * 3 / 2) as u16,
        data_bridge::ABILITY_GUTS
            if category == MoveCategory::Physical && status != STATUS_NONE
            => (a as u32 * 3 / 2) as u16,
        data_bridge::ABILITY_SOLAR_POWER
            if category == MoveCategory::Special
            && matches!(weather, WEATHER_SUN | WEATHER_HARSH_SUN)
            => (a as u32 * 3 / 2) as u16,
        data_bridge::ABILITY_GORILLA_TACTICS
            if category == MoveCategory::Physical => (a as u32 * 3 / 2) as u16,
        // Stakeout: 2x when target just switched in (turns_active==0 and game has started)
        // In Showdown, activeTurns is incremented at turn start, so leads have activeTurns=1
        // on their first real action. field_turn>0 ensures we don't fire on turn 1 leads
        // or in calc_damage mode (where field.turn=0).
        data_bridge::ABILITY_STAKEOUT
            if def_turns_active == 0 && field_turn > 0 => a * 2,
        data_bridge::ABILITY_SLOW_START
            if turns_active < 5 => a / 2,
        data_bridge::ABILITY_DEFEATIST
            if hp * 2 <= max_hp => a / 2,
        data_bridge::ABILITY_FLOWER_GIFT
            if category == MoveCategory::Physical
            && matches!(weather, WEATHER_SUN | WEATHER_HARSH_SUN)
            => (a as u32 * 3 / 2) as u16,
        // Type-specific stat mods (Showdown uses onModifyAtk/onModifySpA)
        data_bridge::ABILITY_WATER_BUBBLE
            if move_type == Type::Water => a * 2,
        data_bridge::ABILITY_DRAGONS_MAW
            if move_type == Type::Dragon => (a as u32 * 3 / 2) as u16,
        data_bridge::ABILITY_TRANSISTOR
            if move_type == Type::Electric => (a as u32 * 5325 / 4096) as u16, // 1.3×
        data_bridge::ABILITY_STEELWORKER
            if move_type == Type::Steel => (a as u32 * 3 / 2) as u16,
        data_bridge::ABILITY_ROCKY_PAYLOAD
            if move_type == Type::Rock => (a as u32 * 3 / 2) as u16,
        // Orichalcum Pulse: 5461/4096 (~1.33×) Atk in sun
        data_bridge::ABILITY_ORICHALCUM_PULSE
            if category == MoveCategory::Physical
            && matches!(weather, WEATHER_SUN | WEATHER_HARSH_SUN)
            => (a as u32 * 5461 / 4096) as u16,
        // Hadron Engine: 5461/4096 (~1.33×) SpA on Electric Terrain
        data_bridge::ABILITY_HADRON_ENGINE
            if category == MoveCategory::Special
            && terrain == TERRAIN_ELECTRIC
            => (a as u32 * 5461 / 4096) as u16,

        // Pinch abilities: 1.5× attacking stat when HP ≤ 1/3 and move type matches.
        // Showdown applies these via onModifyAtk/SpA with chainModify(1.5), so we
        // must round on the stat (not on base power) to match.
        data_bridge::ABILITY_OVERGROW
            if move_type == Type::Grass && hp * 3 <= max_hp
            => chain_mod(a as u32, 6144) as u16,
        data_bridge::ABILITY_BLAZE
            if move_type == Type::Fire && hp * 3 <= max_hp
            => chain_mod(a as u32, 6144) as u16,
        data_bridge::ABILITY_TORRENT
            if move_type == Type::Water && hp * 3 <= max_hp
            => chain_mod(a as u32, 6144) as u16,
        data_bridge::ABILITY_SWARM
            if move_type == Type::Bug && hp * 3 <= max_hp
            => chain_mod(a as u32, 6144) as u16,
        _ => a,
    }
}

/// Modify the defensive stat D based on defender's ability.
#[inline]
pub fn ability_def_stat_mod(
    d: u16, ability: u16, category: MoveCategory, move_type: Type,
    status: u8, weather: u8, terrain: u8,
) -> u16 {
    match ability {
        data_bridge::ABILITY_FUR_COAT if category == MoveCategory::Physical => d * 2,
        // Thick Fat moved to defender_atk_stat_mod (Showdown onSourceModifyAtk/SpA
        // applies it as 0.5× on the attacker, not 2× on the defender — different
        // truncation point produces a different damage value).
        data_bridge::ABILITY_MARVEL_SCALE
            if category == MoveCategory::Physical && status != STATUS_NONE
            => (d as u32 * 3 / 2) as u16,
        data_bridge::ABILITY_GRASS_PELT
            if category == MoveCategory::Physical && terrain == TERRAIN_GRASSY
            => (d as u32 * 3 / 2) as u16,
        data_bridge::ABILITY_FLOWER_GIFT
            if category == MoveCategory::Special
            && matches!(weather, WEATHER_SUN | WEATHER_HARSH_SUN)
            => (d as u32 * 3 / 2) as u16,
        _ => d,
    }
}

/// Returns (num, den) for defender's ability effect on final damage.
/// `atk_ability` is used to check for Mold Breaker bypassing defensive abilities.
#[inline]
pub fn defender_ability_final_mod(
    state: &BattleState, md: &MoveData, def_side: usize, effectiveness: u8, atk_ability: u16,
) -> (u32, u32) {
    // Mold Breaker / Turboblaze / Teravolt bypass defensive abilities
    // (unless the defender holds Ability Shield).
    if mold_breaks(state, def_side, atk_ability) { return (4096, 4096); }

    let ability = effective_ability(state, def_side);
    let def_mon = state.active_mon(def_side);

    match ability {
        // Multiscale / Shadow Shield: 0.5× when at full HP
        data_bridge::ABILITY_MULTISCALE | data_bridge::ABILITY_SHADOW_SHIELD
            if def_mon.current_hp == def_mon.max_hp => (2048, 4096),

        // Filter / Solid Rock / Prism Armor: 0.75× on super effective
        data_bridge::ABILITY_FILTER | data_bridge::ABILITY_SOLID_ROCK | data_bridge::ABILITY_PRISM_ARMOR
            if effectiveness > 4 => (3072, 4096),

        // Fluffy: 0.5× contact AND 2× Fire (independent modifiers — both apply on
        // a Fire contact move like Fire Punch, netting 1×).
        data_bridge::ABILITY_FLUFFY => {
            let contact = md.flags & MoveFlags::CONTACT != 0;
            let fire = md.move_type == Type::Fire;
            match (contact, fire) {
                (true, true)  => (4096, 4096), // 0.5× × 2× = 1×
                (true, false) => (2048, 4096), // 0.5×
                (false, true) => (8192, 4096), // 2×
                _ => (4096, 4096),
            }
        }

        // Heatproof moved to defender stat-side modifier in calc.rs (Showdown
        // onSourceModifyAtk/SpA applies it as 0.5× on attacker's Atk/SpA to match
        // truncation order).
        // Dry Skin moved to defender base-power modifier in calc.rs (Showdown
        // onSourceBasePower applies 1.25× on move base power, not on final damage).

        // Punk Rock: 0.5× damage from Sound moves received
        data_bridge::ABILITY_PUNK_ROCK if md.flags & MoveFlags::SOUND != 0 => (2048, 4096),

        // Ice Scales: 0.5× damage from Special moves received
        data_bridge::ABILITY_ICE_SCALES if md.category == MoveCategory::Special => (2048, 4096),

        _ => (4096, 4096),
    }
}

/// Returns (num, den) for attacker's ability effect on final damage.
#[inline]
pub fn attacker_ability_final_mod(
    atk_ability: u16, effectiveness: u8,
) -> (u32, u32) {
    match atk_ability {
        // Tinted Lens: not-very-effective hits do 2× (becomes neutral)
        data_bridge::ABILITY_TINTED_LENS if effectiveness < 4 && effectiveness > 0 => (8192, 4096),
        // Neuroforce: 1.25× on super effective
        data_bridge::ABILITY_NEUROFORCE if effectiveness > 4 => (5120, 4096),
        _ => (4096, 4096),
    }
}

/// Returns (num, den) for attacker's item effect on base power.
#[inline]
pub fn item_power_mod(
    item: &ItemData, item_id: u16, move_type: Type,
    category: MoveCategory, flags: u16,
) -> (u32, u32) {
    if item.has(ItemFlag::TYPE_BOOST) && item.type_param == move_type as u8 {
        return (4915, 4096); // 1.2×
    }
    if item.has(ItemFlag::GEM) && item.type_param == move_type as u8 {
        return (5325, 4096); // 1.3×
    }
    // Life Orb and Metronome: moved to item_final_mod (onModifyDamage, not onBasePower)

    match item_id {
        data_bridge::ITEM_MUSCLE_BAND if category == MoveCategory::Physical => (4505, 4096), // 1.1×
        data_bridge::ITEM_WISE_GLASSES if category == MoveCategory::Special => (4505, 4096), // 1.1×
        data_bridge::ITEM_PUNCHING_GLOVE if flags & MoveFlags::PUNCH != 0 => (4506, 4096), // 1.1× (Showdown: [4506, 4096])
        _ => (4096, 4096),
    }
}

/// Returns (num, den) for items that modify final damage.
/// Also computes extra recoil (Life Orb) and item consumption (resist berry).
#[inline]
pub fn item_final_mod(
    atk_item: &ItemData, def_item: &ItemData,
    move_type: Type, effectiveness: u8, consec_move_count: u8,
) -> (u32, u32, bool) {
    let mut num: u32 = 4096;
    let den: u32 = 4096;
    let mut berry_consumed = false;

    // Life Orb: onModifyDamage (final damage, not base power)
    if atk_item.has(ItemFlag::LIFE_ORB) {
        num = chain_mod(num, 5324); // 1.3×
    }
    if atk_item.has(ItemFlag::EXPERT_BELT) && effectiveness > 4 {
        num = chain_mod(num, 4915); // 1.2×
    }

    // Metronome is onModifyDamage in Showdown, not onBasePower — base-power placement truncates the remainder twice and under-damages by 1-2.
    if atk_item.has(ItemFlag::METRONOME) && consec_move_count > 1 {
        const METRONOME_DMG: [u32; 6] = [4096, 4915, 5734, 6553, 7372, 8192];
        let idx = (consec_move_count as usize - 1).min(5);
        num = chain_mod(num, METRONOME_DMG[idx]);
    }

    if def_item.has(ItemFlag::RESIST_BERRY)
        && def_item.type_param == move_type as u8
        && effectiveness > 4
    {
        num = num / 2; // 0.5×
        berry_consumed = true;
    }

    (num, den, berry_consumed)
}

/// Determine the number of hits for a multi-hit move.
#[inline]
pub fn resolve_hits(md: &MoveData, ability: u16, item_flags: u64, rng: &mut impl FnMut(u32) -> u32) -> u8 {
    let lo = md.multihit_lo();
    let hi = md.multihit_hi();
    if lo == 0 { return 1; }
    if lo == hi {
        if item_flags & ItemFlag::LOADED_DICE != 0 && lo == 10 {
            return 4 + rng(7) as u8; // 4-10 hits (Population Bomb)
        }
        return lo;
    }
    if ability == data_bridge::ABILITY_SKILL_LINK { return hi; }
    if item_flags & ItemFlag::LOADED_DICE != 0 {
        return if rng(2) == 0 { 4 } else { 5 };
    }
    // Showdown gen>=5 picks via sample([2×7, 3×7, 4×3, 5×3]) (35/35/15/15) — a
    // single random(20) indexing that 20-element array. Mirror it exactly so the
    // forced-RNG harness (rng(20)->0) lands on index 0 = 2 hits, matching
    // Showdown's force_all sample()->items[0].
    const HIT_TABLE: [u8; 20] = [2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 5, 5, 5];
    HIT_TABLE[rng(20) as usize]
}

/// Describes the side-effect of an ability-based immunity.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AbilityImmunityEffect {
    /// Absorb: heal the defender by the given amount.
    Heal(u16),
    /// Boost: raise a stat on the defender.
    Boost(usize, i8),          // (stat_index, stages)
    /// Flash Fire: set VOL_FLASH_FIRE on the defender.
    FlashFire,
    /// Pure immunity with no side effect (Levitate, Bulletproof, etc.).
    Nullify,
}

/// Check for ability-based type immunities.
/// Returns Some(effect) if the move is nullified, None otherwise.
/// `atk_ability` is used to check for Mold Breaker bypassing defensive abilities.
#[inline]
pub fn ability_type_immunity(
    state: &BattleState, def_side: usize, move_type: Type, atk_ability: u16,
) -> Option<AbilityImmunityEffect> {
    // Mold Breaker / Turboblaze / Teravolt bypass all defensive abilities
    // (unless the defender holds Ability Shield).
    if mold_breaks(state, def_side, atk_ability) { return None; }

    let ability = effective_ability(state, def_side);
    let def_mon = state.active_mon(def_side);

    match (ability, move_type) {
        // Absorb abilities: nullify + heal 25%
        (data_bridge::ABILITY_WATER_ABSORB, Type::Water) => Some(AbilityImmunityEffect::Heal(def_mon.max_hp / 4)),
        (data_bridge::ABILITY_VOLT_ABSORB, Type::Electric) => Some(AbilityImmunityEffect::Heal(def_mon.max_hp / 4)),
        (data_bridge::ABILITY_DRY_SKIN, Type::Water) => Some(AbilityImmunityEffect::Heal(def_mon.max_hp / 4)),

        // Redirect/boost abilities: nullify + stat boost
        (data_bridge::ABILITY_LIGHTNING_ROD, Type::Electric) => Some(AbilityImmunityEffect::Boost(SPA, 1)),
        (data_bridge::ABILITY_STORM_DRAIN, Type::Water) => Some(AbilityImmunityEffect::Boost(SPA, 1)),
        (data_bridge::ABILITY_MOTOR_DRIVE, Type::Electric) => Some(AbilityImmunityEffect::Boost(SPE, 1)),
        (data_bridge::ABILITY_SAP_SIPPER, Type::Grass) => Some(AbilityImmunityEffect::Boost(ATK, 1)),

        // Flash Fire: nullify + set volatile
        (data_bridge::ABILITY_FLASH_FIRE, Type::Fire) => Some(AbilityImmunityEffect::FlashFire),

        // Well-Baked Body: immune to Fire, +2 Def
        (data_bridge::ABILITY_WELL_BAKED_BODY, Type::Fire) => Some(AbilityImmunityEffect::Boost(DEF, 2)),

        // Earth Eater: immune to Ground, heal 25%
        (data_bridge::ABILITY_EARTH_EATER, Type::Ground) => Some(AbilityImmunityEffect::Heal(def_mon.max_hp / 4)),

        // Levitate: immune to Ground
        (data_bridge::ABILITY_LEVITATE, Type::Ground) => {
            // Grounded mons (Gravity, Smack Down, Ingrain) lose Levitate immunity
            if !is_grounded(state, def_side) {
                Some(AbilityImmunityEffect::Nullify)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Check for ability-based flag immunities (Bulletproof, Soundproof, Overcoat, Wind Rider).
/// Separate from type immunities because these check MoveFlags, not move type.
/// `atk_ability` is used to check for Mold Breaker bypassing defensive abilities.
#[inline]
pub fn ability_flag_immunity(
    state: &BattleState, def_side: usize, flags: u16, atk_ability: u16,
) -> Option<AbilityImmunityEffect> {
    // Mold Breaker / Turboblaze / Teravolt bypass all defensive abilities
    // (unless the defender holds Ability Shield).
    if mold_breaks(state, def_side, atk_ability) { return None; }

    let ability = effective_ability(state, def_side);
    match ability {
        data_bridge::ABILITY_BULLETPROOF if flags & MoveFlags::BULLET != 0 => Some(AbilityImmunityEffect::Nullify),
        data_bridge::ABILITY_SOUNDPROOF  if flags & MoveFlags::SOUND != 0 => Some(AbilityImmunityEffect::Nullify),
        data_bridge::ABILITY_OVERCOAT    if flags & MoveFlags::POWDER != 0 => Some(AbilityImmunityEffect::Nullify),
        data_bridge::ABILITY_WIND_RIDER  if flags & MoveFlags::WIND != 0 => Some(AbilityImmunityEffect::Boost(ATK, 1)),
        _ => None,
    }
}

/// Check if terrain blocks a status condition on the target.
/// Electric Terrain: blocks sleep on grounded mons.
/// Misty Terrain: blocks all status on grounded mons.
#[inline]
pub fn terrain_blocks_status(state: &BattleState, side: usize, status: u8) -> bool {
    match state.field.terrain {
        TERRAIN_ELECTRIC => status == STATUS_SLEEP && is_grounded(state, side),
        TERRAIN_MISTY => status != STATUS_NONE && is_grounded(state, side),
        _ => false,
    }
}

/// Check if Good as Gold blocks a status move (non-self-targeting).
#[inline]
pub fn good_as_gold_immunity(state: &BattleState, def_side: usize) -> bool {
    effective_ability(state, def_side) == data_bridge::ABILITY_GOOD_AS_GOLD
}

/// Check if Dazzling / Queenly Majesty / Armor Tail / Psychic Terrain blocks a priority move.
#[inline]
pub fn priority_block_immunity(state: &BattleState, def_side: usize, priority: i8) -> bool {
    if priority <= 0 { return false; }
    let ability = effective_ability(state, def_side);
    if matches!(ability,
        data_bridge::ABILITY_DAZZLING |
        data_bridge::ABILITY_QUEENLY_MAJESTY |
        data_bridge::ABILITY_ARMOR_TAIL
    ) {
        return true;
    }
    // Psychic Terrain blocks priority moves against grounded targets
    state.field.terrain == TERRAIN_PSYCHIC && is_grounded(state, def_side)
}

/// Legacy wrapper used by calc_damage (returns Option<u16> for backward compat).
/// `atk_ability` is used to check for Mold Breaker bypassing defensive abilities.
#[inline]
pub fn ability_immunity(
    state: &BattleState, def_side: usize, move_type: Type, atk_ability: u16,
) -> Option<u16> {
    ability_type_immunity(state, def_side, move_type, atk_ability).map(|eff| {
        match eff {
            AbilityImmunityEffect::Heal(hp) => hp,
            _ => 0,
        }
    })
}

/// Apply weather-based defensive stat boosts (Sand → Rock SpD, Snow → Ice Def).
/// Returns the modified defensive stat.
#[inline]
pub fn weather_def_stat_mod(
    d: u16, weather: u8, category: MoveCategory,
    def_type1: u8, def_type2: u8,
) -> u16 {
    match (weather, category) {
        (WEATHER_SAND, MoveCategory::Special)
            if def_type1 == Type::Rock as u8 || def_type2 == Type::Rock as u8
            => (d as u32 * 3 / 2) as u16,
        (WEATHER_SNOW, MoveCategory::Physical)
            if def_type1 == Type::Ice as u8 || def_type2 == Type::Ice as u8
            => (d as u32 * 3 / 2) as u16,
        _ => d,
    }
}

/// Resolve the effective type of a move (WeatherBall, TerrainPulse).
#[inline]
pub fn resolve_move_type(
    state: &BattleState, md: &MoveData, atk_side: usize,
) -> Type {
    match md.effect {
        MoveEffect::WeatherBall => {
            match effective_weather_for(state, atk_side) {
                WEATHER_SUN | WEATHER_HARSH_SUN => Type::Fire,
                WEATHER_RAIN | WEATHER_HEAVY_RAIN => Type::Water,
                WEATHER_SAND => Type::Rock,
                WEATHER_SNOW => Type::Ice,
                _ => md.move_type,
            }
        }
        MoveEffect::TerrainPulse => {
            if is_grounded(state, atk_side) {
                match state.field.terrain {
                    TERRAIN_ELECTRIC => Type::Electric,
                    TERRAIN_GRASSY => Type::Grass,
                    TERRAIN_MISTY => Type::Fairy,
                    TERRAIN_PSYCHIC => Type::Psychic,
                    _ => md.move_type,
                }
            } else {
                md.move_type
            }
        }
        MoveEffect::TeraBlast => {
            let mon = state.active_mon(atk_side);
            if mon.is_terastallized() {
                unsafe { core::mem::transmute::<u8, Type>(mon.tera_type) }
            } else {
                md.move_type
            }
        }
        _ => md.move_type,
    }
}

/// Resolve the effective move type, applying type-change abilities.
/// Returns (final_type, ate_boost) — ate_boost is true if an -ate ability
/// changed the type and the 1.2× power boost should apply.
#[inline]
pub fn resolve_move_type_with_ability(
    state: &BattleState, md: &MoveData, atk_side: usize, atk_ability: u16,
) -> (Type, bool) {
    // First apply normal move-type overrides (WeatherBall, TerrainPulse)
    let base_type = resolve_move_type(state, md, atk_side);

    // Normalize: all moves become Normal (1.2× boost)
    if atk_ability == data_bridge::ABILITY_NORMALIZE {
        return (Type::Normal, base_type != Type::Normal);
    }

    // Liquid Voice: Sound-flagged moves become Water (no power boost)
    if atk_ability == data_bridge::ABILITY_LIQUID_VOICE
        && md.flags & MoveFlags::SOUND != 0
    {
        return (Type::Water, false);
    }

    // -ate abilities: Normal → specific type (1.2×)
    if base_type == Type::Normal {
        match atk_ability {
            data_bridge::ABILITY_GALVANIZE  => return (Type::Electric, true),
            data_bridge::ABILITY_PIXILATE   => return (Type::Fairy, true),
            data_bridge::ABILITY_AERILATE   => return (Type::Flying, true),
            data_bridge::ABILITY_REFRIGERATE => return (Type::Ice, true),
            _ => {}
        }
    }

    (base_type, false)
}

/// Returns (num, den) in 4096-scale for move-specific onBasePower effects.
/// Dispatches on MoveEffect for moves with chainModify callbacks.
#[inline]
pub fn move_effect_power_mod(
    state: &BattleState, md: &MoveData, atk_side: usize, def_side: usize,
) -> (u32, u32) {
    match md.effect {
        // Knock Off: 1.5× if target has a removable item
        MoveEffect::KnockOff => {
            let def_mon = state.active_mon(def_side);
            if def_mon.item_id != 0 {
                let def_ability = effective_ability(state, def_side);
                let sticky = def_ability == data_bridge::ABILITY_STICKY_HOLD;
                let def_item = data_bridge::item(def_mon.item_id);
                let base = data_bridge::base_species(def_mon.species_id);
                if !sticky && !def_item.is_forme_locked(base) {
                    (6144, 4096) // 1.5×
                } else {
                    (4096, 4096)
                }
            } else {
                (4096, 4096)
            }
        }

        // Expanding Force: 1.5× in Psychic Terrain (source grounded)
        MoveEffect::ExpandingForce => {
            if state.field.terrain == TERRAIN_PSYCHIC
                && is_grounded(state, atk_side)
            {
                (6144, 4096)
            } else {
                (4096, 4096)
            }
        }

        // Psyblade: 1.5× in Electric Terrain
        MoveEffect::Psyblade => {
            if state.field.terrain == TERRAIN_ELECTRIC {
                (6144, 4096)
            } else {
                (4096, 4096)
            }
        }

        // Solar Beam / Solar Blade: 0.5× in rain, sand, snow
        MoveEffect::SolarBeam => {
            match effective_weather_for(state, atk_side) {
                WEATHER_RAIN | WEATHER_HEAVY_RAIN
                | WEATHER_SAND | WEATHER_SNOW => (2048, 4096), // 0.5×
                _ => (4096, 4096),
            }
        }

        MoveEffect::WeatherBall => {
            match effective_weather_for(state, atk_side) {
                WEATHER_SUN | WEATHER_HARSH_SUN
                | WEATHER_RAIN | WEATHER_HEAVY_RAIN
                | WEATHER_SAND | WEATHER_SNOW => (8192, 4096), // 2×
                _ => (4096, 4096),
            }
        }

        // Terrain Pulse: 2× power if terrain active + user grounded
        MoveEffect::TerrainPulse => {
            if state.field.terrain != TERRAIN_NONE
                && is_grounded(state, atk_side)
            {
                (8192, 4096) // 2×
            } else {
                (4096, 4096)
            }
        }

        _ => (4096, 4096),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weather_mods() {
        assert_eq!(weather_modifier(WEATHER_SUN, Type::Fire), (6144, 4096));
        assert_eq!(weather_modifier(WEATHER_SUN, Type::Water), (2048, 4096));
        assert_eq!(weather_modifier(WEATHER_RAIN, Type::Water), (6144, 4096));
        assert_eq!(weather_modifier(WEATHER_HARSH_SUN, Type::Water), (0, 4096));
        assert_eq!(weather_modifier(WEATHER_NONE, Type::Normal), (4096, 4096));
    }

    #[test]
    fn test_burn_mod() {
        assert_eq!(burn_modifier(STATUS_BURN, MoveCategory::Physical, 0, false), (2048, 4096));
        assert_eq!(burn_modifier(STATUS_BURN, MoveCategory::Special, 0, false), (4096, 4096));
        assert_eq!(burn_modifier(STATUS_BURN, MoveCategory::Physical, data_bridge::ABILITY_GUTS, false), (4096, 4096));
        assert_eq!(burn_modifier(STATUS_NONE, MoveCategory::Physical, 0, false), (4096, 4096));
        assert_eq!(burn_modifier(STATUS_BURN, MoveCategory::Physical, 0, true), (4096, 4096)); // Facade bypasses burn
    }

    #[test]
    fn test_stab_default() {
        let state = BattleState::default();
        // Default mon has Normal type (from stub), Normal move → STAB
        assert_eq!(stab_modifier(&state, 0, Type::Normal), (6144, 4096));
        // Fire move → no STAB
        assert_eq!(stab_modifier(&state, 0, Type::Fire), (4096, 4096));
    }

    #[test]
    fn test_hex_var_power() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[0].team[0].stats = [100; 5];
        state.sides[1].team[0].species_id = 2;
        state.sides[1].team[0].stats = [100; 5];

        let md = MoveData {
            base_power: 65,
            var_power: VarPower::Hex,
            category: MoveCategory::Special,
            move_type: Type::Ghost,
            ..unsafe { core::mem::zeroed() }
        };

        // No status → 65 BP
        assert_eq!(resolve_power(&state, &md, 0, 1), 65);

        // Poisoned → 130 BP
        state.sides[1].team[0].status = STATUS_POISON;
        assert_eq!(resolve_power(&state, &md, 0, 1), 130);
    }

    #[test]
    fn test_acrobatics_var_power() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[0].team[0].stats = [100; 5];
        state.sides[1].team[0].species_id = 2;
        state.sides[1].team[0].stats = [100; 5];

        let md = MoveData {
            base_power: 55,
            var_power: VarPower::Acrobatics,
            category: MoveCategory::Physical,
            move_type: Type::Flying,
            ..unsafe { core::mem::zeroed() }
        };

        // With item → 55 BP
        state.sides[0].team[0].item_id = 100;
        assert_eq!(resolve_power(&state, &md, 0, 1), 55);

        // No item → 110 BP
        state.sides[0].team[0].item_id = 0;
        assert_eq!(resolve_power(&state, &md, 0, 1), 110);
    }

    #[test]
    fn test_rising_voltage_var_power() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[0].team[0].stats = [100; 5];
        state.sides[1].team[0].species_id = 2;
        state.sides[1].team[0].stats = [100; 5];

        let md = MoveData {
            base_power: 70,
            var_power: VarPower::RisingVoltage,
            category: MoveCategory::Special,
            move_type: Type::Electric,
            ..unsafe { core::mem::zeroed() }
        };

        // No terrain → 70 BP
        assert_eq!(resolve_power(&state, &md, 0, 1), 70);

        // Electric Terrain + grounded target → 140 BP
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;
        assert_eq!(resolve_power(&state, &md, 0, 1), 140);
    }

    #[test]
    fn test_knock_off_power_mod() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        let md = MoveData {
            effect: MoveEffect::KnockOff,
            ..unsafe { core::mem::zeroed() }
        };

        // Target has no item → no boost
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (4096, 4096));

        // Target has item → 1.5×
        state.sides[1].team[0].item_id = 100;
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (6144, 4096));
    }

    #[test]
    fn test_expanding_force_power_mod() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        let md = MoveData {
            effect: MoveEffect::ExpandingForce,
            ..unsafe { core::mem::zeroed() }
        };

        // No terrain → no boost
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (4096, 4096));

        // Psychic Terrain + grounded source → 1.5×
        state.field.terrain = TERRAIN_PSYCHIC;
        state.field.terrain_turns = 5;
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (6144, 4096));
    }

    #[test]
    fn test_psyblade_power_mod() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        let md = MoveData {
            effect: MoveEffect::Psyblade,
            ..unsafe { core::mem::zeroed() }
        };

        // No terrain → no boost
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (4096, 4096));

        // Electric Terrain → 1.5×
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (6144, 4096));
    }

    #[test]
    fn test_solar_beam_power_mod() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        let md = MoveData {
            effect: MoveEffect::SolarBeam,
            ..unsafe { core::mem::zeroed() }
        };

        // No weather → normal
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (4096, 4096));

        // Sun → normal (not weakened)
        state.field.weather = WEATHER_SUN;
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (4096, 4096));

        // Rain → 0.5×
        state.field.weather = WEATHER_RAIN;
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (2048, 4096));

        // Sand → 0.5×
        state.field.weather = WEATHER_SAND;
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (2048, 4096));

        // Snow → 0.5×
        state.field.weather = WEATHER_SNOW;
        assert_eq!(move_effect_power_mod(&state, &md, 0, 1), (2048, 4096));
    }

    fn make_state_with_ability(ability: u16) -> BattleState {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[0].team[0].ability_id = ability;
        state.sides[0].team[0].current_hp = 300;
        state.sides[0].team[0].max_hp = 300;
        state.sides[1].team[0].species_id = 2;
        state.sides[1].team[0].current_hp = 300;
        state.sides[1].team[0].max_hp = 300;
        state
    }

    #[test]
    fn test_technician_60bp() {
        let state = make_state_with_ability(data_bridge::ABILITY_TECHNICIAN);
        let md = MoveData {
            base_power: 60,
            category: MoveCategory::Physical,
            move_type: Type::Normal,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(ability_power_mod(&state, &md, 0, 60), (6144, 4096)); // 1.5×
    }

    #[test]
    fn test_technician_61bp_no_boost() {
        let state = make_state_with_ability(data_bridge::ABILITY_TECHNICIAN);
        let md = MoveData {
            base_power: 61,
            category: MoveCategory::Physical,
            move_type: Type::Normal,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(ability_power_mod(&state, &md, 0, 61), (4096, 4096)); // no boost
    }

    #[test]
    fn test_iron_fist_punch() {
        let state = make_state_with_ability(data_bridge::ABILITY_IRON_FIST);
        let md = MoveData {
            base_power: 75,
            flags: MoveFlags::PUNCH,
            category: MoveCategory::Physical,
            move_type: Type::Fighting,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(ability_power_mod(&state, &md, 0, 75), (4915, 4096)); // 1.2×
    }

    #[test]
    fn test_strong_jaw_bite() {
        let state = make_state_with_ability(data_bridge::ABILITY_STRONG_JAW);
        let md = MoveData {
            base_power: 80,
            flags: MoveFlags::BITE,
            category: MoveCategory::Physical,
            move_type: Type::Dark,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(ability_power_mod(&state, &md, 0, 80), (6144, 4096)); // 1.5×
    }

    #[test]
    fn test_sheer_force_boost_no_secondary() {
        let state = make_state_with_ability(data_bridge::ABILITY_SHEER_FORCE);
        let md = MoveData {
            base_power: 80,
            secondary_chance: 30,
            category: MoveCategory::Physical,
            move_type: Type::Normal,
            ..unsafe { core::mem::zeroed() }
        };
        // Power gets 1.3× boost
        assert_eq!(ability_power_mod(&state, &md, 0, 80), (5325, 4096));
        // Secondary suppression is tested in move_exec tests (Hook 17)
    }

    #[test]
    fn test_sand_force_in_sand() {
        let mut state = make_state_with_ability(data_bridge::ABILITY_SAND_FORCE);
        state.field.weather = WEATHER_SAND;
        let md = MoveData {
            base_power: 80,
            category: MoveCategory::Physical,
            move_type: Type::Rock,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(ability_power_mod(&state, &md, 0, 80), (5325, 4096)); // 1.3×
    }

    #[test]
    fn test_sand_force_no_sand() {
        let state = make_state_with_ability(data_bridge::ABILITY_SAND_FORCE);
        // No sandstorm — weather is WEATHER_NONE by default
        let md = MoveData {
            base_power: 80,
            category: MoveCategory::Physical,
            move_type: Type::Rock,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(ability_power_mod(&state, &md, 0, 80), (4096, 4096)); // no boost
    }

    #[test]
    fn test_tough_claws_contact() {
        let state = make_state_with_ability(data_bridge::ABILITY_TOUGH_CLAWS);
        let md = MoveData {
            base_power: 90,
            flags: MoveFlags::CONTACT,
            category: MoveCategory::Physical,
            move_type: Type::Normal,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(ability_power_mod(&state, &md, 0, 90), (5325, 4096)); // 1.3×
    }

    #[test]
    fn test_reckless_crash_damage() {
        let state = make_state_with_ability(data_bridge::ABILITY_RECKLESS);
        // Crash damage move (e.g. High Jump Kick): drain=0, self_effect=CrashDamage
        let md = MoveData {
            base_power: 130,
            category: MoveCategory::Physical,
            move_type: Type::Fighting,
            self_effect: SelfEffect::CrashDamage,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(ability_power_mod(&state, &md, 0, 130), (4915, 4096)); // 1.2×

        // Recoil move (e.g. Flare Blitz): drain=-33
        let md_recoil = MoveData {
            base_power: 120,
            drain: -33,
            category: MoveCategory::Physical,
            move_type: Type::Fire,
            ..unsafe { core::mem::zeroed() }
        };
        assert_eq!(ability_power_mod(&state, &md_recoil, 0, 120), (4915, 4096)); // 1.2×
    }

    #[test]
    fn test_supreme_overlord_fainted() {
        let mut state = make_state_with_ability(data_bridge::ABILITY_SUPREME_OVERLORD);
        // Set up a team of 6 mons
        for i in 0..6 {
            state.sides[0].team[i].species_id = (i + 1) as u16;
            state.sides[0].team[i].current_hp = 300;
            state.sides[0].team[i].max_hp = 300;
        }
        state.sides[0].active_index = 0;

        let md = MoveData {
            base_power: 80,
            category: MoveCategory::Physical,
            move_type: Type::Normal,
            ..unsafe { core::mem::zeroed() }
        };

        // Exact Showdown lookup table values
        let expected: [(usize, u32); 6] = [
            (0, 4096), (1, 4506), (2, 4915), (3, 5325), (4, 5734), (5, 6144),
        ];

        for (faint_count, expected_num) in expected {
            // Reset all allies to alive
            for i in 1..6 {
                state.sides[0].team[i].current_hp = 300;
            }
            // Faint the first `faint_count` allies (slots 1..=faint_count)
            for i in 1..=faint_count {
                state.sides[0].team[i].current_hp = 0;
            }
            assert_eq!(
                ability_power_mod(&state, &md, 0, 80),
                (expected_num, 4096),
                "Supreme Overlord with {} fainted should be ({}, 4096)",
                faint_count, expected_num,
            );
        }
    }

    #[test]
    fn test_electric_terrain_boost() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        // No terrain → no boost
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Electric, 1), (4096, 4096));

        // Electric Terrain + Electric type + grounded → 1.3×
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Electric, 1), (5325, 4096));

        // Non-Electric type → no boost
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Normal, 1), (4096, 4096));
    }

    #[test]
    fn test_grassy_terrain_boost() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        state.field.terrain = TERRAIN_GRASSY;
        state.field.terrain_turns = 5;

        // Grass type + grounded → 1.3×
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Grass, 1), (5325, 4096));

        // Earthquake weakened when defender grounded → 0.5×
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Ground, MOVE_EARTHQUAKE as u16), (2048, 4096));

        // Bulldoze also weakened
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Ground, MOVE_BULLDOZE as u16), (2048, 4096));
    }

    #[test]
    fn test_psychic_terrain_boost() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        state.field.terrain = TERRAIN_PSYCHIC;
        state.field.terrain_turns = 5;
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Psychic, 1), (5325, 4096));
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Normal, 1), (4096, 4096));
    }

    #[test]
    fn test_misty_terrain_dragon_weaken() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        state.field.terrain = TERRAIN_MISTY;
        state.field.terrain_turns = 5;

        // Dragon type on grounded defender → 0.5×
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Dragon, 1), (2048, 4096));

        // Non-Dragon → no change
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Fire, 1), (4096, 4096));
    }

    #[test]
    fn test_terrain_not_grounded() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        // Make attacker Flying-type (not grounded) via override_types
        state.sides[0].active.override_types = [Type::Flying as u8, Type::Flying as u8];
        state.sides[0].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;

        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;

        // Flying-type attacker doesn't get Electric Terrain boost
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Electric, 1), (4096, 4096));

        // Make defender Flying for Misty
        state.field.terrain = TERRAIN_MISTY;
        state.sides[1].active.override_types = [Type::Flying as u8, Type::Flying as u8];
        state.sides[1].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;
        assert_eq!(terrain_modifier(&state, 0, 1, Type::Dragon, 1), (4096, 4096));
    }

    #[test]
    fn test_terrain_blocks_status_electric() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;

        // No terrain → no block
        assert!(!terrain_blocks_status(&state, 0, STATUS_SLEEP));

        // Electric Terrain blocks sleep for grounded
        state.field.terrain = TERRAIN_ELECTRIC;
        state.field.terrain_turns = 5;
        assert!(terrain_blocks_status(&state, 0, STATUS_SLEEP));
        assert!(!terrain_blocks_status(&state, 0, STATUS_BURN)); // only sleep
    }

    #[test]
    fn test_terrain_blocks_status_misty() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;

        state.field.terrain = TERRAIN_MISTY;
        state.field.terrain_turns = 5;

        // Misty blocks all statuses for grounded
        assert!(terrain_blocks_status(&state, 0, STATUS_BURN));
        assert!(terrain_blocks_status(&state, 0, STATUS_PARALYSIS));
        assert!(terrain_blocks_status(&state, 0, STATUS_POISON));
        assert!(terrain_blocks_status(&state, 0, STATUS_BAD_POISON));
        assert!(terrain_blocks_status(&state, 0, STATUS_SLEEP));
        assert!(terrain_blocks_status(&state, 0, STATUS_FREEZE));

        // Flying-type → not grounded → not blocked
        state.sides[0].active.override_types = [Type::Flying as u8, Type::Flying as u8];
        state.sides[0].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;
        assert!(!terrain_blocks_status(&state, 0, STATUS_BURN));
    }

    #[test]
    fn test_psychic_terrain_priority_block() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 1;
        state.sides[1].team[0].species_id = 2;

        // No terrain → no block
        assert!(!priority_block_immunity(&state, 1, 1));

        // Psychic Terrain + grounded defender → blocks priority
        state.field.terrain = TERRAIN_PSYCHIC;
        state.field.terrain_turns = 5;
        assert!(priority_block_immunity(&state, 1, 1));

        // Zero or negative priority → not blocked
        assert!(!priority_block_immunity(&state, 1, 0));
        assert!(!priority_block_immunity(&state, 1, -1));

        // Flying defender → not grounded → not blocked
        state.sides[1].active.override_types = [Type::Flying as u8, Type::Flying as u8];
        state.sides[1].active.volatile_flags |= VOL_TYPES_OVERRIDDEN;
        assert!(!priority_block_immunity(&state, 1, 1));
    }

    #[test]
    fn test_tera_stab_matching_type() {
        // Charmander (Fire/Fire), tera type = Fire, terastallized
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 4; // Charmander: Fire/Fire
        state.sides[0].team[0].tera_type = Type::Fire as u8;
        state.sides[0].team[0].flags |= MON_FLAG_TERASTALLIZED;
        state.sides[0].team[0].current_hp = 300;
        state.sides[0].team[0].max_hp = 300;

        // Fire move on Fire tera matching original Fire → 2.0× STAB
        assert_eq!(stab_modifier(&state, 0, Type::Fire), (8192, 4096));
    }

    #[test]
    fn test_tera_stab_new_type() {
        // Charmander (Fire/Fire), tera type = Water (doesn't match original)
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 4; // Charmander: Fire/Fire
        state.sides[0].team[0].tera_type = Type::Water as u8;
        state.sides[0].team[0].flags |= MON_FLAG_TERASTALLIZED;
        state.sides[0].team[0].current_hp = 300;
        state.sides[0].team[0].max_hp = 300;

        // Water move matches tera but not original → 1.5× STAB
        assert_eq!(stab_modifier(&state, 0, Type::Water), (6144, 4096));
        // Fire move matches original but not tera → 1.5× STAB
        assert_eq!(stab_modifier(&state, 0, Type::Fire), (6144, 4096));
        // Grass matches neither → no STAB
        assert_eq!(stab_modifier(&state, 0, Type::Grass), (4096, 4096));
    }

    #[test]
    fn test_tera_stab_adaptability() {
        // Charmander (Fire/Fire) with Adaptability, tera type = Fire
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 4;
        state.sides[0].team[0].tera_type = Type::Fire as u8;
        state.sides[0].team[0].flags |= MON_FLAG_TERASTALLIZED;
        state.sides[0].team[0].ability_id = data_bridge::ABILITY_ADAPTABILITY;
        state.sides[0].team[0].current_hp = 300;
        state.sides[0].team[0].max_hp = 300;

        // Fire matches both tera and original → 2.25× with Adaptability
        assert_eq!(stab_modifier(&state, 0, Type::Fire), (9216, 4096));

        // Now tera type = Water (doesn't match original)
        state.sides[0].team[0].tera_type = Type::Water as u8;
        // Water matches tera only → 2.0× with Adaptability
        assert_eq!(stab_modifier(&state, 0, Type::Water), (8192, 4096));
    }

    #[test]
    fn test_tera_blast_type_changes() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 4;
        state.sides[0].team[0].tera_type = Type::Water as u8;
        state.sides[0].team[0].current_hp = 300;
        state.sides[0].team[0].max_hp = 300;

        let md = MoveData {
            effect: MoveEffect::TeraBlast,
            move_type: Type::Normal,
            category: MoveCategory::Special,
            base_power: 80,
            ..unsafe { core::mem::zeroed() }
        };

        // Not terastallized → Normal type
        assert_eq!(resolve_move_type(&state, &md, 0), Type::Normal);

        // Terastallized → Water type
        state.sides[0].team[0].flags |= MON_FLAG_TERASTALLIZED;
        assert_eq!(resolve_move_type(&state, &md, 0), Type::Water);
    }
}
