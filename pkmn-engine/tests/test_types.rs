use pkmn_engine::data::types::{dual_type_effectiveness, type_effectiveness, Type, NUM_TYPES};

#[test]
fn test_all_single_type_matchups() {
    let matchups = [
        // Normal
        (Type::Normal, Type::Rock, 2),
        (Type::Normal, Type::Ghost, 0),
        (Type::Normal, Type::Steel, 2),
        (Type::Normal, Type::Normal, 4),
        // Fire
        (Type::Fire, Type::Fire, 2),
        (Type::Fire, Type::Water, 2),
        (Type::Fire, Type::Grass, 8),
        (Type::Fire, Type::Ice, 8),
        (Type::Fire, Type::Bug, 8),
        (Type::Fire, Type::Rock, 2),
        (Type::Fire, Type::Dragon, 2),
        (Type::Fire, Type::Steel, 8),
        // Water
        (Type::Water, Type::Fire, 8),
        (Type::Water, Type::Water, 2),
        (Type::Water, Type::Grass, 2),
        (Type::Water, Type::Ground, 8),
        (Type::Water, Type::Rock, 8),
        (Type::Water, Type::Dragon, 2),
        // Electric
        (Type::Electric, Type::Water, 8),
        (Type::Electric, Type::Electric, 2),
        (Type::Electric, Type::Grass, 2),
        (Type::Electric, Type::Ground, 0),
        (Type::Electric, Type::Flying, 8),
        (Type::Electric, Type::Dragon, 2),
        // Grass
        (Type::Grass, Type::Fire, 2),
        (Type::Grass, Type::Water, 8),
        (Type::Grass, Type::Grass, 2),
        (Type::Grass, Type::Poison, 2),
        (Type::Grass, Type::Ground, 8),
        (Type::Grass, Type::Flying, 2),
        (Type::Grass, Type::Bug, 2),
        (Type::Grass, Type::Rock, 8),
        (Type::Grass, Type::Dragon, 2),
        (Type::Grass, Type::Steel, 2),
        // Ice
        (Type::Ice, Type::Fire, 2),
        (Type::Ice, Type::Water, 2),
        (Type::Ice, Type::Grass, 8),
        (Type::Ice, Type::Ice, 2),
        (Type::Ice, Type::Ground, 8),
        (Type::Ice, Type::Flying, 8),
        (Type::Ice, Type::Dragon, 8),
        (Type::Ice, Type::Steel, 2),
        // Fighting
        (Type::Fighting, Type::Normal, 8),
        (Type::Fighting, Type::Ice, 8),
        (Type::Fighting, Type::Poison, 2),
        (Type::Fighting, Type::Flying, 2),
        (Type::Fighting, Type::Psychic, 2),
        (Type::Fighting, Type::Bug, 2),
        (Type::Fighting, Type::Rock, 8),
        (Type::Fighting, Type::Ghost, 0),
        (Type::Fighting, Type::Dark, 8),
        (Type::Fighting, Type::Steel, 8),
        (Type::Fighting, Type::Fairy, 2),
        // Poison
        (Type::Poison, Type::Grass, 8),
        (Type::Poison, Type::Poison, 2),
        (Type::Poison, Type::Ground, 2),
        (Type::Poison, Type::Rock, 2),
        (Type::Poison, Type::Ghost, 2),
        (Type::Poison, Type::Steel, 0),
        (Type::Poison, Type::Fairy, 8),
        // Ground
        (Type::Ground, Type::Fire, 8),
        (Type::Ground, Type::Electric, 8),
        (Type::Ground, Type::Grass, 2),
        (Type::Ground, Type::Poison, 8),
        (Type::Ground, Type::Flying, 0),
        (Type::Ground, Type::Bug, 2),
        (Type::Ground, Type::Rock, 8),
        (Type::Ground, Type::Steel, 8),
        // Flying
        (Type::Flying, Type::Electric, 2),
        (Type::Flying, Type::Grass, 8),
        (Type::Flying, Type::Fighting, 8),
        (Type::Flying, Type::Bug, 8),
        (Type::Flying, Type::Rock, 2),
        (Type::Flying, Type::Steel, 2),
        // Psychic
        (Type::Psychic, Type::Fighting, 8),
        (Type::Psychic, Type::Poison, 8),
        (Type::Psychic, Type::Psychic, 2),
        (Type::Psychic, Type::Dark, 0),
        (Type::Psychic, Type::Steel, 2),
        // Bug
        (Type::Bug, Type::Fire, 2),
        (Type::Bug, Type::Grass, 8),
        (Type::Bug, Type::Fighting, 2),
        (Type::Bug, Type::Poison, 2),
        (Type::Bug, Type::Flying, 2),
        (Type::Bug, Type::Psychic, 8),
        (Type::Bug, Type::Ghost, 2),
        (Type::Bug, Type::Dark, 8),
        (Type::Bug, Type::Steel, 2),
        (Type::Bug, Type::Fairy, 2),
        // Rock
        (Type::Rock, Type::Fire, 8),
        (Type::Rock, Type::Ice, 8),
        (Type::Rock, Type::Fighting, 2),
        (Type::Rock, Type::Ground, 2),
        (Type::Rock, Type::Flying, 8),
        (Type::Rock, Type::Bug, 8),
        (Type::Rock, Type::Steel, 2),
        // Ghost
        (Type::Ghost, Type::Normal, 0),
        (Type::Ghost, Type::Psychic, 8),
        (Type::Ghost, Type::Ghost, 8),
        (Type::Ghost, Type::Dark, 2),
        // Dragon
        (Type::Dragon, Type::Dragon, 8),
        (Type::Dragon, Type::Steel, 2),
        (Type::Dragon, Type::Fairy, 0),
        // Dark
        (Type::Dark, Type::Fighting, 2),
        (Type::Dark, Type::Psychic, 8),
        (Type::Dark, Type::Ghost, 8),
        (Type::Dark, Type::Dark, 2),
        (Type::Dark, Type::Fairy, 2),
        // Steel
        (Type::Steel, Type::Fire, 2),
        (Type::Steel, Type::Water, 2),
        (Type::Steel, Type::Electric, 2),
        (Type::Steel, Type::Ice, 8),
        (Type::Steel, Type::Rock, 8),
        (Type::Steel, Type::Steel, 2),
        (Type::Steel, Type::Fairy, 8),
        // Fairy
        (Type::Fairy, Type::Fire, 2),
        (Type::Fairy, Type::Fighting, 8),
        (Type::Fairy, Type::Poison, 2),
        (Type::Fairy, Type::Dragon, 8),
        (Type::Fairy, Type::Dark, 8),
        (Type::Fairy, Type::Steel, 2),
    ];

    for &(atk, def, expected) in &matchups {
        assert_eq!(
            type_effectiveness(atk, def),
            expected,
            "Failed match-up: {:?} attacking {:?}",
            atk,
            def
        );
    }
}

#[test]
fn test_dual_type_effectiveness() {
    // 4x weakness
    assert_eq!(
        dual_type_effectiveness(Type::Fire, Type::Grass, Type::Bug),
        16,
        "Fire should be 4x super effective against Grass/Bug"
    );
    assert_eq!(
        dual_type_effectiveness(Type::Ice, Type::Dragon, Type::Flying),
        16,
        "Ice should be 4x super effective against Dragon/Flying"
    );

    // 4x resistance
    assert_eq!(
        dual_type_effectiveness(Type::Bug, Type::Steel, Type::Flying),
        1,
        "Bug should be 4x resisted by Steel/Flying"
    );
    assert_eq!(
        dual_type_effectiveness(Type::Grass, Type::Steel, Type::Bug),
        1,
        "Grass should be 4x resisted by Steel/Bug"
    );

    // Immunity overrides weakness/resistance
    assert_eq!(
        dual_type_effectiveness(Type::Electric, Type::Water, Type::Ground),
        0,
        "Electric should be immune against Water/Ground"
    );
    assert_eq!(
        dual_type_effectiveness(Type::Poison, Type::Fairy, Type::Steel),
        0,
        "Poison should be immune against Fairy/Steel"
    );

    // Neutral due to weakness cancelling resistance
    assert_eq!(
        dual_type_effectiveness(Type::Fire, Type::Grass, Type::Water),
        4,
        "Fire should be neutral against Grass/Water"
    );
    assert_eq!(
        dual_type_effectiveness(Type::Fighting, Type::Normal, Type::Flying),
        4,
        "Fighting should be neutral against Normal/Flying"
    );

    // Mono-type passed to dual_type_effectiveness
    assert_eq!(
        dual_type_effectiveness(Type::Water, Type::Fire, Type::Fire),
        8,
        "Water should be 2x super effective against pure Fire"
    );
}

#[test]
fn test_type_table_bounds() {
    for atk in 0..NUM_TYPES {
        for def in 0..NUM_TYPES {
            // Will panic if out of bounds. Also we want to ensure no effectiveness multiplier is something invalid
            // Note: NUM_TYPES is 18.
            let eff = type_effectiveness(
                unsafe { std::mem::transmute(atk as u8) },
                unsafe { std::mem::transmute(def as u8) },
            );
            assert!(eff == 0 || eff == 2 || eff == 4 || eff == 8);
        }
    }
}
