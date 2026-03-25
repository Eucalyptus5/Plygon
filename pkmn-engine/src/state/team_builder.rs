//! Team builder: compute stats and populate MonSlot + MonBuildData.

use crate::state::structs::*;
use crate::state::data_bridge;
use crate::data::base_stats::SpeciesData;

#[derive(Debug, Clone)]
pub struct MonBuildInput {
    pub species_id: u16,
    pub ability_id: u16,
    pub item_id: u16,
    pub moves: [u16; 4],
    pub ivs: [u8; 6],
    pub evs: [u8; 6],
    pub nature: u8,
    pub level: u8,
    pub tera_type: u8,
    pub is_female: bool,
}

fn calc_hp(base: u8, iv: u8, ev: u8, level: u8) -> u16 {
    let (base, iv, ev, level) = (base as u32, iv as u32, ev as u32, level as u32);
    ((2 * base + iv + ev / 4) * level / 100 + level + 10) as u16
}

fn calc_stat(base: u8, iv: u8, ev: u8, level: u8, nature: u8, stat_index: usize) -> u16 {
    let (base, iv, ev, level) = (base as u32, iv as u32, ev as u32, level as u32);
    let raw = (2 * base + iv + ev / 4) * level / 100 + 5;
    let (num, den) = data_bridge::nature_modifier(nature, stat_index);
    (raw * num as u32 / den as u32) as u16
}

/// Extract base stats from SpeciesData as [HP, Atk, Def, SpA, SpD, Spe].
#[inline]
fn base_stats(sp: &SpeciesData) -> [u8; 6] {
    [sp.hp, sp.atk, sp.def, sp.spa, sp.spd, sp.spe]
}

pub fn build_mon(input: &MonBuildInput) -> (MonSlot, MonBuildData) {
    let sp = data_bridge::species(input.species_id);
    let bases = base_stats(sp);

    let hp = calc_hp(bases[0], input.ivs[0], input.evs[0], input.level);
    let mut stats = [0u16; 5];
    for i in 0..5 {
        stats[i] = calc_stat(bases[i+1], input.ivs[i+1], input.evs[i+1],
                             input.level, input.nature, i+1);
    }

    let mut pp = [0u8; 4];
    for i in 0..4 {
        if input.moves[i] != 0 {
            let base_pp = data_bridge::move_base_pp(input.moves[i]);
            pp[i] = (base_pp as u16 * 8 / 5) as u8; // max PP ups
        }
    }

    let mon = MonSlot {
        species_id: input.species_id,
        ability_id: input.ability_id,
        item_id: input.item_id,
        current_hp: hp, max_hp: hp,
        stats, moves: input.moves, pp,
        status: STATUS_NONE, status_counter: 0,
        tera_type: input.tera_type,
        flags: if input.is_female { MON_FLAG_FEMALE } else { 0 },
        level: input.level,
        _pad: 0,
    };
    let build_data = MonBuildData { ivs: input.ivs, evs: input.evs, nature: input.nature };
    (mon, build_data)
}

pub fn build_team(inputs: &[MonBuildInput; 6]) -> ([MonSlot; 6], [MonBuildData; 6], [u8; 6]) {
    let mut team = [MonSlot::default(); 6];
    let mut build = [MonBuildData::default(); 6];
    let mut levels = [0u8; 6];
    for i in 0..6 {
        let (mon, bd) = build_mon(&inputs[i]);
        team[i] = mon; build[i] = bd; levels[i] = inputs[i].level;
    }
    (team, build, levels)
}

pub fn recompute_stats(mon: &mut MonSlot, new_species: &SpeciesData, build: &MonBuildData, level: u8) {
    let bases = base_stats(new_species);
    mon.max_hp = calc_hp(bases[0], build.ivs[0], build.evs[0], level);
    for i in 0..5 {
        mon.stats[i] = calc_stat(bases[i+1], build.ivs[i+1], build.evs[i+1],
                                  level, build.nature, i+1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_hp_calc() {
        assert_eq!(calc_hp(108, 31, 252, 100), 420); // Garchomp
    }
    #[test]
    fn test_stat_calc_adamant_garchomp() {
        assert_eq!(calc_stat(130, 31, 252, 100, 3, 1), 394);
    }
}
