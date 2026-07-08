use crate::gen_sets::{SetEntry, SpeciesSets, GEN9_SET_POOL};

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
}

#[derive(Clone, Copy, Default)]
pub struct Belief { pub mons: [MonBelief; 6] }

impl Belief {
    /// Returns the slot index for this species, claiming a fresh slot on first sight.
    pub fn note_species(&mut self, species_id: u16, level: u8) -> usize {
        // forme changes (Shields Down, Tera Shift, ...) rewrite the team slot's
        // species_id mid-battle; one belief slot per base species, tracking the latest forme
        let base = pkmn_engine::state::data_bridge::base_species(species_id);
        if let Some(i) = (0..6).find(|&i| {
            self.mons[i].species_id != 0
                && pkmn_engine::state::data_bridge::base_species(self.mons[i].species_id) == base
        }) {
            self.mons[i].species_id = species_id;
            return i;
        }
        // When all slots are filled and no base-species match, an Illusion mon was
        // masquerading as a different species — overwrite the last slot as a fallback.
        let i = (0..6).find(|&i| self.mons[i].species_id == 0).unwrap_or(5);
        self.mons[i] = MonBelief { species_id, level, ..Default::default() };
        i
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

// Negative-knowledge set-rejection belief was implemented and washed (2026-06; see .decompose/mcts-exploration/findings-log.md). Preserved on dead-end branch mcts/chance-belief, not merged.
pub fn set_consistent(set: &SetEntry, b: &MonBelief) -> bool {
    if b.tera_revealed && set.tera_type != b.tera_type { return false; }
    if b.ability_id != 0 && set.ability_id != b.ability_id { return false; }
    if b.item_id != 0 && set.item_id != b.item_id { return false; }
    b.moves[..b.n_moves as usize].iter().all(|m| set.moves.contains(m))
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
}
