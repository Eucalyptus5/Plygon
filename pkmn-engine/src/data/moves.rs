// src/data/moves.rs
//
// Struct definitions, enums, flag constants, variable base power resolvers,
// and accessor functions. The actual data lives in generated/gen_moves.rs.

use crate::data::types::Type;

// ─── Enums ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum MoveCategory {
    Physical = 0,
    Special  = 1,
    Status   = 2,
}

#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum MoveTarget {
    Normal,          // single adjacent foe
    Self_,           // user only
    AllAdjacentFoes, // hits both foes in doubles (Earthquake, Surf)
    AllAdjacent,     // hits foes AND ally in doubles
    AllyOrSelf,
    Any,             // can target non-adjacent
    FoeSide,         // Stealth Rock, Spikes
    AllySide,        // Reflect, Light Screen
    All,             // Weather, Trick Room
}

/// Variable base power tag. When base_power == 0 in MoveData,
/// the engine calls a resolver function using this discriminant + runtime state.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum VarPower {
    None       = 0,
    Weight     = 1,  // Low Kick, Grass Knot (defender weight → BP table)
    GyroBall   = 2,  // 25 × target_speed / user_speed, capped at 150
    Facade     = 3,  // 70 normally, 140 if user burned/poisoned/paralyzed
    Eruption   = 4,  // 150 × current_hp / max_hp (also Water Spout)
    Flail      = 5,  // inverse HP scale (also Reversal)
    HeavySlam  = 6,  // weight ratio attacker/defender (also Heat Crash)
    Punishment = 7,  // 60 + 20 per positive stat stage on target
    StoredPower= 8,  // 20 + 20 per positive stat stage on user
    ElectroBall= 9,  // speed ratio user/target
    Return     = 10, // 102 × happiness / 255
    Frustration= 11, // 102 × (255 - happiness) / 255
    Magnitude  = 12, // random power tiers
    Present    = 13, // random damage or heal
    TrumpCard  = 14, // depends on remaining PP
    NaturalGift= 15, // type+power from held Berry
    TechnoBlast= 16, // type from Drive item
}

// ─── Flags (u16 — we only need 15 bits) ───────────────────────────────────

#[allow(non_snake_case)]
pub mod MoveFlags {
    pub const CONTACT:     u16 = 1 << 0;
    pub const SOUND:       u16 = 1 << 1;
    pub const PUNCH:       u16 = 1 << 2;
    pub const PULSE:       u16 = 1 << 3;
    pub const BITE:        u16 = 1 << 4;
    pub const POWDER:      u16 = 1 << 5;
    pub const DANCE:       u16 = 1 << 6;
    pub const WIND:        u16 = 1 << 7;
    pub const SLICE:       u16 = 1 << 8;
    pub const PROTECT:     u16 = 1 << 9;
    pub const REFLECTABLE: u16 = 1 << 10;
    pub const RECHARGE:    u16 = 1 << 11;
    pub const CHARGE:      u16 = 1 << 12;
    pub const HEAL:        u16 = 1 << 13;
    pub const BYPASSSUB:   u16 = 1 << 14;
}

// ─── Hot path struct: everything the damage calc touches ──────────────────

/// 14 bytes with #[repr(C)], zero padding.
///
/// Field order: u16 first (strictest alignment), then all u8/i8 fields.
/// This struct contains ONLY data needed during damage calculation.
/// PP, target, etc. live in MoveMeta (cold path).
#[derive(Copy, Clone)]
#[repr(C)]
pub struct MoveData {
    pub flags:       u16,          // MoveFlags bitmask
    pub base_power:  u8,           // 0 = variable, see var_power
    pub accuracy:    u8,           // 0 = never misses
    pub category:    MoveCategory, // u8 enum
    pub move_type:   Type,         // u8 enum
    pub var_power:   VarPower,     // u8 enum
    pub crit_ratio:  u8,           // 0=normal, 1=high crit, 2=guaranteed
    pub drain:       i8,           // +50 = drain 50%, -33 = 33% recoil
    pub priority:    i8,           // -7..+5
    pub multihit_lo: u8,           // min hits (1 for single-hit moves)
    pub multihit_hi: u8,           // max hits (1 for single-hit, 5 for Bullet Seed)
    pub secondary_chance: u8,      // 0 = none, else percent
    pub secondary_stat:   i8,      // stat direction (positive = raise, negative = drop)
}

const _: () = assert!(core::mem::size_of::<MoveData>() == 14);

/// Cold path: data NOT needed during damage calc rollouts.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct MoveMeta {
    pub pp:     u8,
    pub target: MoveTarget, // u8 enum
}

const _: () = assert!(core::mem::size_of::<MoveMeta>() == 2);

// ─── Accessors ────────────────────────────────────────────────────────────

/// Hot-path move lookup. O(1), bounds-checked in debug, unchecked in release.
#[inline(always)]
pub fn move_data(id: usize) -> &'static MoveData {
    use crate::data::gen_moves::GEN_MOVES;
    debug_assert!(id < GEN_MOVES.len(), "move id {} out of range", id);
    unsafe { GEN_MOVES.get_unchecked(id) }
}

/// Cold-path move lookup (PP, target).
#[inline(always)]
pub fn move_meta(id: usize) -> &'static MoveMeta {
    use crate::data::gen_moves::GEN_MOVE_META;
    debug_assert!(id < GEN_MOVE_META.len(), "move id {} out of range", id);
    unsafe { GEN_MOVE_META.get_unchecked(id) }
}

// ─── Variable base power resolvers ────────────────────────────────────────
// All integer math. All #[inline]. Pass raw values from SpeciesData / game state.

/// Weight-to-BP table for Low Kick and Grass Knot.
/// Thresholds in tenths-of-kg (same unit as SpeciesData.weight).
const WEIGHT_BP: [(u16, u8); 6] = [
    (2000, 120), // >= 200.0 kg
    (1000, 100), // >= 100.0 kg
    (500,   80), // >=  50.0 kg
    (250,   60), // >=  25.0 kg
    (100,   40), // >=  10.0 kg
    (0,     20), // <   10.0 kg
];

#[inline]
pub fn weight_based_bp(defender_weight: u16) -> u8 {
    for &(threshold, bp) in &WEIGHT_BP {
        if defender_weight >= threshold {
            return bp;
        }
    }
    20
}

/// Heavy Slam / Heat Crash: BP based on attacker/defender weight ratio.
#[inline]
pub fn heavy_slam_bp(atk_weight: u16, def_weight: u16) -> u8 {
    if def_weight == 0 { return 120; }
    if def_weight * 5 <= atk_weight { 120 }
    else if def_weight * 4 <= atk_weight { 100 }
    else if def_weight * 3 <= atk_weight { 80 }
    else if def_weight * 2 <= atk_weight { 60 }
    else { 40 }
}

/// Gyro Ball: floor(25 × target_speed / user_speed), capped at 150.
#[inline]
pub fn gyro_ball_bp(user_speed: u16, target_speed: u16) -> u8 {
    if user_speed == 0 { return 150; }
    let bp = (25u32 * target_speed as u32) / user_speed as u32;
    bp.min(150) as u8
}

/// Eruption / Water Spout: floor(150 × current_hp / max_hp), min 1.
#[inline]
pub fn eruption_bp(current_hp: u16, max_hp: u16) -> u8 {
    if max_hp == 0 { return 1; }
    let bp = (150u32 * current_hp as u32) / max_hp as u32;
    bp.max(1).min(150) as u8
}

/// Flail / Reversal: BP tiers based on HP fraction remaining.
#[inline]
pub fn flail_bp(current_hp: u16, max_hp: u16) -> u8 {
    if max_hp == 0 { return 200; }
    let ratio = (48u32 * current_hp as u32) / max_hp as u32;
    match ratio {
        0..=1   => 200,
        2..=5   => 150,
        6..=12  => 100,
        13..=21 => 80,
        22..=32 => 40,
        _       => 20,
    }
}

/// Electro Ball: BP based on user_speed / target_speed ratio.
#[inline]
pub fn electro_ball_bp(user_speed: u16, target_speed: u16) -> u8 {
    if target_speed == 0 { return 150; }
    let ratio = user_speed / target_speed;
    match ratio {
        0 => 40,
        1 => 60,
        2 => 80,
        3 => 120,
        _ => 150,
    }
}

/// Stored Power: 20 + 20 per positive stat boost on user.
#[inline]
pub fn stored_power_bp(positive_boosts: u8) -> u8 {
    (20u16 + 20u16 * positive_boosts as u16).min(255) as u8
}

/// Punishment: 60 + 20 per positive stat boost on target, capped at 200.
#[inline]
pub fn punishment_bp(target_positive_boosts: u8) -> u8 {
    (60u16 + 20u16 * target_positive_boosts as u16).min(200) as u8
}
