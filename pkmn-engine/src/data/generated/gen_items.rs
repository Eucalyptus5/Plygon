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
    // [  1] Pretty Feather
    t[1] = ItemData { flags: 0, type_param: 0xFF, power_param: 20, forme_species: 0 };
    // [  2] Absorb Bulb
    t[2] = ItemData { flags: F::ABSORB_BULB | F::CONSUMABLE, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [  4] Adamant Orb
    t[4] = ItemData { flags: F::SIGNATURE_ORB, type_param: 16, power_param: 60, forme_species: 0 };
    // [  5] Aguav Berry
    t[5] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [  6] Air Balloon
    t[6] = ItemData { flags: F::AIR_BALLOON | F::CONSUMABLE, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 10] Apicot Berry
    t[10] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 3, power_param: 10, forme_species: 0 };
    // [ 12] Armor Fossil
    t[12] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [ 13] Aspear Berry
    t[13] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 17] Babiri Berry
    t[17] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 16, power_param: 10, forme_species: 0 };
    // [ 21] Belue Berry
    t[21] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 22] Berry Juice
    t[22] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [ 27] Big Nugget
    t[27] = ItemData { flags: 0, type_param: 0xFF, power_param: 130, forme_species: 0 };
    // [ 29] Big Root
    t[29] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 31] Binding Band
    t[31] = ItemData { flags: F::BINDING_BOOST, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [ 32] Black Belt
    t[32] = ItemData { flags: F::TYPE_BOOST, type_param: 6, power_param: 30, forme_species: 0 };
    // [ 34] Black Sludge
    t[34] = ItemData { flags: F::BLACK_SLUDGE, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [ 35] Black Glasses
    t[35] = ItemData { flags: F::TYPE_BOOST, type_param: 15, power_param: 30, forme_species: 0 };
    // [ 44] Bluk Berry
    t[44] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 51] Bright Powder
    t[51] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 53] Bug Gem
    t[53] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 11, power_param: 0, forme_species: 0 };
    // [ 54] Burn Drive
    t[54] = ItemData { flags: 0, type_param: 0xFF, power_param: 70, forme_species: 649 };
    // [ 60] Cell Battery
    t[60] = ItemData { flags: F::CELL_BATTERY | F::CONSUMABLE, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [ 61] Charcoal
    t[61] = ItemData { flags: F::TYPE_BOOST, type_param: 1, power_param: 30, forme_species: 0 };
    // [ 62] Charti Berry
    t[62] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 12, power_param: 10, forme_species: 0 };
    // [ 63] Cheri Berry
    t[63] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 65] Chesto Berry
    t[65] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 66] Chilan Berry
    t[66] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 0, power_param: 10, forme_species: 0 };
    // [ 67] Chill Drive
    t[67] = ItemData { flags: 0, type_param: 0xFF, power_param: 70, forme_species: 649 };
    // [ 68] Choice Band
    t[68] = ItemData { flags: F::CHOICE_ATK, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 69] Choice Scarf
    t[69] = ItemData { flags: F::CHOICE_SPE, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 70] Choice Specs
    t[70] = ItemData { flags: F::CHOICE_SPA, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 71] Chople Berry
    t[71] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 6, power_param: 10, forme_species: 0 };
    // [ 72] Claw Fossil
    t[72] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [ 76] Coba Berry
    t[76] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 9, power_param: 10, forme_species: 0 };
    // [ 78] Colbur Berry
    t[78] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 15, power_param: 10, forme_species: 0 };
    // [ 81] Cornn Berry
    t[81] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 85] Cover Fossil
    t[85] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [ 86] Custap Berry
    t[86] = ItemData { flags: F::CONSUMABLE | F::CUSTAP | F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [ 88] Damp Rock
    t[88] = ItemData { flags: 0, type_param: 0xFF, power_param: 60, forme_species: 0 };
    // [ 89] Dark Gem
    t[89] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 15, power_param: 0, forme_species: 0 };
    // [ 92] Dawn Stone
    t[92] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [ 93] Deep Sea Scale
    t[93] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [ 94] Deep Sea Tooth
    t[94] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [ 95] Destiny Knot
    t[95] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [102] Dome Fossil
    t[102] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [103] Douse Drive
    t[103] = ItemData { flags: 0, type_param: 0xFF, power_param: 70, forme_species: 649 };
    // [105] Draco Plate
    t[105] = ItemData { flags: F::TYPE_BOOST, type_param: 14, power_param: 90, forme_species: 493 };
    // [106] Dragon Fang
    t[106] = ItemData { flags: F::TYPE_BOOST, type_param: 14, power_param: 70, forme_species: 0 };
    // [107] Dragon Gem
    t[107] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 14, power_param: 0, forme_species: 0 };
    // [108] Dragon Scale
    t[108] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [110] Dread Plate
    t[110] = ItemData { flags: F::TYPE_BOOST, type_param: 15, power_param: 90, forme_species: 493 };
    // [113] Dubious Disc
    t[113] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 0 };
    // [114] Durin Berry
    t[114] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [116] Dusk Stone
    t[116] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [117] Earth Plate
    t[117] = ItemData { flags: F::TYPE_BOOST, type_param: 8, power_param: 90, forme_species: 493 };
    // [118] Eject Button
    t[118] = ItemData { flags: F::CONSUMABLE | F::EJECT_BUTTON, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [119] Electirizer
    t[119] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [120] Electric Gem
    t[120] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 3, power_param: 0, forme_species: 0 };
    // [124] Enigma Berry
    t[124] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [130] Eviolite
    t[130] = ItemData { flags: F::EVIOLITE, type_param: 0xFF, power_param: 40, forme_species: 0 };
    // [132] Expert Belt
    t[132] = ItemData { flags: F::EXPERT_BELT, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [139] Fighting Gem
    t[139] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 6, power_param: 0, forme_species: 0 };
    // [140] Figy Berry
    t[140] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [141] Fire Gem
    t[141] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 1, power_param: 0, forme_species: 0 };
    // [142] Fire Stone
    t[142] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [143] Fist Plate
    t[143] = ItemData { flags: F::TYPE_BOOST, type_param: 6, power_param: 90, forme_species: 493 };
    // [145] Flame Orb
    t[145] = ItemData { flags: F::FLAME_ORB, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [146] Flame Plate
    t[146] = ItemData { flags: F::TYPE_BOOST, type_param: 1, power_param: 90, forme_species: 493 };
    // [147] Float Stone
    t[147] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [149] Flying Gem
    t[149] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 9, power_param: 0, forme_species: 0 };
    // [150] Focus Band
    t[150] = ItemData { flags: F::FOCUS_BAND, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [151] Focus Sash
    t[151] = ItemData { flags: F::CONSUMABLE | F::FOCUS_SASH, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [155] Full Incense
    t[155] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [158] Ganlon Berry
    t[158] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 1, power_param: 10, forme_species: 0 };
    // [161] Ghost Gem
    t[161] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 13, power_param: 0, forme_species: 0 };
    // [172] Grass Gem
    t[172] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 4, power_param: 0, forme_species: 0 };
    // [178] Grepa Berry
    t[178] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [179] Grip Claw
    t[179] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [180] Griseous Orb
    t[180] = ItemData { flags: F::SIGNATURE_ORB, type_param: 13, power_param: 60, forme_species: 487 };
    // [182] Ground Gem
    t[182] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 8, power_param: 0, forme_species: 0 };
    // [185] Haban Berry
    t[185] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 14, power_param: 10, forme_species: 0 };
    // [187] Hard Stone
    t[187] = ItemData { flags: F::TYPE_BOOST, type_param: 12, power_param: 100, forme_species: 0 };
    // [193] Heat Rock
    t[193] = ItemData { flags: 0, type_param: 0xFF, power_param: 60, forme_species: 0 };
    // [195] Helix Fossil
    t[195] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [213] Hondew Berry
    t[213] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [217] Iapapa Berry
    t[217] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [218] Ice Gem
    t[218] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 5, power_param: 0, forme_species: 0 };
    // [220] Icicle Plate
    t[220] = ItemData { flags: F::TYPE_BOOST, type_param: 5, power_param: 90, forme_species: 493 };
    // [221] Icy Rock
    t[221] = ItemData { flags: 0, type_param: 0xFF, power_param: 40, forme_species: 0 };
    // [223] Insect Plate
    t[223] = ItemData { flags: F::TYPE_BOOST, type_param: 11, power_param: 90, forme_species: 493 };
    // [224] Iron Ball
    t[224] = ItemData { flags: F::HALF_SPEED, type_param: 0xFF, power_param: 130, forme_species: 0 };
    // [225] Iron Plate
    t[225] = ItemData { flags: F::TYPE_BOOST, type_param: 16, power_param: 90, forme_species: 493 };
    // [230] Jaboca Berry
    t[230] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [233] Kasib Berry
    t[233] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 13, power_param: 10, forme_species: 0 };
    // [234] Kebia Berry
    t[234] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 7, power_param: 10, forme_species: 0 };
    // [235] Kelpsy Berry
    t[235] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [236] King's Rock
    t[236] = ItemData { flags: F::KINGS_ROCK, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [237] Lagging Tail
    t[237] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [238] Lansat Berry
    t[238] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [240] Lax Incense
    t[240] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [241] Leaf Stone
    t[241] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [242] Leftovers
    t[242] = ItemData { flags: F::LEFTOVERS, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [244] Leppa Berry
    t[244] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [248] Liechi Berry
    t[248] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 0, power_param: 10, forme_species: 0 };
    // [249] Life Orb
    t[249] = ItemData { flags: F::LIFE_ORB, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [251] Light Ball
    t[251] = ItemData { flags: F::LIGHT_BALL, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [252] Light Clay
    t[252] = ItemData { flags: F::EXTENDS_SCREENS, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [261] Lucky Punch
    t[261] = ItemData { flags: 0, type_param: 0xFF, power_param: 40, forme_species: 0 };
    // [262] Lum Berry
    t[262] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [265] Lustrous Orb
    t[265] = ItemData { flags: F::SIGNATURE_ORB, type_param: 2, power_param: 60, forme_species: 0 };
    // [269] Macho Brace
    t[269] = ItemData { flags: F::HALF_SPEED, type_param: 0xFF, power_param: 60, forme_species: 0 };
    // [272] Magmarizer
    t[272] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [273] Magnet
    t[273] = ItemData { flags: F::TYPE_BOOST, type_param: 3, power_param: 30, forme_species: 0 };
    // [274] Mago Berry
    t[274] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [275] Magost Berry
    t[275] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [282] Meadow Plate
    t[282] = ItemData { flags: F::TYPE_BOOST, type_param: 4, power_param: 90, forme_species: 493 };
    // [285] Mental Herb
    t[285] = ItemData { flags: F::CONSUMABLE | F::MENTAL_HERB, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [286] Metal Coat
    t[286] = ItemData { flags: F::TYPE_BOOST, type_param: 16, power_param: 30, forme_species: 0 };
    // [287] Metal Powder
    t[287] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [289] Metronome
    t[289] = ItemData { flags: F::METRONOME, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [290] Micle Berry
    t[290] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [291] Mind Plate
    t[291] = ItemData { flags: F::TYPE_BOOST, type_param: 10, power_param: 90, forme_species: 493 };
    // [292] Miracle Seed
    t[292] = ItemData { flags: F::TYPE_BOOST, type_param: 4, power_param: 30, forme_species: 0 };
    // [295] Moon Stone
    t[295] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [297] Muscle Band
    t[297] = ItemData { flags: F::MUSCLE_BAND, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [300] Mystic Water
    t[300] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 30, forme_species: 0 };
    // [302] Nanab Berry
    t[302] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [305] Never-Melt Ice
    t[305] = ItemData { flags: F::TYPE_BOOST, type_param: 5, power_param: 30, forme_species: 0 };
    // [306] Nomel Berry
    t[306] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [307] Normal Gem
    t[307] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 0, power_param: 0, forme_species: 0 };
    // [311] Occa Berry
    t[311] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 1, power_param: 10, forme_species: 0 };
    // [312] Odd Incense
    t[312] = ItemData { flags: F::TYPE_BOOST, type_param: 10, power_param: 10, forme_species: 0 };
    // [314] Old Amber
    t[314] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [319] Oran Berry
    t[319] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [321] Oval Stone
    t[321] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [323] Pamtre Berry
    t[323] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [329] Passho Berry
    t[329] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 2, power_param: 10, forme_species: 0 };
    // [330] Payapa Berry
    t[330] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 10, power_param: 10, forme_species: 0 };
    // [333] Pecha Berry
    t[333] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [334] Persim Berry
    t[334] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [335] Petaya Berry
    t[335] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 2, power_param: 10, forme_species: 0 };
    // [337] Pinap Berry
    t[337] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [339] Plume Fossil
    t[339] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [343] Poison Barb
    t[343] = ItemData { flags: F::TYPE_BOOST, type_param: 7, power_param: 70, forme_species: 0 };
    // [344] Poison Gem
    t[344] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 7, power_param: 0, forme_species: 0 };
    // [351] Pomeg Berry
    t[351] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [354] Power Anklet
    t[354] = ItemData { flags: F::HALF_SPEED, type_param: 0xFF, power_param: 70, forme_species: 0 };
    // [355] Power Band
    t[355] = ItemData { flags: F::HALF_SPEED, type_param: 0xFF, power_param: 70, forme_species: 0 };
    // [356] Power Belt
    t[356] = ItemData { flags: F::HALF_SPEED, type_param: 0xFF, power_param: 70, forme_species: 0 };
    // [357] Power Bracer
    t[357] = ItemData { flags: F::HALF_SPEED, type_param: 0xFF, power_param: 70, forme_species: 0 };
    // [358] Power Herb
    t[358] = ItemData { flags: F::CONSUMABLE | F::POWER_HERB, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [359] Power Lens
    t[359] = ItemData { flags: F::HALF_SPEED, type_param: 0xFF, power_param: 70, forme_species: 0 };
    // [360] Power Weight
    t[360] = ItemData { flags: F::HALF_SPEED, type_param: 0xFF, power_param: 70, forme_species: 0 };
    // [365] Prism Scale
    t[365] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [367] Protector
    t[367] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [369] Psychic Gem
    t[369] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 10, power_param: 0, forme_species: 0 };
    // [371] Qualot Berry
    t[371] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [373] Quick Claw
    t[373] = ItemData { flags: F::QUICK_CLAW, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [374] Quick Powder
    t[374] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [375] Rabuta Berry
    t[375] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [379] Rare Bone
    t[379] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [381] Rawst Berry
    t[381] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [382] Razor Claw
    t[382] = ItemData { flags: F::CRIT_BOOST, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [383] Razor Fang
    t[383] = ItemData { flags: F::KINGS_ROCK, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [384] Razz Berry
    t[384] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [385] Reaper Cloth
    t[385] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [387] Red Card
    t[387] = ItemData { flags: F::CONSUMABLE | F::RED_CARD, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [409] Rindo Berry
    t[409] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 4, power_param: 10, forme_species: 0 };
    // [410] Ring Target
    t[410] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [415] Rock Gem
    t[415] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 12, power_param: 0, forme_species: 0 };
    // [416] Rock Incense
    t[416] = ItemData { flags: F::TYPE_BOOST, type_param: 12, power_param: 10, forme_species: 0 };
    // [417] Rocky Helmet
    t[417] = ItemData { flags: F::ROCKY_HELMET, type_param: 0xFF, power_param: 60, forme_species: 0 };
    // [418] Root Fossil
    t[418] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [419] Rose Incense
    t[419] = ItemData { flags: F::TYPE_BOOST, type_param: 4, power_param: 10, forme_species: 0 };
    // [420] Rowap Berry
    t[420] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [426] Salac Berry
    t[426] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::PINCH_BERRY, type_param: 4, power_param: 10, forme_species: 0 };
    // [429] Scope Lens
    t[429] = ItemData { flags: F::CRIT_BOOST, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [430] Sea Incense
    t[430] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 10, forme_species: 0 };
    // [436] Sharp Beak
    t[436] = ItemData { flags: F::TYPE_BOOST, type_param: 9, power_param: 50, forme_species: 0 };
    // [437] Shed Shell
    t[437] = ItemData { flags: F::TRAP_IMMUNE, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [438] Shell Bell
    t[438] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [439] Shiny Stone
    t[439] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [442] Shock Drive
    t[442] = ItemData { flags: 0, type_param: 0xFF, power_param: 70, forme_species: 649 };
    // [443] Shuca Berry
    t[443] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 8, power_param: 10, forme_species: 0 };
    // [444] Silk Scarf
    t[444] = ItemData { flags: F::TYPE_BOOST, type_param: 0, power_param: 10, forme_species: 0 };
    // [447] Silver Powder
    t[447] = ItemData { flags: F::TYPE_BOOST, type_param: 11, power_param: 10, forme_species: 0 };
    // [448] Sitrus Berry
    t[448] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [449] Skull Fossil
    t[449] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [450] Sky Plate
    t[450] = ItemData { flags: F::TYPE_BOOST, type_param: 9, power_param: 90, forme_species: 493 };
    // [453] Smooth Rock
    t[453] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [456] Soft Sand
    t[456] = ItemData { flags: F::TYPE_BOOST, type_param: 8, power_param: 10, forme_species: 0 };
    // [459] Soul Dew
    t[459] = ItemData { flags: F::SIGNATURE_ORB, type_param: 10, power_param: 30, forme_species: 0 };
    // [461] Spell Tag
    t[461] = ItemData { flags: F::TYPE_BOOST, type_param: 13, power_param: 30, forme_species: 0 };
    // [462] Spelon Berry
    t[462] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [463] Splash Plate
    t[463] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 90, forme_species: 493 };
    // [464] Spooky Plate
    t[464] = ItemData { flags: F::TYPE_BOOST, type_param: 13, power_param: 90, forme_species: 493 };
    // [472] Starf Berry
    t[472] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [473] Steel Gem
    t[473] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 16, power_param: 0, forme_species: 0 };
    // [475] Leek
    t[475] = ItemData { flags: 0, type_param: 0xFF, power_param: 60, forme_species: 0 };
    // [476] Sticky Barb
    t[476] = ItemData { flags: F::STICKY_BARB, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [477] Stone Plate
    t[477] = ItemData { flags: F::TYPE_BOOST, type_param: 12, power_param: 90, forme_species: 493 };
    // [480] Sun Stone
    t[480] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [486] Tamato Berry
    t[486] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [487] Tanga Berry
    t[487] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 11, power_param: 10, forme_species: 0 };
    // [491] Thick Club
    t[491] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [492] Thunder Stone
    t[492] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [515] Toxic Orb
    t[515] = ItemData { flags: F::TOXIC_ORB, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [516] Toxic Plate
    t[516] = ItemData { flags: F::TYPE_BOOST, type_param: 7, power_param: 90, forme_species: 493 };
    // [520] Twisted Spoon
    t[520] = ItemData { flags: F::TYPE_BOOST, type_param: 10, power_param: 30, forme_species: 0 };
    // [523] Up-Grade
    t[523] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [526] Wacan Berry
    t[526] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 3, power_param: 10, forme_species: 0 };
    // [528] Water Gem
    t[528] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 2, power_param: 0, forme_species: 0 };
    // [529] Water Stone
    t[529] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [530] Watmel Berry
    t[530] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [531] Wave Incense
    t[531] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 10, forme_species: 0 };
    // [533] Wepear Berry
    t[533] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [535] White Herb
    t[535] = ItemData { flags: F::CONSUMABLE | F::WHITE_HERB, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [537] Wide Lens
    t[537] = ItemData { flags: F::WIDE_LENS, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [538] Wiki Berry
    t[538] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [539] Wise Glasses
    t[539] = ItemData { flags: F::WISE_GLASSES, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [544] Clefablite
    t[544] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [545] Victreebelite
    t[545] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [546] Starminite
    t[546] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [547] Dragoninite
    t[547] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [548] Meganiumite
    t[548] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [549] Feraligite
    t[549] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [550] Skarmorite
    t[550] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [551] Froslassite
    t[551] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [552] Emboarite
    t[552] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [553] Excadrite
    t[553] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [554] Scolipite
    t[554] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [555] Scraftinite
    t[555] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [556] Eelektrossite
    t[556] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [557] Chandelurite
    t[557] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [558] Chesnaughtite
    t[558] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [559] Delphoxite
    t[559] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [560] Greninjite
    t[560] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [561] Pyroarite
    t[561] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [562] Floettite
    t[562] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [563] Malamarite
    t[563] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [564] Barbaracite
    t[564] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [565] Dragalgite
    t[565] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [566] Hawluchanite
    t[566] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [567] Yache Berry
    t[567] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 5, power_param: 10, forme_species: 0 };
    // [568] Zygardite
    t[568] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [569] Drampanite
    t[569] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [570] Falinksite
    t[570] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [572] Zap Plate
    t[572] = ItemData { flags: F::TYPE_BOOST, type_param: 3, power_param: 90, forme_species: 493 };
    // [573] Garchompite
    t[573] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [574] Zoom Lens
    t[574] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [575] Abomasite
    t[575] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [576] Absolite
    t[576] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [577] Aerodactylite
    t[577] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [578] Aggronite
    t[578] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [579] Alakazite
    t[579] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [580] Ampharosite
    t[580] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [581] Assault Vest
    t[581] = ItemData { flags: F::ASSAULT_VEST, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [582] Banettite
    t[582] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [583] Blastoisinite
    t[583] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [584] Blazikenite
    t[584] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [585] Charizardite X
    t[585] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [586] Charizardite Y
    t[586] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [587] Gardevoirite
    t[587] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [588] Gengarite
    t[588] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [589] Gyaradosite
    t[589] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [590] Heracronite
    t[590] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [591] Houndoominite
    t[591] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [592] Kangaskhanite
    t[592] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [593] Kee Berry
    t[593] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [594] Lucarionite
    t[594] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [595] Luminous Moss
    t[595] = ItemData { flags: F::CONSUMABLE | F::LUMINOUS_MOSS, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [596] Manectite
    t[596] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [597] Maranga Berry
    t[597] = ItemData { flags: F::IS_BERRY, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [598] Mawilite
    t[598] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [599] Medichamite
    t[599] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [600] Mewtwonite X
    t[600] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [601] Mewtwonite Y
    t[601] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [602] Pinsirite
    t[602] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [603] Roseli Berry
    t[603] = ItemData { flags: F::CONSUMABLE | F::IS_BERRY | F::RESIST_BERRY, type_param: 17, power_param: 10, forme_species: 0 };
    // [604] Safety Goggles
    t[604] = ItemData { flags: F::SAFETY_GOGGLES, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [605] Scizorite
    t[605] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [606] Snowball
    t[606] = ItemData { flags: F::CONSUMABLE | F::SNOWBALL, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [607] Tyranitarite
    t[607] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [608] Venusaurite
    t[608] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [609] Weakness Policy
    t[609] = ItemData { flags: F::CONSUMABLE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [610] Pixie Plate
    t[610] = ItemData { flags: F::TYPE_BOOST, type_param: 17, power_param: 90, forme_species: 493 };
    // [611] Fairy Gem
    t[611] = ItemData { flags: F::CONSUMABLE | F::GEM, type_param: 17, power_param: 0, forme_species: 0 };
    // [612] Swampertite
    t[612] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [613] Sceptilite
    t[613] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [614] Sablenite
    t[614] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [615] Altarianite
    t[615] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [616] Galladite
    t[616] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [617] Audinite
    t[617] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [618] Metagrossite
    t[618] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [619] Sharpedonite
    t[619] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [620] Slowbronite
    t[620] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [621] Steelixite
    t[621] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [622] Pidgeotite
    t[622] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [623] Glalitite
    t[623] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [624] Diancite
    t[624] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [625] Cameruptite
    t[625] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [626] Lopunnite
    t[626] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [627] Salamencite
    t[627] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [628] Beedrillite
    t[628] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [629] Latiasite
    t[629] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [630] Latiosite
    t[630] = ItemData { flags: F::MEGA_STONE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [631] Normalium Z
    t[631] = ItemData { flags: F::Z_CRYSTAL, type_param: 0xFF, power_param: 0, forme_species: 0 };
    // [632] Firium Z
    t[632] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 1, power_param: 90, forme_species: 0 };
    // [633] Waterium Z
    t[633] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 2, power_param: 90, forme_species: 0 };
    // [634] Electrium Z
    t[634] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 3, power_param: 90, forme_species: 0 };
    // [635] Grassium Z
    t[635] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 4, power_param: 90, forme_species: 0 };
    // [636] Icium Z
    t[636] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 5, power_param: 90, forme_species: 0 };
    // [637] Fightinium Z
    t[637] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 6, power_param: 90, forme_species: 0 };
    // [638] Poisonium Z
    t[638] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 7, power_param: 90, forme_species: 0 };
    // [639] Groundium Z
    t[639] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 8, power_param: 90, forme_species: 0 };
    // [640] Flyinium Z
    t[640] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 9, power_param: 90, forme_species: 0 };
    // [641] Psychium Z
    t[641] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 10, power_param: 90, forme_species: 0 };
    // [642] Buginium Z
    t[642] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 11, power_param: 90, forme_species: 0 };
    // [643] Rockium Z
    t[643] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 12, power_param: 90, forme_species: 0 };
    // [644] Ghostium Z
    t[644] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 13, power_param: 90, forme_species: 0 };
    // [645] Dragonium Z
    t[645] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 14, power_param: 90, forme_species: 0 };
    // [646] Darkinium Z
    t[646] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 15, power_param: 90, forme_species: 0 };
    // [647] Steelium Z
    t[647] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 16, power_param: 90, forme_species: 0 };
    // [648] Fairium Z
    t[648] = ItemData { flags: F::TYPE_BOOST | F::Z_CRYSTAL, type_param: 17, power_param: 90, forme_species: 0 };
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
    // [660] Adrenaline Orb
    t[660] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [662] Terrain Extender
    t[662] = ItemData { flags: 0, type_param: 0xFF, power_param: 60, forme_species: 0 };
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
    t[668] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [669] Flying Memory
    t[669] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [670] Poison Memory
    t[670] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [671] Ground Memory
    t[671] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [672] Rock Memory
    t[672] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [673] Bug Memory
    t[673] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [674] Ghost Memory
    t[674] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [675] Steel Memory
    t[675] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [676] Fire Memory
    t[676] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [677] Water Memory
    t[677] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [678] Grass Memory
    t[678] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [679] Electric Memory
    t[679] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [680] Psychic Memory
    t[680] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [681] Ice Memory
    t[681] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [682] Dragon Memory
    t[682] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [683] Dark Memory
    t[683] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
    // [684] Fairy Memory
    t[684] = ItemData { flags: 0, type_param: 0xFF, power_param: 50, forme_species: 773 };
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
    // [691] Sachet
    t[691] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [692] Whipped Dream
    t[692] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [693] Ice Stone
    t[693] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [694] Jaw Fossil
    t[694] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [695] Sail Fossil
    t[695] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [696] Bottle Cap
    t[696] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [697] Gold Bottle Cap
    t[697] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [698] Rusted Sword
    t[698] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 888 };
    // [699] Rusted Shield
    t[699] = ItemData { flags: 0, type_param: 0xFF, power_param: 0, forme_species: 889 };
    // [700] Fossilized Bird
    t[700] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [701] Fossilized Fish
    t[701] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [702] Fossilized Drake
    t[702] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [703] Fossilized Dino
    t[703] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [704] Strawberry Sweet
    t[704] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [705] Love Sweet
    t[705] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [706] Berry Sweet
    t[706] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [707] Clover Sweet
    t[707] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [709] Star Sweet
    t[709] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [710] Ribbon Sweet
    t[710] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [711] Sweet Apple
    t[711] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [712] Tart Apple
    t[712] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [713] Throat Spray
    t[713] = ItemData { flags: F::CONSUMABLE | F::THROAT_SPRAY, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [714] Eject Pack
    t[714] = ItemData { flags: F::CONSUMABLE | F::EJECT_PACK, type_param: 0xFF, power_param: 50, forme_species: 0 };
    // [715] Heavy-Duty Boots
    t[715] = ItemData { flags: F::HAZARD_IMMUNE, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [716] Blunder Policy
    t[716] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [717] Room Service
    t[717] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [718] Utility Umbrella
    t[718] = ItemData { flags: F::UTILITY_UMBRELLA, type_param: 0xFF, power_param: 60, forme_species: 0 };
    // [719] Cracked Pot
    t[719] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [720] Chipped Pot
    t[720] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [721] TR00
    t[721] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [722] TR07
    t[722] = ItemData { flags: 0, type_param: 0xFF, power_param: 10, forme_species: 0 };
    // [723] TR66
    t[723] = ItemData { flags: 0, type_param: 0xFF, power_param: 120, forme_species: 0 };
    // [724] TR22
    t[724] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [725] TR10
    t[725] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [726] TR63
    t[726] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [727] TR18
    t[727] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [728] TR33
    t[728] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [729] TR31
    t[729] = ItemData { flags: 0, type_param: 0xFF, power_param: 100, forme_species: 0 };
    // [730] TR02
    t[730] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [731] TR03
    t[731] = ItemData { flags: 0, type_param: 0xFF, power_param: 110, forme_species: 0 };
    // [732] TR50
    t[732] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [733] TR08
    t[733] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [734] TR11
    t[734] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [735] TR05
    t[735] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [736] TR24
    t[736] = ItemData { flags: 0, type_param: 0xFF, power_param: 120, forme_species: 0 };
    // [737] TR32
    t[737] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [738] TR90
    t[738] = ItemData { flags: 0, type_param: 0xFF, power_param: 90, forme_species: 0 };
    // [739] Galarica Cuff
    t[739] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [740] Galarica Wreath
    t[740] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [741] Adamant Crystal
    t[741] = ItemData { flags: F::SIGNATURE_ORB, type_param: 16, power_param: 0, forme_species: 483 };
    // [742] Lustrous Globe
    t[742] = ItemData { flags: F::SIGNATURE_ORB, type_param: 2, power_param: 0, forme_species: 484 };
    // [743] Griseous Core
    t[743] = ItemData { flags: F::SIGNATURE_ORB, type_param: 13, power_param: 0, forme_species: 487 };
    // [744] Malicious Armor
    t[744] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
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
    // [753] Auspicious Armor
    t[753] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [754] Fairy Feather
    t[754] = ItemData { flags: F::TYPE_BOOST, type_param: 17, power_param: 10, forme_species: 0 };
    // [755] Syrupy Apple
    t[755] = ItemData { flags: 0, type_param: 0xFF, power_param: 30, forme_species: 0 };
    // [756] Unremarkable Teacup
    t[756] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [757] Masterpiece Teacup
    t[757] = ItemData { flags: 0, type_param: 0xFF, power_param: 80, forme_species: 0 };
    // [758] Cornerstone Mask
    t[758] = ItemData { flags: F::OGERPON_MASK, type_param: 0xFF, power_param: 60, forme_species: 1017 };
    // [759] Wellspring Mask
    t[759] = ItemData { flags: F::OGERPON_MASK, type_param: 0xFF, power_param: 60, forme_species: 1017 };
    // [760] Hearthflame Mask
    t[760] = ItemData { flags: F::OGERPON_MASK, type_param: 0xFF, power_param: 60, forme_species: 1017 };
    t
};

/// Arceus/Silvally forme produced by a held Plate/Memory, indexed by item
/// id. 0 = item produces no forme (revert holder to its base forme).
pub static ITEM_FORME: [u16; 762] = {
    let mut t = [0u16; 762];
    t[105] = 1115;
    t[110] = 1114;
    t[117] = 1123;
    t[143] = 1118;
    t[146] = 1119;
    t[220] = 1124;
    t[223] = 1113;
    t[225] = 1128;
    t[282] = 1122;
    t[291] = 1126;
    t[450] = 1120;
    t[463] = 1129;
    t[464] = 1121;
    t[477] = 1127;
    t[516] = 1125;
    t[572] = 1116;
    t[610] = 1117;
    t[632] = 1119;
    t[633] = 1129;
    t[634] = 1116;
    t[635] = 1122;
    t[636] = 1124;
    t[637] = 1118;
    t[638] = 1125;
    t[639] = 1123;
    t[640] = 1120;
    t[641] = 1126;
    t[642] = 1113;
    t[643] = 1127;
    t[644] = 1121;
    t[645] = 1115;
    t[646] = 1114;
    t[647] = 1128;
    t[648] = 1117;
    t[668] = 1376;
    t[669] = 1378;
    t[670] = 1383;
    t[671] = 1381;
    t[672] = 1385;
    t[673] = 1371;
    t[674] = 1379;
    t[675] = 1386;
    t[676] = 1377;
    t[677] = 1387;
    t[678] = 1380;
    t[679] = 1374;
    t[680] = 1384;
    t[681] = 1382;
    t[682] = 1373;
    t[683] = 1372;
    t[684] = 1375;
    t
};

