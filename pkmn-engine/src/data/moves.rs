use crate::data::types::Type;

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
    Hex        = 17,    // 2× if target has status
    Acrobatics = 18,    // 2× if attacker has no item
    RisingVoltage = 19, // 2× if Electric Terrain + target grounded
}

// ── Move effect enum ────────────────────────────────────────────────
// Replaces hardcoded move IDs in the executor.  Populated by the data
// generation pipeline.  MoveEffect::None means "no special dispatch."

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum MoveEffect {
    None         = 0,

    // -- Status move effects --
    Protect      = 1,   // Protect, Detect, King's Shield, Baneful Bunker
    StealthRock  = 2,
    Spikes       = 3,
    ToxicSpikes  = 4,
    StickyWeb    = 5,
    Defog        = 6,
    WillOWisp    = 7,
    ThunderWave  = 8,
    Toxic        = 9,
    Sleep        = 10,  // Spore, Sleep Powder, Hypnosis
    SwordsDance  = 11,
    NastyPlot    = 12,
    DragonDance  = 13,
    CalmMind     = 14,
    BulkUp       = 15,
    IronDefense  = 16,
    Agility      = 17,
    QuiverDance  = 18,
    ShellSmash   = 19,
    Coil         = 20,
    ShiftGear    = 21,
    Reflect      = 22,
    LightScreen  = 23,
    AuroraVeil   = 24,
    Tailwind     = 25,
    TrickRoom    = 26,
    Substitute   = 27,
    Wish         = 28,
    Taunt        = 29,
    LeechSeed    = 30,
    Encore       = 31,

    // -- Damaging move side effects --
    ForceSwitch  = 32,  // U-turn, Volt Switch, Flip Turn
    RapidSpin    = 33,  // Physical + clears hazards + Speed boost

    // -- Move-specific damage modifiers (Step 1) --
    KnockOff       = 34,  // 1.5× if target has removable item + remove item post-damage
    FreezeDry      = 35,  // Override: super effective vs Water
    ExpandingForce = 36,  // 1.5× chain in Psychic Terrain (source grounded)
    Psyblade       = 37,  // 1.5× chain in Electric Terrain
    SolarBeam      = 38,  // 0.5× power in rain/sand/snow; charge skip in sun (Step 3)
    WeatherAccRain = 39,  // Thunder/Hurricane: 100% in rain, 50% in sun
    WeatherAccSnow = 40,  // Blizzard: 100% in snow/hail

    // -- Pivot moves (Step 2) --
    PartingShot  = 41,  // -1 Atk -1 SpA on opponent, then self-switch
    BatonPass    = 42,  // Self-switch preserving boosts + select volatiles

    // -- Charge moves (Step 3) --
    // Semi-invulnerable charge moves (dodge most attacks during charge turn)
    ChargeFly       = 43,  // Fly, Bounce: airborne
    ChargeDig       = 44,  // Dig: underground
    ChargeDive      = 45,  // Dive: underwater
    ChargePhantom   = 46,  // Phantom Force, Shadow Force: vanished + bypasses Protect
    // Non-semi-invulnerable charge moves
    ChargeSkyAttack = 47,  // Sky Attack: plain charge
    ChargeSkullBash = 48,  // Skull Bash: +1 Def on charge turn
    ChargeMeteorBeam= 49,  // Meteor Beam: +1 SpA on charge turn
    ChargeElectroShot=50,  // Electro Shot: +1 SpA on charge, skip in rain
    ChargeGeomancy  = 51,  // Geomancy: status move, +2 SpA/SpD/Spe on execute
    // SolarBeam (= 38) also has charge logic: skip in sun.

    // Recovery (Recover, Roost, etc.) is detected by MoveFlags::HEAL.
    // Recharge (Hyper Beam, etc.) is detected by MoveFlags::RECHARGE.

    // -- Locked/thrashing moves (Step 4) --
    Thrash       = 52,  // Outrage, Petal Dance, Thrash, Raging Fury: 2-3 turns locked, confuse on end
}

// Flags (16 bits)

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
    pub const BULLET:      u16 = 1 << 15;
}

// Hot path struct: everything the damage calc and executor touch

/// 16 bytes with #[repr(C)], zero padding.
/// Field order: u16 first (strictest alignment), then all u8/i8 fields.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct MoveData {
    pub flags:            u16,
    pub base_power:       u8,
    pub accuracy:         u8,
    pub category:         MoveCategory,
    pub move_type:        Type,
    pub var_power:        VarPower,
    pub crit_ratio:       u8,
    pub drain:            i8,
    pub priority:         i8,
    pub multihit_lo:      u8,
    pub multihit_hi:      u8,
    pub secondary_chance: u8,
    pub secondary_stat:   i8,
    /// What the executor dispatches on (status moves, force-switch, etc.).
    pub effect:           MoveEffect,
    /// Status inflicted as a secondary effect (STATUS_BURN, etc.), or 0.
    /// Populated by data generation.  When 0, the executor falls back to
    /// a type-based heuristic (Fire→burn, Electric→paralysis, etc.).
    pub secondary_status: u8,
}

const _: () = assert!(core::mem::size_of::<MoveData>() == 16);

/// Cold path: data NOT needed during damage calc rollouts.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct MoveMeta {
    pub pp:     u8,
    pub target: MoveTarget,
}

const _: () = assert!(core::mem::size_of::<MoveMeta>() == 2);

// Accessors
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

// Variable base power resolvers
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
