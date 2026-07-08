use poke_mcts::gen_sets::GEN9_SET_POOL;
use std::collections::HashMap;

// F3b matches a recorded true set to its index via this key. It MUST resolve a unique row inside
// every species slice or position() returns the WRONG set. is_female is the sole discriminant that
// closes all collisions (dropping it collides 312/507 species); EVs/IVs are never needed. Corpus-
// independent: runs over the generated table, so it catches a key that LOOSENS the match.
#[test]
fn true_set_key_is_unique_within_every_species() {
    let mut ambiguous_groups = 0usize;
    let mut ambiguous_rows = 0usize;
    let mut worst: Option<(u16, usize)> = None;
    for sp in GEN9_SET_POOL {
        let mut seen: HashMap<([u16; 4], u16, u16, u8, u8, bool), usize> = HashMap::new();
        for s in sp.sets {
            let key = (s.moves, s.item_id, s.ability_id, s.tera_type, s.level, s.is_female);
            *seen.entry(key).or_insert(0) += 1;
        }
        for &n in seen.values() {
            if n > 1 {
                ambiguous_groups += 1;
                ambiguous_rows += n;
                if worst.map_or(true, |(_, w)| n > w) {
                    worst = Some((sp.species_id, n));
                }
            }
        }
    }
    assert_eq!(
        ambiguous_groups, 0,
        "true-set key collides: {ambiguous_groups} ambiguous groups ({ambiguous_rows} rows); worst {worst:?}"
    );
}
