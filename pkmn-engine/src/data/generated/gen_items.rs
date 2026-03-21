//! Generated item data table — do not edit by hand.
//!
//! Regenerate with: `python3 codegen.py --items`
//!
//! Indexed by Showdown spritenum. The array is sparse: most slots are N
//! (ItemData::NONE). Only items with engine-relevant flags or forme_species
//! locks get populated entries.
//!
//! To add a new item: add it to SPECIFIC_ITEMS in codegen.py, then re-run.

use crate::data::items::ItemData;
use crate::data::items::ItemFlag as F;

pub static GEN_ITEMS: [ItemData; 762] = {
    const N: ItemData = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 0 };
    let mut t = [N; 762];
    // [  4] Adamant Orb
    t[4] = ItemData { flags: F::TYPE_BOOST, type_param: 16, power_param: 60, forme_species: 0 };
    // [  5] Aguav Berry
    t[5] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [  6] Air Balloon
    t[6] = ItemData { flags: F::AIR_BALLOON | F::CONSUMABLE, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 10] Apicot Berry
    t[10] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 3, power_param: 0, forme_species: 0 };
    // [ 13] Aspear Berry
    t[13] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [ 17] Babiri Berry
    t[17] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 16, power_param: 0, forme_species: 0 };
    // [ 21] Belue Berry
    t[21] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [ 31] Binding Band
    t[31] = ItemData { flags: F::BINDING_BOOST, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [ 32] Black Belt
    t[32] = ItemData { flags: F::TYPE_BOOST, type_param: 6, power_param: 30, forme_species: 0 };
    // [ 34] Black Sludge
    t[34] = ItemData { flags: F::BLACK_SLUDGE, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [ 35] Black Glasses
    t[35] = ItemData { flags: F::TYPE_BOOST, type_param: 15, power_param: 30, forme_species: 0 };
    // [ 44] Bluk Berry
    t[44] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [ 53] Bug Gem
    t[53] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 11, power_param: 0, forme_species: 0 };
    // [ 54] Burn Drive
    t[54] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 649 };
    // [ 61] Charcoal
    t[61] = ItemData { flags: F::TYPE_BOOST, type_param: 1, power_param: 30, forme_species: 0 };
    // [ 62] Charti Berry
    t[62] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 12, power_param: 0, forme_species: 0 };
    // [ 63] Cheri Berry
    t[63] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [ 65] Chesto Berry
    t[65] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [ 66] Chilan Berry
    t[66] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 0, power_param: 0, forme_species: 0 };
    // [ 67] Chill Drive
    t[67] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 649 };
    // [ 68] Choice Band
    t[68] = ItemData { flags: F::CHOICE_ATK, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 69] Choice Scarf
    t[69] = ItemData { flags: F::CHOICE_SPE, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 70] Choice Specs
    t[70] = ItemData { flags: F::CHOICE_SPA, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 71] Chople Berry
    t[71] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 6, power_param: 0, forme_species: 0 };
    // [ 76] Coba Berry
    t[76] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 9, power_param: 0, forme_species: 0 };
    // [ 78] Colbur Berry
    t[78] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 15, power_param: 0, forme_species: 0 };
    // [ 81] Cornn Berry
    t[81] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [ 86] Custap Berry
    t[86] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [ 89] Dark Gem
    t[89] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 15, power_param: 0, forme_species: 0 };
    // [103] Douse Drive
    t[103] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 649 };
    // [105] Draco Plate
    t[105] = ItemData { flags: F::TYPE_BOOST, type_param: 14, power_param: 0, forme_species: 493 };
    // [106] Dragon Fang
    t[106] = ItemData { flags: F::TYPE_BOOST, type_param: 14, power_param: 70, forme_species: 0 };
    // [107] Dragon Gem
    t[107] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 14, power_param: 0, forme_species: 0 };
    // [110] Dread Plate
    t[110] = ItemData { flags: F::TYPE_BOOST, type_param: 15, power_param: 0, forme_species: 493 };
    // [114] Durin Berry
    t[114] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [117] Earth Plate
    t[117] = ItemData { flags: F::TYPE_BOOST, type_param: 8, power_param: 0, forme_species: 493 };
    // [120] Electric Gem
    t[120] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 3, power_param: 0, forme_species: 0 };
    // [124] Enigma Berry
    t[124] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [130] Eviolite
    t[130] = ItemData { flags: F::EVIOLITE, type_param: 0xFF, power_param: 40, forme_species: 0 };
    // [132] Expert Belt
    t[132] = ItemData { flags: F::EXPERT_BELT, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [139] Fighting Gem
    t[139] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 6, power_param: 0, forme_species: 0 };
    // [140] Figy Berry
    t[140] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [141] Fire Gem
    t[141] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 1, power_param: 0, forme_species: 0 };
    // [143] Fist Plate
    t[143] = ItemData { flags: F::TYPE_BOOST, type_param: 6, power_param: 0, forme_species: 493 };
    // [145] Flame Orb
    t[145] = ItemData { flags: F::FLAME_ORB, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [146] Flame Plate
    t[146] = ItemData { flags: F::TYPE_BOOST, type_param: 1, power_param: 0, forme_species: 493 };
    // [149] Flying Gem
    t[149] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 9, power_param: 0, forme_species: 0 };
    // [151] Focus Sash
    t[151] = ItemData { flags: F::CONSUMABLE | F::FOCUS_SASH, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [158] Ganlon Berry
    t[158] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 1, power_param: 0, forme_species: 0 };
    // [161] Ghost Gem
    t[161] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 13, power_param: 0, forme_species: 0 };
    // [172] Grass Gem
    t[172] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 4, power_param: 0, forme_species: 0 };
    // [178] Grepa Berry
    t[178] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [180] Griseous Orb
    t[180] = ItemData { flags: F::TYPE_BOOST, type_param: 13, power_param: 60, forme_species: 487 };
    // [182] Ground Gem
    t[182] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 8, power_param: 0, forme_species: 0 };
    // [185] Haban Berry
    t[185] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 14, power_param: 0, forme_species: 0 };
    // [187] Hard Stone
    t[187] = ItemData { flags: F::TYPE_BOOST, type_param: 12, power_param: 100, forme_species: 0 };
    // [213] Hondew Berry
    t[213] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [217] Iapapa Berry
    t[217] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [218] Ice Gem
    t[218] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 5, power_param: 0, forme_species: 0 };
    // [220] Icicle Plate
    t[220] = ItemData { flags: F::TYPE_BOOST, type_param: 5, power_param: 0, forme_species: 493 };
    // [223] Insect Plate
    t[223] = ItemData { flags: F::TYPE_BOOST, type_param: 11, power_param: 0, forme_species: 493 };
    // [225] Iron Plate
    t[225] = ItemData { flags: F::TYPE_BOOST, type_param: 16, power_param: 0, forme_species: 493 };
    // [230] Jaboca Berry
    t[230] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [233] Kasib Berry
    t[233] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 13, power_param: 0, forme_species: 0 };
    // [234] Kebia Berry
    t[234] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 7, power_param: 0, forme_species: 0 };
    // [235] Kelpsy Berry
    t[235] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [238] Lansat Berry
    t[238] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [242] Leftovers
    t[242] = ItemData { flags: F::LEFTOVERS, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [244] Leppa Berry
    t[244] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [248] Liechi Berry
    t[248] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 0, power_param: 0, forme_species: 0 };
    // [249] Life Orb
    t[249] = ItemData { flags: F::LIFE_ORB, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [252] Light Clay
    t[252] = ItemData { flags: F::EXTENDS_SCREENS, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [262] Lum Berry
    t[262] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [265] Lustrous Orb
    t[265] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 60, forme_species: 0 };
    // [273] Magnet
    t[273] = ItemData { flags: F::TYPE_BOOST, type_param: 3, power_param: 30, forme_species: 0 };
    // [274] Mago Berry
    t[274] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [275] Magost Berry
    t[275] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [282] Meadow Plate
    t[282] = ItemData { flags: F::TYPE_BOOST, type_param: 4, power_param: 0, forme_species: 493 };
    // [286] Metal Coat
    t[286] = ItemData { flags: F::TYPE_BOOST, type_param: 16, power_param: 30, forme_species: 0 };
    // [289] Metronome
    t[289] = ItemData { flags: F::METRONOME, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [290] Micle Berry
    t[290] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [291] Mind Plate
    t[291] = ItemData { flags: F::TYPE_BOOST, type_param: 10, power_param: 0, forme_species: 493 };
    // [292] Miracle Seed
    t[292] = ItemData { flags: F::TYPE_BOOST, type_param: 4, power_param: 30, forme_species: 0 };
    // [300] Mystic Water
    t[300] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 30, forme_species: 0 };
    // [302] Nanab Berry
    t[302] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [305] Never-Melt Ice
    t[305] = ItemData { flags: F::TYPE_BOOST, type_param: 5, power_param: 30, forme_species: 0 };
    // [306] Nomel Berry
    t[306] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [307] Normal Gem
    t[307] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 0, power_param: 0, forme_species: 0 };
    // [311] Occa Berry
    t[311] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 1, power_param: 0, forme_species: 0 };
    // [312] Odd Incense
    t[312] = ItemData { flags: F::TYPE_BOOST, type_param: 10, power_param: 10, forme_species: 0 };
    // [319] Oran Berry
    t[319] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [323] Pamtre Berry
    t[323] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [329] Passho Berry
    t[329] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 2, power_param: 0, forme_species: 0 };
    // [330] Payapa Berry
    t[330] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 10, power_param: 0, forme_species: 0 };
    // [333] Pecha Berry
    t[333] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [334] Persim Berry
    t[334] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [335] Petaya Berry
    t[335] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 2, power_param: 0, forme_species: 0 };
    // [337] Pinap Berry
    t[337] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [343] Poison Barb
    t[343] = ItemData { flags: F::TYPE_BOOST, type_param: 7, power_param: 70, forme_species: 0 };
    // [344] Poison Gem
    t[344] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 7, power_param: 0, forme_species: 0 };
    // [351] Pomeg Berry
    t[351] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [358] Power Herb
    t[358] = ItemData { flags: F::CONSUMABLE | F::POWER_HERB, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [369] Psychic Gem
    t[369] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 10, power_param: 0, forme_species: 0 };
    // [371] Qualot Berry
    t[371] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [375] Rabuta Berry
    t[375] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [381] Rawst Berry
    t[381] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [382] Razor Claw
    t[382] = ItemData { flags: F::CRIT_BOOST, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [384] Razz Berry
    t[384] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [409] Rindo Berry
    t[409] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 4, power_param: 0, forme_species: 0 };
    // [415] Rock Gem
    t[415] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 12, power_param: 0, forme_species: 0 };
    // [416] Rock Incense
    t[416] = ItemData { flags: F::TYPE_BOOST, type_param: 12, power_param: 10, forme_species: 0 };
    // [417] Rocky Helmet
    t[417] = ItemData { flags: F::ROCKY_HELMET, type_param: 0xFF, power_param: 60, forme_species: 0 };
    // [419] Rose Incense
    t[419] = ItemData { flags: F::TYPE_BOOST, type_param: 4, power_param: 10, forme_species: 0 };
    // [420] Rowap Berry
    t[420] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [426] Salac Berry
    t[426] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 4, power_param: 0, forme_species: 0 };
    // [429] Scope Lens
    t[429] = ItemData { flags: F::CRIT_BOOST, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [430] Sea Incense
    t[430] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 10, forme_species: 0 };
    // [436] Sharp Beak
    t[436] = ItemData { flags: F::TYPE_BOOST, type_param: 9, power_param: 50, forme_species: 0 };
    // [437] Shed Shell
    t[437] = ItemData { flags: F::TRAP_IMMUNE, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [442] Shock Drive
    t[442] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 649 };
    // [443] Shuca Berry
    t[443] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 8, power_param: 0, forme_species: 0 };
    // [444] Silk Scarf
    t[444] = ItemData { flags: F::TYPE_BOOST, type_param: 0, power_param: 10, forme_species: 0 };
    // [447] Silver Powder
    t[447] = ItemData { flags: F::TYPE_BOOST, type_param: 11, power_param: 10, forme_species: 0 };
    // [448] Sitrus Berry
    t[448] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [450] Sky Plate
    t[450] = ItemData { flags: F::TYPE_BOOST, type_param: 9, power_param: 0, forme_species: 493 };
    // [456] Soft Sand
    t[456] = ItemData { flags: F::TYPE_BOOST, type_param: 8, power_param: 10, forme_species: 0 };
    // [459] Soul Dew
    t[459] = ItemData { flags: F::TYPE_BOOST, type_param: 10, power_param: 30, forme_species: 0 };
    // [461] Spell Tag
    t[461] = ItemData { flags: F::TYPE_BOOST, type_param: 13, power_param: 30, forme_species: 0 };
    // [462] Spelon Berry
    t[462] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [463] Splash Plate
    t[463] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 0, forme_species: 493 };
    // [464] Spooky Plate
    t[464] = ItemData { flags: F::TYPE_BOOST, type_param: 13, power_param: 0, forme_species: 493 };
    // [472] Starf Berry
    t[472] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [473] Steel Gem
    t[473] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 16, power_param: 0, forme_species: 0 };
    // [477] Stone Plate
    t[477] = ItemData { flags: F::TYPE_BOOST, type_param: 12, power_param: 0, forme_species: 493 };
    // [486] Tamato Berry
    t[486] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [487] Tanga Berry
    t[487] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 11, power_param: 0, forme_species: 0 };
    // [515] Toxic Orb
    t[515] = ItemData { flags: F::TOXIC_ORB, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [516] Toxic Plate
    t[516] = ItemData { flags: F::TYPE_BOOST, type_param: 7, power_param: 0, forme_species: 493 };
    // [520] Twisted Spoon
    t[520] = ItemData { flags: F::TYPE_BOOST, type_param: 10, power_param: 30, forme_species: 0 };
    // [526] Wacan Berry
    t[526] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 3, power_param: 0, forme_species: 0 };
    // [528] Water Gem
    t[528] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 2, power_param: 0, forme_species: 0 };
    // [530] Watmel Berry
    t[530] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [531] Wave Incense
    t[531] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 10, forme_species: 0 };
    // [533] Wepear Berry
    t[533] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [537] Wide Lens
    t[537] = ItemData { flags: F::WIDE_LENS, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [538] Wiki Berry
    t[538] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [544] Clefablite
    t[544] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [545] Victreebelite
    t[545] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [546] Starminite
    t[546] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [547] Dragoninite
    t[547] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [548] Meganiumite
    t[548] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [549] Feraligite
    t[549] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [550] Skarmorite
    t[550] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [551] Froslassite
    t[551] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [552] Emboarite
    t[552] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [553] Excadrite
    t[553] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [554] Scolipite
    t[554] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [555] Scraftinite
    t[555] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [556] Eelektrossite
    t[556] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [557] Chandelurite
    t[557] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [558] Chesnaughtite
    t[558] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [559] Delphoxite
    t[559] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [560] Greninjite
    t[560] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [561] Pyroarite
    t[561] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [562] Floettite
    t[562] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [563] Malamarite
    t[563] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [564] Barbaracite
    t[564] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [565] Dragalgite
    t[565] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [566] Hawluchanite
    t[566] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [567] Yache Berry
    t[567] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 5, power_param: 0, forme_species: 0 };
    // [568] Zygardite
    t[568] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [569] Drampanite
    t[569] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [570] Falinksite
    t[570] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [572] Zap Plate
    t[572] = ItemData { flags: F::TYPE_BOOST, type_param: 3, power_param: 0, forme_species: 493 };
    // [573] Garchompite
    t[573] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [575] Abomasite
    t[575] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [576] Absolite
    t[576] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [577] Aerodactylite
    t[577] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [578] Aggronite
    t[578] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [579] Alakazite
    t[579] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [580] Ampharosite
    t[580] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [581] Assault Vest
    t[581] = ItemData { flags: F::ASSAULT_VEST, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [582] Banettite
    t[582] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [583] Blastoisinite
    t[583] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [584] Blazikenite
    t[584] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [585] Charizardite X
    t[585] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [586] Charizardite Y
    t[586] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [587] Gardevoirite
    t[587] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [588] Gengarite
    t[588] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [589] Gyaradosite
    t[589] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [590] Heracronite
    t[590] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [591] Houndoominite
    t[591] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [592] Kangaskhanite
    t[592] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [593] Kee Berry
    t[593] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [594] Lucarionite
    t[594] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [596] Manectite
    t[596] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [597] Maranga Berry
    t[597] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [598] Mawilite
    t[598] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [599] Medichamite
    t[599] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [600] Mewtwonite X
    t[600] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [601] Mewtwonite Y
    t[601] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [602] Pinsirite
    t[602] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [603] Roseli Berry
    t[603] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 17, power_param: 0, forme_species: 0 };
    // [604] Safety Goggles
    t[604] = ItemData { flags: F::SAFETY_GOGGLES, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [605] Scizorite
    t[605] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [607] Tyranitarite
    t[607] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [608] Venusaurite
    t[608] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [609] Weakness Policy
    t[609] = ItemData { flags: F::CONSUMABLE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [610] Pixie Plate
    t[610] = ItemData { flags: F::TYPE_BOOST, type_param: 17, power_param: 0, forme_species: 493 };
    // [611] Fairy Gem
    t[611] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 17, power_param: 0, forme_species: 0 };
    // [612] Swampertite
    t[612] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [613] Sceptilite
    t[613] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [614] Sablenite
    t[614] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [615] Altarianite
    t[615] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [616] Galladite
    t[616] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [617] Audinite
    t[617] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [618] Metagrossite
    t[618] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [619] Sharpedonite
    t[619] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [620] Slowbronite
    t[620] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [621] Steelixite
    t[621] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [622] Pidgeotite
    t[622] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [623] Glalitite
    t[623] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [624] Diancite
    t[624] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [625] Cameruptite
    t[625] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [626] Lopunnite
    t[626] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [627] Salamencite
    t[627] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [628] Beedrillite
    t[628] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [629] Latiasite
    t[629] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [630] Latiosite
    t[630] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [631] Normalium Z
    t[631] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [632] Firium Z
    t[632] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 1, power_param: 0, forme_species: 0 };
    // [633] Waterium Z
    t[633] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 2, power_param: 0, forme_species: 0 };
    // [634] Electrium Z
    t[634] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 3, power_param: 0, forme_species: 0 };
    // [635] Grassium Z
    t[635] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 4, power_param: 0, forme_species: 0 };
    // [636] Icium Z
    t[636] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 5, power_param: 0, forme_species: 0 };
    // [637] Fightinium Z
    t[637] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 6, power_param: 0, forme_species: 0 };
    // [638] Poisonium Z
    t[638] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 7, power_param: 0, forme_species: 0 };
    // [639] Groundium Z
    t[639] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 8, power_param: 0, forme_species: 0 };
    // [640] Flyinium Z
    t[640] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 9, power_param: 0, forme_species: 0 };
    // [641] Psychium Z
    t[641] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 10, power_param: 0, forme_species: 0 };
    // [642] Buginium Z
    t[642] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 11, power_param: 0, forme_species: 0 };
    // [643] Rockium Z
    t[643] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 12, power_param: 0, forme_species: 0 };
    // [644] Ghostium Z
    t[644] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 13, power_param: 0, forme_species: 0 };
    // [645] Dragonium Z
    t[645] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 14, power_param: 0, forme_species: 0 };
    // [646] Darkinium Z
    t[646] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 15, power_param: 0, forme_species: 0 };
    // [647] Steelium Z
    t[647] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 16, power_param: 0, forme_species: 0 };
    // [648] Fairium Z
    t[648] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 17, power_param: 0, forme_species: 0 };
    // [649] Pikanium Z
    t[649] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [650] Decidium Z
    t[650] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [651] Incinium Z
    t[651] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [652] Primarium Z
    t[652] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [653] Tapunium Z
    t[653] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [654] Marshadium Z
    t[654] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [655] Aloraichium Z
    t[655] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [656] Snorlium Z
    t[656] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [657] Eevium Z
    t[657] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [658] Mewnium Z
    t[658] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [659] Pikashunium Z
    t[659] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [663] Protective Pads
    t[663] = ItemData { flags: F::PROTECTIVE_PADS, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [664] Electric Seed
    t[664] = ItemData { flags: F::CONSUMABLE | F::TERRAIN_SEED, type_param: 1, power_param: 10, forme_species: 0 };
    // [665] Psychic Seed
    t[665] = ItemData { flags: F::CONSUMABLE | F::TERRAIN_SEED, type_param: 3, power_param: 10, forme_species: 0 };
    // [666] Misty Seed
    t[666] = ItemData { flags: F::CONSUMABLE | F::TERRAIN_SEED, type_param: 4, power_param: 10, forme_species: 0 };
    // [667] Grassy Seed
    t[667] = ItemData { flags: F::CONSUMABLE | F::TERRAIN_SEED, type_param: 2, power_param: 10, forme_species: 0 };
    // [668] Fighting Memory
    t[668] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [669] Flying Memory
    t[669] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [670] Poison Memory
    t[670] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [671] Ground Memory
    t[671] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [672] Rock Memory
    t[672] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [673] Bug Memory
    t[673] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [674] Ghost Memory
    t[674] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [675] Steel Memory
    t[675] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [676] Fire Memory
    t[676] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [677] Water Memory
    t[677] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [678] Grass Memory
    t[678] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [679] Electric Memory
    t[679] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [680] Psychic Memory
    t[680] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [681] Ice Memory
    t[681] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [682] Dragon Memory
    t[682] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [683] Dark Memory
    t[683] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [684] Fairy Memory
    t[684] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 773 };
    // [685] Solganium Z
    t[685] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [686] Lunalium Z
    t[686] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [687] Ultranecrozium Z
    t[687] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [688] Mimikium Z
    t[688] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [689] Lycanium Z
    t[689] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [690] Kommonium Z
    t[690] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [698] Rusted Sword
    t[698] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 888 };
    // [699] Rusted Shield
    t[699] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 889 };
    // [713] Throat Spray
    t[713] = ItemData { flags: F::CONSUMABLE | F::THROAT_SPRAY, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [715] Heavy-Duty Boots
    t[715] = ItemData { flags: F::HAZARD_IMMUNE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [718] Utility Umbrella
    t[718] = ItemData { flags: F::UTILITY_UMBRELLA, type_param: 0xFF, power_param: 60, forme_species: 0 };
    // [741] Adamant Crystal
    t[741] = ItemData { flags: F::TYPE_BOOST, type_param: 16, power_param: 0, forme_species: 483 };
    // [742] Lustrous Globe
    t[742] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 0, forme_species: 484 };
    // [743] Griseous Core
    t[743] = ItemData { flags: F::TYPE_BOOST, type_param: 13, power_param: 0, forme_species: 487 };
    // [745] Booster Energy
    t[745] = ItemData { flags: F::CONSUMABLE, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [746] Ability Shield
    t[746] = ItemData { flags: F::ABILITY_SHIELD, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [747] Clear Amulet
    t[747] = ItemData { flags: F::CLEAR_AMULET, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [748] Mirror Herb
    t[748] = ItemData { flags: F::CONSUMABLE | F::MIRROR_HERB, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [749] Punching Glove
    t[749] = ItemData { flags: F::PUNCHING_GLOVE, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [750] Covert Cloak
    t[750] = ItemData { flags: F::COVERT_CLOAK, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [751] Loaded Dice
    t[751] = ItemData { flags: F::LOADED_DICE, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [754] Fairy Feather
    t[754] = ItemData { flags: F::TYPE_BOOST, type_param: 17, power_param: 10, forme_species: 0 };
    // [758] Cornerstone Mask
    t[758] = ItemData { flags: 0, type_param: 0xFF, power_param: 60, forme_species: 1017 };
    // [759] Wellspring Mask
    t[759] = ItemData { flags: 0, type_param: 0xFF, power_param: 60, forme_species: 1017 };
    // [760] Hearthflame Mask
    t[760] = ItemData { flags: 0, type_param: 0xFF, power_param: 60, forme_species: 1017 };
    t
};

