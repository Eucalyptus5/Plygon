use crate::data::types::Type;

/// Compact species data: 10 bytes, no padding with #[repr(C)].
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SpeciesData {
    pub hp:     u8,
    pub atk:    u8,
    pub def:    u8,
    pub spa:    u8,
    pub spd:    u8,
    pub spe:    u8,
    pub type1:  Type,
    pub type2:  Type,
    pub weight: u16,
}

const _: () = assert!(core::mem::size_of::<SpeciesData>() == 10);

// ─── Accessor ─────────────────────────────────────────────────────────────

/// Branchless species lookup. O(1).
/// Bounds-checked in debug, unchecked in release.
#[inline(always)]
pub fn species(id: usize) -> &'static SpeciesData {
    use crate::data::gen_species::GEN_SPECIES;
    debug_assert!(id < GEN_SPECIES.len(), "species id {} out of range", id);
    unsafe { GEN_SPECIES.get_unchecked(id) }
}

/// Base stat total.
#[inline]
pub fn bst(s: &SpeciesData) -> u16 {
    s.hp as u16 + s.atk as u16 + s.def as u16
        + s.spa as u16 + s.spd as u16 + s.spe as u16
}
