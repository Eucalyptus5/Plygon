use poke_mcts::belief::*;

// The Rust const bit positions are the single source of truth's mirror; data/belief_curated_bits.json
// holds the bit MEANINGS. This test fails if the two drift apart.
#[test]
fn curated_bits_match_json() {
    // strip whitespace so the JSON layout does not matter
    let json: String = include_str!("../data/belief_curated_bits.json")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    let entries: &[(&str, u32, u32)] = &[
        ("choiceband", 0, BIT_CHOICEBAND),
        ("choicescarf", 1, BIT_CHOICESCARF),
        ("choicespecs", 2, BIT_CHOICESPECS),
        ("assaultvest", 3, BIT_ASSAULTVEST),
        ("lifeorb", 4, BIT_LIFEORB),
        ("leftovers", 5, BIT_LEFTOVERS),
        ("blacksludge", 6, BIT_BLACKSLUDGE),
        ("heavydutyboots", 7, BIT_HEAVYDUTYBOOTS),
        ("airballoon", 8, BIT_AIRBALLOON),
        ("boosterenergy", 9, BIT_BOOSTERENERGY),
        ("flameorb", 10, BIT_FLAMEORB),
        ("toxicorb", 11, BIT_TOXICORB),
        ("lumberry", 12, BIT_LUMBERRY),
        ("intimidate", 13, BIT_INTIMIDATE),
        ("drought", 14, BIT_DROUGHT),
        ("drizzle", 15, BIT_DRIZZLE),
        ("sandstream", 16, BIT_SANDSTREAM),
        ("snowwarning", 17, BIT_SNOWWARNING),
        ("pressure", 18, BIT_PRESSURE),
        ("neutralizinggas", 19, BIT_NEUTRALIZINGGAS),
    ];

    for &(name, bit, konst) in entries {
        assert!(
            json.contains(&format!("\"{name}\":{bit}")),
            "json missing or mismatched entry \"{name}\":{bit}"
        );
        assert_eq!(konst, 1 << bit, "const for {name} != 1 << {bit}");
    }
    assert_eq!(CHOICE_ITEMS_MASK, (1 << 0) | (1 << 1) | (1 << 2));
}
