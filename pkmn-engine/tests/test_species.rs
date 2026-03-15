use pkmn_engine::data::moves::{move_data, move_meta};
use pkmn_engine::data::{GEN_MOVES, GEN_MOVE_META};

#[test]
fn test_all_moves_valid() {
    assert_eq!(
        GEN_MOVES.len(),
        GEN_MOVE_META.len(),
        "GEN_MOVES and GEN_MOVE_META lengths must match"
    );

    for i in 0..GEN_MOVES.len() {
        let m = move_data(i);
        let meta = move_meta(i);

        let t = m.move_type as u8;
        assert!(t < 18, "Move {} has invalid type: {}", i, t);

        assert!(
            m.accuracy <= 100 || m.accuracy == 0,
            "Move {} has invalid accuracy: {}",
            i,
            m.accuracy
        );
        
        assert!(
            m.crit_ratio <= 2,
            "Move {} has invalid crit_ratio: {}",
            i,
            m.crit_ratio
        );
        
        assert!(
            m.multihit_lo >= 1,
            "Move {} has invalid multihit_lo: {}",
            i,
            m.multihit_lo
        );
        
        assert!(
            m.multihit_hi >= m.multihit_lo,
            "Move {} has multihit_hi < multihit_lo",
            i
        );

        assert!(
            m.secondary_chance <= 100,
            "Move {} has invalid secondary chance: {}",
            i,
            m.secondary_chance
        );

        // Sanity check on pp
        // Moves like Struggle or Z-moves have 0 or 1 PP
        assert!(
            meta.pp <= 40,
            "Move {} has unusually high PP: {}",
            i,
            meta.pp
        );
    }
}
