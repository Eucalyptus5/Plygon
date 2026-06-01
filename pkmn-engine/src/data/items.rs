//! Item data layer: ItemData struct, ItemFlag constants, item() accessor.
//!
//! Follows the same pattern as moves.rs: a compact struct with a flags
//! bitmask, indexed by item ID from the GEN_ITEMS static array.

#[allow(non_snake_case)]
pub mod ItemFlag {
    // Damage calc: stat modifiers
    pub const CHOICE_ATK: u64       = 1 << 0;
    pub const CHOICE_SPA: u64       = 1 << 1;
    pub const CHOICE_SPE: u64       = 1 << 2;
    pub const ASSAULT_VEST: u64     = 1 << 3;
    pub const EVIOLITE: u64         = 1 << 4;

    // Damage calc: damage modifiers
    pub const LIFE_ORB: u64         = 1 << 5;
    pub const EXPERT_BELT: u64      = 1 << 6;
    pub const TYPE_BOOST: u64       = 1 << 7;
    pub const RESIST_BERRY: u64     = 1 << 8;
    pub const METRONOME: u64        = 1 << 9;

    // Crit / accuracy
    pub const CRIT_BOOST: u64       = 1 << 10;
    pub const WIDE_LENS: u64        = 1 << 11;

    // Defensive
    pub const FOCUS_SASH: u64       = 1 << 12;
    pub const AIR_BALLOON: u64      = 1 << 13;
    pub const SAFETY_GOGGLES: u64   = 1 << 14;
    pub const ROCKY_HELMET: u64     = 1 << 15;

    // End-of-turn
    pub const LEFTOVERS: u64        = 1 << 16;
    pub const BLACK_SLUDGE: u64     = 1 << 17;
    pub const FLAME_ORB: u64        = 1 << 18;
    pub const TOXIC_ORB: u64        = 1 << 19;

    // Switch / hazard
    pub const HAZARD_IMMUNE: u64    = 1 << 20;
    pub const TRAP_IMMUNE: u64      = 1 << 21;
    pub const EXTENDS_SCREENS: u64  = 1 << 22;
    pub const BINDING_BOOST: u64    = 1 << 23;

    // Berry / consumable / seed
    pub const TERRAIN_SEED: u64     = 1 << 24;
    pub const IS_BERRY: u64         = 1 << 25;
    pub const PINCH_BERRY: u64      = 1 << 26;
    pub const MEGA_STONE: u64       = 1 << 27;
    pub const Z_CRYSTAL: u64        = 1 << 28;
    pub const CONSUMABLE: u64       = 1 << 29;
    pub const GEM: u64              = 1 << 30;
    pub const POWER_HERB: u64       = 1 << 31;

    // Gen 9 items
    pub const LOADED_DICE: u64       = 1 << 32;
    pub const COVERT_CLOAK: u64      = 1 << 33;
    pub const CLEAR_AMULET: u64      = 1 << 34;
    pub const ABILITY_SHIELD: u64    = 1 << 35;
    pub const PUNCHING_GLOVE: u64    = 1 << 36;
    pub const MIRROR_HERB: u64       = 1 << 37;
    pub const UTILITY_UMBRELLA: u64  = 1 << 38;
    pub const THROAT_SPRAY: u64      = 1 << 39;
    pub const PROTECTIVE_PADS: u64   = 1 << 40;

    // Damage-modifying items
    pub const MUSCLE_BAND: u64       = 1 << 41;  // 1.1x physical
    pub const WISE_GLASSES: u64      = 1 << 42;  // 1.1x special
    pub const LIGHT_BALL: u64        = 1 << 43;  // 2x Atk/SpA for Pikachu

    // Speed-halving items
    pub const HALF_SPEED: u64        = 1 << 44;  // Iron Ball, Power items

    // Reactive items (onDamagingHit)
    pub const ABSORB_BULB: u64       = 1 << 45;  // +1 SpA when hit by Water
    pub const CELL_BATTERY: u64      = 1 << 46;  // +1 Atk when hit by Electric
    pub const LUMINOUS_MOSS: u64     = 1 << 47;  // +1 SpD when hit by Water
    pub const SNOWBALL: u64          = 1 << 48;  // +1 Atk when hit by Ice

    // Triggered/residual items
    pub const EJECT_BUTTON: u64      = 1 << 49;  // switch out when hit
    pub const EJECT_PACK: u64        = 1 << 50;  // switch out on stat drop
    pub const RED_CARD: u64          = 1 << 51;  // force opponent switch when hit
    pub const STICKY_BARB: u64       = 1 << 52;  // residual damage + transfer on contact
    pub const WHITE_HERB: u64        = 1 << 53;  // restore negative stat changes
    pub const MENTAL_HERB: u64       = 1 << 54;  // cure Taunt/Encore/Torment/Disable/Infatuation/Heal Block

    // Ogerpon masks (1.2x for specific Ogerpon form)
    pub const OGERPON_MASK: u64      = 1 << 55;

    // Kings Rock / Razor Fang: 10% flinch chance on damaging moves
    pub const KINGS_ROCK: u64        = 1 << 56;

    // Quick Claw: 20% chance to bump priority on same/lower-priority moves
    pub const QUICK_CLAW: u64        = 1 << 57;

    // Custap Berry: bump priority once within bracket on a pri<=0 move at <=25% HP
    pub const CUSTAP: u64            = 1 << 58;

    // Lustrous/Adamant/Griseous Orb (+forme item) and Soul Dew: 1.2× on the
    // signature legendary's two move types. Species-gated (calc.rs), unlike the
    // universal TYPE_BOOST held items (Charcoal, Mystic Water, …).
    pub const SIGNATURE_ORB: u64    = 1 << 59;

    // Focus Band: 1/10 survive-at-1HP on a would-be KO Move hit, at any HP, reusable.
    pub const FOCUS_BAND: u64       = 1 << 60;

    // Convenience masks
    pub const IS_CHOICE: u64 = CHOICE_ATK | CHOICE_SPA | CHOICE_SPE;
}

/// Compact item data: flags bitmask + type parameter for type-specific items.
/// 16 bytes with #[repr(C)] (u64 alignment adds 4 bytes padding).
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ItemData {
    pub flags: u64,
    pub type_param: u8,   // Overloaded: type ID for boost/resist/gem; stat index for pinch berries; terrain ID (1-4) for seeds; 0xFF = N/A
    pub power_param: u8,  // Fling base power
    /// Base species ID that locks this item (e.g. 493 for Arceus Plates).
    /// 0 = not forme-locked.  Knock Off / Thief cannot remove the item when
    /// `base_species(holder) == forme_species`.
    pub forme_species: u16,
}

const _: () = assert!(core::mem::size_of::<ItemData>() == 16);

impl ItemData {
    pub const NONE: Self = Self {
        flags: 0, type_param: 0xFF, power_param: 0, forme_species: 0,
    };

    /// Check if a flag is set.
    #[inline(always)]
    pub fn has(&self, flag: u64) -> bool {
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
