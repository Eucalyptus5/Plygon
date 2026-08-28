use crate::gen_sets::{SetEntry, SpeciesSets, GEN9_SET_POOL};

#[inline(always)]
pub fn pm_get(m: &[u64; 4], i: usize) -> bool { (m[i >> 6] >> (i & 63)) & 1 != 0 }
#[inline(always)]
pub fn pm_set(m: &mut [u64; 4], i: usize) { m[i >> 6] |= 1u64 << (i & 63); }

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct MonBelief {
    pub species_id: u16,          // 0 = slot not yet revealed
    pub moves: [u16; 4],          // revealed moves (0-padded)
    pub n_moves: u8,
    #[serde(default)]
    pub move_uses: [u8; 4],       // PP spent on moves[k], index-aligned with `moves`
    pub item_id: u16,             // 0 = unknown (v1 offline never learns items)
    #[serde(default)]
    pub scarf_item_id: u16,       // scarf pinned by turn-order deduction; survives an item_id clear
    pub ability_id: u16,          // 0 = unknown
    pub tera_type: u8,            // valid only if tera_revealed
    pub tera_revealed: bool,
    pub level: u8,                // 0 = unknown
    pub excluded_bits: u32,       // negative knowledge (#2/#6/#7): OR of curated bits this mon CANNOT be
    pub pool_mask: [u64; 4],      // live candidate pool (#1), 256-bit: bit i = "set i of this species still possible"
    pub pool_active: bool,        // false until species revealed / #1 prunes; mask ignored while false
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Belief {
    pub mons: [MonBelief; 6],
    #[serde(default)]
    pub screen: ScreenMask,
}

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
    pub fn note_move_use(&mut self, slot: usize, move_id: u16, cost: u8) {
        let m = &mut self.mons[slot];
        if move_id == 0 { return; }
        let Some(k) = m.moves[..m.n_moves as usize].iter().position(|&x| x == move_id) else { return; };
        m.move_uses[k] = m.move_uses[k].saturating_add(cost);
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

// #3 choice-scarf. gen9 randbats fixes the speed spread to 85 EV / 31 IV (verified: distinct pool
// spe-EV {0,85}, spe-IV {0,31}); the 85 EV is load-bearing for precision (252 over-estimates the max
// non-scarf speed by ~37-43 points, soundness-safe but it silently misses every scarf user a defender
// in that band explains).
pub const RANDBATS_SPE_EV: u32 = 85;
pub const RANDBATS_SPE_IV: u32 = 31;

// True IFF the opp's max non-scarf effective speed (caller folds in para/Tailwind/boost via
// mult_num/mult_den) is below ours, i.e. the observed "opp first at equal priority" is impossible
// without a 1.5x item. The integer-floor order mirrors team_builder::calc_stat fed the same EV.
pub fn scarf_forced(opp_sid: u16, opp_level: u8, mult_num: u32, mult_den: u32, our_eff_speed: u32) -> bool {
    let base = pkmn_engine::state::data_bridge::species(opp_sid).spe as u32;
    let raw = (2 * base + RANDBATS_SPE_IV + RANDBATS_SPE_EV / 4) * opp_level as u32 / 100 + 5;
    let max_spe = raw * 11 / 10; // +speed nature
    let opp_eff = max_spe * mult_num / mult_den;
    opp_eff < our_eff_speed
}

// Runtime +1 priority sources absent from the static move_hot().priority field. Without this guard a slow Prankster status-move lead reads as
// equal-priority-opp-first and false-pins choice-scarf. Takes the move the species used; the live
// terrain and the engine's Grassy Glide move id are supplied by the caller (no engine const exists for
// Grassy Glide, and only the caller knows the field).
pub fn can_have_priority_modified(species_id: u16, move_id: u16, grassy_glide_id: u16, terrain_is_grassy: bool) -> bool {
    use pkmn_engine::state::data_bridge;
    if species_can_have_ability(species_id, data_bridge::ABILITY_PRANKSTER) { return true; }
    if move_id == grassy_glide_id && terrain_is_grassy { return true; }
    data_bridge::move_hot(move_id).category == data_bridge::MoveCategory::Status
        && species_can_have_ability(species_id, data_bridge::ABILITY_MYCELIUM_MIGHT)
}

// Which belief components a determinization is allowed to use. Bit set = component ON; the
// default is every component on, so an unconfigured Belief behaves exactly as production does.
// Bit MEANINGS are fixed here and downstream tooling keys off them; never renumber.
pub const SCREEN_C1_POOL_PRUNE: u16   = 1 << 0;
pub const SCREEN_C2_CHOICE_LOCK: u16  = 1 << 1;
pub const SCREEN_C3_IMPOSS_ITEM: u16  = 1 << 2;
pub const SCREEN_C4_IMPOSS_ABIL: u16  = 1 << 3;
pub const SCREEN_C5_ABILITY: u16      = 1 << 4;
pub const SCREEN_C6_ITEM: u16         = 1 << 5;
pub const SCREEN_C7_MOVES: u16        = 1 << 6;
pub const SCREEN_C8_TERA: u16         = 1 << 7;
pub const SCREEN_C9_SCARF: u16        = 1 << 8;
pub const SCREEN_C10_TYPING: u16      = 1 << 9;
pub const SCREEN_C11_GRAFT: u16       = 1 << 10;
pub const SCREEN_ALL: u16 = 0x07FF;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct ScreenMask(pub u16);

impl Default for ScreenMask {
    fn default() -> Self { ScreenMask(SCREEN_ALL) }
}

impl ScreenMask {
    #[inline(always)]
    pub fn on(self, bit: u16) -> bool { self.0 & bit != 0 }
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

// The three disjoint negative-knowledge groups a component screen switches independently.
pub const OTHER_ITEM_BITS: u32 = BIT_ASSAULTVEST | BIT_LIFEORB | BIT_LEFTOVERS | BIT_BLACKSLUDGE
    | BIT_HEAVYDUTYBOOTS | BIT_AIRBALLOON | BIT_BOOSTERENERGY | BIT_FLAMEORB | BIT_TOXICORB
    | BIT_LUMBERRY;
pub const ABILITY_EXCL_BITS: u32 = BIT_INTIMIDATE | BIT_DROUGHT | BIT_DRIZZLE | BIT_SANDSTREAM
    | BIT_SNOWWARNING | BIT_PRESSURE | BIT_NEUTRALIZINGGAS;

// Negative-knowledge set-rejection belief was implemented and washed (2026-06; see .decompose/mcts-exploration/findings-log.md). Preserved on dead-end branch mcts/chance-belief, not merged.
pub fn set_consistent(set: &SetEntry, b: &MonBelief) -> bool {
    if b.tera_revealed && set.tera_type != b.tera_type { return false; }
    if b.ability_id != 0 && set.ability_id != b.ability_id { return false; }
    if b.item_id != 0 && set.item_id != b.item_id { return false; }
    if b.scarf_item_id != 0 && set.item_id != b.scarf_item_id { return false; }
    // negative knowledge: #2 choice-lock, #6 impossible-items, #7 impossible-abilities (01 §2a)
    if set.infer_bits & b.excluded_bits != 0 { return false; }
    b.moves[..b.n_moves as usize].iter().all(|m| set.moves.contains(m))
}

// pool membership (when active) AND negative-knowledge consistency (01 §2)
pub fn possible(set_idx: usize, set: &SetEntry, b: &MonBelief) -> bool {
    (!b.pool_active || pm_get(&b.pool_mask, set_idx)) && set_consistent(set, b)
}

// #1 damage-elim re-exports so both harness layers reach the prune through one path.
pub use crate::belief_calc::Conditions;
pub use crate::belief_prune::{
    apply_prune, attribute_false_elim, bail_ability, compute_survivors, mask_and, mask_is_empty,
    should_bail, DivergenceVerdict, ObservedHit,
};

// Arena A/B switch: BRIDGE_NO_BELIEF_POOL=1 keeps pool_active false so the determinizer samples the
// full set pool (the pre-pool baseline), isolating the pool-belief path. Default off (belief on).
fn belief_pool_disabled() -> bool {
    static D: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *D.get_or_init(|| std::env::var("BRIDGE_NO_BELIEF_POOL").as_deref() == Ok("1"))
}

// Production seam: on species reveal, light every set bit over the species pool and activate the
// mask. Without this the determinizer ignores pool_mask and the prune measures nothing.
// Idempotent: note_species fires from two tracker sites (handle_switch AND handle_detailschange),
// so a forme flip would re-seed and wipe the battle's accumulated prunes. Bail when already active.
pub fn seed_pool(b: &mut MonBelief, species_id: u16) {
    if belief_pool_disabled() {
        return;
    }
    if b.pool_active {
        return;
    }
    let Some(sets) = species_sets_with_base_fallback(species_id) else {
        return;
    };
    b.pool_mask = [0u64; 4];
    for i in 0..sets.sets.len() {
        pm_set(&mut b.pool_mask, i);
    }
    b.pool_active = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_pool_is_idempotent_and_preserves_prunes() {
        const GARCHOMP: u16 = 445;
        let mut b = MonBelief::default();
        seed_pool(&mut b, GARCHOMP);
        assert!(b.pool_active);
        b.pool_mask[0] &= !1u64; // clear bit 0: simulate a prune this battle
        let after_prune = b.pool_mask;
        seed_pool(&mut b, GARCHOMP); // forme flip re-fires note_species
        assert_eq!(b.pool_mask, after_prune, "an active pool must not be re-seeded — prunes preserved");
    }

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
    fn note_move_use_accumulates_per_revealed_move() {
        let mut b = Belief::default();
        let s = b.note_species(445, 78);
        b.note_move(s, 89);
        b.note_move(s, 14);
        b.note_move_use(s, 89, 1);
        b.note_move_use(s, 89, 2);
        b.note_move_use(s, 14, 1);
        assert_eq!(b.mons[s].move_uses[0], 3, "uses land on the revealed move's index");
        assert_eq!(b.mons[s].move_uses[1], 1);
    }

    #[test]
    fn note_move_use_ignores_unknown_moves_and_saturates() {
        let mut b = Belief::default();
        let s = b.note_species(445, 78);
        b.note_move(s, 89);
        b.note_move_use(s, 0, 1);
        b.note_move_use(s, 200, 1);
        assert_eq!(b.mons[s].move_uses, [0; 4], "no move id and an unrevealed move both record nothing");
        b.note_move_use(s, 89, 250);
        b.note_move_use(s, 89, 250);
        assert_eq!(b.mons[s].move_uses[0], 255, "count saturates instead of wrapping");
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
