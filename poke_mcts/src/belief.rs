use crate::gen_sets::{SetEntry, SpeciesSets, GEN9_SET_POOL};

#[inline(always)]
pub fn pm_get(m: &[u64; 4], i: usize) -> bool { (m[i >> 6] >> (i & 63)) & 1 != 0 }
#[inline(always)]
pub fn pm_set(m: &mut [u64; 4], i: usize) { m[i >> 6] |= 1u64 << (i & 63); }

#[derive(Clone, Copy, Default)]
pub struct MonBelief {
    pub species_id: u16,          // 0 = slot not yet revealed
    pub moves: [u16; 4],          // revealed moves (0-padded)
    pub n_moves: u8,
    pub item_id: u16,             // 0 = unknown (v1 offline never learns items)
    pub ability_id: u16,          // 0 = unknown
    pub tera_type: u8,            // valid only if tera_revealed
    pub tera_revealed: bool,
    pub level: u8,                // 0 = unknown
    pub excluded_bits: u32,       // negative knowledge (#2/#6/#7): OR of curated bits this mon CANNOT be
    pub pool_mask: [u64; 4],      // live candidate pool (#1), 256-bit: bit i = "set i of this species still possible"
    pub pool_active: bool,        // false until species revealed / #1 prunes; mask ignored while false
}

#[derive(Clone, Copy, Default)]
pub struct Belief { pub mons: [MonBelief; 6] }

impl Belief {
    /// Returns the slot index for this species, claiming a fresh slot on first sight.
    pub fn note_species(&mut self, species_id: u16, level: u8) -> usize {
        self.note_species_tracked(species_id, level).0
    }

    /// Like `note_species`, but also reports whether the resolved slot was OVERWRITTEN by a different
    /// mon: `overwrote == true` on a fresh claim or a distinct-base full-slot overwrite (Illusion),
    /// `false` on a same-base re-entry. The caller uses this to drop the prior occupant's per-slot
    /// Tracker state (e.g. #8's leave-HP) so a switched-in mon is never compared against a stranger.
    pub fn note_species_tracked(&mut self, species_id: u16, level: u8) -> (usize, bool) {
        // forme changes (Shields Down, Tera Shift, ...) rewrite the team slot's
        // species_id mid-battle; one belief slot per base species, tracking the latest forme
        let base = pkmn_engine::state::data_bridge::base_species(species_id);
        if let Some(i) = (0..6).find(|&i| {
            self.mons[i].species_id != 0
                && pkmn_engine::state::data_bridge::base_species(self.mons[i].species_id) == base
        }) {
            self.mons[i].species_id = species_id;
            return (i, false);
        }
        // When all slots are filled and no base-species match, an Illusion mon was
        // masquerading as a different species — overwrite the last slot as a fallback.
        let i = (0..6).find(|&i| self.mons[i].species_id == 0).unwrap_or(5);
        self.mons[i] = MonBelief { species_id, level, ..Default::default() };
        (i, true)
    }
    pub fn note_move(&mut self, slot: usize, move_id: u16) {
        let m = &mut self.mons[slot];
        if move_id == 0 || m.moves[..m.n_moves as usize].contains(&move_id) || m.n_moves >= 4 { return; }
        m.moves[m.n_moves as usize] = move_id;
        m.n_moves += 1;
    }
    pub fn note_tera(&mut self, slot: usize, tera_type: u8) {
        self.mons[slot].tera_revealed = true;
        self.mons[slot].tera_type = tera_type;
    }
    pub fn revealed_count(&self) -> usize { (0..6).filter(|&i| self.mons[i].species_id != 0).count() }
}

pub const ENGINE_STELLAR: u8 = 18;   // pkmn_engine::data::types::Type::Stellar as u8
pub const SHOWDOWN_STELLAR: u8 = 18;
pub fn engine_type_to_showdown(t: u8) -> u8 {
    if t == ENGINE_STELLAR { return SHOWDOWN_STELLAR; }
    use pkmn_engine::state::team_builder::showdown_type_to_engine;
    for sd in 0u8..18 { if showdown_type_to_engine(sd) == t { return sd; } }
    0 // matches the forward chart's OOB clamp to Normal
}

pub fn species_sets(species_id: u16) -> Option<&'static SpeciesSets> {
    GEN9_SET_POOL.binary_search_by_key(&species_id, |s| s.species_id).ok().map(|i| &GEN9_SET_POOL[i])
}

// set slice for a species, falling back to the base species (battle formes are absent from the teambuilder-keyed pool)
pub fn species_sets_with_base_fallback(species_id: u16) -> Option<&'static SpeciesSets> {
    species_sets(species_id)
        .or_else(|| species_sets(pkmn_engine::state::data_bridge::base_species(species_id)))
}

// First item_id any pooled set carries the given curated bit on (boots=bit 7, leftovers=bit 5).
// Stays in the exact SetEntry.item_id space; conservative-0 if no pooled set carries it.
pub fn item_id_for_bit(bit: u32) -> u16 {
    GEN9_SET_POOL
        .iter()
        .flat_map(|ss| ss.sets.iter())
        .find(|s| s.infer_bits & bit != 0)
        .map(|s| s.item_id)
        .unwrap_or(0)
}

// Conservative-true on an unrevealed species (no pool) so the caller abstains rather than pin.
pub fn species_can_have_ability(species_id: u16, ability_id: u16) -> bool {
    match species_sets(species_id) {
        Some(ss) => ss.sets.iter().any(|s| s.ability_id == ability_id),
        None => true,
    }
}

// First ability id any pooled set of this species carries (for the D8 Layer-1 non-regen oracle).
// Conservative-0 if the species has no pool.
pub fn first_pooled_ability_for_species(species_id: u16) -> u16 {
    species_sets(species_id)
        .and_then(|ss| ss.sets.first())
        .map(|s| s.ability_id)
        .unwrap_or(0)
}

// Flying is the only type that negates grounded hazards (Spikes) by typing.
pub fn species_is_flying(species_id: u16) -> bool {
    use pkmn_engine::state::team_builder::showdown_type_to_engine;
    let sp = pkmn_engine::state::data_bridge::species(species_id);
    let flying = showdown_type_to_engine(2); // Flying showdown index = 2 (maps.rs)
    sp.type1 as u8 == flying || sp.type2 as u8 == flying
}

// Curated inferable item/ability bit positions (01 §1c). Single source of truth for the bit
// MEANINGS lives in data/belief_curated_bits.json; these MUST match it (F2 adds a drift test).
pub const BIT_CHOICEBAND: u32     = 1 << 0;
pub const BIT_CHOICESCARF: u32    = 1 << 1;
pub const BIT_CHOICESPECS: u32    = 1 << 2;
pub const CHOICE_ITEMS_MASK: u32  = BIT_CHOICEBAND | BIT_CHOICESCARF | BIT_CHOICESPECS;
pub const BIT_ASSAULTVEST: u32    = 1 << 3;
pub const BIT_LIFEORB: u32        = 1 << 4;
pub const BIT_LEFTOVERS: u32      = 1 << 5;
pub const BIT_BLACKSLUDGE: u32    = 1 << 6;
pub const BIT_HEAVYDUTYBOOTS: u32 = 1 << 7;
pub const BIT_AIRBALLOON: u32     = 1 << 8;
pub const BIT_BOOSTERENERGY: u32  = 1 << 9;
pub const BIT_FLAMEORB: u32       = 1 << 10;
pub const BIT_TOXICORB: u32       = 1 << 11;
pub const BIT_LUMBERRY: u32       = 1 << 12;
pub const BIT_INTIMIDATE: u32     = 1 << 13;
pub const BIT_DROUGHT: u32        = 1 << 14;
pub const BIT_DRIZZLE: u32        = 1 << 15;
pub const BIT_SANDSTREAM: u32     = 1 << 16;
pub const BIT_SNOWWARNING: u32    = 1 << 17;
pub const BIT_PRESSURE: u32       = 1 << 18;
pub const BIT_NEUTRALIZINGGAS: u32 = 1 << 19;
// bits 20-31 reserved. No regenerator/gem bit (01 §1c).

// Negative-knowledge set-rejection belief was implemented and washed (2026-06; see .decompose/mcts-exploration/findings-log.md). Preserved on dead-end branch mcts/chance-belief, not merged.
pub fn set_consistent(set: &SetEntry, b: &MonBelief) -> bool {
    if b.tera_revealed && set.tera_type != b.tera_type { return false; }
    if b.ability_id != 0 && set.ability_id != b.ability_id { return false; }
    if b.item_id != 0 && set.item_id != b.item_id { return false; }
    // negative knowledge: #2 choice-lock, #6 impossible-items, #7 impossible-abilities (01 §2a)
    if set.infer_bits & b.excluded_bits != 0 { return false; }
    b.moves[..b.n_moves as usize].iter().all(|m| set.moves.contains(m))
}

// pool membership (when active) AND negative-knowledge consistency (01 §2)
pub fn possible(set_idx: usize, set: &SetEntry, b: &MonBelief) -> bool {
    (!b.pool_active || pm_get(&b.pool_mask, set_idx)) && set_consistent(set, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_species_claims_and_dedups_slots() {
        let mut b = Belief::default();
        let s0 = b.note_species(445, 78);
        let s1 = b.note_species(25, 84);
        assert_ne!(s0, s1);
        assert_eq!(b.note_species(445, 78), s0, "same species, same slot");
        assert_eq!(b.revealed_count(), 2);
    }

    #[test]
    fn forme_flip_shares_one_belief_slot() {
        // Minior: Meteor (1291) <-> Core (774) flips rewrite the true team slot mid-battle
        let mut b = Belief::default();
        let s0 = b.note_species(1291, 80);
        let s1 = b.note_species(774, 80);
        assert_eq!(s0, s1, "same base species, same slot");
        assert_eq!(b.mons[s0].species_id, 774, "latest forme tracked");
        assert_eq!(b.revealed_count(), 1);
    }

    #[test]
    fn note_move_dedups() {
        let mut b = Belief::default();
        let s = b.note_species(445, 78);
        b.note_move(s, 89);
        b.note_move(s, 89);
        assert_eq!(b.mons[s].n_moves, 1);
    }

    #[test]
    fn consistency_narrows_by_moves_and_tera() {
        let set = SetEntry {
            moves: [14, 89, 200, 328], item_id: 234, ability_id: 24,
            level: 78, tera_type: 9, is_female: false, evs: [85; 6], ivs: [31; 6], count: 5,
            infer_bits: 0,
        };
        let mut mb = MonBelief { species_id: 445, ..Default::default() };
        assert!(set_consistent(&set, &mb), "no reveals: everything consistent");
        mb.moves[0] = 89; mb.n_moves = 1;
        assert!(set_consistent(&set, &mb));
        mb.moves[1] = 85; mb.n_moves = 2;
        assert!(!set_consistent(&set, &mb), "revealed move not in set");
        mb.n_moves = 1;
        mb.tera_revealed = true; mb.tera_type = 3;
        assert!(!set_consistent(&set, &mb), "revealed tera type mismatch");
    }

    #[test]
    fn type_map_round_trips_all_showdown_types() {
        let forward = |sd: u8| -> u8 {
            if sd == SHOWDOWN_STELLAR { ENGINE_STELLAR }
            else { pkmn_engine::state::team_builder::showdown_type_to_engine(sd) }
        };
        for sd in 0u8..=18 {
            assert_eq!(engine_type_to_showdown(forward(sd)), sd, "round-trip failed for showdown type {sd}");
        }
    }

    #[test]
    fn pool_lookup_finds_common_species() {
        assert!(species_sets(445).is_some(), "Garchomp is in gen9 randbats");
        assert!(species_sets(0).is_none());
    }

    #[test]
    fn excluded_bits_reject_overlapping_set() {
        let mut b = MonBelief::default();
        b.excluded_bits = 1 << 3; // pretend "assaultvest" bit is excluded
        let mut s = SetEntry::default();      // a set that carries assaultvest
        s.infer_bits = 1 << 3;
        assert!(!set_consistent(&s, &b), "set with an excluded bit must be rejected");
        s.infer_bits = 1 << 4;                // carries a different inferable item
        assert!(set_consistent(&s, &b), "non-overlapping set must remain consistent");
    }
}
