//! Core type definitions for the battle state layer.
//!
//! The `Type` enum comes from `crate::data::types`.
//! `MoveCategory` / `MoveTarget` come from `crate::data::moves`.
//! This module defines only the mutable state structs and battle constants.

use core::mem::size_of;

pub use crate::data::types::Type;
pub use crate::data::types::dual_type_effectiveness;
pub use crate::data::moves::MoveCategory;

pub const STATUS_NONE: u8       = 0;
pub const STATUS_BURN: u8       = 1;
pub const STATUS_PARALYSIS: u8  = 2;
pub const STATUS_POISON: u8     = 3;
pub const STATUS_BAD_POISON: u8 = 4;
pub const STATUS_SLEEP: u8      = 5;
pub const STATUS_FREEZE: u8     = 6;

pub const PHASE_ACTIONS: u8     = 0;
pub const PHASE_SWITCH_P1: u8   = 1;
pub const PHASE_SWITCH_P2: u8   = 2;
pub const PHASE_SWITCH_BOTH: u8 = 3;
pub const PHASE_GAME_OVER: u8   = 4;

pub const SUBPHASE_NORMAL: u8       = 0;
pub const SUBPHASE_AFTER_MOVE1: u8  = 1;
pub const SUBPHASE_AFTER_MOVE2: u8  = 2;

pub const ATK: usize = 0;
pub const DEF: usize = 1;
pub const SPA: usize = 2;
pub const SPD: usize = 3;
pub const SPE: usize = 4;
pub const ACC: usize = 5;
pub const EVA: usize = 6;

pub const WEATHER_NONE: u8       = 0;
pub const WEATHER_SUN: u8        = 1;
pub const WEATHER_RAIN: u8       = 2;
pub const WEATHER_SAND: u8       = 3;
pub const WEATHER_SNOW: u8       = 4;
pub const WEATHER_HARSH_SUN: u8  = 5;
pub const WEATHER_HEAVY_RAIN: u8 = 6;
pub const WEATHER_STRONG_WINDS: u8 = 7;

pub const TERRAIN_NONE: u8     = 0;
pub const TERRAIN_ELECTRIC: u8 = 1;
pub const TERRAIN_GRASSY: u8   = 2;
pub const TERRAIN_PSYCHIC: u8  = 3;
pub const TERRAIN_MISTY: u8    = 4;

pub const VOL_SUBSTITUTE: u32        = 1 << 0;
pub const VOL_LEECH_SEED: u32       = 1 << 1;
pub const VOL_TRAPPED: u32          = 1 << 2;
pub const VOL_CHARGING: u32         = 1 << 3;
pub const VOL_SEMI_INVULNERABLE: u32 = 1 << 4;
pub const VOL_RECHARGING: u32       = 1 << 5;
pub const VOL_FLINCHED: u32         = 1 << 6;
pub const VOL_MOVED_THIS_TURN: u32  = 1 << 7;
pub const VOL_PROTECT_THIS_TURN: u32 = 1 << 8;
pub const VOL_ENDURE: u32           = 1 << 9;
pub const VOL_FOCUS_ENERGY: u32     = 1 << 10;
pub const VOL_TORMENT: u32          = 1 << 11;
pub const VOL_IMPRISON: u32         = 1 << 12;
pub const VOL_ABILITY_SUPPRESSED: u32 = 1 << 13;
pub const VOL_UNBURDEN: u32         = 1 << 14;
pub const VOL_FLASH_FIRE: u32       = 1 << 15;
pub const VOL_MINIMIZE: u32         = 1 << 16;
pub const VOL_SMACKED_DOWN: u32     = 1 << 17;
pub const VOL_AQUA_RING: u32        = 1 << 18;
pub const VOL_INGRAIN: u32          = 1 << 19;
pub const VOL_MAGNET_RISE: u32      = 1 << 20;
pub const VOL_PERISH_SONG: u32      = 1 << 21;
pub const VOL_DESTINY_BOND: u32     = 1 << 22;
pub const VOL_GRUDGE: u32           = 1 << 23;
pub const VOL_MOVE_LOCKED: u32      = 1 << 24;
pub const VOL_TYPES_OVERRIDDEN: u32 = 1 << 25;
pub const VOL_TRANSFORMED: u32      = 1 << 26;
pub const VOL_ABILITY_OVERRIDDEN: u32 = 1 << 27;
pub const VOL_MUST_SWITCH: u32      = 1 << 28;
pub const VOL_BOUND: u32            = 1 << 29;
pub const VOL_YAWN: u32             = 1 << 30;
pub const VOL_LASER_FOCUS: u32      = 1 << 31;

/// Flags cleared at end of every turn.
pub const VOL_PER_TURN_MASK: u32 =
    VOL_FLINCHED | VOL_MOVED_THIS_TURN | VOL_PROTECT_THIS_TURN | VOL_ENDURE
    | VOL_DESTINY_BOND;

pub const MON_FLAG_TERASTALLIZED: u8 = 1 << 0;
pub const MON_FLAG_FEMALE: u8       = 1 << 1;
pub const MON_FLAG_TRANSFORMED: u8  = 1 << 2;
// Palafin: Zero to Hero triggered
pub const MON_FLAG_HERO_ACTIVATED: u8 = 1 << 3;
pub const MON_FLAG_SWORD_BOOSTED: u8  = 1 << 4;
pub const MON_FLAG_SHIELD_BOOSTED: u8 = 1 << 5;
pub const MON_FLAG_SYRUP_TRIGGERED: u8 = 1 << 6;
pub const MON_FLAG_BOND_TRIGGERED: u8 = 1 << 7;

pub const HAZARD_STEALTH_ROCK: u8 = 1 << 0;
pub const HAZARD_STICKY_WEB: u8   = 1 << 1;

pub const SIDE_HEALING_WISH: u8  = 1 << 3;
pub const SIDE_LUNAR_DANCE: u8   = 1 << 4;

pub const FIELD_MAGIC_ROOM: u8  = 1 << 0;
pub const FIELD_WONDER_ROOM: u8 = 1 << 1;
pub const FIELD_WEATHER_SUPPRESSED: u8 = 1 << 2;

pub const ACTION_MOVE_0: u8   = 0;
pub const ACTION_MOVE_3: u8   = 3;
pub const ACTION_SWITCH_0: u8 = 4;
pub const ACTION_SWITCH_5: u8 = 9;
pub const ACTION_TERA: u8     = 10;
pub const ACTION_STRUGGLE: u8 = 255;

pub const BATTLE_LEVEL: u16 = 100;

pub const BOOST_TABLE: [(u16, u16); 13] = [
    (2, 8), (2, 7), (2, 6), (2, 5), (2, 4), (2, 3), // -6..-1
    (2, 2),                                            //  0
    (3, 2), (4, 2), (5, 2), (6, 2), (7, 2), (8, 2),  // +1..+6
];

#[inline(always)]
pub fn boost_index(stage: i8) -> usize {
    (stage as i16 + 6) as usize
}

#[inline(always)]
pub fn boosted_stat(raw: u16, stage: i8) -> u16 {
    let (num, den) = BOOST_TABLE[boost_index(stage)];
    (raw as u32 * num as u32 / den as u32) as u16
}

/// A single Pokémon's persistent identity.  Survives switching.  36 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct MonSlot {
    pub species_id: u16,
    pub ability_id: u16,
    pub item_id: u16,
    pub current_hp: u16,
    pub max_hp: u16,
    pub stats: [u16; 5],    // Atk, Def, SpA, SpD, Spe
    pub moves: [u16; 4],
    pub pp: [u8; 4],
    pub status: u8,
    pub status_counter: u8,
    pub tera_type: u8,
    pub flags: u8,
}

/// Volatile battlefield presence.  Zeroed on switch-out.  72 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct ActiveMon {
    pub volatile_flags: u32,
    pub substitute_hp: u16,
    pub last_move: u16,
    pub last_move_hit_by: u16,
    pub override_species: u16,
    pub override_ability: u16,
    pub override_stats: [u16; 5],
    pub override_moves: [u16; 4],
    pub choice_locked_move: u16,
    pub disabled_move: u16,
    pub encore_move: u16,
    pub boosts: [i8; 7],
    pub override_pp: [u8; 4],
    pub override_types: [u8; 2],
    pub confusion_turns: u8,
    pub taunt_turns: u8,
    pub encore_turns: u8,
    pub disable_turns: u8,
    pub protect_consecutive: u8,
    pub toxic_counter: u8,
    pub turns_active: u8,
    pub times_hit: u8,
    pub consec_move_count: u8,
    pub stockpile: u8,
    pub magnet_rise_turns: u8,
    pub telekinesis_turns: u8,
    pub heal_block_turns: u8,
    pub perish_count: u8,
    // _padding[0]: baton_pass flag (1 = Baton Pass switch pending, preserve boosts/volatiles)
    // _padding[1]: charge location (0=none, 1=air, 2=underground, 3=underwater, 4=vanished)
    // _padding[2]: move-lock turns remaining (Outrage/Thrash: 0=not locked, 1-2=turns left)
    // _padding[3]: bit 0 = protean_activated, bit 1 = attracted, bit 2 = paradox_from_booster,
    //              bits 4-7 = paradox stat+1 (0=inactive, 1=Atk, 2=Def, 3=SpA, 4=SpD, 5=Spe)
    // _padding[4]: shield bits (bit 0 = Disguise broken, bit 1 = Ice Face broken, bit 2 = charge),
    //              bind_turns (bits 3-6: 0-15 turns remaining for partial trap)
    pub _padding: [u8; 5],
}

/// Side conditions: hazards, screens, field effects.  12 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct SideConditions {
    pub wish_hp: u16,
    pub spikes: u8,
    pub toxic_spikes: u8,
    pub hazard_flags: u8,
    pub reflect_turns: u8,
    pub light_screen_turns: u8,
    pub aurora_veil_turns: u8,
    pub tailwind_turns: u8,
    pub wish_turns: u8,
    /// Packed: low nibble = safeguard_turns (0-5), high nibble = mist_turns (0-5)
    pub safeguard_mist: u8,
    /// Packed: bits 0-2 = lucky_chant_turns (0-5), bit 3 = healing_wish, bit 4 = lunar_dance
    pub side_extra: u8,
}

/// Global field state: weather, terrain, trick room, gravity.  10 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct FieldState {
    pub turn: u16,
    pub weather: u8,
    pub weather_turns: u8,
    pub terrain: u8,
    pub terrain_turns: u8,
    pub trick_room_turns: u8,
    pub gravity_turns: u8,
    pub field_flags: u8,
    pub _padding: u8,
}

/// One player's complete side state.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct SideState {
    pub active: ActiveMon,
    pub team: [MonSlot; 6],
    pub side_conditions: SideConditions,
    pub active_index: u8,
    pub _padding: [u8; 3],
}

impl SideState {
    // _padding[1..3] stores the item_id of the last berry consumed by the active mon.
    // Cleared on switch-out. Used by Harvest to restore consumed berries at EOT.
    #[inline(always)]
    pub fn last_consumed_berry(&self) -> u16 {
        u16::from_le_bytes([self._padding[1], self._padding[2]])
    }

    #[inline(always)]
    pub fn set_last_consumed_berry(&mut self, id: u16) {
        let bytes = id.to_le_bytes();
        self._padding[1] = bytes[0];
        self._padding[2] = bytes[1];
    }
}

/// The complete mutable battle state.  ≤ 640 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct BattleState {
    pub zobrist: u64,
    pub sides: [SideState; 2],
    pub field: FieldState,
    pub phase: u8,
    pub _padding: u8,
}

/// Immutable per-Pokémon build data.  Lives in Tier 2 (never copied by MCTS).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
#[repr(C)]
pub struct MonBuildData {
    pub ivs: [u8; 6],
    pub evs: [u8; 6],
    pub nature: u8,
}

/// Build-time sidecar for both teams.  Shared by reference, never copied.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct TeamData {
    pub mons: [[MonBuildData; 6]; 2],
    pub levels: [[u8; 6]; 2],
}

const _: () = assert!(size_of::<MonSlot>() == 36);
const _: () = assert!(size_of::<ActiveMon>() == 72);
const _: () = assert!(size_of::<SideConditions>() == 12);
const _: () = assert!(size_of::<FieldState>() == 10);
const _: () = assert!(size_of::<SideState>() == 304);
const _: () = assert!(size_of::<BattleState>() <= 640);
const _: () = assert!(size_of::<MonBuildData>() == 13);

const _: () = {
    fn _assert_copy<T: Copy>() {}
    fn _check() { _assert_copy::<BattleState>(); }
};

impl core::fmt::Debug for MonSlot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MonSlot")
            .field("species_id", &self.species_id)
            .field("hp", &format_args!("{}/{}", self.current_hp, self.max_hp))
            .field("status", &self.status)
            .finish()
    }
}

impl core::fmt::Debug for ActiveMon {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ActiveMon")
            .field("flags", &format_args!("0x{:08x}", self.volatile_flags))
            .field("boosts", &self.boosts)
            .finish()
    }
}

impl core::fmt::Debug for BattleState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BattleState")
            .field("zobrist", &format_args!("0x{:016x}", self.zobrist))
            .field("phase", &self.phase)
            .field("turn", &self.field.turn)
            .finish()
    }
}

impl SideConditions {
    #[inline(always)]
    pub fn safeguard_turns(&self) -> u8 { self.safeguard_mist & 0x0F }
    #[inline(always)]
    pub fn set_safeguard_turns(&mut self, t: u8) {
        self.safeguard_mist = (self.safeguard_mist & 0xF0) | (t & 0x0F);
    }
    #[inline(always)]
    pub fn mist_turns(&self) -> u8 { self.safeguard_mist >> 4 }
    #[inline(always)]
    pub fn set_mist_turns(&mut self, t: u8) {
        self.safeguard_mist = (self.safeguard_mist & 0x0F) | ((t & 0x0F) << 4);
    }
    #[inline(always)]
    pub fn lucky_chant_turns(&self) -> u8 { self.side_extra & 0x07 }
    #[inline(always)]
    pub fn set_lucky_chant_turns(&mut self, t: u8) {
        self.side_extra = (self.side_extra & 0xF8) | (t & 0x07);
    }
    #[inline(always)]
    pub fn has_healing_wish(&self) -> bool { self.side_extra & SIDE_HEALING_WISH != 0 }
    #[inline(always)]
    pub fn set_healing_wish(&mut self, v: bool) {
        if v { self.side_extra |= SIDE_HEALING_WISH; }
        else { self.side_extra &= !SIDE_HEALING_WISH; }
    }
    #[inline(always)]
    pub fn has_lunar_dance(&self) -> bool { self.side_extra & SIDE_LUNAR_DANCE != 0 }
    #[inline(always)]
    pub fn set_lunar_dance(&mut self, v: bool) {
        if v { self.side_extra |= SIDE_LUNAR_DANCE; }
        else { self.side_extra &= !SIDE_LUNAR_DANCE; }
    }
}

impl MonSlot {
    #[inline(always)]
    pub fn is_fainted(&self) -> bool { self.current_hp == 0 }

    #[inline(always)]
    pub fn is_terastallized(&self) -> bool { self.flags & MON_FLAG_TERASTALLIZED != 0 }
}

impl ActiveMon {
    #[inline(always)]
    pub fn has_volatile(&self, flag: u32) -> bool { self.volatile_flags & flag != 0 }

    #[inline(always)]
    pub fn set_volatile(&mut self, flag: u32) -> bool {
        let was_set = self.volatile_flags & flag != 0;
        self.volatile_flags |= flag;
        !was_set
    }

    #[inline(always)]
    pub fn clear_volatile(&mut self, flag: u32) -> bool {
        let was_set = self.volatile_flags & flag != 0;
        self.volatile_flags &= !flag;
        was_set
    }

    #[inline(always)]
    pub fn bind_turns(&self) -> u8 { (self._padding[4] >> 3) & 0x0F }

    #[inline(always)]
    pub fn set_bind_turns(&mut self, turns: u8) {
        self._padding[4] = (self._padding[4] & 0x07) | ((turns & 0x0F) << 3);
    }

    #[inline(always)]
    pub fn is_attracted(&self) -> bool { self._padding[3] & 2 != 0 }

    #[inline(always)]
    pub fn set_attracted(&mut self, val: bool) {
        if val { self._padding[3] |= 2; } else { self._padding[3] &= !2; }
    }

    #[inline(always)]
    pub fn paradox_stat(&self) -> u8 { self._padding[3] >> 4 }

    #[inline(always)]
    pub fn paradox_from_booster(&self) -> bool { self._padding[3] & 4 != 0 }

    #[inline(always)]
    pub fn set_paradox(&mut self, stat_plus_1: u8, from_booster: bool) {
        // Preserve bits 0-1 (protean_activated, attracted), set bit 2 and upper nibble
        let low = self._padding[3] & 0x03;
        self._padding[3] = low | (if from_booster { 4 } else { 0 }) | (stat_plus_1 << 4);
    }

    #[inline(always)]
    pub fn clear_paradox(&mut self) {
        // Clear bits 2-7, preserve bits 0-1
        self._padding[3] &= 0x03;
    }

    #[inline(always)]
    pub fn zero(&mut self) { *self = Self::default(); }
}

impl BattleState {
    #[inline(always)]
    pub fn active_mon(&self, side: usize) -> &MonSlot {
        &self.sides[side].team[self.sides[side].active_index as usize]
    }

    #[inline(always)]
    pub fn active_mon_mut(&mut self, side: usize) -> &mut MonSlot {
        let idx = self.sides[side].active_index as usize;
        &mut self.sides[side].team[idx]
    }

    #[inline(always)]
    pub fn is_game_over(&self) -> bool { self.phase == PHASE_GAME_OVER }

    // -- Turn resumption helpers (mid-turn forced switch) --

    #[inline(always)]
    pub fn turn_subphase(&self) -> u8 { self._padding & 0x03 }

    #[inline(always)]
    pub fn second_mover_side(&self) -> usize { ((self._padding >> 2) & 1) as usize }

    #[inline(always)]
    pub fn pending_action(&self) -> u8 { self.field._padding }

    #[inline(always)]
    pub fn set_turn_resume(&mut self, subphase: u8, second_side: usize, action: u8) {
        self._padding = (subphase & 0x03) | (((second_side as u8) & 1) << 2);
        self.field._padding = action;
    }

    #[inline(always)]
    pub fn clear_turn_resume(&mut self) {
        self._padding = 0;
        self.field._padding = 0;
    }
}
