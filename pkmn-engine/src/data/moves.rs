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
    Normal,
    Self_,
    AllAdjacentFoes,
    AllAdjacent,
    AllyOrSelf,
    Any,
    FoeSide,
    AllySide,
    All,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum VarPower {
    None       = 0,
    Weight     = 1,
    GyroBall   = 2,
    Facade     = 3,
    Eruption   = 4,
    Flail      = 5,
    HeavySlam  = 6,
    Punishment = 7,
    StoredPower= 8,
    ElectroBall= 9,
    Return     = 10,
    Frustration= 11,
    Magnitude  = 12,
    Present    = 13,
    TrumpCard  = 14,
    NaturalGift= 15,
    TechnoBlast= 16,
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
/// Field order: u16 first (strictest alignment), then all u8/i8 fields.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct MoveData {
    pub flags:       u16,
    pub base_power:  u8,
    pub accuracy:    u8,
    pub category:    MoveCategory,
    pub move_type:   Type,
    pub var_power:   VarPower,
    pub crit_ratio:  u8,
    pub drain:       i8,
    pub priority:    i8,
    pub multihit_lo: u8,
    pub multihit_hi: u8,
    pub secondary_chance: u8,
    pub secondary_stat:   i8,
}

const _: () = assert!(core::mem::size_of::<MoveData>() == 14);

/// Cold path: data NOT needed during damage calc rollouts.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct MoveMeta {
    pub pp:     u8,
    pub target: MoveTarget,
}

const _: () = assert!(core::mem::size_of::<MoveMeta>() == 2);

// ─── Accessors ────────────────────────────────────────────────────────────

/// Hot-path move lookup. O(1).
#[inline(always)]
pub fn move_data(id: usize) -> &'static MoveData {
    use crate::data::gen_moves::GEN_MOVES;
    debug_assert!(id < GEN_MOVES.len(), "move id {} out of range", id);
    unsafe { GEN_MOVES.get_unchecked(id) }
}

/// Cold-path move lookup. O(1).
#[inline(always)]
pub fn move_meta(id: usize) -> &'static MoveMeta {
    use crate::data::gen_moves::GEN_MOVE_META;
    debug_assert!(id < GEN_MOVE_META.len(), "move id {} out of range", id);
    unsafe { GEN_MOVE_META.get_unchecked(id) }
}

// ─── Variable base power resolvers ────────────────────────────────────────

const WEIGHT_BP: [(u16, u8); 6] = [
    (2000, 120),
    (1000, 100),
    (500,   80),
    (250,   60),
    (100,   40),
    (0,     20),
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

#[inline]
pub fn heavy_slam_bp(atk_weight: u16, def_weight: u16) -> u8 {
    if def_weight == 0 { return 120; }
    if def_weight * 5 <= atk_weight { 120 }
    else if def_weight * 4 <= atk_weight { 100 }
    else if def_weight * 3 <= atk_weight { 80 }
    else if def_weight * 2 <= atk_weight { 60 }
    else { 40 }
}

#[inline]
pub fn gyro_ball_bp(user_speed: u16, target_speed: u16) -> u8 {
    if user_speed == 0 { return 150; }
    let bp = (25u32 * target_speed as u32) / user_speed as u32;
    bp.min(150) as u8
}

#[inline]
pub fn eruption_bp(current_hp: u16, max_hp: u16) -> u8 {
    if max_hp == 0 { return 1; }
    let bp = (150u32 * current_hp as u32) / max_hp as u32;
    bp.max(1).min(150) as u8
}

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

#[inline]
pub fn stored_power_bp(positive_boosts: u8) -> u8 {
    (20u16 + 20u16 * positive_boosts as u16).min(255) as u8
}

#[inline]
pub fn punishment_bp(target_positive_boosts: u8) -> u8 {
    (60u16 + 20u16 * target_positive_boosts as u16).min(200) as u8
}
