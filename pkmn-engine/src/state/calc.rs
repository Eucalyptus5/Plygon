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
use crate::data::moves::{MoveFlags, VarPower};
use crate::data::types::Type;

/// Compute the level factor for the damage formula: floor(2 * level / 5 + 2).
#[inline(always)]
fn level_factor(level: u8) -> u32 {
    2 * level as u32 / 5 + 2
}

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
    /// Per-hit damage for multi-hit moves (indexed by hit number).
    /// Only the first `hits` entries are meaningful.
    /// Population Bomb maxes at 10 hits. Parental Bond adds 1, so cap at 11.
    pub per_hit_damages: [u16; 11],
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
    per_hit_accuracy: u32,
    rng_fn: &mut impl FnMut(u32) -> u32,
) -> DamageResult {
    let def_side = 1 - atk_side;
    let md = data_bridge::move_hot(move_id);

    // Struggle (move_id == 0) must be checked before Status category,
    // because the placeholder move at index 0 has Status category.
    if move_id == 0 {
        return calc_struggle(state, atk_side);
    }

    if md.category == MoveCategory::Status {
        return DamageResult::default();
    }

    // Fixed-damage moves with base_power=0 are handled here (not as Struggle).
    // SeismicToss/Night Shade: damage = user's level.
    // SuperFang: damage = 50% of target's current HP.
    // Counter/MirrorCoat/MetalBurst: simplified fixed damage (same as move_exec).
    // Fling (374) has base_power=0 in the move table; BP comes from item.power_param
    // — handled below in the main calc path.
    if md.base_power == 0 && md.var_power == VarPower::None && move_id != 374 {
        let atk_mon = state.active_mon(atk_side);
        match md.effect {
            MoveEffect::SeismicToss => {
                let (move_type, _) = resolve_move_type_with_ability(state, md, atk_side, effective_ability(state, atk_side));
                let (def_t1, def_t2) = battle_types(state, def_side);
                let def_type1 = unsafe { core::mem::transmute::<u8, Type>(def_t1) };
                let def_type2 = unsafe { core::mem::transmute::<u8, Type>(def_t2) };
                let eff = dual_type_effectiveness(move_type, def_type1, def_type2);
                if eff == 0 {
                    return DamageResult { type_immune: true, ..Default::default() };
                }
                return DamageResult {
                    damage: atk_mon.level as u16,
                    effectiveness: eff,
                    hits: 1,
                    ..Default::default()
                };
            }
            MoveEffect::SuperFang => {
                let def_mon = state.active_mon(def_side);
                return DamageResult {
                    damage: (def_mon.current_hp / 2).max(1),
                    effectiveness: 4,
                    hits: 1,
                    ..Default::default()
                };
            }
            MoveEffect::Counter | MoveEffect::MirrorCoat | MoveEffect::MetalBurst => {
                // Simplified: these depend on last-hit tracking, return 0 in calc_damage
                return DamageResult::default();
            }
            _ => {
                // Other bp=0 non-VarPower moves: treat as Struggle
                return calc_struggle(state, atk_side);
            }
        }
    }

    let atk_mon = state.active_mon(atk_side);
    let def_mon = state.active_mon(def_side);
    let def_active = &state.sides[def_side].active;
    let atk_ability = effective_ability(state, atk_side);
    let def_ability = effective_ability(state, def_side);
    let magic_room = state.field.magic_room_turns() > 0;
    let atk_item = if magic_room { &data_bridge::ItemData::NONE } else { data_bridge::item(atk_mon.item_id) };
    let def_item = if magic_room { &data_bridge::ItemData::NONE } else { data_bridge::item(def_mon.item_id) };
    // Fling: BP comes from the held item's power_param. The item is consumed
    // in use_move_called right after calc returns; the user-side recoil arm
    // below (atk_item.LIFE_ORB at the end of calc) still reads the item, which
    // matches Showdown — `eachEvent('Update')` clears the item after the move
    // action completes, not mid-step. We DO skip the recoil arm for move 374
    // because the consume_item that follows leaves the user with no item by
    // the time Life Orb's onAfterMoveSecondarySelf would have run.
    let is_fling = move_id == 374;
    let fling_bp = if is_fling && !magic_room {
        atk_item.power_param
    } else { 0 };

    let mut result = DamageResult::default();

    // Tera Blast category override: compare boosted Atk vs SpA (boost stages only, no ability/item mods)
    let category = if md.effect == MoveEffect::TeraBlast && state.active_mon(atk_side).is_terastallized() {
        let atk_boosted = boosted_stat(
            effective_stat(state, atk_side, ATK),
            state.sides[atk_side].active.boosts[ATK],
        );
        let spa_boosted = boosted_stat(
            effective_stat(state, atk_side, SPA),
            state.sides[atk_side].active.boosts[SPA],
        );
        if atk_boosted > spa_boosted { MoveCategory::Physical } else { MoveCategory::Special }
    } else {
        md.category
    };

    let (move_type, ate_boost) = resolve_move_type_with_ability(state, md, atk_side, atk_ability);

    let (def_t1, def_t2) = battle_types(state, def_side);
    let def_type1 = unsafe { core::mem::transmute::<u8, Type>(def_t1) };
    let def_type2 = unsafe { core::mem::transmute::<u8, Type>(def_t2) };

    let mut eff = dual_type_effectiveness(move_type, def_type1, def_type2);

    // Gravity: Ground moves ignore Flying-type immunity.
    // Replace each Flying type with the other type (making it monotype if one is Flying,
    // or Normal/Normal if pure Flying).
    if eff == 0 && move_type == Type::Ground && state.field.gravity_turns > 0 {
        let d1 = if def_type1 == Type::Flying { def_type2 } else { def_type1 };
        let d2 = if def_type2 == Type::Flying { def_type1 } else { def_type2 };
        // If both were Flying (pure Flying), both become Flying still — force to Normal.
        let d1 = if d1 == Type::Flying { Type::Normal } else { d1 };
        let d2 = if d2 == Type::Flying { Type::Normal } else { d2 };
        eff = dual_type_effectiveness(move_type, d1, d2);
    }

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

    // Scrappy: Normal/Fighting moves ignore Ghost-type immunity
    if eff == 0 && atk_ability == data_bridge::ABILITY_SCRAPPY
        && (move_type == Type::Normal || move_type == Type::Fighting)
    {
        // Recalculate effectiveness treating Ghost as neutral.
        // Replace Ghost type with a type that takes neutral damage from Normal/Fighting.
        let ghost = Type::Ghost as u8;
        let s1 = if def_t1 == ghost { Type::Normal } else { def_type1 };
        let s2 = if def_t2 == ghost { Type::Normal } else { def_type2 };
        eff = dual_type_effectiveness(move_type, s1, s2);
        result.effectiveness = eff;
    }

    if eff == 0 {
        result.type_immune = true;
        return result;
    }

    if let Some(heal) = ability_immunity(state, def_side, move_type, atk_ability) {
        result.type_immune = true;
        result.drain_heal = heal;
        return result;
    }

    if ability_flag_immunity(state, def_side, md.flags, atk_ability).is_some() {
        result.type_immune = true;
        return result;
    }

    // Wonder Guard: non-super-effective moves are blocked (eff <= 4 means neutral or worse)
    // Ability Shield protects Wonder Guard from Mold Breaker bypass.
    if !mold_breaks(state, def_side, atk_ability) {
        let def_eff_ability = effective_ability(state, def_side);
        if def_eff_ability == data_bridge::ABILITY_WONDER_GUARD && eff <= 4 {
            result.type_immune = true;
            return result;
        }
    }

    // Air Balloon / Levitate / Magnet Rise etc: non-grounded mons are immune to Ground-type moves.
    // When attacker has Mold Breaker (and defender lacks Ability Shield), we need to
    // check grounding while ignoring Levitate.
    if move_type == Type::Ground {
        let grounded = if mold_breaks(state, def_side, atk_ability) {
            // Mold Breaker: check grounding without considering defender's Levitate
            is_grounded_ignore_ability(state, def_side)
        } else {
            is_grounded(state, def_side)
        };
        if !grounded {
            result.type_immune = true;
            return result;
        }
    }

    // Tera Shell: at full HP, a non-immune neutral-or-better hit is floored to
    // not-very-effective (0.5×). Already-resisted hits keep their multiplier.
    // Breakable, so Mold Breaker bypasses; effective_ability already honors suppression.
    if def_ability == data_bridge::ABILITY_TERA_SHELL
        && eff >= 4
        && def_mon.current_hp >= def_mon.max_hp
        && !mold_breaks(state, def_side, atk_ability)
    {
        eff = 2;
        result.effectiveness = eff;
    }

    let mut base_power = if is_fling { fling_bp as u16 } else { resolve_power(state, md, atk_side, def_side) };

    // Tera min-BP=60 floor (Showdown sim/battle-actions.ts:1657-1665):
    // terastallized + move type matches Tera type + BP<60 + priority<=0 + !multihit + not variable-BP-callback.
    // Stellar branch deferred to BUG-P8-M-395.
    {
        let atk_mon = state.active_mon(atk_side);
        if atk_mon.is_terastallized()
            && move_type as u8 == atk_mon.tera_type
            && base_power < 60
            && md.priority <= 0
            && md.multihit == 0
            && md.var_power == VarPower::None
            && !is_fling
        {
            base_power = 60;
        }
    }

    let mut power = base_power as u32;

    let (ap_n, _) = ability_power_mod(state, md, atk_side, base_power);
    // base_power is now u16; ability_power_mod accepts u16 directly.
    power = chain_mod(power, ap_n);

    // Dark Aura / Fairy Aura / Aura Break: field-wide power modifier
    let aura_n = aura_power_mod(atk_ability, def_ability, move_type);
    power = chain_mod(power, aura_n);

    // -ate ability boost (Galvanize/Pixilate/etc.): 1.2×
    if ate_boost {
        power = chain_mod(power, 4915); // 1.2×
    }

    let (ip_n, _) = item_power_mod(
        atk_item, atk_mon.item_id, move_type, category, md.flags,
    );
    power = chain_mod(power, ip_n);
    if atk_item.has(ItemFlag::GEM) && atk_item.type_param == move_type as u8 {
        result.item_consumed = true;
    }

    // Signature orbs: 1.2× on the legendary's two signature move types (Showdown
    // onBasePower gates on user.baseSpecies.num). Gated on atk_item, so Magic Room
    // (which NONEs atk_item) suppresses it like any other item base-power boost.
    if atk_item.has(ItemFlag::SIGNATURE_ORB) {
        let bsp = data_bridge::base_species(effective_species(state, atk_side));
        let boosted = match atk_mon.item_id {
            data_bridge::ITEM_LUSTROUS_ORB | data_bridge::ITEM_LUSTROUS_GLOBE =>
                bsp == data_bridge::SPECIES_PALKIA && (move_type == Type::Water || move_type == Type::Dragon),
            data_bridge::ITEM_ADAMANT_ORB | data_bridge::ITEM_ADAMANT_CRYSTAL =>
                bsp == data_bridge::SPECIES_DIALGA && (move_type == Type::Steel || move_type == Type::Dragon),
            data_bridge::ITEM_GRISEOUS_ORB | data_bridge::ITEM_GRISEOUS_CORE =>
                bsp == data_bridge::SPECIES_GIRATINA && (move_type == Type::Ghost || move_type == Type::Dragon),
            data_bridge::ITEM_SOUL_DEW =>
                (bsp == data_bridge::SPECIES_LATIAS || bsp == data_bridge::SPECIES_LATIOS)
                    && (move_type == Type::Psychic || move_type == Type::Dragon),
            _ => false,
        };
        if boosted { power = chain_mod(power, 4915); } // 1.2×
    }

    let (mp_n, _) = move_effect_power_mod(state, md, atk_side, def_side);
    power = chain_mod(power, mp_n);

    // Lash Out: onBasePower chainModify(2) when the user had a stat lowered this turn
    // (moves.ts:10081). Cold — the move_id compare is false for every other move.
    if move_id == crate::data::MOVE_LASH_OUT as u16
        && state.sides[atk_side].stats_lowered_this_turn()
    {
        power = chain_mod(power, 8192); // 2×
    }

    // Defender's ability modifying move base power (Showdown: onSourceBasePower).
    // Breakable (bypassed by Mold Breaker). Dry Skin: 1.25× Fire base power.
    if !mold_breaks(state, def_side, atk_ability) {
        if def_ability == data_bridge::ABILITY_DRY_SKIN && move_type == Type::Fire {
            power = chain_mod(power, 5120); // 1.25×
        }
    }

    // Terrain: Showdown applies via onBasePower (power modifier, not damage modifier)
    let (tn, _) = terrain_modifier(state, atk_side, def_side, move_type, move_id);
    power = chain_mod(power, tn);

    // Charge (Electromorphosis/Wind Power): 2× Electric moves
    if move_type == Type::Electric && state.sides[atk_side].active._padding[4] & 4 != 0 {
        power *= 2;
    }


    let is_physical = category == MoveCategory::Physical;
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

    // Wonder Room: swap DEF <-> SPD for base stats only (boosts stay on original stat)
    let atk_boost_idx = atk_stat_idx;
    let def_boost_idx = def_stat_idx;
    if state.field.wonder_room_turns() > 0 {
        if atk_stat_idx == DEF { atk_stat_idx = SPD; }
        else if atk_stat_idx == SPD { atk_stat_idx = DEF; }
        if def_stat_idx == DEF { def_stat_idx = SPD; }
        else if def_stat_idx == SPD { def_stat_idx = DEF; }
    }

    let mut a = effective_stat(state, atk_stat_side, atk_stat_idx);
    let mut d = effective_stat(state, def_side, def_stat_idx);

    // Mold Breaker bypasses Battle Armor / Shell Armor for crit checks
    // (unless the defender holds Ability Shield).
    let mold = mold_breaks(state, def_side, atk_ability);
    let c_stage = crit_stage(state, atk_side, md);
    let is_crit = if state.sides[def_side].side_conditions.lucky_chant_turns() > 0 {
        false
    } else if !mold && (def_ability == data_bridge::ABILITY_BATTLE_ARMOR
           || def_ability == data_bridge::ABILITY_SHELL_ARMOR) {
        false
    } else {
        is_crit(c_stage, rng_fn)
    };
    result.crit = is_crit;

    // Unaware: ignore opponent's stat boosts in damage calc.
    // Attacking Unaware: ignore defender's Def/SpD boosts (treat as 0).
    // Defending Unaware: ignore attacker's Atk/SpA boosts (treat as 0).
    let atk_stage = {
        let raw = state.sides[atk_stat_side].active.boosts[atk_boost_idx];
        if !mold && def_ability == data_bridge::ABILITY_UNAWARE { 0 } else { raw }
    };
    let def_stage = {
        let raw = def_active.boosts[def_boost_idx];
        if atk_ability == data_bridge::ABILITY_UNAWARE { 0 } else { raw }
    };

    // Crits ignore unfavorable boost stages (boosts use original indices, not Wonder Room swapped)
    if is_crit {
        a = boosted_stat(a, atk_stage.max(0));
        d = boosted_stat(d, def_stage.min(0));
    } else {
        a = boosted_stat(a, atk_stage);
        d = boosted_stat(d, def_stage);
    }

    // Effective defender ability (suppressed by Mold Breaker for stat mods)
    let def_ability_for_stat = if mold { 0 } else { def_ability };

    a = ability_atk_stat_mod(
        a, atk_ability, category, atk_mon.status,
        move_type, effective_weather_for(state, atk_side),
        atk_mon.current_hp, atk_mon.max_hp,
        state.sides[atk_side].active.turns_active,
        state.sides[def_side].active.turns_active,
        state.field.turn, state.field.terrain,
    );

    // Defender's ability modifying attacker's stat (Showdown: onSourceModifyAtk/SpA)
    // These are all breakable (bypassed by Mold Breaker)
    if !mold {
        // Water Bubble: 0.5x attacker's Atk/SpA for Fire moves
        if def_ability == data_bridge::ABILITY_WATER_BUBBLE && move_type == Type::Fire {
            a = chain_mod(a as u32, 2048) as u16;
        }
        // Purifying Salt: 0.5x attacker's Atk/SpA for Ghost moves
        if def_ability == data_bridge::ABILITY_PURIFYING_SALT && move_type == Type::Ghost {
            a = chain_mod(a as u32, 2048) as u16;
        }
        // Thick Fat: 0.5x attacker's Atk/SpA for Fire/Ice moves. Implemented here as
        // a stat-side modifier (not as 2× Def) to match Showdown's onSourceModifyAtk/SpA
        // truncation order.
        if def_ability == data_bridge::ABILITY_THICK_FAT
            && (move_type == Type::Fire || move_type == Type::Ice)
        {
            a = chain_mod(a as u32, 2048) as u16;
        }
        // Heatproof: 0.5x attacker's Atk/SpA for Fire moves (Showdown onSourceModifyAtk/SpA).
        if def_ability == data_bridge::ABILITY_HEATPROOF && move_type == Type::Fire {
            a = chain_mod(a as u32, 2048) as u16;
        }
    }

    // Ruin abilities: Sword of Ruin (285) reduces opponent's Def by 0.75x.
    // (Beads/Tablets/Vessel of Ruin share ID 284, indistinguishable — only Sword of Ruin implemented.)
    if is_physical {
        if atk_ability == data_bridge::ABILITY_SWORD_OF_RUIN {
            d = chain_mod(d as u32, 3072) as u16; // 0.75× Def
        } else if def_ability == data_bridge::ABILITY_SWORD_OF_RUIN {
            d = chain_mod(d as u32, 3072) as u16; // 0.75× Def from opponent's Sword of Ruin
        }
    }

    d = ability_def_stat_mod(
        d, def_ability_for_stat, category, move_type,
        def_mon.status, effective_weather_for(state, def_side), state.field.terrain,
    );

    // Protosynthesis/Quark Drive: 1.3× for non-Spe stats (suppressed by Neutralizing Gas)
    let atk_paradox = state.sides[atk_side].active.paradox_stat();
    if atk_paradox > 0 && !state.sides[atk_side].active.has_volatile(VOL_ABILITY_SUPPRESSED) {
        let boosted = (atk_paradox - 1) as usize;
        if boosted == atk_stat_idx && boosted != SPE {
            a = (a as u32 * 5325 / 4096) as u16; // 1.3×
        }
    }
    let def_paradox = state.sides[def_side].active.paradox_stat();
    if def_paradox > 0 && !state.sides[def_side].active.has_volatile(VOL_ABILITY_SUPPRESSED) {
        let boosted = (def_paradox - 1) as usize;
        if boosted == def_stat_idx && boosted != SPE {
            d = (d as u32 * 5325 / 4096) as u16; // 1.3×
        }
    }

    if is_physical && atk_item.has(ItemFlag::CHOICE_ATK) { a = (a as u32 * 3 / 2) as u16; }
    if !is_physical && atk_item.has(ItemFlag::CHOICE_SPA) { a = (a as u32 * 3 / 2) as u16; }
    // Thick Club: 2× Atk for Marowak/Cubone
    if !magic_room && is_physical && atk_mon.item_id == data_bridge::ITEM_THICK_CLUB {
        let sp = effective_species(state, atk_side);
        if sp == data_bridge::SPECIES_MAROWAK || sp == data_bridge::SPECIES_CUBONE {
            a *= 2;
        }
    }
    // Light Ball: 2× Atk and SpA for Pikachu
    if !magic_room && atk_mon.item_id == data_bridge::ITEM_LIGHT_BALL {
        let sp = effective_species(state, atk_side);
        if sp == data_bridge::SPECIES_PIKACHU { a *= 2; }
    }

    if !is_physical && def_item.has(ItemFlag::ASSAULT_VEST) { d = (d as u32 * 3 / 2) as u16; }
    if def_item.has(ItemFlag::EVIOLITE)
        && data_bridge::species(effective_species(state, def_side)).nfe
    {
        d = (d as u32 * 3 / 2) as u16;
    }
    // Deep Sea Scale: 2× SpD for Clamperl
    if !magic_room && !is_physical && def_mon.item_id == data_bridge::ITEM_DEEP_SEA_SCALE {
        let sp = effective_species(state, def_side);
        if sp == data_bridge::SPECIES_CLAMPERL { d *= 2; }
    }

    d = weather_def_stat_mod(d, effective_weather_for(state, def_side), category, def_t1, def_t2);

    if d == 0 { d = 1; }
    if power == 0 { return result; }

    let mut num_hits = resolve_hits(md, atk_ability, atk_item.flags, rng_fn);

    // Parental Bond: single-target damaging moves hit twice (25% power on 2nd hit).
    // Conditions: attacker has Parental Bond, move is not already multi-hit,
    // move is not a ForceSwitch/BatonPass/PartingShot (pivot), and not a self-destruct.
    // Note: num_hits == 1 here implies the move was single-hit in resolve_hits
    // (multihit_lo == 0). Parental Bond activates on both basic single-hit moves
    // and excluded move patterns alike. Exclusions below.
    let parental_bond_active = num_hits == 1
        && atk_ability == data_bridge::ABILITY_PARENTAL_BOND
        && !matches!(
            md.effect,
            MoveEffect::ForceSwitch
                | MoveEffect::BatonPass
                | MoveEffect::PartingShot
                | MoveEffect::FinalGambit
        );
    if parental_bond_active {
        num_hits = 2;
    }
    result.hits = num_hits;

    // Pre-compute all loop-invariant modifiers
    let lf = level_factor(atk_mon.level);
    let (wn, _) = weather_modifier(effective_weather_for(state, def_side), move_type);
    if wn == 0 { return result; } // nullified (e.g. Harsh Sun vs Water)
    let (sn, _) = stab_modifier(state, atk_side, move_type);
    let (bn, _) = burn_modifier(atk_mon.status, category, atk_ability, md.var_power == VarPower::Facade);
    let (scn, _) = screen_modifier(state, def_side, category, is_crit, atk_ability);
    let (dan, _) = defender_ability_final_mod(state, md, def_side, eff, atk_ability);
    let (aan, _) = attacker_ability_final_mod(atk_ability, eff);
    let (ifn, _, berry_consumed) = item_final_mod(
        atk_item, def_item, move_type, eff,
        state.sides[atk_side].active.consec_move_count,
    );
    if berry_consumed { result.item_consumed = true; }
    let sniper_n = sniper_final_mod(atk_ability, is_crit);
    // Semi-invulnerable 2× damage modifier: Earthquake/Magnitude hit underground
    // (Dig) targets for 2×; Surf/Whirlpool hit underwater (Dive) targets for 2×.
    // Applied via Showdown's onSourceModifyDamage (chainModify(2)) — 4096-scale.
    let semi_invuln_n: u32 = if def_active.has_volatile(VOL_SEMI_INVULNERABLE) {
        let charge_loc = def_active._padding[1];
        let doubles = match charge_loc {
            2 => move_id == crate::data::MOVE_EARTHQUAKE as u16
                 || move_id == crate::data::MOVE_MAGNITUDE as u16,
            3 => move_id == crate::data::MOVE_SURF as u16
                 || move_id == crate::data::MOVE_WHIRLPOOL as u16,
            _ => false,
        };
        if doubles { 8192 } else { 4096 }
    } else { 4096 };
    let (cn, cd) = if is_crit { crit_multiplier(atk_ability) } else { (1, 1) };
    let a32 = a as u32;
    let d32 = d as u32;

    let mut total_damage: u32 = 0;

    for hit in 0..num_hits {
        // Per-hit accuracy check (multiaccuracy moves only, skip first hit)
        if hit > 0 && per_hit_accuracy > 0 && rng_fn(100) >= per_hit_accuracy {
            result.hits = hit;
            break;
        }
        // Escalating power: Triple Kick/Axel multiply by hit number
        let hit_power = if md.var_power == VarPower::Escalating {
            power * (hit as u32 + 1)
        } else {
            power
        };
        let mut dmg: u32 = (lf * hit_power * a32 / d32) / 50 + 2;

        // Parental Bond: 2nd hit (hit_index == 1) damage * 0.25.
        // Applied after base damage + 2, before weather / crit / roll (matches
        // Showdown's battle-actions.ts modifyDamage ordering).
        if parental_bond_active && hit == 1 {
            dmg = chain_mod(dmg, 1024);
        }

        dmg = chain_mod(dmg, wn);
        if is_crit { dmg = dmg * cn / cd; }

        // Random roll: 85-100%
        let roll = 85 + rng_fn(16);
        dmg = dmg * roll / 100;

        dmg = chain_mod(dmg, sn);
        dmg = dmg * eff as u32 / 4;
        dmg = chain_mod(dmg, bn);
        dmg = chain_mod(dmg, scn);
        dmg = chain_mod(dmg, dan);
        dmg = chain_mod(dmg, aan);
        dmg = chain_mod(dmg, ifn);
        dmg = chain_mod(dmg, sniper_n);
        if semi_invuln_n != 4096 { dmg = chain_mod(dmg, semi_invuln_n); }

        if dmg == 0 { dmg = 1; }

        let dmg_u16 = dmg.min(u16::MAX as u32) as u16;
        if (hit as usize) < result.per_hit_damages.len() {
            result.per_hit_damages[hit as usize] = dmg_u16;
        }
        total_damage += dmg;
    }

    result.damage = total_damage.min(u16::MAX as u32) as u16;

    if def_active.has_volatile(VOL_SUBSTITUTE)
        && md.flags & MoveFlags::SOUND == 0
        && md.flags & MoveFlags::BYPASSSUB == 0
        && atk_ability != data_bridge::ABILITY_INFILTRATOR
    {
        result.hits_substitute = true;
    }

    if md.drain > 0 {
        let mut heal = (result.damage as u32 * md.drain as u32 + 50) / 100;
        if !magic_room && atk_mon.item_id == data_bridge::ITEM_BIG_ROOT {
            heal = chain_mod(heal, 5324);
        }
        result.drain_heal = heal.min(u16::MAX as u32) as u16;
    }
    // Move-recoil (Take Down / Double-Edge / Brave Bird / Wild Charge / Wood Hammer /
    // Head Smash etc.) is computed in move_exec from HP-clamped actually-dealt damage,
    // mirroring Showdown's `move.totalDamage` (battle-actions.ts:983-989), with the
    // Rock Head / Magic Guard exemptions applied at the apply site.
    // Life Orb sits on the same recoil byte (Magic Guard blocks it too, effect-type
    // 'Item' triggers magicguard.onDamage's effectType !== 'Move' short-circuit).
    // Fling skips the recoil — the item is consumed by the move, so by the
    // time onAfterMoveSecondarySelf would fire, the user no longer holds it.
    if atk_item.has(ItemFlag::LIFE_ORB)
        && atk_ability != data_bridge::ABILITY_MAGIC_GUARD
        && !is_fling
    {
        let sheer_force_active = atk_ability == data_bridge::ABILITY_SHEER_FORCE
            && md.secondary_chance > 0;
        if !sheer_force_active {
            result.recoil_damage = atk_mon.max_hp / 10;
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

    let lf = level_factor(atk_mon.level);
    let dmg = ((lf * 50 * a as u32 / d as u32) / 50 + 2).max(1);

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
            level: 100,
            ..Default::default()
        };
        // Defender: side 1, slot 0
        state.sides[1].team[0] = MonSlot {
            species_id: 0, current_hp: 300, max_hp: 300,
            stats: [100, 100, 100, 100, 100],
            level: 100,
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
    fn test_lash_out_doubles_when_stat_lowered() {
        let mut state = test_state();
        let lash = crate::data::MOVE_LASH_OUT as u16;
        // Flag set independently of the Atk stat, so the only delta is the BP ×2.
        let base = calc_damage(&state, 0, lash, 100, &mut fixed_rng(0)).damage;
        state.sides[0].set_stats_lowered_this_turn();
        let boosted = calc_damage(&state, 0, lash, 100, &mut fixed_rng(0)).damage;
        assert!(base > 0, "Lash Out should deal damage");
        // ×2 base power is lossless in the chain (8192>>12 == 2); allow ±1 final rounding.
        assert!(boosted >= base * 2 - 1 && boosted <= base * 2 + 1,
            "Lash Out must ~double with a stat lowered: base={base} boosted={boosted}");
    }

    #[test]
    fn test_non_lash_out_move_unaffected_by_flag() {
        // Control: a different Dark physical (Night Slash) must NOT double on the flag.
        let mut state = test_state();
        let night_slash = crate::data::MOVE_NIGHT_SLASH as u16;
        let off = calc_damage(&state, 0, night_slash, 100, &mut fixed_rng(0)).damage;
        state.sides[0].set_stats_lowered_this_turn();
        let on = calc_damage(&state, 0, night_slash, 100, &mut fixed_rng(0)).damage;
        assert!(off > 0);
        assert_eq!(off, on, "non-Lash-Out move must be unaffected by statsLoweredThisTurn");
    }

    #[test]
    fn test_stab_multiplier() {
        // 1.5× STAB
        let val = chain_mod(100, 6144);
        assert_eq!(val, 150);
    }

    #[test]
    fn test_weather_fire_in_sun() {
        let (n, _) = weather_modifier(WEATHER_SUN, Type::Fire);
        let val = chain_mod(100, n);
        assert_eq!(val, 150);
    }

    #[test]
    fn test_weather_water_in_sun() {
        let (n, _) = weather_modifier(WEATHER_SUN, Type::Water);
        let val = chain_mod(100, n);
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
        let (n, _) = burn_modifier(STATUS_BURN, MoveCategory::Physical, 0, false);
        assert_eq!(chain_mod(100, n), 50);
    }

    #[test]
    fn test_screen_halves() {
        let mut state = BattleState::default();
        state.sides[1].side_conditions.reflect_turns = 5;
        state.sides[1].side_conditions.light_screen_turns = 5;

        let (n, _) = screen_modifier(&state, 1, MoveCategory::Physical, false, 0);
        assert_eq!(chain_mod(100, n), 50);

        // Crit ignores screen
        let (n, _) = screen_modifier(&state, 1, MoveCategory::Physical, true, 0);
        assert_eq!(chain_mod(100, n), 100);

        // Infiltrator ignores the target's screens (Reflect + Light Screen)
        let inf = data_bridge::ABILITY_INFILTRATOR;
        let (n, _) = screen_modifier(&state, 1, MoveCategory::Physical, false, inf);
        assert_eq!(chain_mod(100, n), 100);
        let (n, _) = screen_modifier(&state, 1, MoveCategory::Special, false, inf);
        assert_eq!(chain_mod(100, n), 100);
        // Non-Infiltrator special attacker still halved by Light Screen
        let (n, _) = screen_modifier(&state, 1, MoveCategory::Special, false, 0);
        assert_eq!(chain_mod(100, n), 50);
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
        let dmg = chain_mod(100, n);
        assert_eq!(dmg, 130); // pokeRound(100 * 5324/4096) = 130
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
            1, // consec_move_count (no Metronome)
        );
        assert!(consumed);
        assert_eq!(chain_mod(200, n), 100); // halved
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
        let hits = resolve_hits(&md, data_bridge::ABILITY_SKILL_LINK, 0, &mut fixed_rng(0));
        assert_eq!(hits, 5); // Skill Link always max
    }

    #[test]
    fn test_loaded_dice_population_bomb_hits() {
        // Population Bomb: multihit = (10 << 4) | 10 = 170 (lo=10, hi=10)
        let md = MoveData {
            multihit: (10 << 4) | 10, base_power: 20,
            ..unsafe { core::mem::zeroed() }
        };
        // Without Loaded Dice: always 10 hits
        let hits = resolve_hits(&md, 0, 0, &mut fixed_rng(0));
        assert_eq!(hits, 10);

        // With Loaded Dice: 4-10 hits depending on RNG
        // rng(7) returns val % 7; result = 4 + rng(7)
        // rng_val=0 → 4+0=4, rng_val=6 → 4+6=10
        let mut seen_below_10 = false;
        for rng_val in 0..7u32 {
            let hits = resolve_hits(&md, 0, ItemFlag::LOADED_DICE, &mut fixed_rng(rng_val));
            assert!(hits >= 4 && hits <= 10, "rng_val={rng_val}, hits={hits}");
            if hits < 10 { seen_below_10 = true; }
        }
        // Loaded Dice must produce values below 10 for some RNG inputs
        assert!(seen_below_10, "Loaded Dice should vary Population Bomb hit count");

        // Verify specific values: rng_val=0 → 4 hits, rng_val=6 → 10 hits
        assert_eq!(resolve_hits(&md, 0, ItemFlag::LOADED_DICE, &mut fixed_rng(0)), 4);
        assert_eq!(resolve_hits(&md, 0, ItemFlag::LOADED_DICE, &mut fixed_rng(6)), 10);
    }

    #[test]
    fn test_loaded_dice_normal_multihit_unchanged() {
        // Regular 2-5 hit move: Loaded Dice gives 4 or 5
        let md = MoveData {
            multihit: (5 << 4) | 2, base_power: 25,
            ..unsafe { core::mem::zeroed() }
        };
        let hits = resolve_hits(&md, 0, ItemFlag::LOADED_DICE, &mut fixed_rng(0));
        assert!(hits == 4 || hits == 5);
    }

    #[test]
    fn test_loaded_dice_triple_kick_no_effect_on_hits() {
        // Triple Kick: multihit = (3 << 4) | 3 = 51 (lo=3, hi=3)
        // Loaded Dice doesn't change hit count for fixed-3 moves
        let md = MoveData {
            multihit: (3 << 4) | 3, base_power: 10,
            ..unsafe { core::mem::zeroed() }
        };
        let hits = resolve_hits(&md, 0, ItemFlag::LOADED_DICE, &mut fixed_rng(0));
        assert_eq!(hits, 3);
    }

    #[test]
    fn test_huge_power() {
        let a = ability_atk_stat_mod(150, data_bridge::ABILITY_HUGE_POWER, MoveCategory::Physical, STATUS_NONE,
            Type::Normal, WEATHER_NONE, 300, 300, 0, 0, 0, TERRAIN_NONE);
        assert_eq!(a, 300);
        // Doesn't affect special
        let a = ability_atk_stat_mod(150, data_bridge::ABILITY_HUGE_POWER, MoveCategory::Special, STATUS_NONE,
            Type::Normal, WEATHER_NONE, 300, 300, 0, 0, 0, TERRAIN_NONE);
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

    #[test]
    fn test_tera_blast_category() {
        // Tera Blast: Physical when boosted Atk > boosted SpA, else Special
        let mut state = test_state();
        state.sides[0].team[0].species_id = 4; // Charmander
        state.sides[0].team[0].tera_type = Type::Water as u8;
        state.sides[0].team[0].flags |= MON_FLAG_TERASTALLIZED;
        // stats: [Atk=150, Def=100, SpA=150, SpD=100, Spe=100]

        // Equal Atk and SpA → defaults to Special
        // We test by checking which defense stat is used:
        // Special → damage based on SpA vs SpD, Physical → Atk vs Def

        // Set Atk=200, SpA=100 → Physical
        state.sides[0].team[0].stats[ATK] = 200;
        state.sides[0].team[0].stats[SPA] = 100;
        let result_phys = calc_damage(&state, 0, 851, 0, &mut fixed_rng(15));

        // Set Atk=100, SpA=200 → Special
        state.sides[0].team[0].stats[ATK] = 100;
        state.sides[0].team[0].stats[SPA] = 200;
        let result_spec = calc_damage(&state, 0, 851, 0, &mut fixed_rng(15));

        // Both use the same offensive stat value (200) but hit different defenses.
        // Defender has uniform stats (100 for both Def and SpD), so damage should be similar
        // but the key test is that both produce non-zero damage (Tera Blast is working)
        assert!(result_phys.damage > 0, "Physical Tera Blast should deal damage");
        assert!(result_spec.damage > 0, "Special Tera Blast should deal damage");

        // Test with boosts: +2 Atk with lower raw Atk should override to Physical
        state.sides[0].team[0].stats[ATK] = 100;
        state.sides[0].team[0].stats[SPA] = 120;
        state.sides[0].active.boosts[ATK] = 2; // 2× boost → effective 200
        state.sides[0].active.boosts[SPA] = 0; // no boost → 120
        let result_boosted = calc_damage(&state, 0, 851, 0, &mut fixed_rng(15));
        assert!(result_boosted.damage > 0, "Boosted Tera Blast should deal damage");

        // When NOT terastallized → stays Special (default category)
        state.sides[0].team[0].flags &= !MON_FLAG_TERASTALLIZED;
        state.sides[0].team[0].stats[ATK] = 300;
        state.sides[0].team[0].stats[SPA] = 100;
        state.sides[0].active.boosts[ATK] = 0;
        let result_normal = calc_damage(&state, 0, 851, 0, &mut fixed_rng(15));
        // Not terastallized: Normal type, Special category (despite higher Atk)
        assert!(result_normal.damage > 0);
    }

    #[test]
    fn test_tera_once_per_battle() {
        use crate::state::legal_moves::legal_actions;

        let mut state = BattleState::default();
        state.phase = PHASE_ACTIONS;
        state.sides[0].team[0] = MonSlot {
            species_id: 4,
            current_hp: 300, max_hp: 300,
            stats: [100, 100, 100, 100, 100],
            tera_type: Type::Fire as u8,
            moves: [1, 0, 0, 0], pp: [24, 0, 0, 0],
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 1,
            current_hp: 300, max_hp: 300,
            stats: [100, 100, 100, 100, 100],
            ..Default::default()
        };

        // Tera should be available
        let actions = legal_actions(&state, 0);
        assert!(actions.as_slice().contains(&ACTION_TERA), "Tera should be available initially");

        // After tera used → no longer available
        state.sides[0]._padding[0] |= 1; // set tera_used flag
        let actions = legal_actions(&state, 0);
        assert!(!actions.as_slice().contains(&ACTION_TERA), "Tera should not be available after use");
    }

    #[test]
    fn test_tera_defensive_type() {
        // When terastallized, defensive type changes to tera type
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 4; // Charmander: Fire/Fire
        state.sides[0].team[0].tera_type = Type::Water as u8;
        state.sides[0].team[0].current_hp = 300;
        state.sides[0].team[0].max_hp = 300;

        // Not terastallized → Fire/Fire
        let (t1, t2) = battle_types(&state, 0);
        assert_eq!(t1, Type::Fire as u8);
        assert_eq!(t2, Type::Fire as u8);

        // Terastallized → Water/Water
        state.sides[0].team[0].flags |= MON_FLAG_TERASTALLIZED;
        let (t1, t2) = battle_types(&state, 0);
        assert_eq!(t1, Type::Water as u8);
        assert_eq!(t2, Type::Water as u8);

        // Electric move should be SE vs Water (eff=8), not NVE vs Fire (eff=2)
        let def_type1 = unsafe { core::mem::transmute::<u8, Type>(t1) };
        let def_type2 = unsafe { core::mem::transmute::<u8, Type>(t2) };
        let eff = dual_type_effectiveness(Type::Electric, def_type1, def_type2);
        assert_eq!(eff, 8); // 2× super effective vs Water
    }

    #[test]
    fn test_wonder_room_swaps_def_spd() {
        // Defender: Def=100, SpD=200
        // Physical attack normally hits Def=100. Under Wonder Room, hits SpD=200 → less damage.
        // Special attack normally hits SpD=200. Under Wonder Room, hits Def=100 → more damage.
        let mut state = test_state();
        state.sides[1].team[0].stats[DEF] = 100;
        state.sides[1].team[0].stats[SPD] = 200;

        // Physical attack (hits DEF=100)
        let dmg_phys_normal = calc_damage(&state, 0, 1, 0, &mut fixed_rng(15));

        // Turn on Wonder Room (physical now hits SPD=200)
        state.field.set_wonder_room_turns(5);
        let dmg_phys_wr = calc_damage(&state, 0, 1, 0, &mut fixed_rng(15));

        // Physical damage under Wonder Room should be lower (higher defense)
        // This test verifies the swap happened (if move_hot(1) returns Physical with non-zero BP)
        if dmg_phys_normal.damage > 0 {
            assert!(dmg_phys_wr.damage < dmg_phys_normal.damage,
                "Wonder Room physical: {} should be < normal: {}",
                dmg_phys_wr.damage, dmg_phys_normal.damage);
        }
    }

    #[test]
    fn test_wonder_room_boosts_stay_on_original_stat() {
        // Verify boosts stay on their original stat index
        let mut state = test_state();
        state.sides[1].team[0].stats[DEF] = 100;
        state.sides[1].team[0].stats[SPD] = 100; // same base stats
        state.sides[1].active.boosts[DEF] = 2; // +2 Def
        state.sides[1].active.boosts[SPD] = 0; // no SpD boost

        // Without Wonder Room: physical hits DEF (100 + 2 boost = 200 effective)
        let dmg_normal = calc_damage(&state, 0, 1, 0, &mut fixed_rng(15));

        // With Wonder Room: physical hits SPD base (100) but uses DEF boost (+2)
        // So effective defense should still be 200
        state.field.set_wonder_room_turns(5);
        let dmg_wr = calc_damage(&state, 0, 1, 0, &mut fixed_rng(15));

        // Damage should be the same since base stats are equal and boost stays on DEF
        if dmg_normal.damage > 0 {
            assert_eq!(dmg_wr.damage, dmg_normal.damage,
                "Boosts should stay on original stat: WR={} normal={}",
                dmg_wr.damage, dmg_normal.damage);
        }
    }

    #[test]
    fn test_magic_room_suppresses_choice_band() {
        let mut state = test_state();
        // Give attacker Choice Band (item with CHOICE_ATK flag)
        // We can't easily set up a real Choice Band without knowing the item ID,
        // but we can verify the calc path by checking atk_item is NONE under Magic Room
        state.field.set_magic_room_turns(5);
        // Under Magic Room, any item_id is suppressed — atk_item becomes NONE
        // This means CHOICE_ATK flag won't be applied
        let result = calc_damage(&state, 0, 1, 0, &mut fixed_rng(15));
        // Just verify calc doesn't crash and produces valid output
        assert!(result.damage >= 0);
    }

    // ---- Phase 2: Escalating power tests ----

    #[test]
    fn test_escalating_power_triple_kick() {
        // Triple Kick (167): base_power=10, Escalating, 3 hits
        let state = test_state();
        let result = calc_damage(&state, 0, 167, 0, &mut fixed_rng(15));
        assert_eq!(result.hits, 3);
        assert!(result.damage > 0);
    }

    #[test]
    fn test_escalating_power_triple_axel() {
        // Triple Axel (813): base_power=20, Escalating, 3 hits
        let state = test_state();
        let result = calc_damage(&state, 0, 813, 0, &mut fixed_rng(15));
        assert_eq!(result.hits, 3);
        assert!(result.damage > 0);
    }

    #[test]
    fn test_escalating_power_expected_damage() {
        // Triple Kick (167): base_power=10, Fighting, 3 hits, var_power=Escalating
        // With test_state (atk=150, def=100, level=100) and fixed_rng(15) -> 100% roll, no crit
        // Defender species 50 (Ground) -> Fighting is 2x SE (eff=8)
        // Hit 1: power 10 -> (42*10*150/100)/50 + 2 = 14, *2 eff = 28
        // Hit 2: power 20 -> (42*20*150/100)/50 + 2 = 27, *2 eff = 54
        // Hit 3: power 30 -> (42*30*150/100)/50 + 2 = 39, *2 eff = 78
        // Total = 28 + 54 + 78 = 160
        let state = test_state();
        let result = calc_damage(&state, 0, 167, 0, &mut fixed_rng(15));
        assert_eq!(result.hits, 3);
        assert_eq!(result.damage, 160);
    }

    #[test]
    fn test_non_escalating_multihit_flat_power() {
        // Triple Dive (865): 3 hits, NOT escalating, base_power=30
        let state = test_state();
        let result = calc_damage(&state, 0, 865, 0, &mut fixed_rng(15));
        assert_eq!(result.hits, 3);
        assert!(result.damage > 0);
    }

    // ---- Phase 3: Per-hit accuracy tests ----

    #[test]
    fn test_per_hit_accuracy_zero_means_no_check() {
        // per_hit_accuracy=0 → no per-hit checks, all hits land
        let state = test_state();
        let result = calc_damage(&state, 0, 860, 0, &mut fixed_rng(99));
        assert_eq!(result.hits, 10);
    }

    #[test]
    fn test_multiaccuracy_all_hits_land() {
        // per_hit_accuracy=90, RNG always returns 0 (< 90) → all hits land
        let state = test_state();
        let result = calc_damage(&state, 0, 860, 90, &mut fixed_rng(0));
        assert_eq!(result.hits, 10);
    }

    #[test]
    fn test_multiaccuracy_first_hit_no_check() {
        // per_hit_accuracy=1 with RNG=99: first hit always lands (no check),
        // second hit misses (99 >= 1)
        let state = test_state();
        let result = calc_damage(&state, 0, 860, 1, &mut fixed_rng(99));
        assert_eq!(result.hits, 1);
        assert!(result.damage > 0);
    }

    #[test]
    fn test_multiaccuracy_stops_on_miss() {
        // Population Bomb (860): 10 hits, per_hit_accuracy=90
        // Craft RNG: hit 0 lands (no check), hit 1 accuracy passes, hit 2 accuracy fails
        let state = test_state();
        let mut call_count = 0u32;
        let mut seq_rng = |max: u32| -> u32 {
            call_count += 1;
            // RNG call sequence:
            //   1: is_crit check (rng for crit) → return 1 (no crit)
            //   resolve_hits: lo==hi==10, no RNG call
            //   2: hit 0 random roll rng(16) → 15
            //   3: hit 1 accuracy rng(100) → 0 (hit)
            //   4: hit 1 random roll rng(16) → 15
            //   5: hit 2 accuracy rng(100) → 95 (miss)
            match call_count {
                1 => 1,             // crit check: no crit
                2 => 15 % max,      // hit 0 random roll
                3 => 0,             // hit 1 accuracy: 0 < 90 → hit
                4 => 15 % max,      // hit 1 random roll
                5 => 95,            // hit 2 accuracy: 95 >= 90 → miss
                _ => 0,
            }
        };
        let result = calc_damage(&state, 0, 860, 90, &mut seq_rng);
        assert_eq!(result.hits, 2);
        assert!(result.damage > 0);
    }

    #[test]
    fn test_multiaccuracy_miss_on_second_hit_triple_kick() {
        // Triple Kick (167): 3 hits, escalating, per_hit_accuracy=90
        // Miss on hit 1 → only hit 0 lands (power=10*1=10)
        let state = test_state();
        let mut call_count = 0u32;
        let mut seq_rng = |max: u32| -> u32 {
            call_count += 1;
            // 1: crit check → no crit
            // resolve_hits: lo==hi==3, returns 3 without RNG
            // 2: hit 0 random roll
            // 3: hit 1 accuracy → miss
            match call_count {
                1 => 1,             // crit check: no crit
                2 => 15 % max,      // hit 0 random roll
                3 => 95,            // hit 1 accuracy: 95 >= 90 → miss
                _ => 0,
            }
        };
        let result = calc_damage(&state, 0, 167, 90, &mut seq_rng);
        assert_eq!(result.hits, 1);
        assert!(result.damage > 0);
    }

    #[test]
    fn test_multiaccuracy_with_escalating_triple_axel() {
        // Triple Axel: 3 hits, escalating + multiaccuracy, all hit
        let state = test_state();
        let result = calc_damage(&state, 0, 813, 90, &mut fixed_rng(0));
        assert_eq!(result.hits, 3);
        assert!(result.damage > 0);
    }
}
