//! Damage calculator modifier helpers.
//!
//! All modifier functions return (numerator, denominator) pairs in
//! 4096-scale, or modify values in-place.  Zero heap allocations.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemData, ItemFlag, MoveCategory};
use crate::state::accessors::*;
use crate::data::moves::{MoveData, MoveFlags, MoveEffect, VarPower};
use crate::data::types::Type;

// ── 4096-scale chain helper ───────────────────────────────────────────

/// Apply a (num, den) modifier to a value, flooring the result.
#[inline(always)]
pub fn chain_mod(value: u32, num: u32, den: u32) -> u32 {
    value * num / den
}

// ── Weather modifier ──────────────────────────────────────────────────

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

// ── Screen modifier ───────────────────────────────────────────────────

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

// ── STAB ──────────────────────────────────────────────────────────────

/// Returns (num, den) for STAB.
#[inline]
pub fn stab_modifier(state: &BattleState, atk_side: usize, move_type: Type) -> (u32, u32) {
    let has_stab = has_type(state, atk_side, move_type as u8);
    if !has_stab {
        return (4096, 4096);
    }
    let ability = effective_ability(state, atk_side);
    if ability == data_bridge::ABILITY_ADAPTABILITY {
        (8192, 4096) // 2.0×
    } else {
        (6144, 4096) // 1.5×
    }
}

// ── Critical hit ──────────────────────────────────────────────────────

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

    let item = data_bridge::item(mon.item_id);
    if item.has(ItemFlag::CRIT_BOOST) { stage += 1; }

    stage
}

/// Determine if this hit is a crit.
#[inline]
pub fn is_crit(stage: u8, rng: &mut impl FnMut(u32) -> u32) -> bool {
    let threshold = match stage {
        0 => 24,   // 1/24
        1 => 8,    // 1/8
        2 => 2,    // 1/2
        _ => 1,    // guaranteed
    };
    rng(threshold) == 0
}

/// Crit damage multiplier. Sniper makes it 2.25× instead of 1.5×.
#[inline]
pub fn crit_multiplier(atk_ability: u16) -> (u32, u32) {
    if atk_ability == data_bridge::ABILITY_SNIPER {
        (9216, 4096) // 2.25×
    } else {
        (6144, 4096) // 1.5×
    }
}

// ── Burn modifier ─────────────────────────────────────────────────────

/// Returns (num, den) for the burn penalty on physical moves.
#[inline]
pub fn burn_modifier(
    atk_status: u8, category: MoveCategory, atk_ability: u16,
) -> (u32, u32) {
    if atk_status != STATUS_BURN { return (4096, 4096); }
    if category != MoveCategory::Physical { return (4096, 4096); }
    if atk_ability == data_bridge::ABILITY_GUTS { return (4096, 4096); } // Guts ignores burn
    (2048, 4096) // 0.5×
}

// ── Variable power resolution ─────────────────────────────────────────

/// Resolve variable base power moves.  Returns the effective base power.
#[inline]
pub fn resolve_power(
    state: &BattleState, md: &MoveData, atk_side: usize, def_side: usize,
) -> u8 {
    if md.var_power == VarPower::None {
        return md.base_power;
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
        VarPower::Weight => crate::data::moves::weight_based_bp(def_species.weight),
        VarPower::HeavySlam => crate::data::moves::heavy_slam_bp(atk_species.weight, def_species.weight),
        VarPower::GyroBall => crate::data::moves::gyro_ball_bp(atk_spe, def_spe),
        VarPower::Eruption => crate::data::moves::eruption_bp(atk_mon.current_hp, atk_mon.max_hp),
        VarPower::Flail => crate::data::moves::flail_bp(atk_mon.current_hp, atk_mon.max_hp),
        VarPower::ElectroBall => crate::data::moves::electro_ball_bp(atk_spe, def_spe),
        VarPower::StoredPower => {
            let pos: u8 = state.sides[atk_side].active.boosts.iter()
                .filter(|&&b| b > 0).map(|&b| b as u8).sum();
            crate::data::moves::stored_power_bp(pos)
        }
        VarPower::Punishment => {
            let pos: u8 = state.sides[def_side].active.boosts.iter()
                .filter(|&&b| b > 0).map(|&b| b as u8).sum();
            crate::data::moves::punishment_bp(pos)
        }
        VarPower::Facade => {
            if atk_mon.status != STATUS_NONE { 140 } else { 70 }
        }
        VarPower::Hex => {
            if def_mon.status != STATUS_NONE { 130 } else { 65 }
        }
        VarPower::Acrobatics => {
            if atk_mon.item_id == 0 { 110 } else { 55 }
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
        _ => md.base_power,
    }
}

// ── Attacker ability power modifiers ──────────────────────────────────

/// Returns (num, den) in 4096-scale for attacker's ability effect on power.
#[inline]
pub fn ability_power_mod(
    state: &BattleState, md: &MoveData, atk_side: usize, power: u8,
) -> (u32, u32) {
    let ability = effective_ability(state, atk_side);
    let atk_mon = state.active_mon(atk_side);

    match ability {
        data_bridge::ABILITY_TECHNICIAN if power <= 60 => (6144, 4096), // 1.5×
        data_bridge::ABILITY_RECKLESS if md.drain < 0 => (4915, 4096), // 1.2×
        data_bridge::ABILITY_IRON_FIST if md.flags & MoveFlags::PUNCH != 0 => (4915, 4096),
        data_bridge::ABILITY_MEGA_LAUNCHER if md.flags & MoveFlags::PULSE != 0 => (6144, 4096),
        data_bridge::ABILITY_STRONG_JAW if md.flags & MoveFlags::BITE != 0 => (6144, 4096),
        data_bridge::ABILITY_TOUGH_CLAWS if md.flags & MoveFlags::CONTACT != 0 => (5325, 4096), // 1.3×
        data_bridge::ABILITY_SHEER_FORCE if md.secondary_chance > 0 => (5325, 4096),

        // Pinch abilities: 1.5× when HP ≤ 1/3 and matching type
        data_bridge::ABILITY_OVERGROW if md.move_type == Type::Grass
            && atk_mon.current_hp * 3 <= atk_mon.max_hp => (6144, 4096),
        data_bridge::ABILITY_BLAZE if md.move_type == Type::Fire
            && atk_mon.current_hp * 3 <= atk_mon.max_hp => (6144, 4096),
        data_bridge::ABILITY_TORRENT if md.move_type == Type::Water
            && atk_mon.current_hp * 3 <= atk_mon.max_hp => (6144, 4096),
        data_bridge::ABILITY_SWARM if md.move_type == Type::Bug
            && atk_mon.current_hp * 3 <= atk_mon.max_hp => (6144, 4096),

        // Flash Fire: 1.5× Fire when activated
        data_bridge::ABILITY_FLASH_FIRE if md.move_type == Type::Fire
            && state.sides[atk_side].active.has_volatile(VOL_FLASH_FIRE) => (6144, 4096),

        _ => (4096, 4096),
    }
}

// ── Attacker ability stat modifiers ───────────────────────────────────

/// Modify the offensive stat A based on attacker's ability.
#[inline]
pub fn ability_atk_stat_mod(a: u16, ability: u16, category: MoveCategory, status: u8) -> u16 {
    match ability {
        data_bridge::ABILITY_HUGE_POWER | data_bridge::ABILITY_PURE_POWER
            if category == MoveCategory::Physical => a * 2,
        data_bridge::ABILITY_HUSTLE
            if category == MoveCategory::Physical => (a as u32 * 3 / 2) as u16,
        data_bridge::ABILITY_GUTS
            if category == MoveCategory::Physical && status != STATUS_NONE
            => (a as u32 * 3 / 2) as u16,
        data_bridge::ABILITY_SOLAR_POWER
            if category == MoveCategory::Special => (a as u32 * 3 / 2) as u16,
            // Note: Solar Power only active in Sun — caller should check weather
        _ => a,
    }
}

/// Modify the defensive stat D based on defender's ability.
#[inline]
pub fn ability_def_stat_mod(d: u16, ability: u16, category: MoveCategory, move_type: Type) -> u16 {
    match ability {
        data_bridge::ABILITY_FUR_COAT if category == MoveCategory::Physical => d * 2,
        data_bridge::ABILITY_ICE_SCALES if category == MoveCategory::Special => d * 2,
        data_bridge::ABILITY_THICK_FAT
            if move_type == Type::Fire || move_type == Type::Ice
            // Thick Fat effectively halves the attacking power, implemented as doubling def
            => d * 2,
        _ => d,
    }
}

// ── Defender ability final damage modifiers ────────────────────────────

/// Returns (num, den) for defender's ability effect on final damage.
#[inline]
pub fn defender_ability_final_mod(
    state: &BattleState, md: &MoveData, def_side: usize, effectiveness: u8,
) -> (u32, u32) {
    let ability = effective_ability(state, def_side);
    let def_mon = state.active_mon(def_side);

    match ability {
        // Multiscale / Shadow Shield: 0.5× when at full HP
        data_bridge::ABILITY_MULTISCALE | data_bridge::ABILITY_SHADOW_SHIELD
            if def_mon.current_hp == def_mon.max_hp => (2048, 4096),

        // Filter / Solid Rock / Prism Armor: 0.75× on super effective
        data_bridge::ABILITY_FILTER | data_bridge::ABILITY_SOLID_ROCK | data_bridge::ABILITY_PRISM_ARMOR
            if effectiveness > 4 => (3072, 4096),

        // Fluffy: 0.5× contact, but 2× Fire
        data_bridge::ABILITY_FLUFFY if md.flags & MoveFlags::CONTACT != 0
            && md.move_type != Type::Fire => (2048, 4096),
        data_bridge::ABILITY_FLUFFY if md.move_type == Type::Fire => (8192, 4096), // 2×

        // Heatproof: 0.5× Fire
        data_bridge::ABILITY_HEATPROOF if md.move_type == Type::Fire => (2048, 4096),

        _ => (4096, 4096),
    }
}

// ── Attacker ability final damage modifiers ───────────────────────────

/// Returns (num, den) for attacker's ability effect on final damage.
#[inline]
pub fn attacker_ability_final_mod(
    atk_ability: u16, effectiveness: u8,
) -> (u32, u32) {
    match atk_ability {
        // Tinted Lens: not-very-effective hits do 2× (becomes neutral)
        data_bridge::ABILITY_TINTED_LENS if effectiveness < 4 && effectiveness > 0 => (8192, 4096),
        _ => (4096, 4096),
    }
}

// ── Item power modifiers ──────────────────────────────────────────────

/// Returns (num, den) for attacker's item effect on base power.
#[inline]
pub fn item_power_mod(item: &ItemData, move_type: Type) -> (u32, u32) {
    if item.has(ItemFlag::TYPE_BOOST) && item.type_param == move_type as u8 {
        return (4915, 4096); // 1.2×
    }
    if item.has(ItemFlag::GEM) && item.type_param == move_type as u8 {
        return (5325, 4096); // 1.3×
    }
    (4096, 4096)
}

// ── Item final damage modifiers ───────────────────────────────────────

/// Returns (num, den) for items that modify final damage.
/// Also computes extra recoil (Life Orb) and item consumption (resist berry).
#[inline]
pub fn item_final_mod(
    atk_item: &ItemData, def_item: &ItemData,
    move_type: Type, effectiveness: u8,
) -> (u32, u32, bool) {
    // Attacker items
    let mut num: u32 = 4096;
    let den: u32 = 4096;
    let mut berry_consumed = false;

    if atk_item.has(ItemFlag::LIFE_ORB) {
        num = num * 5324 / 4096; // 1.3×
    }
    if atk_item.has(ItemFlag::EXPERT_BELT) && effectiveness > 4 {
        num = num * 4915 / 4096; // 1.2×
    }

    // Metronome item — caller should pass consec_move_count for proper scaling
    // Simplified: not applied here, caller can handle via consec_move_count

    // Defender resist berry
    if def_item.has(ItemFlag::RESIST_BERRY)
        && def_item.type_param == move_type as u8
        && effectiveness > 4
    {
        num = num / 2; // 0.5×
        berry_consumed = true;
    }

    (num, den, berry_consumed)
}

// ── Multi-hit resolution ──────────────────────────────────────────────

/// Determine the number of hits for a multi-hit move.
#[inline]
pub fn resolve_hits(md: &MoveData, ability: u16, rng: &mut impl FnMut(u32) -> u32) -> u8 {
    if md.multihit_lo == 0 { return 1; }
    if md.multihit_lo == md.multihit_hi { return md.multihit_lo; }
    if ability == data_bridge::ABILITY_SKILL_LINK { return md.multihit_hi; }
    // 2-5 distribution: 35%, 35%, 15%, 15%
    match rng(100) {
        0..=34 => 2,
        35..=69 => 3,
        70..=84 => 4,
        _ => 5,
    }
}

// ── Immunity checks ───────────────────────────────────────────────────

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
#[inline]
pub fn ability_type_immunity(
    state: &BattleState, def_side: usize, move_type: Type,
) -> Option<AbilityImmunityEffect> {
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

/// Check for ability-based flag immunities (Bulletproof, Soundproof, Overcoat).
/// Separate from type immunities because these check MoveFlags, not move type.
#[inline]
pub fn ability_flag_immunity(
    state: &BattleState, def_side: usize, flags: u16,
) -> bool {
    let ability = effective_ability(state, def_side);
    match ability {
        data_bridge::ABILITY_BULLETPROOF if flags & MoveFlags::BULLET != 0 => true,
        data_bridge::ABILITY_SOUNDPROOF  if flags & MoveFlags::SOUND != 0 => true,
        data_bridge::ABILITY_OVERCOAT    if flags & MoveFlags::POWDER != 0 => true,
        _ => false,
    }
}

/// Legacy wrapper used by calc_damage (returns Option<u16> for backward compat).
#[inline]
pub fn ability_immunity(
    state: &BattleState, def_side: usize, move_type: Type,
) -> Option<u16> {
    ability_type_immunity(state, def_side, move_type).map(|eff| {
        match eff {
            AbilityImmunityEffect::Heal(hp) => hp,
            _ => 0,
        }
    })
}

// ── Weather stat boosts ───────────────────────────────────────────────

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

// ── Move-effect power modifiers ──────────────────────────────────────

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
            let def_item = data_bridge::item(def_mon.item_id);
            // Item is removable if it exists and isn't a Mega Stone or Z-Crystal
            if def_mon.item_id != 0
                && !def_item.has(ItemFlag::MEGA_STONE)
                && !def_item.has(ItemFlag::Z_CRYSTAL)
            {
                (6144, 4096) // 1.5×
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
            match state.field.weather {
                WEATHER_RAIN | WEATHER_HEAVY_RAIN
                | WEATHER_SAND | WEATHER_SNOW => (2048, 4096), // 0.5×
                _ => (4096, 4096),
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
        assert_eq!(burn_modifier(STATUS_BURN, MoveCategory::Physical, 0), (2048, 4096));
        assert_eq!(burn_modifier(STATUS_BURN, MoveCategory::Special, 0), (4096, 4096));
        assert_eq!(burn_modifier(STATUS_BURN, MoveCategory::Physical, data_bridge::ABILITY_GUTS), (4096, 4096));
        assert_eq!(burn_modifier(STATUS_NONE, MoveCategory::Physical, 0), (4096, 4096));
    }

    #[test]
    fn test_stab_default() {
        let state = BattleState::default();
        // Default mon has Normal type (from stub), Normal move → STAB
        assert_eq!(stab_modifier(&state, 0, Type::Normal), (6144, 4096));
        // Fire move → no STAB
        assert_eq!(stab_modifier(&state, 0, Type::Fire), (4096, 4096));
    }

    // ── Step 1: Move-specific damage modifier tests ───────────

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
}
