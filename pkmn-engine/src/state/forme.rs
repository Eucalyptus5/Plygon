//! Forme change: stat recomputation using TeamData, and Transform (Ditto).

use crate::state::structs::*;
use crate::state::data_bridge;
use crate::state::team_builder::recompute_stats;
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

    // Copy boosts — update Zobrist
    for stat in 0..7 {
        let old = state.sides[side].active.boosts[stat];
        if old != 0 { state.zobrist ^= keys.boosts[side][stat][(old + 6) as usize]; }
    }
    state.sides[side].active.boosts = t_boosts;
    for stat in 0..7 {
        if t_boosts[stat] != 0 { state.zobrist ^= keys.boosts[side][stat][(t_boosts[stat] + 6) as usize]; }
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
}
