use poke_mcts::gen_sets::*;

#[test]
fn table_covers_the_metagame() {
    assert!(GEN9_SET_POOL.len() >= 400, "got {} species", GEN9_SET_POOL.len());
    assert!(GEN9_SET_POOL_TOTAL > 0);
}

#[test]
fn table_is_well_formed() {
    let mut prev = 0u16;
    for sp in GEN9_SET_POOL {
        assert!(sp.species_id > prev, "sorted, unique species ids");
        prev = sp.species_id;
        assert_eq!(sp.total_count, sp.sets.iter().map(|s| s.count).sum::<u32>());
        for s in sp.sets {
            // <4-move sets 0-pad then sort ascending, so [0,0,0,id] is real (e.g. Ditto).
            assert!(s.moves.iter().any(|&m| m != 0), "every set has at least one move");
            assert!(s.level >= 1 && s.level <= 100);
            // tera_type is a Showdown index 0..=18; 18 == Stellar (Terapagos).
            assert!(s.tera_type <= 18);
        }
    }
}
