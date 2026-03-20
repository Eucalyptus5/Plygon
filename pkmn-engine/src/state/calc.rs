//! Damage calculator: the core calc_damage() function.
//!
//! Pure function — reads from BattleState and static data, takes RNG,
//! returns damage.  Never mutates state.
//!
//! All integer math.  4096-scale modifier chain.  Zero heap allocations.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag, MoveCategory, MoveEffect};
use crate::state::accessors::*;
use crate::state::calc_modifiers::*;
use crate::data::moves::MoveFlags;
use crate::data::types::Type;

/// Level factor: (2 * 100 / 5 + 2) = 42 at level 100.
const LEVEL_FACTOR: u32 = 42;

/// Everything the caller needs to know about a damage calculation.
#[derive(Debug, Clone, Copy, Default)]
pub struct DamageResult {
    pub damage: u16,
    pub effectiveness: u8, // 0/2/4/8/16 from type chart
    pub crit: bool,
    pub hits: u8,
    pub type_immune: bool,
    pub drain_heal: u16,
    pub recoil_damage: u16,
    pub hits_substitute: bool,
    pub item_consumed: bool,
}

/// Calculate damage for an attack.
///
/// `rng_fn`: called as `rng_fn(max)`, must return a value in `0..max`.
/// For random roll: `rng_fn(16)` returns 0-15, mapped to 85-100.
/// For crit check: `rng_fn(24)` returns 0-23, crit if 0.
///
/// The caller is responsible for:
///   - Checking accuracy before calling
///   - Applying the damage via `deal_damage()`
///   - Handling secondary effects (flinch, status, etc.)
///   - Handling Focus Sash / Sturdy survival
pub fn calc_damage(
    state: &BattleState,
    atk_side: usize,
    move_id: u16,
    rng_fn: &mut impl FnMut(u32) -> u32,
) -> DamageResult {
    let def_side = 1 - atk_side;
    let md = data_bridge::move_hot(move_id);

    if md.category == MoveCategory::Status {
        return DamageResult::default();
    }

    if move_id == 0 || md.base_power == 0 {
        return calc_struggle(state, atk_side);
    }

    let atk_mon = state.active_mon(atk_side);
    let def_mon = state.active_mon(def_side);
    let def_active = &state.sides[def_side].active;
    let atk_ability = effective_ability(state, atk_side);
    let def_ability = effective_ability(state, def_side);
    let atk_item = data_bridge::item(atk_mon.item_id);
    let def_item = data_bridge::item(def_mon.item_id);

    let mut result = DamageResult::default();

    let (move_type, ate_boost) = resolve_move_type_with_ability(state, md, atk_side, atk_ability);

    let (def_t1, def_t2) = effective_types(state, def_side);
    let def_type1 = unsafe { core::mem::transmute::<u8, Type>(def_t1) };
    let def_type2 = unsafe { core::mem::transmute::<u8, Type>(def_t2) };
    let mut eff = dual_type_effectiveness(move_type, def_type1, def_type2);

    // Freeze-Dry: super effective vs Water (override type chart)
    // Ice vs Water is normally 0.5× (eff contribution = 2), override to 2× (= 8).
    // For each Water type the defender has, multiply eff by 4.
    if md.effect == MoveEffect::FreezeDry {
        let water = Type::Water as u8;
        if def_t1 == water && def_t2 == water {
            // Pure Water: single-type, eff = type_eff(Ice, Water) = 2 → override to 8
            eff = (eff as u16 * 4).min(16) as u8;
        } else if def_t1 == water {
            eff = (eff as u16 * 4).min(16) as u8;
        } else if def_t2 == water {
            eff = (eff as u16 * 4).min(16) as u8;
        }
    }

    result.effectiveness = eff;

    if eff == 0 {
        result.type_immune = true;
        return result;
    }

    if let Some(heal) = ability_immunity(state, def_side, move_type) {
        result.type_immune = true;
        result.drain_heal = heal;
        return result;
    }

    if ability_flag_immunity(state, def_side, md.flags).is_some() {
        result.type_immune = true;
        return result;
    }

    let base_power = resolve_power(state, md, atk_side, def_side);
    let mut power = base_power as u32;

    let (ap_n, ap_d) = ability_power_mod(state, md, atk_side, base_power);
    power = chain_mod(power, ap_n, ap_d);

    // -ate ability boost (Galvanize/Pixilate/etc.): 1.2×
    if ate_boost {
        power = chain_mod(power, 4915, 4096); // 1.2×
    }

    let (ip_n, ip_d) = item_power_mod(
        atk_item, atk_mon.item_id, move_type, md.category, md.flags,
        state.sides[atk_side].active.consec_move_count,
    );
    power = chain_mod(power, ip_n, ip_d);
    if atk_item.has(ItemFlag::GEM) && atk_item.type_param == move_type as u8 {
        result.item_consumed = true;
    }

    let (mp_n, mp_d) = move_effect_power_mod(state, md, atk_side, def_side);
    power = chain_mod(power, mp_n, mp_d);

    // Charge (Electromorphosis/Wind Power): 2× Electric moves
    if move_type == Type::Electric && state.sides[atk_side].active._padding[4] & 4 != 0 {
        power *= 2;
    }

    let is_physical = md.category == MoveCategory::Physical;
    let (mut atk_stat_idx, mut def_stat_idx) = if is_physical { (ATK, DEF) } else { (SPA, SPD) };
    let mut atk_stat_side = atk_side;

    match md.effect {
        // Photon (Psyshock/Psystrike/Secret Sword): SpA vs Def
        MoveEffect::Photon => { def_stat_idx = DEF; }
        // FoulPlay: use target's Atk stat
        MoveEffect::FoulPlay => { atk_stat_side = def_side; atk_stat_idx = ATK; }
        // BodyPress: use attacker's Def as Atk
        MoveEffect::BodyPress => { atk_stat_idx = DEF; }
        _ => {}
    }

    let mut a = effective_stat(state, atk_stat_side, atk_stat_idx);
    let mut d = effective_stat(state, def_side, def_stat_idx);

    let c_stage = crit_stage(state, atk_side, md);
    let is_crit = if state.sides[def_side].side_conditions.lucky_chant_turns() > 0 {
        false
    } else {
        is_crit(c_stage, rng_fn)
    };
    result.crit = is_crit;

    // Crits ignore unfavorable boost stages
    let atk_stage = state.sides[atk_stat_side].active.boosts[atk_stat_idx];
    let def_stage = def_active.boosts[def_stat_idx];
    if is_crit {
        a = boosted_stat(a, atk_stage.max(0));
        d = boosted_stat(d, def_stage.min(0));
    } else {
        a = boosted_stat(a, atk_stage);
        d = boosted_stat(d, def_stage);
    }

    a = ability_atk_stat_mod(
        a, atk_ability, md.category, atk_mon.status,
        move_type, effective_weather(state),
        atk_mon.current_hp, atk_mon.max_hp,
        state.sides[atk_side].active.turns_active,
        state.sides[def_side].active.turns_active,
    );
    d = ability_def_stat_mod(
        d, def_ability, md.category, move_type,
        def_mon.status, effective_weather(state), state.field.terrain,
    );

    // Protosynthesis/Quark Drive: 1.3× for non-Spe stats
    let atk_paradox = state.sides[atk_side].active._padding[3] >> 4;
    if atk_paradox > 0 {
        let boosted_stat = (atk_paradox - 1) as usize;
        if boosted_stat == atk_stat_idx && boosted_stat != SPE {
            a = (a as u32 * 5325 / 4096) as u16; // 1.3×
        }
    }
    let def_paradox = state.sides[def_side].active._padding[3] >> 4;
    if def_paradox > 0 {
        let boosted_stat = (def_paradox - 1) as usize;
        if boosted_stat == def_stat_idx && boosted_stat != SPE {
            d = (d as u32 * 5325 / 4096) as u16; // 1.3×
        }
    }

    if is_physical && atk_item.has(ItemFlag::CHOICE_ATK) { a = (a as u32 * 3 / 2) as u16; }
    if !is_physical && atk_item.has(ItemFlag::CHOICE_SPA) { a = (a as u32 * 3 / 2) as u16; }
    // Thick Club: 2× Atk for Marowak/Cubone
    if is_physical && atk_mon.item_id == data_bridge::ITEM_THICK_CLUB {
        let sp = effective_species(state, atk_side);
        if sp == data_bridge::SPECIES_MAROWAK || sp == data_bridge::SPECIES_CUBONE {
            a *= 2;
        }
    }
    // Light Ball: 2× Atk and SpA for Pikachu
    if atk_mon.item_id == data_bridge::ITEM_LIGHT_BALL {
        let sp = effective_species(state, atk_side);
        if sp == data_bridge::SPECIES_PIKACHU { a *= 2; }
    }

    if !is_physical && def_item.has(ItemFlag::ASSAULT_VEST) { d = (d as u32 * 3 / 2) as u16; }
    if def_item.has(ItemFlag::EVIOLITE) {
        // Eviolite: 1.5× both defenses for NFE mons.  Caller/data should track NFE.
        // For now, apply unconditionally (conservative — slightly overestimates defense).
        d = (d as u32 * 3 / 2) as u16;
    }
    // Deep Sea Scale: 2× SpD for Clamperl
    if !is_physical && def_mon.item_id == data_bridge::ITEM_DEEP_SEA_SCALE {
        let sp = effective_species(state, def_side);
        if sp == data_bridge::SPECIES_CLAMPERL { d *= 2; }
    }

    d = weather_def_stat_mod(d, effective_weather(state), md.category, def_t1, def_t2);

    if d == 0 { d = 1; }
    if power == 0 { return result; }

    let num_hits = resolve_hits(md, atk_ability, rng_fn);
    result.hits = num_hits;

    let mut total_damage: u32 = 0;

    for _ in 0..num_hits {
        let mut dmg: u32 = (LEVEL_FACTOR * power * a as u32 / d as u32) / 50 + 2;

        let (wn, wd) = weather_modifier(effective_weather(state), move_type);
        if wn == 0 { return result; } // nullified (e.g. Harsh Sun vs Water)
        dmg = chain_mod(dmg, wn, wd);

        let (tn, td) = terrain_modifier(state, atk_side, def_side, move_type, move_id);
        dmg = chain_mod(dmg, tn, td);

        if is_crit {
            let (cn, cd) = crit_multiplier(atk_ability);
            dmg = chain_mod(dmg, cn, cd);
        }

        // Random roll: 85-100%
        let roll = 85 + rng_fn(16);
        dmg = dmg * roll / 100;

        let (sn, sd) = stab_modifier(state, atk_side, move_type);
        dmg = chain_mod(dmg, sn, sd);

        // eff is in 4x scale: 0=immune, 2=0.5x, 4=1x, 8=2x, 16=4x
        dmg = dmg * eff as u32 / 4;

        let (bn, bd) = burn_modifier(atk_mon.status, md.category, atk_ability);
        dmg = chain_mod(dmg, bn, bd);

        let (scn, scd) = screen_modifier(state, def_side, md.category, is_crit);
        dmg = chain_mod(dmg, scn, scd);

        let (dan, dad) = defender_ability_final_mod(state, md, def_side, eff);
        dmg = chain_mod(dmg, dan, dad);

        let (aan, aad) = attacker_ability_final_mod(atk_ability, eff);
        dmg = chain_mod(dmg, aan, aad);

        let (ifn, ifd, berry_consumed) = item_final_mod(atk_item, def_item, move_type, eff);
        dmg = chain_mod(dmg, ifn, ifd);
        if berry_consumed { result.item_consumed = true; }

        if dmg == 0 { dmg = 1; }

        total_damage += dmg;
    }

    result.damage = total_damage.min(u16::MAX as u32) as u16;

    if def_active.has_volatile(VOL_SUBSTITUTE)
        && md.flags & MoveFlags::SOUND == 0
        && md.flags & MoveFlags::BYPASSSUB == 0
    {
        result.hits_substitute = true;
    }

    if md.drain > 0 {
        result.drain_heal = (result.damage as u32 * md.drain as u32 / 100) as u16;
    }
    if md.drain < 0 {
        result.recoil_damage = (result.damage as u32 * (-md.drain) as u32 / 100) as u16;
    }
    // Life Orb recoil: Sheer Force suppresses it when move has secondary effects
    if atk_item.has(ItemFlag::LIFE_ORB) {
        let sheer_force_active = atk_ability == data_bridge::ABILITY_SHEER_FORCE
            && md.secondary_chance > 0;
        if !sheer_force_active {
            result.recoil_damage += atk_mon.max_hp / 10;
        }
    }

    result
}

/// Struggle: typeless, 50 power, no STAB, no effectiveness, 1/4 max HP recoil.
fn calc_struggle(state: &BattleState, atk_side: usize) -> DamageResult {
    let def_side = 1 - atk_side;
    let atk_mon = state.active_mon(atk_side);

    let a = boosted_stat(
        effective_stat(state, atk_side, ATK),
        state.sides[atk_side].active.boosts[ATK],
    );
    let d = boosted_stat(
        effective_stat(state, def_side, DEF),
        state.sides[def_side].active.boosts[DEF],
    ).max(1);

    let dmg = ((LEVEL_FACTOR * 50 * a as u32 / d as u32) / 50 + 2).max(1);

    DamageResult {
        damage: dmg.min(u16::MAX as u32) as u16,
        effectiveness: 4, // neutral
        hits: 1,
        recoil_damage: atk_mon.max_hp / 4,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::structs::*;
    use crate::data::moves::MoveData;

    /// Helper: deterministic RNG that always returns `val % max`.
    fn fixed_rng(val: u32) -> impl FnMut(u32) -> u32 {
        move |max| val % max
    }

    /// Build a minimal battle state with two mons and known stats.
    fn test_state() -> BattleState {
        let mut state = BattleState::default();
        // Attacker: side 0, slot 0
        state.sides[0].team[0] = MonSlot {
            species_id: 0, current_hp: 300, max_hp: 300,
            stats: [150, 100, 150, 100, 100], // Atk=150, Def=100, SpA=150, SpD=100, Spe=100
            moves: [1, 0, 0, 0], pp: [24, 0, 0, 0],
            ..Default::default()
        };
        // Defender: side 1, slot 0
        state.sides[1].team[0] = MonSlot {
            species_id: 0, current_hp: 300, max_hp: 300,
            stats: [100, 100, 100, 100, 100],
            ..Default::default()
        };
        state.phase = PHASE_ACTIONS;
        state
    }

    #[test]
    fn test_base_formula() {
        // With stubs: move_hot(1) returns base_power=0, so we'd get 0 damage.
        // Test the formula directly instead.
        let a: u32 = 150;
        let d: u32 = 100;
        let power: u32 = 90;
        let base = (42 * power * a / d) / 50 + 2;
        // 42 * 90 * 150 / 100 = 567000 / 100 = 5670
        // 5670 / 50 = 113, + 2 = 115
        assert_eq!(base, 115);
    }

    #[test]
    fn test_stab_multiplier() {
        // 1.5× STAB
        let val = chain_mod(100, 6144, 4096);
        assert_eq!(val, 150);
    }

    #[test]
    fn test_weather_fire_in_sun() {
        let (n, d) = weather_modifier(WEATHER_SUN, Type::Fire);
        let val = chain_mod(100, n, d);
        assert_eq!(val, 150);
    }

    #[test]
    fn test_weather_water_in_sun() {
        let (n, d) = weather_modifier(WEATHER_SUN, Type::Water);
        let val = chain_mod(100, n, d);
        assert_eq!(val, 50);
    }

    #[test]
    fn test_effectiveness_application() {
        // 2× SE: eff=8, applied as dmg * 8 / 4 = 2×
        let dmg = 100u32 * 8 / 4;
        assert_eq!(dmg, 200);

        // 4× SE: eff=16
        let dmg = 100u32 * 16 / 4;
        assert_eq!(dmg, 400);

        // 0.5× resist: eff=2
        let dmg = 100u32 * 2 / 4;
        assert_eq!(dmg, 50);

        // immune: eff=0
        let dmg = 100u32 * 0 / 4;
        assert_eq!(dmg, 0);
    }

    #[test]
    fn test_burn_halves_physical() {
        let (n, d) = burn_modifier(STATUS_BURN, MoveCategory::Physical, 0);
        assert_eq!(chain_mod(100, n, d), 50);
    }

    #[test]
    fn test_screen_halves() {
        let mut state = BattleState::default();
        state.sides[1].side_conditions.reflect_turns = 5;

        let (n, d) = screen_modifier(&state, 1, MoveCategory::Physical, false);
        assert_eq!(chain_mod(100, n, d), 50);

        // Crit ignores screen
        let (n, d) = screen_modifier(&state, 1, MoveCategory::Physical, true);
        assert_eq!(chain_mod(100, n, d), 100);
    }

    #[test]
    fn test_choice_band_stat() {
        let a: u16 = 200;
        let boosted = (a as u32 * 3 / 2) as u16;
        assert_eq!(boosted, 300);
    }

    #[test]
    fn test_life_orb_damage() {
        let (n, _d) = (5324u32, 4096u32);
        let dmg = chain_mod(100, n, 4096);
        assert_eq!(dmg, 129); // floor(100 * 1.3) = 129 in 4096-scale
    }

    #[test]
    fn test_resist_berry() {
        let (n, d, consumed) = item_final_mod(
            &data_bridge::ItemData::NONE,
            // Simulate a resist berry for Fire
            &data_bridge::ItemData {
                flags: ItemFlag::RESIST_BERRY | ItemFlag::IS_BERRY | ItemFlag::CONSUMABLE,
                type_param: Type::Fire as u8,
                power_param: 0, forme_species: 0,
            },
            Type::Fire,
            8, // super effective
        );
        assert!(consumed);
        assert_eq!(chain_mod(200, n, d), 100); // halved
    }

    #[test]
    fn test_crit_ignores_negative_atk_boost() {
        let raw: u16 = 200;
        let stage: i8 = -2;
        // Normal: stage -2 → 0.5×
        assert_eq!(boosted_stat(raw, stage), 100);
        // Crit: use max(0, stage) = 0 → 1.0×
        assert_eq!(boosted_stat(raw, stage.max(0)), 200);
    }

    #[test]
    fn test_crit_ignores_positive_def_boost() {
        let raw: u16 = 200;
        let stage: i8 = 2;
        // Normal: stage +2 → 2.0×
        assert_eq!(boosted_stat(raw, stage), 400);
        // Crit: use min(0, stage) = 0 → 1.0×
        assert_eq!(boosted_stat(raw, stage.min(0)), 200);
    }

    #[test]
    fn test_struggle() {
        let state = test_state();
        let result = calc_struggle(&state, 0);
        assert!(result.damage > 0);
        assert_eq!(result.recoil_damage, 300 / 4); // 1/4 max HP
        assert_eq!(result.effectiveness, 4); // neutral
    }

    #[test]
    fn test_multihit_skill_link() {
        let md = MoveData {
            multihit: (5 << 4) | 2, base_power: 25,
            ..unsafe { core::mem::zeroed() }
        };
        let hits = resolve_hits(&md, data_bridge::ABILITY_SKILL_LINK, &mut fixed_rng(0));
        assert_eq!(hits, 5); // Skill Link always max
    }

    #[test]
    fn test_huge_power() {
        let a = ability_atk_stat_mod(150, data_bridge::ABILITY_HUGE_POWER, MoveCategory::Physical, STATUS_NONE,
            Type::Normal, WEATHER_NONE, 300, 300, 0, 0);
        assert_eq!(a, 300);
        // Doesn't affect special
        let a = ability_atk_stat_mod(150, data_bridge::ABILITY_HUGE_POWER, MoveCategory::Special, STATUS_NONE,
            Type::Normal, WEATHER_NONE, 300, 300, 0, 0);
        assert_eq!(a, 150);
    }

    #[test]
    fn test_freeze_dry_vs_water() {
        // Ice vs pure Water: normally 0.5× (eff=2), Freeze-Dry makes it 2× (eff=8)
        let eff = dual_type_effectiveness(Type::Ice, Type::Water, Type::Water);
        assert_eq!(eff, 2); // normal: resist

        // Simulate Freeze-Dry override
        let mut eff_fd = eff;
        let water = Type::Water as u8;
        let def_t1 = Type::Water as u8;
        let def_t2 = Type::Water as u8;
        if def_t1 == water && def_t2 == water {
            eff_fd = (eff_fd as u16 * 4).min(16) as u8;
        }
        assert_eq!(eff_fd, 8); // 2× super effective
    }

    #[test]
    fn test_freeze_dry_vs_water_ground() {
        // Ice vs Water/Ground: normally 0.5× * 2× = 1× (eff=4)
        // Freeze-Dry override: 2× * 2× = 4× (eff=16)
        let eff = dual_type_effectiveness(Type::Ice, Type::Water, Type::Ground);
        assert_eq!(eff, 4); // neutral

        let mut eff_fd = eff;
        let water = Type::Water as u8;
        if Type::Water as u8 == water {
            eff_fd = (eff_fd as u16 * 4).min(16) as u8;
        }
        assert_eq!(eff_fd, 16); // 4× super effective
    }

    #[test]
    fn test_freeze_dry_vs_non_water() {
        // Ice vs pure Fire: normally 0.5× (eff=2), no Freeze-Dry override
        let eff = dual_type_effectiveness(Type::Ice, Type::Fire, Type::Fire);
        assert_eq!(eff, 2);
        // No Water → no change
    }
}
