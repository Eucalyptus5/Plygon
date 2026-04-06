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
    SpitUp        = 20, // 100 BP per stockpile layer
    Escalating    = 21, // base_power * hit_number (Triple Kick, Triple Axel)
    Brine         = 22, // 2× if target HP ≤ 50%
    Payback       = 23, // 2× if user moves after target this turn
    Avalanche     = 24, // 2× if user was hit by target this turn (Avalanche, Revenge)
    FuryCutter    = 25, // 40 BP, doubles on each consecutive successful hit (cap 160)
}

// Encodes guaranteed self-stat changes, crash damage, and other effects
// that apply to the attacker after a damaging move lands.
// Stored in MoveData.self_effect (byte 15).

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum SelfEffect {
    None            = 0,
    // Self-stat drops on damaging moves
    DefSpDDown1     = 1,   // Close Combat, Armor Cannon, Dragon Ascent
    AtkDefDown1     = 2,   // Superpower
    DefSpDSpeDown1  = 3,   // V-Create
    SpADown2        = 4,   // Draco Meteor, Leaf Storm, Overheat, Fleur Cannon, Psycho Boost
    SpeDown1        = 5,   // Hammer Arm, Ice Hammer
    AtkDown1        = 6,
    SpDDown1        = 7,
    DefDown1        = 8,
    SpADown1        = 9,
    SpeDown2        = 10,  // Spin Out
    // Self-stat boosts on damaging moves
    AtkUp1          = 11,
    SpeUp1          = 12,
    DefUp1          = 13,
    SpAUp1          = 14,
    // Combined self-effects
    DefDown1SpeUp1  = 21,  // Scale Shot: -1 Def, +1 Spe after all hits
    // Special self-effects
    CrashDamage     = 15,  // High Jump Kick, Jump Kick, Axe Kick, Supercell Slam — 50% max HP on miss
    SelfSwitch      = 16,  // U-turn, Volt Switch, Flip Turn (placeholder; uses MoveEffect::ForceSwitch)
    BatonPass       = 17,  // Baton Pass (placeholder; uses MoveEffect::BatonPass)
    PartingShot     = 18,  // Parting Shot (placeholder; uses MoveEffect::PartingShot)
    Heal50          = 19,  // Recover, Slack Off, Roost, etc. (placeholder; uses MoveFlags::HEAL)
    ThawSelf        = 20,  // Scald, Steam Eruption (non-Fire moves that thaw user)

    // -- Opponent-target stat changes (dispatched by MoveEffect::OpponentStatDrop) --
    // Drops (apply to def_side via try_opponent_stat_drop)
    OppAtkDown1     = 22,  // Growl, Play Nice, Baby-Doll Eyes
    OppAtkDown2     = 23,  // Charm, Feather Dance
    OppDefDown1     = 24,  // Tail Whip, Leer
    OppDefDown2     = 25,  // Screech
    OppSpADown1     = 26,  // Confide
    OppSpADown2     = 27,  // Eerie Impulse
    OppSpDDown2     = 28,  // Fake Tears, Metal Sound
    OppSpeDown1     = 29,  // Tar Shot (spe drop component)
    OppSpeDown2     = 30,  // String Shot, Cotton Spore, Scary Face
    OppAccDown1     = 31,  // Sand Attack, Smokescreen
    OppEvaDown2     = 32,  // Sweet Scent
    OppAtkDefDown1  = 33,  // Tickle
    OppAtkSpADown1  = 34,  // Noble Roar, Tearful Look
    OppAtkSpADown2  = 35,  // Memento (user faints separately via MoveEffect::Memento)
    // Boosts applied to opponent target (apply_boost on def_side)
    OppAtkUp2       = 36,  // Swagger (+2 atk, also confuses via MoveEffect::Confuse)
    OppSpAUp1       = 37,  // Flatter (+1 spa, also confuses via MoveEffect::Confuse)
    OppAtkSpAUp2    = 38,  // Decorate (+2 atk, +2 spa on target)
    OppAtkUp2DefDown2 = 39,// Spicy Extract (+2 atk, -2 def on target)

    // -- Ally-target / self-target boosts (dispatched by MoveEffect::AllyBoost) --
    // In singles these resolve to atk_side (the user).
    AllyAtkUp1      = 40,  // Howl
    AllySpDUp1      = 41,  // Aromatic Mist
    AllyAtkDefUp1   = 42,  // Coaching
}

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

    // -- Stat-override moves (Step 2: Phase 2) --
    FoulPlay     = 53,  // Use target's Atk instead of attacker's
    BodyPress    = 54,  // Use attacker's Def as Atk
    Photon       = 55,  // Psyshock/Psystrike/Secret Sword: SpA vs Def
    WeatherBall  = 56,  // Type + 2× power by active weather
    TerrainPulse = 57,  // Type + 2× power by active terrain (grounded)
    GrassyGlide  = 58,  // +1 priority in Grassy Terrain (grounded)

    // -- Locked/thrashing moves (Step 4) --
    Thrash       = 52,  // Outrage, Petal Dance, Thrash, Raging Fury: 2-3 turns locked, confuse on end

    // -- Phase 7: Status move effects --
    BellyDrum    = 59,  // -50% HP, +6 Atk (fail if <50% HP)
    PainSplit    = 60,  // Average both mons' current HP
    Endeavor     = 61,  // Set target HP = user HP (fail if target HP ≤ user HP)
    SuperFang    = 62,  // Halve target's current HP
    SeismicToss  = 63,  // Deal damage equal to user's level (100 at L100)
    Counter      = 64,  // Return 2× physical damage taken this turn
    MirrorCoat   = 65,  // Return 2× special damage taken this turn
    MetalBurst   = 66,  // Return 1.5× last damage taken this turn
    FinalGambit  = 67,  // Deal user's current HP as damage, user faints
    PerishSong   = 68,  // Set 3-turn perish counter on both active mons
    DestinyBond  = 69,  // If user faints before next move, KO attacker
    Trick        = 70,  // Swap user's item with target's item
    Disable      = 71,  // Prevent target's last-used move for 4 turns
    Torment      = 72,  // Cannot use same move consecutively
    HealingWish  = 73,  // User faints, next switch-in fully heals
    LunarDance   = 74,  // User faints, next switch-in fully heals + PP
    CourtChange  = 75,  // Swap side conditions between sides
    Roost        = 76,  // Heal 50% HP, lose Flying type for rest of turn
    SaltCure     = 77,  // 1/4 EOT if Water/Steel, 1/8 otherwise
    Gravity      = 78,  // Set Gravity for 5 turns
    Safeguard    = 79,  // Prevent status from opponents for 5 turns
    Mist         = 80,  // Prevent opponent-caused stat drops for 5 turns
    LuckyChant   = 81,  // Prevent crits for 5 turns
    Whirlwind    = 82,  // Force random switch (phazing)
    Haze         = 83,  // Reset all stat changes
    Yawn         = 84,  // Sleep target next turn
    Confuse      = 85,  // Confuse target (Confuse Ray, Sweet Kiss)
    MagnetRise   = 86,  // Levitate for 5 turns
    FocusEnergy  = 87,  // +2 crit stage
    Imprison     = 88,  // Block opponent's shared moves
    Aromatherapy = 89,  // Cure team status
    Minimize     = 90,  // +2 Evasion, set VOL_MINIMIZE
    Stockpile    = 91,  // +1 Def/SpD, stockpile count++
    SpitUp       = 92,  // Deal 100/200/300 damage by stockpile, reset
    Swallow      = 93,  // Heal 25/50/100% by stockpile, reset
    TeraBlast    = 94,  // Physical or Special based on higher stat, Normal → Tera type
    PartialTrap  = 95,  // Bind, Wrap, Fire Spin, etc.: trap + EOT 1/8 damage
    MagicRoom    = 96,  // Suppress all held item effects for 5 turns
    WonderRoom   = 97,  // Swap Def and SpD for damage calc for 5 turns

    // -- Phase 3 Task 07: Remaining SelfEffects --
    ClangorousSoul = 98, // -33% HP, +1 all stats (fail if HP ≤ 33% or maxhp==1)
    Curse          = 99, // Non-Ghost: +1 Atk/Def, -1 Spe; Ghost: -50% HP, curse volatile on target
    NoRetreat      = 100, // +1 all stats, trap self (fail if already used)
    TidyUp         = 101, // +1 Atk/Spe, clear hazards + subs from both sides
    SetTerrain     = 102, // Electric/Grassy/Psychic/Misty Terrain
    AquaRing       = 103, // Set VOL_AQUA_RING on user, heal 1/16 EOT
    Ingrain        = 104, // Set VOL_INGRAIN on user, heal 1/16 EOT + grounded + no switch

    // -- Round 22: Opponent-target stat-modifying status moves --
    OpponentStatDrop = 105, // Dispatch via self_effect (Opp* variants) on def_side
    AllyBoost        = 106, // Dispatch via self_effect (Ally* variants) on atk_side
    Memento          = 107, // -2 atk/-2 spa on target, then user faints
    ToxicThread      = 108, // Inflict poison + -1 spe on target
    Charge           = 109, // +1 SpD, set charge bit (2x power for next Electric move)
    PsychUp          = 110, // Copy target's stat boosts (and crit-up volatiles) to user
    HoneClaws        = 111, // +1 Atk, +1 Accuracy on user

    // -- Call* family --
    SleepTalk        = 112, // Pick a random move from user's moveset, dispatch it
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
    /// Packed multi-hit: lo = bits[3:0], hi = bits[7:4].
    /// 0 means single-hit.
    pub multihit:         u8,
    pub secondary_chance: u8,
    pub secondary_stat:   i8,
    /// What the executor dispatches on (status moves, force-switch, etc.).
    pub effect:           MoveEffect,
    /// Status inflicted as a secondary effect (STATUS_BURN, etc.), or 0.
    /// Populated by data generation.  When 0, the executor falls back to
    /// a type-based heuristic (Fire→burn, Electric→paralysis, etc.).
    pub secondary_status: u8,
    /// Self-effect applied to the attacker after damage (stat drops, crash, etc.).
    pub self_effect:      SelfEffect,
}

impl MoveData {
    /// Low bound of multi-hit range (0 = single-hit move).
    #[inline(always)]
    pub fn multihit_lo(&self) -> u8 { self.multihit & 0xF }
    /// High bound of multi-hit range.
    #[inline(always)]
    pub fn multihit_hi(&self) -> u8 { self.multihit >> 4 }
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
    let bp = (25u32 * target_speed as u32) / user_speed as u32 + 1;
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
    // Showdown: ratio = max(floor(hp * 48 / maxhp), 1)
    let ratio = ((48u32 * current_hp as u32) / max_hp as u32).max(1);
    match ratio {
        0..=1   => 200,
        2..=4   => 150,
        5..=9   => 100,
        10..=16 => 80,
        17..=32 => 40,
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
