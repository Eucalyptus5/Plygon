//! Data bridge: the single adapter between the battle state layer and
//! the static data layer (`crate::data`).

pub use crate::data::base_stats::SpeciesData;
pub use crate::data::moves::{MoveData, MoveMeta, MoveCategory, MoveEffect};
pub use crate::data::items::{ItemData, ItemFlag};

// ── Species lookup ──────────────────────────────────────────────────

#[inline(always)]
pub fn species(id: u16) -> &'static SpeciesData {
    crate::data::base_stats::species(id as usize)
}

// ── Move lookups ────────────────────────────────────────────────────

#[inline(always)]
pub fn move_hot(id: u16) -> &'static MoveData {
    crate::data::moves::move_data(id as usize)
}

#[inline(always)]
pub fn move_cold(id: u16) -> &'static MoveMeta {
    crate::data::moves::move_meta(id as usize)
}

#[inline(always)]
pub fn move_base_pp(id: u16) -> u8 {
    move_cold(id).pp
}

// ── Item lookup ─────────────────────────────────────────────────────

#[inline(always)]
pub fn item(id: u16) -> &'static ItemData {
    crate::data::items::item(id as usize)
}

// ── Nature modifier ─────────────────────────────────────────────────

pub const fn nature_modifier(nature: u8, stat_index: usize) -> (u8, u8) {
    let boosted = (nature / 5) as usize;
    let reduced = (nature % 5) as usize;
    if boosted == reduced {
        (10, 10)
    } else if stat_index == boosted + 1 {
        (11, 10)
    } else if stat_index == reduced + 1 {
        (9, 10)
    } else {
        (10, 10)
    }
}

// ── Ability ID constants ────────────────────────────────────────────
// Grouped by where they're used in the engine.

// -- General / accessors / switch --
pub const ABILITY_NONE: u16          = 0;
pub const ABILITY_LEVITATE: u16      = 26;
pub const ABILITY_NATURAL_CURE: u16  = 30;
pub const ABILITY_REGENERATOR: u16   = 144;
pub const ABILITY_INTIMIDATE: u16    = 22;

// -- Weather setters --
pub const ABILITY_DRIZZLE: u16       = 2;
pub const ABILITY_DROUGHT: u16       = 70;
pub const ABILITY_SAND_STREAM: u16   = 45;
pub const ABILITY_SNOW_WARNING: u16  = 117;

// -- End-of-turn --
pub const ABILITY_SPEED_BOOST: u16   = 3;
pub const ABILITY_MOODY: u16         = 141;
pub const ABILITY_POISON_HEAL: u16   = 90;
pub const ABILITY_MAGIC_GUARD: u16   = 98;

// -- Damage calc: attacker stat mods --
pub const ABILITY_HUGE_POWER: u16    = 37;
pub const ABILITY_PURE_POWER: u16    = 74;
pub const ABILITY_HUSTLE: u16        = 55;
pub const ABILITY_GUTS: u16          = 62;
pub const ABILITY_SOLAR_POWER: u16   = 94;

// -- Damage calc: attacker damage mods --
pub const ABILITY_TECHNICIAN: u16    = 101;
pub const ABILITY_RECKLESS: u16      = 120;
pub const ABILITY_IRON_FIST: u16     = 89;
pub const ABILITY_SHEER_FORCE: u16   = 125;
pub const ABILITY_ANALYTIC: u16      = 148;
pub const ABILITY_TOUGH_CLAWS: u16   = 181;
pub const ABILITY_MEGA_LAUNCHER: u16 = 178;
pub const ABILITY_STRONG_JAW: u16    = 173;
pub const ABILITY_FLASH_FIRE: u16    = 18;
pub const ABILITY_UNBURDEN: u16      = 84;

// -- Damage calc: STAB / crit --
pub const ABILITY_ADAPTABILITY: u16  = 91;
pub const ABILITY_SNIPER: u16        = 97;
pub const ABILITY_SUPER_LUCK: u16    = 105;

// -- Damage calc: attacker type-pinch abilities --
pub const ABILITY_OVERGROW: u16      = 65;
pub const ABILITY_BLAZE: u16         = 66;
pub const ABILITY_TORRENT: u16       = 67;
pub const ABILITY_SWARM: u16         = 68;

// -- Damage calc: attacker effectiveness mods --
pub const ABILITY_TINTED_LENS: u16   = 110;

// -- Damage calc: defender stat mods --
pub const ABILITY_FUR_COAT: u16      = 169;
pub const ABILITY_ICE_SCALES: u16    = 246;

// -- Damage calc: defender damage reduction --
pub const ABILITY_MULTISCALE: u16    = 136;
pub const ABILITY_SHADOW_SHIELD: u16 = 231;
pub const ABILITY_FILTER: u16        = 111;
pub const ABILITY_SOLID_ROCK: u16    = 116;
pub const ABILITY_PRISM_ARMOR: u16   = 232;
pub const ABILITY_THICK_FAT: u16     = 47;
pub const ABILITY_FLUFFY: u16        = 218;
pub const ABILITY_HEATPROOF: u16     = 85;
pub const ABILITY_ICE_FACE: u16      = 248;

// -- Damage calc: defender immunities --
pub const ABILITY_DRY_SKIN: u16      = 87;
pub const ABILITY_WATER_ABSORB: u16  = 11;
pub const ABILITY_VOLT_ABSORB: u16   = 10;
pub const ABILITY_MOTOR_DRIVE: u16   = 78;
pub const ABILITY_STURDY: u16        = 5;

// -- Multi-hit --
pub const ABILITY_SKILL_LINK: u16    = 92;

// -- Turn executor: speed / accuracy / secondary --
pub const ABILITY_QUICK_FEET: u16    = 95;
pub const ABILITY_NO_GUARD: u16      = 99;
pub const ABILITY_COMPOUND_EYES: u16 = 14;
pub const ABILITY_VICTORY_STAR: u16  = 161;
pub const ABILITY_SERENE_GRACE: u16  = 32;
pub const ABILITY_SAND_VEIL: u16     = 8;
pub const ABILITY_SNOW_CLOAK: u16    = 81;

// ── Legacy item ID constants (kept for tests) ───────────────────────

pub const ITEM_NONE: u16 = 0;
