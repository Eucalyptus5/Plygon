#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Type {
    Normal,   // 0
    Fire,     // 1
    Water,    // 2
    Electric, // 3
    Grass,    // 4
    Ice,      // 5
    Fighting, // 6
    Poison,   // 7
    Ground,   // 8
    Flying,   // 9
    Psychic,  // 10
    Bug,      // 11
    Rock,     // 12
    Ghost,    // 13
    Dragon,   // 14
    Dark,     // 15
    Steel,    // 16
    Fairy,    // 17
    Typeless, // 18
}

const fn build_table() -> [[f32; 19]; 19] {
    let mut t = [[1.0f32; 19]; 19];

    // Immunities (0.0)
    t[0][13]  = 0.0; // Normal   -> Ghost
    t[6][13]  = 0.0; // Fighting -> Ghost
    t[13][0]  = 0.0; // Ghost    -> Normal
    t[8][9]   = 0.0; // Ground   -> Flying
    t[3][8]   = 0.0; // Electric -> Ground
    t[7][16]  = 0.0; // Poison   -> Steel
    t[10][15] = 0.0; // Psychic  -> Dark
    t[14][17] = 0.0; // Dragon   -> Fairy

    // Resists (0.5)
    t[1][2]   = 0.5; t[1][12]  = 0.5; t[1][14]  = 0.5; // Fire
    t[2][2]   = 0.5; t[2][4]   = 0.5; t[2][14]  = 0.5; // Water
    t[3][3]   = 0.5; t[3][4]   = 0.5; t[3][14]  = 0.5; // Electric
    t[4][1]   = 0.5; t[4][4]   = 0.5; t[4][7]   = 0.5; // Grass
    t[4][9]   = 0.5; t[4][11]  = 0.5; t[4][14]  = 0.5; // Grass cont.
    t[4][16]  = 0.5;                                     // Grass cont.
    t[5][2]   = 0.5; t[5][5]   = 0.5;                   // Ice
    t[6][7]   = 0.5; t[6][9]   = 0.5; t[6][10]  = 0.5; // Fighting
    t[6][11]  = 0.5; t[6][17]  = 0.5;                   // Fighting cont.
    t[7][7]   = 0.5; t[7][8]   = 0.5; t[7][12]  = 0.5; // Poison
    t[7][13]  = 0.5;                                     // Poison cont.
    t[8][4]   = 0.5; t[8][11]  = 0.5;                   // Ground
    t[9][3]   = 0.5; t[9][12]  = 0.5; t[9][16]  = 0.5; // Flying
    t[10][10] = 0.5; t[10][16] = 0.5;                   // Psychic
    t[11][1]  = 0.5; t[11][6]  = 0.5; t[11][9]  = 0.5; // Bug
    t[11][13] = 0.5; t[11][16] = 0.5; t[11][17] = 0.5; // Bug cont.
    t[12][6]  = 0.5; t[12][8]  = 0.5; t[12][16] = 0.5; // Rock
    t[13][15] = 0.5;                                     // Ghost
    t[14][16] = 0.5;                                     // Dragon
    t[15][6]  = 0.5; t[15][15] = 0.5; t[15][17] = 0.5; // Dark
    t[16][1]  = 0.5; t[16][2]  = 0.5; t[16][3]  = 0.5; // Steel
    t[16][16] = 0.5;                                     // Steel cont.
    t[17][1]  = 0.5; t[17][7]  = 0.5; t[17][16] = 0.5; // Fairy

    // Super effectives (2.0)
    t[1][4]   = 2.0; t[1][5]   = 2.0; t[1][11]  = 2.0; // Fire
    t[1][16]  = 2.0;                                     // Fire cont.
    t[2][1]   = 2.0; t[2][8]   = 2.0; t[2][12]  = 2.0; // Water
    t[3][2]   = 2.0; t[3][9]   = 2.0;                   // Electric
    t[4][2]   = 2.0; t[4][8]   = 2.0; t[4][12]  = 2.0; // Grass
    t[5][4]   = 2.0; t[5][8]   = 2.0; t[5][9]   = 2.0; // Ice
    t[5][14]  = 2.0;                                     // Ice cont.
    t[6][0]   = 2.0; t[6][5]   = 2.0; t[6][12]  = 2.0; // Fighting
    t[6][15]  = 2.0; t[6][16]  = 2.0;                   // Fighting cont.
    t[7][4]   = 2.0; t[7][17]  = 2.0;                   // Poison
    t[8][1]   = 2.0; t[8][3]   = 2.0; t[8][7]   = 2.0; // Ground
    t[8][12]  = 2.0; t[8][16]  = 2.0;                   // Ground cont.
    t[9][4]   = 2.0; t[9][6]   = 2.0; t[9][11]  = 2.0; // Flying
    t[10][6]  = 2.0; t[10][7]  = 2.0;                   // Psychic
    t[11][4]  = 2.0; t[11][10] = 2.0; t[11][15] = 2.0; // Bug
    t[12][1]  = 2.0; t[12][5]  = 2.0; t[12][9]  = 2.0; // Rock
    t[12][11] = 2.0;                                     // Rock cont.
    t[13][10] = 2.0; t[13][13] = 2.0;                   // Ghost
    t[14][14] = 2.0;                                     // Dragon
    t[15][10] = 2.0; t[15][13] = 2.0;                   // Dark
    t[16][5]  = 2.0; t[16][12] = 2.0; t[16][17] = 2.0; // Steel
    t[17][6]  = 2.0; t[17][14] = 2.0; t[17][15] = 2.0; // Fairy
    t
}

static TABLE: [[f32; 19]; 19] = build_table();

/// Single type matchup. Stellar type should never be passed here.
#[inline(always)]
pub fn effectiveness(attacker: Type, defender: Type) -> f32 {
    TABLE[attacker as usize][defender as usize]
}

/// Dual-type defender. Single-type Pokemon should pass the same type twice.
#[inline(always)]
pub fn type_effectiveness(attacker: Type, defender_types: (Type, Type)) -> f32 {
    effectiveness(attacker, defender_types.0) * effectiveness(attacker, defender_types.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_indices_are_correct() {
        assert_eq!(Type::Normal   as usize, 0);
        assert_eq!(Type::Fire     as usize, 1);
        assert_eq!(Type::Water    as usize, 2);
        assert_eq!(Type::Electric as usize, 3);
        assert_eq!(Type::Grass    as usize, 4);
        assert_eq!(Type::Ice      as usize, 5);
        assert_eq!(Type::Fighting as usize, 6);
        assert_eq!(Type::Poison   as usize, 7);
        assert_eq!(Type::Ground   as usize, 8);
        assert_eq!(Type::Flying   as usize, 9);
        assert_eq!(Type::Psychic  as usize, 10);
        assert_eq!(Type::Bug      as usize, 11);
        assert_eq!(Type::Rock     as usize, 12);
        assert_eq!(Type::Ghost    as usize, 13);
        assert_eq!(Type::Dragon   as usize, 14);
        assert_eq!(Type::Dark     as usize, 15);
        assert_eq!(Type::Steel    as usize, 16);
        assert_eq!(Type::Fairy    as usize, 17);
        assert_eq!(Type::Typeless as usize, 18);
    }

    // Immunities
    #[test]
    fn normal_vs_ghost()    { assert_eq!(effectiveness(Type::Normal,   Type::Ghost),    0.0); }
    #[test]
    fn electric_vs_ground() { assert_eq!(effectiveness(Type::Electric, Type::Ground),   0.0); }
    #[test]
    fn dragon_vs_fairy()    { assert_eq!(effectiveness(Type::Dragon,   Type::Fairy),    0.0); }
    #[test]
    fn psychic_vs_dark()    { assert_eq!(effectiveness(Type::Psychic,  Type::Dark),     0.0); }

    // Resists
    #[test]
    fn fire_vs_water()      { assert_eq!(effectiveness(Type::Fire,     Type::Water),    0.5); }
    #[test]
    fn bug_vs_fairy()       { assert_eq!(effectiveness(Type::Bug,      Type::Fairy),    0.5); }

    // Super effectives
    #[test]
    fn fire_vs_grass()      { assert_eq!(effectiveness(Type::Fire,     Type::Grass),    2.0); }
    #[test]
    fn ghost_vs_psychic()   { assert_eq!(effectiveness(Type::Ghost,    Type::Psychic),  2.0); }
    #[test]
    fn fairy_vs_dragon()    { assert_eq!(effectiveness(Type::Fairy,    Type::Dragon),   2.0); }

    // Typeless is always neutral
    #[test]
    fn typeless_vs_ghost()  { assert_eq!(effectiveness(Type::Typeless, Type::Ghost),    1.0); }
    #[test]
    fn fire_vs_typeless()   { assert_eq!(effectiveness(Type::Fire,     Type::Typeless), 1.0); }

    // Dual type
    #[test]
    fn electric_vs_water_flying() {
        // 2.0 * 2.0 = 4x
        assert_eq!(type_effectiveness(Type::Electric, (Type::Water, Type::Flying)), 4.0);
    }
    #[test]
    fn electric_vs_grass_ground() {
        // 0.5 * 0.0 = immune
        assert_eq!(type_effectiveness(Type::Electric, (Type::Grass, Type::Ground)), 0.0);
    }
}