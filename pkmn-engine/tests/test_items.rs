use pkmn_engine::data::items::ItemFlag::*;
use pkmn_engine::data::items::{item, ItemData};
use pkmn_engine::data::types::Type;

#[test]
fn test_item_none() {
    let none = ItemData::NONE;
    assert_eq!(none.flags, 0);
    assert_eq!(none.type_param, 0xFF);
}

#[test]
fn test_item_0() {
    let i = item(0);
    assert_eq!(i.flags, 0);
}

#[test]
fn test_item_out_of_range() {
    let i = item(9999);
    assert_eq!(i.flags, 0);
    assert_eq!(i.type_param, 0xFF);
}

#[test]
fn test_choice_items() {
    let band = item(68);
    assert!(band.has(CHOICE_ATK));
    assert!(band.has(IS_CHOICE));

    let scarf = item(69);
    assert!(scarf.has(CHOICE_SPE));
    assert!(scarf.has(IS_CHOICE));

    let specs = item(70);
    assert!(specs.has(CHOICE_SPA));
    assert!(specs.has(IS_CHOICE));
}

#[test]
fn test_life_orb() {
    let orb = item(249);
    assert!(orb.has(LIFE_ORB));
}

#[test]
fn test_type_boost_items() {
    // Silk Scarf=444, Charcoal=61, Mystic Water=300, Magnet=273, Miracle Seed=292, Never-Melt Ice=305, Black Belt=32, Poison Barb=343, Soft Sand=456, Sharp Beak=436, Twisted Spoon=520, Silver Powder=447, Hard Stone=187, Spell Tag=461, Dragon Fang=106, Black Glasses=35, Metal Coat=286, Pixie Plate=610.
    let items = vec![
        (444, Type::Normal),
        (61, Type::Fire),
        (300, Type::Water),
        (273, Type::Electric),
        (292, Type::Grass),
        (305, Type::Ice),
        (32, Type::Fighting),
        (343, Type::Poison),
        (456, Type::Ground),
        (436, Type::Flying),
        (520, Type::Psychic),
        (447, Type::Bug),
        (187, Type::Rock),
        (461, Type::Ghost),
        (106, Type::Dragon),
        (35, Type::Dark),
        (286, Type::Steel),
        (610, Type::Fairy),
    ];

    for (id, expected_type) in items {
        let i = item(id);
        assert!(i.has(TYPE_BOOST), "Item {} should have TYPE_BOOST", id);
        assert_eq!(i.type_param, expected_type as u8, "Item {} has wrong type", id);
    }
}

#[test]
fn test_resist_berries() {
    // Chilan=66, Occa=311, Passho=329, Wacan=526, Rindo=409, Yache=567, Chople=71, Kebia=234, Shuca=443, Coba=76, Payapa=330, Tanga=487, Charti=62, Kasib=233, Haban=185, Colbur=78, Babiri=17, Roseli=603.
    let berries = vec![
        (66, Type::Normal),
        (311, Type::Fire),
        (329, Type::Water),
        (526, Type::Electric),
        (409, Type::Grass),
        (567, Type::Ice),
        (71, Type::Fighting),
        (234, Type::Poison),
        (443, Type::Ground),
        (76, Type::Flying),
        (330, Type::Psychic),
        (487, Type::Bug),
        (62, Type::Rock),
        (233, Type::Ghost),
        (185, Type::Dragon),
        (78, Type::Dark),
        (17, Type::Steel),
        (603, Type::Fairy),
    ];

    for (id, expected_type) in berries {
        let b = item(id);
        assert!(b.has(RESIST_BERRY), "Berry {} should have RESIST_BERRY", id);
        assert!(b.has(IS_BERRY), "Berry {} should have IS_BERRY", id);
        assert!(b.has(CONSUMABLE), "Berry {} should have CONSUMABLE", id);
        assert_eq!(b.type_param, expected_type as u8, "Berry {} has wrong type", id);
    }
}

#[test]
fn test_gems() {
    // Gems: Normal=307, Fire=141, Water=528, Electric=120, Grass=172, Ice=218, Fighting=139, Poison=344, Ground=182, Flying=149, Psychic=369, Bug=53, Rock=415, Ghost=161, Dragon=107, Dark=89, Steel=473, Fairy=611.
    let gems = vec![
        (307, Type::Normal),
        (141, Type::Fire),
        (528, Type::Water),
        (120, Type::Electric),
        (172, Type::Grass),
        (218, Type::Ice),
        (139, Type::Fighting),
        (344, Type::Poison),
        (182, Type::Ground),
        (149, Type::Flying),
        (369, Type::Psychic),
        (53, Type::Bug),
        (415, Type::Rock),
        (161, Type::Ghost),
        (107, Type::Dragon),
        (89, Type::Dark),
        (473, Type::Steel),
        (611, Type::Fairy),
    ];

    for (id, expected_type) in gems {
        let g = item(id);
        assert!(g.has(GEM), "Gem {} should have GEM", id);
        assert!(g.has(CONSUMABLE), "Gem {} should have CONSUMABLE", id);
        assert_eq!(g.type_param, expected_type as u8, "Gem {} has wrong type", id);
    }
}

#[test]
fn test_has_method() {
    let sash = item(151);
    assert!(sash.has(FOCUS_SASH));
    assert!(sash.has(CONSUMABLE));
    assert!(!sash.has(CHOICE_ATK));
}

#[test]
fn test_heavy_duty_boots() {
    let boots = item(715);
    assert!(boots.has(HAZARD_IMMUNE));
}
