use pkmn_engine::data::types::{type_effectiveness, dual_type_effectiveness, Type};

#[test]
fn test_all_matchups() {
    const X: u8 = 0;
    const H: u8 = 2;
    const N: u8 = 4;
    const S: u8 = 8;
    
    let expected = [
    /* Normal   */ [  N,  N,  N,  N,  N,  N,  N,  N,  N,  N,  N,  N,  H,  X,  N,  N,  H,  N ],
    /* Fire     */ [  N,  H,  H,  N,  S,  S,  N,  N,  N,  N,  N,  S,  H,  N,  H,  N,  S,  N ],
    /* Water    */ [  N,  S,  H,  N,  H,  N,  N,  N,  S,  N,  N,  N,  S,  N,  H,  N,  N,  N ],
    /* Electric */ [  N,  N,  S,  H,  H,  N,  N,  N,  X,  S,  N,  N,  N,  N,  H,  N,  N,  N ],
    /* Grass    */ [  N,  H,  S,  N,  H,  N,  N,  H,  S,  H,  N,  H,  S,  N,  H,  N,  H,  N ],
    /* Ice      */ [  N,  H,  H,  N,  S,  H,  N,  N,  S,  S,  N,  N,  N,  N,  S,  N,  H,  N ],
    /* Fighting */ [  S,  N,  N,  N,  N,  S,  N,  H,  N,  H,  H,  H,  S,  X,  N,  S,  S,  H ],
    /* Poison   */ [  N,  N,  N,  N,  S,  N,  N,  H,  H,  N,  N,  N,  H,  H,  N,  N,  X,  S ],
    /* Ground   */ [  N,  S,  N,  S,  H,  N,  N,  S,  N,  X,  N,  H,  S,  N,  N,  N,  S,  N ],
    /* Flying   */ [  N,  N,  N,  H,  S,  N,  S,  N,  N,  N,  N,  S,  H,  N,  N,  N,  H,  N ],
    /* Psychic  */ [  N,  N,  N,  N,  N,  N,  S,  S,  N,  N,  H,  N,  N,  N,  N,  X,  H,  N ],
    /* Bug      */ [  N,  H,  N,  N,  S,  N,  H,  H,  N,  H,  S,  N,  N,  H,  N,  S,  H,  H ],
    /* Rock     */ [  N,  S,  N,  N,  N,  S,  H,  N,  H,  S,  N,  S,  N,  N,  N,  N,  H,  N ],
    /* Ghost    */ [  X,  N,  N,  N,  N,  N,  N,  N,  N,  N,  S,  N,  N,  S,  N,  H,  N,  N ],
    /* Dragon   */ [  N,  N,  N,  N,  N,  N,  N,  N,  N,  N,  N,  N,  N,  N,  S,  N,  H,  X ],
    /* Dark     */ [  N,  N,  N,  N,  N,  N,  H,  N,  N,  N,  S,  N,  N,  S,  N,  H,  N,  H ],
    /* Steel    */ [  N,  H,  H,  H,  N,  S,  N,  N,  N,  N,  N,  N,  S,  N,  N,  N,  H,  S ],
    /* Fairy    */ [  N,  H,  N,  N,  N,  N,  S,  H,  N,  N,  N,  N,  N,  N,  S,  S,  H,  N ],
    ];

    let types = [
        Type::Normal, Type::Fire, Type::Water, Type::Electric, Type::Grass, Type::Ice, Type::Fighting, Type::Poison,
        Type::Ground, Type::Flying, Type::Psychic, Type::Bug, Type::Rock, Type::Ghost, Type::Dragon, Type::Dark,
        Type::Steel, Type::Fairy,
    ];

    for atk in 0..18 {
        for def in 0..18 {
            let actual = type_effectiveness(types[atk], types[def]);
            assert_eq!(actual, expected[atk][def], "Mismatch for {:?} vs {:?}", types[atk], types[def]);
        }
    }
}

#[test]
fn test_critical_matchups() {
    assert_eq!(type_effectiveness(Type::Normal, Type::Ghost), 0);
    assert_eq!(type_effectiveness(Type::Ground, Type::Flying), 0);
    assert_eq!(type_effectiveness(Type::Electric, Type::Ground), 0);
    assert_eq!(type_effectiveness(Type::Fighting, Type::Ghost), 0);
    assert_eq!(type_effectiveness(Type::Ghost, Type::Normal), 0);
    assert_eq!(type_effectiveness(Type::Dragon, Type::Fairy), 0);
    assert_eq!(type_effectiveness(Type::Poison, Type::Steel), 0);
}

#[test]
fn test_dual_type_effectiveness() {
    // Fire vs Water/Flying (Gyarados) = neutral (SE + resist cancel)
    // Wait: Fire vs Water = 0.5x, Fire vs Flying = 1x. So Fire vs Water/Flying = 0.5x.
    // The prompt says: "Fire vs Water/Flying (Gyarados) = neutral (SE + resist cancel)" -> Wait, Fire is not SE against Flying. Electric is SE against Water and Flying (4x).
    // Ah, Gyarados is Water/Flying. Fire vs Water is resist (0.5), Fire vs Flying is neutral (1). So Fire vs Water/Flying is 0.5.
    // Let's test what the prompt says: "Fire vs Water/Flying (Gyarados) = neutral (SE + resist cancel)". Wait! The prompt might have made a mistake in its example. But let me write a test for Grass vs Water/Flying. Grass vs Water is SE (2x), Grass vs Flying is resist (0.5x), so Grass vs Water/Flying = 1x (neutral).
    assert_eq!(dual_type_effectiveness(Type::Grass, Type::Water, Type::Flying), 4); // Neutral = 4

    // Rock vs Fire/Flying (Charizard) = 4× SE
    assert_eq!(dual_type_effectiveness(Type::Rock, Type::Fire, Type::Flying), 16); // 16 = 4x

    // Electric vs Water/Flying = SE+SE = 4×
    assert_eq!(dual_type_effectiveness(Type::Electric, Type::Water, Type::Flying), 16);

    // Poison vs Fairy/Steel = immune
    assert_eq!(dual_type_effectiveness(Type::Poison, Type::Fairy, Type::Steel), 0);
}

#[test]
fn test_mono_type() {
    // Mono-type: dual_type_effectiveness with type1 == type2 returns single matchup (not squared)
    assert_eq!(dual_type_effectiveness(Type::Fire, Type::Water, Type::Water), 2); // 2 = 0.5x
    assert_eq!(dual_type_effectiveness(Type::Water, Type::Fire, Type::Fire), 8);  // 8 = 2x
    assert_eq!(dual_type_effectiveness(Type::Fighting, Type::Ghost, Type::Ghost), 0); // 0 = immune
}

#[test]
fn test_immune_returns_0() {
    // Every immune matchup returns 0 in dual types
    assert_eq!(dual_type_effectiveness(Type::Electric, Type::Water, Type::Ground), 0);
    assert_eq!(dual_type_effectiveness(Type::Normal, Type::Steel, Type::Ghost), 0);
}
