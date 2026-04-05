//! Team builder: compute stats and populate MonSlot + MonBuildData.

use crate::state::structs::*;
use crate::state::data_bridge;
use crate::data::base_stats::SpeciesData;

/// Convert from Showdown type index (scenario JSON format) to engine Type enum value.
/// Showdown: Normal=0, Fighting=1, Flying=2, Poison=3, Ground=4, Rock=5,
///           Bug=6, Ghost=7, Steel=8, Fire=9, Water=10, Grass=11,
///           Electric=12, Psychic=13, Ice=14, Dragon=15, Dark=16, Fairy=17
/// Engine:   Normal=0, Fire=1, Water=2, Electric=3, Grass=4, Ice=5,
///           Fighting=6, Poison=7, Ground=8, Flying=9, Psychic=10, Bug=11,
///           Rock=12, Ghost=13, Dragon=14, Dark=15, Steel=16, Fairy=17
#[inline]
pub fn showdown_type_to_engine(sd_type: u8) -> u8 {
    const MAP: [u8; 18] = [
        0,  // SD 0  Normal   -> Engine 0  Normal
        6,  // SD 1  Fighting -> Engine 6  Fighting
        9,  // SD 2  Flying   -> Engine 9  Flying
        7,  // SD 3  Poison   -> Engine 7  Poison
        8,  // SD 4  Ground   -> Engine 8  Ground
        12, // SD 5  Rock     -> Engine 12 Rock
        11, // SD 6  Bug      -> Engine 11 Bug
        13, // SD 7  Ghost    -> Engine 13 Ghost
        16, // SD 8  Steel    -> Engine 16 Steel
        1,  // SD 9  Fire     -> Engine 1  Fire
        2,  // SD 10 Water    -> Engine 2  Water
        4,  // SD 11 Grass    -> Engine 4  Grass
        3,  // SD 12 Electric -> Engine 3  Electric
        10, // SD 13 Psychic  -> Engine 10 Psychic
        5,  // SD 14 Ice      -> Engine 5  Ice
        14, // SD 15 Dragon   -> Engine 14 Dragon
        15, // SD 16 Dark     -> Engine 15 Dark
        17, // SD 17 Fairy    -> Engine 17 Fairy
    ];
    if (sd_type as usize) < MAP.len() { MAP[sd_type as usize] } else { 0 }
}

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
    // Minior with Shields Down starts in Meteor forme (species 1291).
    // The Showdown data uses species 774 as "Minior" (Core), so remap
    // at build time when the ability is Shields Down.
    const MINIOR_CORE: u16 = 774;
    const MINIOR_METEOR: u16 = 1291;
    let species_id = if input.species_id == MINIOR_CORE
        && input.ability_id == data_bridge::ABILITY_SHIELDS_DOWN
    {
        MINIOR_METEOR
    } else {
        input.species_id
    };

    let sp = data_bridge::species(species_id);
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

    let mut flags: u16 = if input.is_female { MON_FLAG_FEMALE } else { 0 };
    if data_bridge::is_genderless_species(input.species_id) {
        flags |= MON_FLAG_GENDERLESS;
    }
    let mon = MonSlot {
        species_id,
        ability_id: input.ability_id,
        item_id: input.item_id,
        current_hp: hp, max_hp: hp,
        stats, moves: input.moves, pp,
        status: STATUS_NONE, status_counter: 0,
        tera_type: showdown_type_to_engine(input.tera_type),
        level: input.level,
        flags,
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
