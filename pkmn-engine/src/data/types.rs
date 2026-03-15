// src/data/types.rs
//
// u8 enum — 18 types fit in 5 bits but u8 avoids bit-packing overhead.
// Type effectiveness is a 18×18 lookup table (324 bytes, fits in L1).

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Type {
    Normal   = 0,
    Fire     = 1,
    Water    = 2,
    Electric = 3,
    Grass    = 4,
    Ice      = 5,
    Fighting = 6,
    Poison   = 7,
    Ground   = 8,
    Flying   = 9,
    Psychic  = 10,
    Bug      = 11,
    Rock     = 12,
    Ghost    = 13,
    Dragon   = 14,
    Dark     = 15,
    Steel    = 16,
    Fairy    = 17,
}

pub const NUM_TYPES: usize = 18;

/// Type effectiveness multiplier × 4 (stored as u8 to avoid floats).
///   0 = immune (0×)
///   2 = not very effective (0.5×)
///   4 = neutral (1×)
///   8 = super effective (2×)
///
/// Access: EFFECTIVENESS[attacking_type as usize][defending_type as usize]
///
/// To apply: damage = base_damage * eff / 4
/// For dual types: damage = base_damage * eff1 * eff2 / 16
pub static EFFECTIVENESS: [[u8; NUM_TYPES]; NUM_TYPES] = {
    const X: u8 = 0; // immune
    const H: u8 = 2; // half (not very effective)
    const N: u8 = 4; // neutral
    const S: u8 = 8; // super effective
    //                  Nor Fir Wat Ele Gra Ice Fig Poi Gro Fly Psy Bug Roc Gho Dra Dar Ste Fai
    [
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
    ]
};

/// Get effectiveness multiplier × 4 for a single attacking type vs single defending type.
#[inline(always)]
pub fn type_effectiveness(atk: Type, def: Type) -> u8 {
    unsafe { *EFFECTIVENESS.get_unchecked(atk as usize).get_unchecked(def as usize) }
}

/// Get combined effectiveness (×4 scale) for attacking type vs defender's types.
/// Handles both mono-type and dual-type defenders correctly.
///
/// Return values: 0=immune, 1=4× resist, 2=2× resist, 4=neutral, 8=2× SE, 16=4× SE.
///
/// Apply to damage: `damage = base_damage * eff / 4`
#[inline]
pub fn dual_type_effectiveness(atk: Type, def1: Type, def2: Type) -> u8 {
    let e1 = type_effectiveness(atk, def1);
    // Mono-type: type2 == type1, so just return the single multiplier.
    // Without this check, e1 * e1 / 4 gives wrong results for non-neutral matchups
    // (e.g. Fire vs pure Water: 2 * 2 / 4 = 1 instead of correct 2).
    if def1 == def2 {
        return e1;
    }
    let e2 = type_effectiveness(atk, def2);
    ((e1 as u16 * e2 as u16) / 4) as u8
}
