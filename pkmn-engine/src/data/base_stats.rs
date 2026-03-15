// src/data/base_stats.rs
//
// SpeciesData struct and accessor function.
// The actual data lives in generated/gen_species.rs.

use crate::data::types::Type;

/// Compact species data: 10 bytes, no padding with #[repr(C)].
///
/// Weight is in tenths-of-kg (u16), so 695 = 69.5 kg.
/// Max real weight is Celesteela at 999.9 kg → 9999, fits u16.
///
/// For mono-typed Pokémon, type2 == type1.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SpeciesData {
    pub hp:     u8,
    pub atk:    u8,
    pub def:    u8,
    pub spa:    u8,
    pub spd:    u8,
    pub spe:    u8,
    pub type1:  Type,   // u8 enum
    pub type2:  Type,   // u8 enum
    pub weight: u16,    // tenths of kg
}

const _: () = assert!(core::mem::size_of::<SpeciesData>() == 10);

// ─── Accessor ─────────────────────────────────────────────────────────────

/// Branchless species lookup. O(1).
/// Bounds-checked in debug, unchecked in release.
///
/// Works for both base species (indices 1–1025) and formes (indices 1100+).
/// See gen_species.rs for FORME_* constants.
#[inline(always)]
pub fn species(id: usize) -> &'static SpeciesData {
    use crate::data::gen_species::GEN_SPECIES;
    debug_assert!(id < GEN_SPECIES.len(), "species id {} out of range", id);
    unsafe { GEN_SPECIES.get_unchecked(id) }
}

/// Base stat total. Useful for tier estimation, not used in damage calc.
#[inline]
pub fn bst(s: &SpeciesData) -> u16 {
    s.hp as u16 + s.atk as u16 + s.def as u16
        + s.spa as u16 + s.spd as u16 + s.spe as u16
}
