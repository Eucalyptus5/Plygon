//! Item data layer: ItemData struct, ItemFlag constants, item() accessor.
//!
//! Follows the same pattern as moves.rs: a compact struct with a flags
//! bitmask, indexed by item ID from the GEN_ITEMS static array.

#[allow(non_snake_case)]
pub mod ItemFlag {
    // Damage calc: stat modifiers
    pub const CHOICE_ATK: u32       = 1 << 0;
    pub const CHOICE_SPA: u32       = 1 << 1;
    pub const CHOICE_SPE: u32       = 1 << 2;
    pub const ASSAULT_VEST: u32     = 1 << 3;
    pub const EVIOLITE: u32         = 1 << 4;

    // Damage calc: damage modifiers
    pub const LIFE_ORB: u32         = 1 << 5;
    pub const EXPERT_BELT: u32      = 1 << 6;
    pub const TYPE_BOOST: u32       = 1 << 7;
    pub const RESIST_BERRY: u32     = 1 << 8;
    pub const METRONOME: u32        = 1 << 9;

    // Crit / accuracy
    pub const CRIT_BOOST: u32       = 1 << 10;
    pub const WIDE_LENS: u32        = 1 << 11;

    // Defensive
    pub const FOCUS_SASH: u32       = 1 << 12;
    pub const AIR_BALLOON: u32      = 1 << 13;
    pub const SAFETY_GOGGLES: u32   = 1 << 14;
    pub const ROCKY_HELMET: u32     = 1 << 15;

    // End-of-turn
    pub const LEFTOVERS: u32        = 1 << 16;
    pub const BLACK_SLUDGE: u32     = 1 << 17;
    pub const FLAME_ORB: u32        = 1 << 18;
    pub const TOXIC_ORB: u32        = 1 << 19;

    // Switch / hazard
    pub const HAZARD_IMMUNE: u32    = 1 << 20;
    pub const TRAP_IMMUNE: u32      = 1 << 21;
    pub const EXTENDS_SCREENS: u32  = 1 << 22;
    pub const BINDING_BOOST: u32    = 1 << 23;

    // Berry / consumable / seed
    pub const TERRAIN_SEED: u32     = 1 << 24;
    pub const IS_BERRY: u32         = 1 << 25;
    pub const PINCH_BERRY: u32      = 1 << 26;
    pub const MEGA_STONE: u32       = 1 << 27;
    pub const Z_CRYSTAL: u32        = 1 << 28;
    pub const CONSUMABLE: u32       = 1 << 29;
    pub const GEM: u32              = 1 << 30;
    pub const POWER_HERB: u32       = 1 << 31;

    // Convenience masks
    pub const IS_CHOICE: u32 = CHOICE_ATK | CHOICE_SPA | CHOICE_SPE;
}

/// Compact item data: flags bitmask + type parameter for type-specific items.
/// 8 bytes with #[repr(C)], zero padding.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ItemData {
    pub flags: u32,
    pub type_param: u8,   // Type as u8 for type-boost/resist/gem items, 0xFF = N/A
    pub power_param: u8,  // Fling base power
    /// Base species ID that locks this item (e.g. 493 for Arceus Plates).
    /// 0 = not forme-locked.  Knock Off / Thief cannot remove the item when
    /// `base_species(holder) == forme_species`.
    pub forme_species: u16,
}

const _: () = assert!(core::mem::size_of::<ItemData>() == 8);

impl ItemData {
    pub const NONE: Self = Self {
        flags: 0, type_param: 0xFF, power_param: 0, forme_species: 0,
    };

    /// Check if a flag is set.
    #[inline(always)]
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// Check if this is a "no item" / empty entry.
    #[inline(always)]
    pub fn is_none(&self) -> bool {
        self.flags == 0 && self.type_param == 0xFF
    }

    /// Returns true when this item cannot be removed from a holder whose
    /// base species is `holder_base_species` (e.g. Plates on Arceus).
    #[inline(always)]
    pub fn is_forme_locked(&self, holder_base_species: u16) -> bool {
        self.forme_species != 0 && self.forme_species == holder_base_species
    }
}

/// Item lookup by ID.  Returns &NONE for id=0 or out-of-range.
#[inline(always)]
pub fn item(id: usize) -> &'static ItemData {
    use crate::data::gen_items::GEN_ITEMS;
    if id >= GEN_ITEMS.len() {
        return &ItemData::NONE;
    }
    unsafe { GEN_ITEMS.get_unchecked(id) }
}
