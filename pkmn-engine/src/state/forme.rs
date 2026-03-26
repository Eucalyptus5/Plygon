//! Forme change: stat recomputation using TeamData, and Transform (Ditto).

use crate::state::structs::*;
use crate::state::data_bridge;
use crate::state::team_builder::recompute_stats;
use crate::state::accessors;
use crate::state::mutations::*;
use crate::state::zobrist::ZobristKeys;

pub fn change_forme(
    state: &mut BattleState, keys: &ZobristKeys, teams: &TeamData,
    side: usize, new_species_id: u16, new_ability_id: Option<u16>,
) {
    let slot = state.sides[side].active_index as usize;
    let mon = &mut state.sides[side].team[slot];

    state.zobrist ^= keys.species[side][slot][mon.species_id as usize];
    mon.species_id = new_species_id;

    let new_species = data_bridge::species(new_species_id);
    let build = &teams.mons[side][slot];
    let level = teams.levels[side][slot];
    recompute_stats(mon, &new_species, build, level);

    state.zobrist ^= keys.species[side][slot][new_species_id as usize];

    if let Some(ability) = new_ability_id {
        state.sides[side].team[slot].ability_id = ability;
    }
}

pub fn mega_evolve(
    state: &mut BattleState, keys: &ZobristKeys, teams: &TeamData,
    side: usize, mega_species_id: u16, mega_ability_id: u16,
) {
    change_forme(state, keys, teams, side, mega_species_id, Some(mega_ability_id));
}

pub fn apply_transform(
    state: &mut BattleState, keys: &ZobristKeys,
    side: usize, target_side: usize,
) {
    let target_slot = state.sides[target_side].active_index as usize;

    // Copy target data into locals before mutating
    let t_species_id = state.sides[target_side].team[target_slot].species_id;
    let t_ability_id = state.sides[target_side].team[target_slot].ability_id;
    let t_stats = state.sides[target_side].team[target_slot].stats;
    let t_moves = state.sides[target_side].team[target_slot].moves;
    let t_boosts = state.sides[target_side].active.boosts;
    let target_species = data_bridge::species(t_species_id);

    set_volatile(state, keys, side, VOL_TRANSFORMED);

    let active = &mut state.sides[side].active;
    active.override_species = t_species_id;
    active.override_ability = t_ability_id;
    active.override_stats = t_stats;
    active.override_moves = t_moves;
    active.override_pp = [5, 5, 5, 5];
    active.override_types = [target_species.type1 as u8, target_species.type2 as u8];

    for stat in 0..7 {
        let old = state.sides[side].active.boosts[stat];
        if old != 0 { state.zobrist ^= keys.boosts[side][stat][(old + 6) as usize]; }
    }
    state.sides[side].active.boosts = t_boosts;
    for stat in 0..7 {
        if t_boosts[stat] != 0 { state.zobrist ^= keys.boosts[side][stat][(t_boosts[stat] + 6) as usize]; }
    }
}

/// In-battle forme change that uses override_stats instead of TeamData.
/// Recomputes stats using the standard formula
/// `stat = (2*base + iv + ev/4) * level / 100 + 5` (neutral nature assumed).
/// The original build's iv+ev/4 contribution (T) is derived from the mon's
/// current persistent stats and the old form's base stats, then re-applied
/// to the new form's base stats. This preserves IVs/EVs exactly (for neutral
/// natures) instead of the old "scale by ratio" approximation which was
/// inaccurate due to the additive `+5` term.
/// Also sets override_types from the new species.
/// To revert, call `revert_battle_forme`.
pub fn apply_battle_forme(
    state: &mut BattleState, keys: &ZobristKeys,
    side: usize, new_species_id: u16,
) {
    let slot = state.sides[side].active_index as usize;
    let old_species_id = state.sides[side].team[slot].species_id;
    let level = state.sides[side].team[slot].level.max(1) as u32;
    let old_sp = data_bridge::species(old_species_id);
    let new_sp = data_bridge::species(new_species_id);

    let old_bases = [old_sp.atk as u32, old_sp.def as u32, old_sp.spa as u32, old_sp.spd as u32, old_sp.spe as u32];
    let new_bases = [new_sp.atk as u32, new_sp.def as u32, new_sp.spa as u32, new_sp.spd as u32, new_sp.spe as u32];
    // Read the persistent (original-build) stats, not effective_stat — the
    // latter can return a previous form's override_stats which would compound
    // errors across successive form changes.
    let base_stats = state.sides[side].team[slot].stats;

    let mut new_stats = [0u16; 5];
    for i in 0..5 {
        let s = base_stats[i] as u32;
        // Derive training (iv + ev/4) from old stat, assuming neutral nature:
        // stat = (2*base + T) * level / 100 + 5  =>  T = (stat - 5) * 100 / level - 2*base
        let s_minus_5 = s.saturating_sub(5);
        let two_base_plus_t = s_minus_5.saturating_mul(100) / level;
        let training = two_base_plus_t.saturating_sub(2 * old_bases[i]);
        // Recompute using new base: stat = (2*new_base + T) * level / 100 + 5
        let new_stat = ((2 * new_bases[i] + training) * level / 100 + 5).max(1) as u16;
        new_stats[i] = new_stat;
    }
    state.sides[side].active.override_stats = new_stats;

    set_volatile(state, keys, side, VOL_TYPES_OVERRIDDEN);
    state.sides[side].active.override_types = [new_sp.type1 as u8, new_sp.type2 as u8];
    state.sides[side].active.override_species = new_species_id;
}

/// Revert an in-battle forme change: clear override_stats, types, species.
pub fn revert_battle_forme(
    state: &mut BattleState, keys: &ZobristKeys, side: usize,
) {
    state.sides[side].active.override_stats = [0; 5];
    state.sides[side].active.override_species = 0;
    if state.sides[side].active.has_volatile(VOL_TYPES_OVERRIDDEN) {
        clear_volatile(state, keys, side, VOL_TYPES_OVERRIDDEN);
    }
}

/// Zen Mode check: Darmanitan transforms at ≤50% HP, reverts at >50%.
/// Called from end-of-turn and after taking damage.
pub fn check_zen_mode(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let ability = accessors::effective_ability(state, side);
    if ability != data_bridge::ABILITY_ZEN_MODE { return; }

    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];
    if mon.is_fainted() { return; }

    const DARMANITAN: u16 = 555;
    const DARMANITAN_ZEN: u16 = 1171;
    const DARMANITAN_GALAR: u16 = 1169;
    const DARMANITAN_GALAR_ZEN: u16 = 1170;

    let species = accessors::effective_species(state, side);
    let half_hp = mon.max_hp / 2;

    if mon.current_hp <= half_hp {
        if species == DARMANITAN {
            apply_battle_forme(state, keys, side, DARMANITAN_ZEN);
        } else if species == DARMANITAN_GALAR {
            apply_battle_forme(state, keys, side, DARMANITAN_GALAR_ZEN);
        }
    } else if mon.current_hp > half_hp {
        if species == DARMANITAN_ZEN {
            revert_battle_forme(state, keys, side);
        } else if species == DARMANITAN_GALAR_ZEN {
            revert_battle_forme(state, keys, side);
        }
    }
}

/// Wishiwashi Schooling: School forme at >25% HP, Solo at ≤25%.
/// Called from end-of-turn and after damage.
pub fn check_schooling(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let ability = accessors::effective_ability(state, side);
    if ability != data_bridge::ABILITY_SCHOOLING { return; }

    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];
    if mon.is_fainted() { return; }

    const WISHIWASHI_SOLO: u16 = 746;
    const WISHIWASHI_SCHOOL: u16 = 1437;

    let species = accessors::effective_species(state, side);
    let quarter_hp = mon.max_hp / 4;

    if mon.current_hp > quarter_hp && species == WISHIWASHI_SOLO {
        apply_battle_forme(state, keys, side, WISHIWASHI_SCHOOL);
    } else if mon.current_hp <= quarter_hp && species == WISHIWASHI_SCHOOL {
        revert_battle_forme(state, keys, side);
    }
}

/// Minior Shields Down: Core forme at ≤50% HP.
/// Called from end-of-turn and after damage.
pub fn check_shields_down(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let ability = accessors::effective_ability(state, side);
    if ability != data_bridge::ABILITY_SHIELDS_DOWN { return; }

    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];
    if mon.is_fainted() { return; }

    const MINIOR_METEOR: u16 = 1291;
    const MINIOR_CORE: u16 = 774;

    let species = accessors::effective_species(state, side);
    let half_hp = mon.max_hp / 2;

    if mon.current_hp <= half_hp && species == MINIOR_METEOR {
        apply_battle_forme(state, keys, side, MINIOR_CORE);
    } else if mon.current_hp > half_hp && species == MINIOR_CORE {
        revert_battle_forme(state, keys, side);
    }
}

/// Returns true if the active mon is Minior in Meteor forme (status-immune).
#[inline(always)]
pub fn is_minior_meteor_forme(state: &BattleState, side: usize) -> bool {
    accessors::effective_ability(state, side) == data_bridge::ABILITY_SHIELDS_DOWN
        && accessors::effective_species(state, side) == 1291 // MINIOR_METEOR
}

/// Palafin Zero to Hero: switch to Hero forme on switch-in if flag is set.
/// Called from switch_in after hazards.
pub fn check_palafin_hero(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];
    if mon.is_fainted() { return; }
    if mon.flags & crate::state::structs::MON_FLAG_HERO_ACTIVATED == 0 { return; }

    const PALAFIN_ZERO: u16 = 964;
    const PALAFIN_HERO: u16 = 1311;

    if mon.species_id == PALAFIN_ZERO {
        apply_battle_forme(state, keys, side, PALAFIN_HERO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::zobrist::{compute_full_hash, validate_hash};
    use crate::state::accessors::*;

    #[test]
    fn test_transform() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot { species_id: 0, current_hp: 200, max_hp: 200,
            stats: [48,48,48,48,48], item_id: 220, ..Default::default() };
        state.sides[1].team[0] = MonSlot { species_id: 0, current_hp: 357, max_hp: 357,
            stats: [394,187,162,175,333], moves: [89,200,337,14], ..Default::default() };
        state.zobrist = compute_full_hash(&state, &keys);

        apply_transform(&mut state, &keys, 0, 1);

        assert!(state.sides[0].active.has_volatile(VOL_TRANSFORMED));
        assert_eq!(effective_stat(&state, 0, ATK), 394);
        assert_eq!(effective_pp(&state, 0, 0), 5);
        assert_eq!(state.sides[0].team[0].current_hp, 200); // HP unchanged
        assert_eq!(state.sides[0].team[0].item_id, 220);    // Item unchanged
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_battle_forme_aegislash() {
        // Aegislash Shield (681): atk:50, def:140, spa:50, spd:140, spe:60
        // Aegislash Blade (1103): atk:140, def:50, spa:140, spd:50, spe:60
        // Stats at L100 neutral 31/0: atk=136, def=316, spa=136, spd=316, spe=156
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 681, current_hp: 300, max_hp: 300,
            ability_id: data_bridge::ABILITY_STANCE_CHANGE,
            stats: [136, 316, 136, 316, 156], level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], level: 100, ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        apply_battle_forme(&mut state, &keys, 0, 1103);

        // Stats should be recomputed using new base + original T (31 for IV/EV 31/0):
        // atk = 2*140 + 31 + 5 = 316, def = 2*50 + 31 + 5 = 136, etc.
        assert_eq!(effective_stat(&state, 0, ATK), 316);
        assert_eq!(effective_stat(&state, 0, DEF), 136);
        assert_eq!(effective_stat(&state, 0, SPA), 316);
        assert_eq!(effective_stat(&state, 0, SPD), 136);
        assert_eq!(effective_stat(&state, 0, SPE), 156); // 60/60 = same
        assert_eq!(effective_species(&state, 0), 1103);
        assert!(validate_hash(&state, &keys));

        // Revert
        revert_battle_forme(&mut state, &keys, 0);
        assert_eq!(effective_stat(&state, 0, ATK), 136); // back to team stats
        assert_eq!(effective_stat(&state, 0, DEF), 316);
        assert_eq!(effective_species(&state, 0), 681);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_zen_mode_triggers_at_half_hp() {
        // Darmanitan (555): atk:140, def:55, spa:30, spd:55, spe:95
        // Darmanitan-Zen (1171): atk:30, def:105, spa:140, spd:105, spe:55
        // At L100 neutral 31/0:
        //   Base atk=(2*140+31)+5=316, spa=(2*30+31)+5=96
        //   Zen atk=(2*30+31)+5=96, spa=(2*140+31)+5=316
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 555, current_hp: 200, max_hp: 400,
            ability_id: data_bridge::ABILITY_ZEN_MODE,
            stats: [316, 146, 96, 146, 226], level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], level: 100, ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // HP is 200/400 = 50% → should trigger (≤50%)
        check_zen_mode(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 1171); // Darmanitan-Zen
        assert_eq!(effective_stat(&state, 0, ATK), 96);
        assert_eq!(effective_stat(&state, 0, SPA), 316);
        assert!(validate_hash(&state, &keys));

        // Heal above 50% → should revert
        heal(&mut state, &keys, 0, 0, 201);
        check_zen_mode(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 555);
        assert_eq!(effective_stat(&state, 0, ATK), 316);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_zen_mode_above_50_reverts() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 555, current_hp: 100, max_hp: 400,
            ability_id: data_bridge::ABILITY_ZEN_MODE,
            stats: [316, 146, 96, 146, 226], level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], level: 100, ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // Trigger Zen Mode at 25% HP
        check_zen_mode(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 1171);

        // Heal above 50% → should revert
        heal(&mut state, &keys, 0, 0, 301);
        assert!(state.sides[0].team[0].current_hp > 200); // > 50%
        check_zen_mode(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 555);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_zen_mode_galar() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        // Darmanitan-Galar (1169): atk:140, def:55, spa:30, spd:55, spe:95
        state.sides[0].team[0] = MonSlot {
            species_id: 1169, current_hp: 100, max_hp: 400,
            ability_id: data_bridge::ABILITY_ZEN_MODE,
            stats: [316, 146, 96, 146, 226], level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], level: 100, ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        check_zen_mode(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 1170); // Darmanitan-Galar-Zen
        assert!(validate_hash(&state, &keys));

        // Heal above 50% → should revert
        heal(&mut state, &keys, 0, 0, 301);
        check_zen_mode(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 1169);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_schooling_below_25() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        // Wishiwashi Solo (746): atk:20, def:20, spa:25, spd:25, spe:40
        // L100 neutral 31/0: atk=76, def=76, spa=86, spd=86, spe=116
        state.sides[0].team[0] = MonSlot {
            species_id: 746, current_hp: 100, max_hp: 400,
            ability_id: data_bridge::ABILITY_SCHOOLING,
            stats: [76, 76, 86, 86, 116], level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], level: 100, ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // HP 100/400 = 25% → ≤25%, stays Solo
        check_schooling(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 746);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_schooling_above_25_becomes_school() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        // Wishiwashi Solo (746): atk:20, def:20, spa:25, spd:25, spe:40
        // L100 neutral 31/0: atk=76, def=76, spa=86, spd=86, spe=116
        state.sides[0].team[0] = MonSlot {
            species_id: 746, current_hp: 101, max_hp: 400,
            ability_id: data_bridge::ABILITY_SCHOOLING,
            stats: [76, 76, 86, 86, 116], level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], level: 100, ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // HP 101/400 > 25% → School Forme
        check_schooling(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 1437); // Wishiwashi-School
        // School (1437): atk:140, def:130, spa:140, spd:135, spe:30
        // L100 neutral 31/0: atk=316, spe=96
        assert_eq!(effective_stat(&state, 0, ATK), 316);
        assert_eq!(effective_stat(&state, 0, SPE), 96);
        assert!(validate_hash(&state, &keys));

        // Damage below 25% → reverts to Solo
        use crate::state::mutations::deal_damage;
        deal_damage(&mut state, &keys, 0, 0, 2);
        assert!(state.sides[0].team[0].current_hp <= 100); // ≤25%
        check_schooling(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 746);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_shields_down_below_50() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        // Minior-Meteor (1291): atk:60, def:100, spa:60, spd:100, spe:60
        // L100 neutral 31/0: atk=156, def=236, spa=156, spd=236, spe=156
        state.sides[0].team[0] = MonSlot {
            species_id: 1291, current_hp: 200, max_hp: 400,
            ability_id: data_bridge::ABILITY_SHIELDS_DOWN,
            stats: [156, 236, 156, 236, 156], level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], level: 100, ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // HP 200/400 = 50% → ≤50%, change to Core
        check_shields_down(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 774); // Minior (Core)
        // Core (774): atk:100, def:60, spa:100, spd:60, spe:120
        // L100 neutral 31/0: atk=236, def=156, spa=236, spd=156, spe=276
        assert_eq!(effective_stat(&state, 0, ATK), 236);
        assert_eq!(effective_stat(&state, 0, DEF), 156);
        assert_eq!(effective_stat(&state, 0, SPE), 276);
        assert!(validate_hash(&state, &keys));

        // Heal above 50% → revert to Meteor
        heal(&mut state, &keys, 0, 0, 201);
        check_shields_down(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 1291);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_shields_down_meteor_blocks_status() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 1291, current_hp: 300, max_hp: 400,
            ability_id: data_bridge::ABILITY_SHIELDS_DOWN,
            stats: [120; 5], level: 100,
            ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // Minior-Meteor at >50% HP should be immune to status
        assert!(is_minior_meteor_forme(&state, 0));
    }

    #[test]
    fn test_shields_down_core_allows_status() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 1291, current_hp: 200, max_hp: 400,
            ability_id: data_bridge::ABILITY_SHIELDS_DOWN,
            stats: [156, 236, 156, 236, 156], level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], level: 100, ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // Transition to Core forme
        check_shields_down(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 774);

        // Core forme is NOT status-immune
        assert!(!is_minior_meteor_forme(&state, 0));
    }

    #[test]
    fn test_palafin_hero_after_switch() {
        use crate::state::switch::{switch_out, switch_in};

        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        // Palafin Zero (964): atk:70, def:72, spa:53, spd:62, spe:100
        state.sides[0].team[0] = MonSlot {
            species_id: 964, current_hp: 300, max_hp: 300,
            ability_id: data_bridge::ABILITY_ZERO_TO_HERO,
            stats: [140, 144, 106, 124, 200], level: 100,
            ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            ability_id: 0,
            stats: [100; 5], level: 100,
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], level: 100, ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // Initially Zero forme, no hero flag
        assert_eq!(state.sides[0].team[0].flags & MON_FLAG_HERO_ACTIVATED, 0);

        // Switch out → sets HERO_ACTIVATED flag
        switch_out(&mut state, &keys, 0);
        assert!(state.sides[0].team[0].flags & MON_FLAG_HERO_ACTIVATED != 0);

        // Switch to slot 1
        switch_in(&mut state, &keys, 0, 1);

        // Switch back to Palafin (slot 0)
        switch_out(&mut state, &keys, 0);
        switch_in(&mut state, &keys, 0, 0);

        // Should now be Hero forme
        // Palafin-Hero (1311): atk:160, def:97, spa:106, spd:87, spe:100
        assert_eq!(effective_species(&state, 0), 1311);
        assert!(validate_hash(&state, &keys));
    }
}
