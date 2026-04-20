//! Data bridge: the single adapter between the battle state layer and
//! the static data layer (`crate::data`).

pub use crate::data::base_stats::SpeciesData;
pub use crate::data::moves::{MoveData, MoveMeta, MoveCategory, MoveEffect, SelfEffect};
pub use crate::data::items::{ItemData, ItemFlag};

#[inline(always)]
pub fn species(id: u16) -> &'static SpeciesData {
    crate::data::base_stats::species(id as usize)
}

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

#[inline(always)]
pub fn item(id: u16) -> &'static ItemData {
    crate::data::items::item(id as usize)
}

/// Arceus/Silvally forme produced by a held Plate/Memory. 0 = no forme.
#[inline(always)]
pub fn item_forme(item_id: u16) -> u16 {
    use crate::data::gen_items::ITEM_FORME;
    let idx = item_id as usize;
    if idx < ITEM_FORME.len() { ITEM_FORME[idx] } else { 0 }
}

/// Base species IDs (positive `num` values from `pokemon-showdown/data/pokedex.ts`
/// entries carrying `gender: "N"`). Sorted ascending for binary_search. Formes
/// inherit via `FORME_TO_BASE` (see `is_genderless_species`).
const GENDERLESS_BASE_IDS: &[u16] = &[
    81, 82, 100, 101, 120, 121, 132, 137, 144, 145,
    146, 150, 151, 201, 233, 243, 244, 245, 249, 250,
    251, 292, 337, 338, 343, 344, 374, 375, 376, 377,
    378, 379, 382, 383, 384, 385, 386, 436, 437, 462,
    474, 479, 480, 481, 482, 483, 484, 486, 487, 489,
    490, 491, 492, 493, 494, 599, 600, 601, 615, 622,
    623, 638, 639, 640, 643, 644, 646, 647, 648, 649,
    703, 716, 717, 718, 719, 720, 721, 772, 773, 774,
    781, 785, 786, 787, 788, 789, 790, 791, 792, 793,
    794, 795, 796, 797, 798, 799, 800, 801, 802, 803,
    804, 805, 806, 807, 808, 809, 854, 855, 870, 880,
    881, 882, 883, 888, 889, 890, 893, 894, 895, 896,
    897, 898, 924, 925, 984, 985, 986, 987, 988, 989,
    990, 991, 992, 993, 994, 995, 999, 1000, 1001, 1002,
    1003, 1004, 1005, 1006, 1007, 1008, 1009, 1010, 1012, 1013,
    1020, 1021, 1022, 1023, 1025,
];

/// True when this species (or its base, for forme IDs ≥ `FORME_OFFSET`) carries
/// `gender: "N"` in Showdown's pokedex. Read by `team_builder::build_mon` to set
/// `MON_FLAG_GENDERLESS`; downstream consumers (Rivalry, Cute Charm, attract, etc.)
/// branch on the flag without re-querying species data.
#[inline]
pub fn is_genderless_species(id: u16) -> bool {
    use crate::data::{FORME_OFFSET, FORME_TO_BASE};
    let base = if (id as usize) >= FORME_OFFSET {
        FORME_TO_BASE[id as usize]
    } else {
        id
    };
    GENDERLESS_BASE_IDS.binary_search(&base).is_ok()
}

// Nature index layout: boosted = nature/5, reduced = nature%5, both map to stat indices 1-5
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

// -- General / accessors / switch --
pub const ABILITY_NONE: u16          = 0;
pub const ABILITY_LEVITATE: u16      = 26;
pub const ABILITY_NATURAL_CURE: u16  = 30;
pub const ABILITY_REGENERATOR: u16   = 144;
pub const ABILITY_INTIMIDATE: u16    = 22;
pub const ABILITY_INFILTRATOR: u16   = 151;
pub const ABILITY_ROCK_HEAD: u16     = 69;

// -- Trapping abilities (legal_moves switch gating) --
pub const ABILITY_SHADOW_TAG: u16    = 23;
pub const ABILITY_MAGNET_PULL: u16   = 42;
pub const ABILITY_ARENA_TRAP: u16    = 71;

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
pub const ABILITY_BAD_DREAMS: u16    = 123;
pub const ABILITY_HYDRATION: u16     = 93;
pub const ABILITY_SHED_SKIN: u16     = 61;
pub const ABILITY_RAIN_DISH: u16     = 44;
pub const ABILITY_ICE_BODY: u16      = 115;
pub const ABILITY_HARVEST: u16       = 139;

// -- Paradox abilities --
pub const ABILITY_PROTOSYNTHESIS: u16 = 281;
pub const ABILITY_QUARK_DRIVE: u16   = 282;

// -- Damage calc: attacker stat mods --
pub const ABILITY_HUGE_POWER: u16    = 37;
pub const ABILITY_PURE_POWER: u16    = 74;
pub const ABILITY_HUSTLE: u16        = 55;
pub const ABILITY_GUTS: u16          = 62;
pub const ABILITY_SOLAR_POWER: u16   = 94;
pub const ABILITY_GORILLA_TACTICS: u16 = 255;
pub const ABILITY_STAKEOUT: u16      = 198;
pub const ABILITY_SLOW_START: u16    = 112;
pub const ABILITY_DEFEATIST: u16     = 129;
pub const ABILITY_FLOWER_GIFT: u16   = 122;
pub const ABILITY_WATER_BUBBLE: u16  = 199;
pub const ABILITY_DRAGONS_MAW: u16   = 263;
pub const ABILITY_TRANSISTOR: u16    = 262;
pub const ABILITY_STEELWORKER: u16   = 200;
pub const ABILITY_ROCKY_PAYLOAD: u16 = 276;

// -- Damage calc: attacker damage mods --
pub const ABILITY_TECHNICIAN: u16    = 101;
pub const ABILITY_RECKLESS: u16      = 120;
pub const ABILITY_IRON_FIST: u16     = 89;
pub const ABILITY_RIVALRY: u16       = 79;
pub const ABILITY_SHEER_FORCE: u16   = 125;
pub const ABILITY_ANALYTIC: u16      = 148;
pub const ABILITY_TOUGH_CLAWS: u16   = 181;
pub const ABILITY_MEGA_LAUNCHER: u16 = 178;
pub const ABILITY_STRONG_JAW: u16    = 173;
pub const ABILITY_FLASH_FIRE: u16    = 18;
pub const ABILITY_UNBURDEN: u16      = 84;
pub const ABILITY_STICKY_HOLD: u16   = 60;
pub const ABILITY_SHARPNESS: u16     = 292;
pub const ABILITY_PUNK_ROCK: u16     = 244;
pub const ABILITY_SAND_FORCE: u16    = 159;
pub const ABILITY_SUPREME_OVERLORD: u16 = 293;

// -- Damage calc: STAB / crit --
pub const ABILITY_ADAPTABILITY: u16  = 91;
pub const ABILITY_SNIPER: u16        = 97;
pub const ABILITY_SUPER_LUCK: u16    = 105;
pub const ABILITY_BATTLE_ARMOR: u16  = 4;
pub const ABILITY_SHELL_ARMOR: u16   = 75;

// -- Damage calc: attacker type-pinch abilities --
pub const ABILITY_OVERGROW: u16      = 65;
pub const ABILITY_BLAZE: u16         = 66;
pub const ABILITY_TORRENT: u16       = 67;
pub const ABILITY_SWARM: u16         = 68;

// -- Damage calc: attacker effectiveness mods --
pub const ABILITY_TINTED_LENS: u16   = 110;
pub const ABILITY_NEUROFORCE: u16    = 233;

// -- Damage calc: defender stat mods --
pub const ABILITY_FUR_COAT: u16      = 169;
pub const ABILITY_ICE_SCALES: u16    = 246;
pub const ABILITY_MARVEL_SCALE: u16  = 63;
pub const ABILITY_GRASS_PELT: u16    = 179;

// -- Pre-move hooks (attacker) --
pub const ABILITY_PROTEAN: u16       = 168;
pub const ABILITY_LIBERO: u16        = 236;
pub const ABILITY_STANCE_CHANGE: u16 = 176;

// -- Forme change: HP-triggered --
// Darmanitan: <=50% HP -> Zen forme
pub const ABILITY_ZEN_MODE: u16      = 161;

// -- Pre-damage hooks (defender) --
pub const ABILITY_DISGUISE: u16      = 209;

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
pub const ABILITY_LIGHTNING_ROD: u16 = 31;
pub const ABILITY_STORM_DRAIN: u16   = 114;
pub const ABILITY_SAP_SIPPER: u16    = 157;
pub const ABILITY_BULLETPROOF: u16   = 171;
pub const ABILITY_SOUNDPROOF: u16    = 43;
pub const ABILITY_OVERCOAT: u16      = 142;
pub const ABILITY_EARTH_EATER: u16   = 297;
pub const ABILITY_WIND_RIDER: u16    = 274;
pub const ABILITY_GOOD_AS_GOLD: u16  = 283;
pub const ABILITY_DAZZLING: u16      = 219;
pub const ABILITY_QUEENLY_MAJESTY: u16 = 214;
pub const ABILITY_ARMOR_TAIL: u16    = 296;

// -- After-damage hooks: defender --
pub const ABILITY_ROUGH_SKIN: u16    = 24;
pub const ABILITY_IRON_BARBS: u16    = 160;
pub const ABILITY_WEAK_ARMOR: u16    = 133;
pub const ABILITY_JUSTIFIED: u16     = 154;
pub const ABILITY_STAMINA: u16       = 192;
pub const ABILITY_ANGER_POINT: u16   = 83;
pub const ABILITY_COLOR_CHANGE: u16  = 16;
pub const ABILITY_FLAME_BODY: u16    = 49;
pub const ABILITY_STATIC: u16        = 9;
pub const ABILITY_POISON_POINT: u16  = 38;
pub const ABILITY_EFFECT_SPORE: u16  = 27;
pub const ABILITY_AFTERMATH: u16     = 106;
pub const ABILITY_CUTE_CHARM: u16    = 56;
pub const ABILITY_COTTON_DOWN: u16   = 238;
pub const ABILITY_RATTLED: u16       = 155;
pub const ABILITY_STEAM_ENGINE: u16  = 243;
pub const ABILITY_BERSERK: u16       = 201;
pub const ABILITY_TOXIC_DEBRIS: u16  = 295;
pub const ABILITY_INNARDS_OUT: u16   = 215;
pub const ABILITY_MUMMY: u16         = 152;
pub const ABILITY_LINGERING_AROMA: u16 = 268;
pub const ABILITY_PERISH_BODY: u16   = 253;
pub const ABILITY_SEED_SOWER: u16    = 269;
pub const ABILITY_ELECTROMORPHOSIS: u16 = 280;
pub const ABILITY_WIND_POWER: u16    = 277;
pub const ABILITY_THERMAL_EXCHANGE: u16 = 270;
pub const ABILITY_ANGER_SHELL: u16   = 271;
pub const ABILITY_CURSED_BODY: u16   = 130;

// -- After-damage hooks: attacker --
pub const ABILITY_MAGICIAN: u16      = 170;
pub const ABILITY_POISON_TOUCH: u16  = 143;
pub const ABILITY_TOXIC_CHAIN: u16   = 305;

// -- After-KO hooks: attacker --
pub const ABILITY_MOXIE: u16         = 153;
pub const ABILITY_BEAST_BOOST: u16   = 224;
pub const ABILITY_CHILLING_NEIGH: u16 = 264;
pub const ABILITY_GRIM_NEIGH: u16    = 265;
pub const ABILITY_AS_ONE_GLASTRIER: u16 = 266;
pub const ABILITY_AS_ONE_SPECTRIER: u16 = 267;
pub const ABILITY_BATTLE_BOND: u16   = 210;
pub const ABILITY_SOUL_HEART: u16    = 220;

// -- Multi-hit --
pub const ABILITY_SKILL_LINK: u16    = 92;
pub const ABILITY_PARENTAL_BOND: u16 = 185;

// -- Turn executor: speed / accuracy / secondary --
pub const ABILITY_QUICK_FEET: u16    = 95;
pub const ABILITY_NO_GUARD: u16      = 99;
pub const ABILITY_COMPOUND_EYES: u16 = 14;
pub const ABILITY_VICTORY_STAR: u16  = 162;
pub const ABILITY_SERENE_GRACE: u16  = 32;
pub const ABILITY_SAND_VEIL: u16     = 8;
pub const ABILITY_SNOW_CLOAK: u16    = 81;
pub const ABILITY_GLUTTONY: u16      = 82;
pub const ABILITY_CHEEK_POUCH: u16   = 167;

// -- Speed modifiers --
pub const ABILITY_CHLOROPHYLL: u16   = 34;
pub const ABILITY_SWIFT_SWIM: u16    = 33;
pub const ABILITY_SAND_RUSH: u16     = 146;
pub const ABILITY_SLUSH_RUSH: u16    = 202;
pub const ABILITY_SURGE_SURFER: u16  = 207;

// -- Priority modifiers --
pub const ABILITY_PRANKSTER: u16     = 158;
pub const ABILITY_GALE_WINGS: u16    = 177;
pub const ABILITY_TRIAGE: u16        = 205;
pub const ABILITY_QUICK_DRAW: u16    = 259;
pub const ABILITY_STALL: u16         = 100;
pub const ABILITY_MYCELIUM_MIGHT: u16 = 298;

// -- Type-change abilities (pre-calc) --
pub const ABILITY_GALVANIZE: u16     = 206;
pub const ABILITY_PIXILATE: u16      = 182;
pub const ABILITY_AERILATE: u16      = 184;
pub const ABILITY_REFRIGERATE: u16   = 174;
pub const ABILITY_NORMALIZE: u16     = 96;
pub const ABILITY_LIQUID_VOICE: u16  = 204;
pub const ABILITY_LIQUID_OOZE: u16   = 64;

// -- Terrain setters --
pub const ABILITY_ELECTRIC_SURGE: u16 = 226;
pub const ABILITY_GRASSY_SURGE: u16   = 229;
pub const ABILITY_MISTY_SURGE: u16    = 228;
pub const ABILITY_PSYCHIC_SURGE: u16  = 227;

// -- Switch-in abilities --
pub const ABILITY_DOWNLOAD: u16       = 88;
pub const ABILITY_TRACE: u16          = 36;
pub const ABILITY_IMPOSTER: u16       = 150;
pub const ABILITY_NEUTRALIZING_GAS: u16 = 256;
pub const ABILITY_UNNERVE: u16        = 127;
pub const ABILITY_AIR_LOCK: u16       = 76;
pub const ABILITY_CLOUD_NINE: u16     = 13;
pub const ABILITY_INTREPID_SWORD: u16 = 234;
pub const ABILITY_DAUNTLESS_SHIELD: u16 = 235;
pub const ABILITY_HOSPITALITY: u16    = 299;
pub const ABILITY_SUPERSWEET_SYRUP: u16 = 306;
pub const ABILITY_EMBODY_ASPECT_TEAL: u16 = 301;
pub const ABILITY_EMBODY_ASPECT_WELLSPRING: u16 = 302;
pub const ABILITY_EMBODY_ASPECT_HEARTHFLAME: u16 = 303;
pub const ABILITY_EMBODY_ASPECT_CORNERSTONE: u16 = 304;

// -- Intimidate blockers --
pub const ABILITY_CLEAR_BODY: u16     = 29;
pub const ABILITY_WHITE_SMOKE: u16    = 73;
pub const ABILITY_FULL_METAL_BODY: u16 = 230;
pub const ABILITY_HYPER_CUTTER: u16   = 52;
pub const ABILITY_INNER_FOCUS: u16    = 39;
pub const ABILITY_OBLIVIOUS: u16      = 12;
pub const ABILITY_OWN_TEMPO: u16      = 20;
pub const ABILITY_SCRAPPY: u16        = 113;
pub const ABILITY_GUARD_DOG: u16      = 275;
pub const ABILITY_SYNCHRONIZE: u16    = 28;

// -- Untraceable abilities (not defined elsewhere) --
pub const ABILITY_ILLUSION: u16       = 149;
pub const ABILITY_MULTITYPE: u16      = 121;
pub const ABILITY_FORECAST: u16       = 59;
pub const ABILITY_COMATOSE: u16       = 213;
pub const ABILITY_COMMANDER: u16      = 279;
pub const ABILITY_HUNGER_SWITCH: u16  = 258;
pub const ABILITY_POWER_CONSTRUCT: u16 = 211;
pub const ABILITY_POWER_OF_ALCHEMY: u16 = 223;
pub const ABILITY_RECEIVER: u16       = 222;
pub const ABILITY_RKS_SYSTEM: u16     = 225;
pub const ABILITY_TERA_SHIFT: u16     = 307;
pub const ABILITY_TERA_SHELL: u16     = 308;
pub const ABILITY_TERAFORM_ZERO: u16  = 309;
pub const ABILITY_POISON_PUPPETEER: u16 = 310;

// -- Switch-out abilities --
pub const ABILITY_ZERO_TO_HERO: u16   = 278;

// -- Forme-change abilities --
pub const ABILITY_SCHOOLING: u16      = 208; // Wishiwashi
pub const ABILITY_SHIELDS_DOWN: u16   = 197; // Minior
pub const ABILITY_GULP_MISSILE: u16   = 241; // Cramorant

// (Dazzling, Queenly Majesty, Armor Tail are in defender immunities above)

// -- Mold Breaker family (bypass target abilities) --
pub const ABILITY_MOLD_BREAKER: u16   = 104;
pub const ABILITY_TURBOBLAZE: u16     = 163;
pub const ABILITY_TERAVOLT: u16       = 164;

// -- Damage-modifying abilities (Phase B) --
pub const ABILITY_WONDER_GUARD: u16   = 25;
pub const ABILITY_UNAWARE: u16        = 109;
pub const ABILITY_PURIFYING_SALT: u16 = 272;
pub const ABILITY_IMMUNITY: u16       = 17;
pub const ABILITY_PASTEL_VEIL: u16    = 257;
// Status-blocker abilities (all breakable: Mold Breaker bypasses).
pub const ABILITY_WATER_VEIL: u16     = 41;
pub const ABILITY_LIMBER: u16         = 7;
pub const ABILITY_MAGMA_ARMOR: u16    = 40;
pub const ABILITY_INSOMNIA: u16       = 15;
pub const ABILITY_VITAL_SPIRIT: u16   = 72;
pub const ABILITY_WELL_BAKED_BODY: u16 = 273;
pub const ABILITY_SWORD_OF_RUIN: u16  = 285;
pub const ABILITY_BEADS_OF_RUIN: u16  = 284;
pub const ABILITY_TABLETS_OF_RUIN: u16 = 282;  // num from Showdown
pub const ABILITY_VESSEL_OF_RUIN: u16 = 283;
pub const ABILITY_DARK_AURA: u16      = 186;
pub const ABILITY_FAIRY_AURA: u16     = 187;
pub const ABILITY_AURA_BREAK: u16     = 188;

// Stat-change modifying abilities
pub const ABILITY_CONTRARY: u16       = 126;
pub const ABILITY_SIMPLE: u16         = 86;
pub const ABILITY_COMPETITIVE: u16    = 172;
pub const ABILITY_DEFIANT: u16        = 128;
pub const ABILITY_MIRROR_ARMOR: u16   = 240;

// Weather/terrain + stat boost on switch-in
pub const ABILITY_ORICHALCUM_PULSE: u16 = 288;
pub const ABILITY_HADRON_ENGINE: u16    = 289;

pub const ITEM_NONE: u16 = 0;
pub const ITEM_AGUAV_BERRY: u16 = 5;
pub const ITEM_AIR_BALLOON: u16 = 6;
pub const ITEM_BIG_ROOT: u16 = 29;
pub const ITEM_BERRY_JUICE: u16 = 22;
pub const ITEM_DEEP_SEA_SCALE: u16 = 93;
pub const ITEM_FIGY_BERRY: u16 = 140;
pub const ITEM_IAPAPA_BERRY: u16 = 217;
pub const ITEM_JABOCA_BERRY: u16 = 230;
pub const ITEM_LIGHT_BALL: u16 = 251;
pub const ITEM_LUM_BERRY: u16 = 262;
pub const ITEM_MAGO_BERRY: u16 = 274;
pub const ITEM_ORAN_BERRY: u16 = 319;
pub const ITEM_MUSCLE_BAND: u16 = 297;
pub const ITEM_ROWAP_BERRY: u16 = 420;
pub const ITEM_SHELL_BELL: u16 = 438;
pub const ITEM_SITRUS_BERRY: u16 = 448;
pub const ITEM_STARF_BERRY: u16 = 472;
pub const ITEM_STICKY_BARB: u16 = 476;
pub const ITEM_THICK_CLUB: u16 = 491;
pub const ITEM_THROAT_SPRAY: u16 = 713;
pub const ITEM_WEAKNESS_POLICY: u16 = 609;
pub const ITEM_WIKI_BERRY: u16 = 538;
pub const ITEM_WISE_GLASSES: u16 = 539;
pub const ITEM_PROTECTIVE_PADS: u16 = 663;
pub const ITEM_UTILITY_UMBRELLA: u16 = 718;
pub const ITEM_ABILITY_SHIELD: u16 = 746;
pub const ITEM_CLEAR_AMULET: u16 = 747;
pub const ITEM_GRIP_CLAW: u16 = 179;
pub const ITEM_MIRROR_HERB: u16 = 748;
pub const ITEM_PUNCHING_GLOVE: u16 = 749;
pub const ITEM_COVERT_CLOAK: u16 = 750;
pub const ITEM_LOADED_DICE: u16 = 751;
pub const ITEM_BOOSTER_ENERGY: u16 = 745;

// Status-cure berries
pub const ITEM_CHERI_BERRY: u16 = 63;   // cures Paralysis
pub const ITEM_CHESTO_BERRY: u16 = 65;  // cures Sleep
pub const ITEM_PECHA_BERRY: u16 = 333;  // cures Poison
pub const ITEM_RAWST_BERRY: u16 = 381;  // cures Burn
pub const ITEM_ASPEAR_BERRY: u16 = 13;  // cures Freeze
pub const ITEM_PERSIM_BERRY: u16 = 334; // cures Confusion

pub const SPECIES_PIKACHU: u16 = 25;
pub const SPECIES_CUBONE: u16 = 104;
pub const SPECIES_MAROWAK: u16 = 105;
pub const SPECIES_CLAMPERL: u16 = 366;
pub const SPECIES_DIALGA: u16 = 483;
pub const SPECIES_PALKIA: u16 = 484;
pub const SPECIES_GIRATINA: u16 = 487;
pub const SPECIES_ARCEUS: u16 = 493;
pub const SPECIES_GENESECT: u16 = 649;
pub const SPECIES_SILVALLY: u16 = 773;
pub const SPECIES_ZACIAN: u16 = 888;
pub const SPECIES_ZAMAZENTA: u16 = 889;
pub const SPECIES_OGERPON: u16 = 1017;
pub const SPECIES_GRENINJA_BOND: u16 = 1230;

/// Map any species/forme ID to its base national-dex species number.
#[inline(always)]
pub fn base_species(species_id: u16) -> u16 {
    use crate::data::gen_species::FORME_TO_BASE;
    let idx = species_id as usize;
    if idx < FORME_TO_BASE.len() {
        FORME_TO_BASE[idx]
    } else {
        0
    }
}
