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
/// Scales current stats by new_base / old_base ratio per stat.
/// Also sets override_types from the new species.
/// To revert, call `revert_battle_forme`.
pub fn apply_battle_forme(
    state: &mut BattleState, keys: &ZobristKeys,
    side: usize, new_species_id: u16,
) {
    let slot = state.sides[side].active_index as usize;
    let old_species_id = state.sides[side].team[slot].species_id;
    let old_sp = data_bridge::species(old_species_id);
    let new_sp = data_bridge::species(new_species_id);

    let old_bases = [old_sp.atk, old_sp.def, old_sp.spa, old_sp.spd, old_sp.spe];
    let new_bases = [new_sp.atk, new_sp.def, new_sp.spa, new_sp.spd, new_sp.spe];

    // Read current stats first (immutable borrow), then write overrides
    let mut scaled_stats = [0u16; 5];
    for i in 0..5 {
        let current = accessors::effective_stat(state, side, i);
        scaled_stats[i] = (current as u32 * new_bases[i] as u32 / old_bases[i].max(1) as u32).max(1) as u16;
    }
    state.sides[side].active.override_stats = scaled_stats;

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

    let species = accessors::effective_species(state, side);
    let half_hp = mon.max_hp / 2;

    if mon.current_hp <= half_hp && species == DARMANITAN {
        apply_battle_forme(state, keys, side, DARMANITAN_ZEN);
    } else if mon.current_hp > half_hp && species == DARMANITAN_ZEN {
        revert_battle_forme(state, keys, side);
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
    const WISHIWASHI_SCHOOL: u16 = 1192;

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

    const MINIOR_METEOR: u16 = 774;
    const MINIOR_CORE: u16 = 1200;

    let species = accessors::effective_species(state, side);
    let half_hp = mon.max_hp / 2;

    if mon.current_hp <= half_hp && species == MINIOR_METEOR {
        apply_battle_forme(state, keys, side, MINIOR_CORE);
    } else if mon.current_hp > half_hp && species == MINIOR_CORE {
        revert_battle_forme(state, keys, side);
    }
}

/// Palafin Zero to Hero: switch to Hero forme on switch-in if flag is set.
/// Called from switch_in after hazards.
pub fn check_palafin_hero(state: &mut BattleState, keys: &ZobristKeys, side: usize) {
    let slot = state.sides[side].active_index as usize;
    let mon = &state.sides[side].team[slot];
    if mon.is_fainted() { return; }
    if mon.flags & crate::state::structs::MON_FLAG_HERO_ACTIVATED == 0 { return; }

    const PALAFIN_ZERO: u16 = 964;
    const PALAFIN_HERO: u16 = 1321;

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
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 681, current_hp: 300, max_hp: 300,
            ability_id: data_bridge::ABILITY_STANCE_CHANGE,
            stats: [100, 280, 100, 280, 120], // base shield-like stats
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        apply_battle_forme(&mut state, &keys, 0, 1103);

        // Stats should scale: atk 100*140/50=280, def 280*50/140=100
        assert_eq!(effective_stat(&state, 0, ATK), 280);
        assert_eq!(effective_stat(&state, 0, DEF), 100);
        assert_eq!(effective_stat(&state, 0, SPA), 280);
        assert_eq!(effective_stat(&state, 0, SPD), 100);
        assert_eq!(effective_stat(&state, 0, SPE), 120); // 60/60 = same
        assert_eq!(effective_species(&state, 0), 1103);
        assert!(validate_hash(&state, &keys));

        // Revert
        revert_battle_forme(&mut state, &keys, 0);
        assert_eq!(effective_stat(&state, 0, ATK), 100); // back to team stats
        assert_eq!(effective_stat(&state, 0, DEF), 280);
        assert_eq!(effective_species(&state, 0), 681);
        assert!(validate_hash(&state, &keys));
    }

    #[test]
    fn test_zen_mode_triggers_at_half_hp() {
        let keys = ZobristKeys::new(42);
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot {
            species_id: 555, current_hp: 200, max_hp: 400,
            ability_id: data_bridge::ABILITY_ZEN_MODE,
            stats: [280, 110, 60, 110, 190], // Darmanitan base stats
            ..Default::default()
        };
        state.sides[1].team[0] = MonSlot {
            species_id: 25, current_hp: 200, max_hp: 200,
            stats: [100; 5], ..Default::default()
        };
        state.zobrist = compute_full_hash(&state, &keys);

        // HP is 200/400 = 50% → should trigger (≤50%)
        check_zen_mode(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 1171); // Darmanitan-Zen
        // Zen Mode: atk:30, def:105, spa:140, spd:105, spe:55
        // Scaling: atk 280*30/140=60, def 110*105/55=210
        assert_eq!(effective_stat(&state, 0, ATK), 60);
        assert_eq!(effective_stat(&state, 0, DEF), 210);
        assert!(validate_hash(&state, &keys));

        // Heal above 50% → should revert
        heal(&mut state, &keys, 0, 0, 201);
        check_zen_mode(&mut state, &keys, 0);
        assert_eq!(effective_species(&state, 0), 555);
        assert_eq!(effective_stat(&state, 0, ATK), 280);
        assert!(validate_hash(&state, &keys));
    }
}
