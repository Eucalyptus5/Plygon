use pkmn_engine::state::team_builder::recompute_stats;
use pkmn_engine::state::structs::{MonSlot, MonBuildData};
use pkmn_engine::data::base_stats::SpeciesData;
use pkmn_engine::data::types::Type;
use pkmn_engine::state::data_bridge::nature_modifier;

fn dummy_species(hp: u8, atk: u8, def: u8, spa: u8, spd: u8, spe: u8) -> SpeciesData {
    SpeciesData {
        hp, atk, def, spa, spd, spe,
        type1: Type::Normal,
        type2: Type::Normal,
        weight: 100,
        nfe: false,
    }
}

fn dummy_build(ivs: [u8; 6], evs: [u8; 6], nature: u8) -> MonBuildData {
    MonBuildData {
        ivs, evs, nature,
    }
}

#[test]
fn test_hp_formula() {
    let mut mon = MonSlot::default();
    let sp = dummy_species(108, 0, 0, 0, 0, 0); // Garchomp base HP
    let build = dummy_build([31, 0, 0, 0, 0, 0], [252, 0, 0, 0, 0, 0], 0);
    
    recompute_stats(&mut mon, &sp, &build, 100);
    assert_eq!(mon.max_hp, 420);
}

#[test]
fn test_hp_formula_shedinja_base1() {
    let mut mon = MonSlot::default();
    let sp = dummy_species(1, 0, 0, 0, 0, 0); 
    let build = dummy_build([31, 0, 0, 0, 0, 0], [252, 0, 0, 0, 0, 0], 100);
    
    recompute_stats(&mut mon, &sp, &build, 100);
    // formula: ((2 * 1 + 31 + 63) * 100) / 100 + 100 + 10 = 96 + 110 = 206
    assert_eq!(mon.max_hp, 206);
}

#[test]
fn test_stat_formula_adamant() {
    // We already test it in test_adamant_nature_stats
}

#[test]
fn test_all_neutral_natures() {
    // 0, 6, 12, 18, 24 are neutral
    for &nature in &[0, 6, 12, 18, 24] {
        for stat in 1..=5 {
            assert_eq!(nature_modifier(nature, stat), (10, 10), "Nature {} stat {} should be neutral", nature, stat);
        }
    }
}

#[test]
fn test_boosting_and_reducing_natures() {
    // By definition in pokemon, nature = inc * 5 + dec (where inc, dec in 0..=4 mapping to Atk, Def, Spe, SpA, SpD)
    // Actually, I don't need to guess the exact index mapping, I can just find which nature boosts which stat.
    // For each stat index 1..=5, there should be 4 natures that boost it and 4 that reduce it.
    for stat in 1..=5 {
        let mut found_boost = false;
        let mut found_reduce = false;
        for nature in 0..25 {
            let (num, den) = nature_modifier(nature, stat);
            if num == 11 && den == 10 {
                found_boost = true;
            } else if num == 9 && den == 10 {
                found_reduce = true;
            } else {
                assert_eq!(num, 10);
                assert_eq!(den, 10);
            }
        }
        assert!(found_boost, "Stat {} should have a boosting nature", stat);
        assert!(found_reduce, "Stat {} should have a reducing nature", stat);
    }
}

#[test]
fn test_nature_modifier_hp() {
    // HP is stat index 0. Should always be 10,10.
    for nature in 0..25 {
        assert_eq!(nature_modifier(nature, 0), (10, 10));
    }
}

#[test]
fn test_level_50_vs_100() {
    let mut mon50 = MonSlot::default();
    let mut mon100 = MonSlot::default();
    
    let sp = dummy_species(100, 100, 100, 100, 100, 100);
    let build = dummy_build([31; 6], [252; 6], 0); // Neutral nature
    
    recompute_stats(&mut mon50, &sp, &build, 50);
    recompute_stats(&mut mon100, &sp, &build, 100);
    
    assert!(mon50.max_hp < mon100.max_hp);
    assert!(mon50.stats[0] < mon100.stats[0]);
    assert!(mon50.stats[1] < mon100.stats[1]);
}

#[test]
fn test_zero_vs_max_evs() {
    let mut mon_zero = MonSlot::default();
    let mut mon_max = MonSlot::default();
    
    let sp = dummy_species(100, 100, 100, 100, 100, 100);
    let build_zero = dummy_build([31; 6], [0; 6], 0);
    let build_max = dummy_build([31; 6], [252; 6], 0);
    
    recompute_stats(&mut mon_zero, &sp, &build_zero, 100);
    recompute_stats(&mut mon_max, &sp, &build_max, 100);
    
    // EV difference of 252 at level 100 -> 252 / 4 = 63 points
    assert_eq!(mon_max.max_hp, mon_zero.max_hp + 63);
    assert_eq!(mon_max.stats[0], mon_zero.stats[0] + 63);
}

#[test]
fn test_zero_vs_max_ivs() {
    let mut mon_zero = MonSlot::default();
    let mut mon_max = MonSlot::default();
    
    let sp = dummy_species(100, 100, 100, 100, 100, 100);
    let build_zero = dummy_build([0; 6], [0; 6], 0);
    let build_max = dummy_build([31; 6], [0; 6], 0);
    
    recompute_stats(&mut mon_zero, &sp, &build_zero, 100);
    recompute_stats(&mut mon_max, &sp, &build_max, 100);
    
    // IV difference of 31 at level 100 -> 31 points
    assert_eq!(mon_max.max_hp, mon_zero.max_hp + 31);
    assert_eq!(mon_max.stats[0], mon_zero.stats[0] + 31);
}

#[test]
fn test_adamant_nature_stats() {
    // Find a nature that boosts Atk (stat=1) and reduces SpA (stat=3)
    let mut adamant = 0;
    for nature in 0..25 {
        if nature_modifier(nature, 1) == (11, 10) && nature_modifier(nature, 3) == (9, 10) {
            adamant = nature;
            break;
        }
    }
    
    let mut mon = MonSlot::default();
    let sp = dummy_species(100, 100, 100, 100, 100, 100);
    let build = dummy_build([31; 6], [252; 6], adamant);
    
    recompute_stats(&mut mon, &sp, &build, 100);
    // Neutral stat would be: (2*100 + 31 + 63)*100/100 + 5 = 299
    // Atk should be 299 * 1.1 = 328
    // SpA should be 299 * 0.9 = 269
    assert_eq!(mon.stats[0], 328); // Atk is index 0 in stats array
    assert_eq!(mon.stats[2], 269); // SpA is index 2 in stats array
}
