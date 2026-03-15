use pkmn_engine::data::base_stats::{bst, species};
use pkmn_engine::data::types::Type;
use pkmn_engine::data::{FORME_ABOMASNOW_MEGA, GEN_SPECIES};

#[test]
fn test_species_lookup() {
    let bulbasaur = species(1);
    assert_eq!(bulbasaur.hp, 45);
    assert_eq!(bulbasaur.atk, 49);
    assert_eq!(bulbasaur.def, 49);
    assert_eq!(bulbasaur.spa, 65);
    assert_eq!(bulbasaur.spd, 65);
    assert_eq!(bulbasaur.spe, 45);
    assert_eq!(bulbasaur.type1, Type::Grass);
    assert_eq!(bulbasaur.type2, Type::Poison);
    assert_eq!(bulbasaur.weight, 69);

    let charizard = species(6);
    assert_eq!(charizard.type1, Type::Fire);
    assert_eq!(charizard.type2, Type::Flying);

    let missingno = species(0);
    assert_eq!(missingno.hp, 0);
    assert_eq!(missingno.type1, Type::Normal);
    assert_eq!(missingno.type2, Type::Normal);
}

#[test]
fn test_bst_calculation() {
    let mew = species(151);
    assert_eq!(
        bst(mew),
        600,
        "Mew should have a BST of 600 (100 in all stats)"
    );

    let sunkern = species(191);
    assert_eq!(
        bst(sunkern),
        180,
        "Sunkern should have a BST of 180 (30 in all stats)"
    );

    let arceus = species(493);
    assert_eq!(
        bst(arceus),
        720,
        "Arceus should have a BST of 720 (120 in all stats)"
    );
}

#[test]
fn test_forme_lookups() {
    let abomasnow_mega = species(FORME_ABOMASNOW_MEGA);
    assert_eq!(abomasnow_mega.type1, Type::Grass);
    assert_eq!(abomasnow_mega.type2, Type::Ice);
    assert_eq!(abomasnow_mega.weight, 1850);
    assert_eq!(bst(abomasnow_mega), 594);
}

#[test]
fn test_all_species_valid_types() {
    for i in 0..GEN_SPECIES.len() {
        let s = species(i);
        let t1 = s.type1 as u8;
        let t2 = s.type2 as u8;
        assert!(t1 < 18, "Species {} has invalid type1: {}", i, t1);
        assert!(t2 < 18, "Species {} has invalid type2: {}", i, t2);
    }
}
