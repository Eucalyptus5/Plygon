//! Generated item data table.
//!
//! Item IDs match Pokémon Showdown's numbering.
//! Populated entries have flags set; all others are ItemData::NONE (flags=0).

use crate::data::items::ItemData;
use crate::data::items::ItemFlag as F;

pub static GEN_ITEMS: [ItemData; 716] = {
    const N: ItemData = ItemData { flags: 0, type_param: 0xFF, power_param: 0, _padding: [0; 2] };
    let mut t = [N; 716];
    // [  6] Air Balloon
    t[6] = ItemData { flags: F::AIR_BALLOON | F::CONSUMABLE, type_param: 0xFF, power_param: 10, _padding: [0; 2] };
    // [ 17] Babiri Berry
    t[17] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 16, power_param: 0, _padding: [0; 2] };
    // [ 31] Binding Band
    t[31] = ItemData { flags: F::BINDING_BOOST, type_param: 0xFF, power_param: 30, _padding: [0; 2] };
    // [ 32] Black Belt
    t[32] = ItemData { flags: F::TYPE_BOOST, type_param: 6, power_param: 30, _padding: [0; 2] };
    // [ 34] Black Sludge
    t[34] = ItemData { flags: F::BLACK_SLUDGE, type_param: 0xFF, power_param: 30, _padding: [0; 2] };
    // [ 35] Black Glasses
    t[35] = ItemData { flags: F::TYPE_BOOST, type_param: 15, power_param: 30, _padding: [0; 2] };
    // [ 53] Bug Gem
    t[53] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 11, power_param: 0, _padding: [0; 2] };
    // [ 61] Charcoal
    t[61] = ItemData { flags: F::TYPE_BOOST, type_param: 1, power_param: 30, _padding: [0; 2] };
    // [ 62] Charti Berry
    t[62] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 12, power_param: 0, _padding: [0; 2] };
    // [ 66] Chilan Berry
    t[66] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 0, power_param: 0, _padding: [0; 2] };
    // [ 68] Choice Band
    t[68] = ItemData { flags: F::CHOICE_ATK, type_param: 0xFF, power_param: 10, _padding: [0; 2] };
    // [ 69] Choice Scarf
    t[69] = ItemData { flags: F::CHOICE_SPE, type_param: 0xFF, power_param: 10, _padding: [0; 2] };
    // [ 70] Choice Specs
    t[70] = ItemData { flags: F::CHOICE_SPA, type_param: 0xFF, power_param: 10, _padding: [0; 2] };
    // [ 71] Chople Berry
    t[71] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 6, power_param: 0, _padding: [0; 2] };
    // [ 76] Coba Berry
    t[76] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 9, power_param: 0, _padding: [0; 2] };
    // [ 78] Colbur Berry
    t[78] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 15, power_param: 0, _padding: [0; 2] };
    // [ 89] Dark Gem
    t[89] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 15, power_param: 0, _padding: [0; 2] };
    // [106] Dragon Fang
    t[106] = ItemData { flags: F::TYPE_BOOST, type_param: 14, power_param: 70, _padding: [0; 2] };
    // [107] Dragon Gem
    t[107] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 14, power_param: 0, _padding: [0; 2] };
    // [120] Electric Gem
    t[120] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 3, power_param: 0, _padding: [0; 2] };
    // [130] Eviolite
    t[130] = ItemData { flags: F::EVIOLITE, type_param: 0xFF, power_param: 40, _padding: [0; 2] };
    // [132] Expert Belt
    t[132] = ItemData { flags: F::EXPERT_BELT, type_param: 0xFF, power_param: 10, _padding: [0; 2] };
    // [139] Fighting Gem
    t[139] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 6, power_param: 0, _padding: [0; 2] };
    // [141] Fire Gem
    t[141] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 1, power_param: 0, _padding: [0; 2] };
    // [145] Flame Orb
    t[145] = ItemData { flags: F::FLAME_ORB, type_param: 0xFF, power_param: 30, _padding: [0; 2] };
    // [149] Flying Gem
    t[149] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 9, power_param: 0, _padding: [0; 2] };
    // [151] Focus Sash
    t[151] = ItemData { flags: F::FOCUS_SASH | F::CONSUMABLE, type_param: 0xFF, power_param: 10, _padding: [0; 2] };
    // [161] Ghost Gem
    t[161] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 13, power_param: 0, _padding: [0; 2] };
    // [172] Grass Gem
    t[172] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 4, power_param: 0, _padding: [0; 2] };
    // [182] Ground Gem
    t[182] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 8, power_param: 0, _padding: [0; 2] };
    // [185] Haban Berry
    t[185] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 14, power_param: 0, _padding: [0; 2] };
    // [187] Hard Stone
    t[187] = ItemData { flags: F::TYPE_BOOST, type_param: 12, power_param: 100, _padding: [0; 2] };
    // [218] Ice Gem
    t[218] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 5, power_param: 0, _padding: [0; 2] };
    // [233] Kasib Berry
    t[233] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 13, power_param: 0, _padding: [0; 2] };
    // [234] Kebia Berry
    t[234] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 7, power_param: 0, _padding: [0; 2] };
    // [242] Leftovers
    t[242] = ItemData { flags: F::LEFTOVERS, type_param: 0xFF, power_param: 10, _padding: [0; 2] };
    // [249] Life Orb
    t[249] = ItemData { flags: F::LIFE_ORB, type_param: 0xFF, power_param: 30, _padding: [0; 2] };
    // [252] Light Clay
    t[252] = ItemData { flags: F::EXTENDS_SCREENS, type_param: 0xFF, power_param: 30, _padding: [0; 2] };
    // [273] Magnet
    t[273] = ItemData { flags: F::TYPE_BOOST, type_param: 3, power_param: 30, _padding: [0; 2] };
    // [286] Metal Coat
    t[286] = ItemData { flags: F::TYPE_BOOST, type_param: 16, power_param: 30, _padding: [0; 2] };
    // [289] Metronome
    t[289] = ItemData { flags: F::METRONOME, type_param: 0xFF, power_param: 30, _padding: [0; 2] };
    // [292] Miracle Seed
    t[292] = ItemData { flags: F::TYPE_BOOST, type_param: 4, power_param: 30, _padding: [0; 2] };
    // [300] Mystic Water
    t[300] = ItemData { flags: F::TYPE_BOOST, type_param: 2, power_param: 30, _padding: [0; 2] };
    // [305] Never-Melt Ice
    t[305] = ItemData { flags: F::TYPE_BOOST, type_param: 5, power_param: 30, _padding: [0; 2] };
    // [307] Normal Gem
    t[307] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 0, power_param: 0, _padding: [0; 2] };
    // [311] Occa Berry
    t[311] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 1, power_param: 0, _padding: [0; 2] };
    // [329] Passho Berry
    t[329] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 2, power_param: 0, _padding: [0; 2] };
    // [330] Payapa Berry
    t[330] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 10, power_param: 0, _padding: [0; 2] };
    // [343] Poison Barb
    t[343] = ItemData { flags: F::TYPE_BOOST, type_param: 7, power_param: 70, _padding: [0; 2] };
    // [344] Poison Gem
    t[344] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 7, power_param: 0, _padding: [0; 2] };
    // [369] Psychic Gem
    t[369] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 10, power_param: 0, _padding: [0; 2] };
    // [382] Razor Claw
    t[382] = ItemData { flags: F::CRIT_BOOST, type_param: 0xFF, power_param: 80, _padding: [0; 2] };
    // [409] Rindo Berry
    t[409] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 4, power_param: 0, _padding: [0; 2] };
    // [415] Rock Gem
    t[415] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 12, power_param: 0, _padding: [0; 2] };
    // [417] Rocky Helmet
    t[417] = ItemData { flags: F::ROCKY_HELMET, type_param: 0xFF, power_param: 60, _padding: [0; 2] };
    // [429] Scope Lens
    t[429] = ItemData { flags: F::CRIT_BOOST, type_param: 0xFF, power_param: 30, _padding: [0; 2] };
    // [436] Sharp Beak
    t[436] = ItemData { flags: F::TYPE_BOOST, type_param: 9, power_param: 50, _padding: [0; 2] };
    // [437] Shed Shell
    t[437] = ItemData { flags: F::TRAP_IMMUNE, type_param: 0xFF, power_param: 10, _padding: [0; 2] };
    // [443] Shuca Berry
    t[443] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 8, power_param: 0, _padding: [0; 2] };
    // [444] Silk Scarf
    t[444] = ItemData { flags: F::TYPE_BOOST, type_param: 0, power_param: 10, _padding: [0; 2] };
    // [447] Silver Powder
    t[447] = ItemData { flags: F::TYPE_BOOST, type_param: 11, power_param: 10, _padding: [0; 2] };
    // [456] Soft Sand
    t[456] = ItemData { flags: F::TYPE_BOOST, type_param: 8, power_param: 10, _padding: [0; 2] };
    // [461] Spell Tag
    t[461] = ItemData { flags: F::TYPE_BOOST, type_param: 13, power_param: 30, _padding: [0; 2] };
    // [473] Steel Gem
    t[473] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 16, power_param: 0, _padding: [0; 2] };
    // [487] Tanga Berry
    t[487] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 11, power_param: 0, _padding: [0; 2] };
    // [515] Toxic Orb
    t[515] = ItemData { flags: F::TOXIC_ORB, type_param: 0xFF, power_param: 30, _padding: [0; 2] };
    // [520] Twisted Spoon
    t[520] = ItemData { flags: F::TYPE_BOOST, type_param: 10, power_param: 30, _padding: [0; 2] };
    // [526] Wacan Berry
    t[526] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 3, power_param: 0, _padding: [0; 2] };
    // [528] Water Gem
    t[528] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 2, power_param: 0, _padding: [0; 2] };
    // [537] Wide Lens
    t[537] = ItemData { flags: F::WIDE_LENS, type_param: 0xFF, power_param: 10, _padding: [0; 2] };
    // [567] Yache Berry
    t[567] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 5, power_param: 0, _padding: [0; 2] };
    // [581] Assault Vest
    t[581] = ItemData { flags: F::ASSAULT_VEST, type_param: 0xFF, power_param: 80, _padding: [0; 2] };
    // [603] Roseli Berry
    t[603] = ItemData { flags: F::RESIST_BERRY | F::IS_BERRY | F::CONSUMABLE, type_param: 17, power_param: 0, _padding: [0; 2] };
    // [604] Safety Goggles
    t[604] = ItemData { flags: F::SAFETY_GOGGLES, type_param: 0xFF, power_param: 80, _padding: [0; 2] };
    // [610] Pixie Plate
    t[610] = ItemData { flags: F::TYPE_BOOST, type_param: 17, power_param: 0, _padding: [0; 2] };
    // [611] Fairy Gem
    t[611] = ItemData { flags: F::GEM | F::CONSUMABLE, type_param: 17, power_param: 0, _padding: [0; 2] };
    // [715] Heavy-Duty Boots
    t[715] = ItemData { flags: F::HAZARD_IMMUNE, type_param: 0xFF, power_param: 80, _padding: [0; 2] };
    t
};
