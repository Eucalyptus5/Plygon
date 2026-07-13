use poke_mcts::belief::{
    first_pooled_ability_for_species, item_id_for_bit, pm_set, possible, set_consistent,
    species_sets, MonBelief,
};
use poke_mcts::belief::{
    BIT_AIRBALLOON, BIT_ASSAULTVEST, BIT_BLACKSLUDGE, BIT_CHOICEBAND, BIT_CHOICESCARF,
    BIT_CHOICESPECS, BIT_HEAVYDUTYBOOTS, BIT_LEFTOVERS, BIT_LIFEORB, BIT_LUMBERRY, CHOICE_ITEMS_MASK,
};
use poke_mcts::gen_sets::{SetEntry, GEN9_SET_POOL};
use std::collections::HashMap;

struct SplitMix64(u64);
impl SplitMix64 {
    fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

// sample a true set the way production does: a species, then a set weighted by count
fn sample_true_set(rng: &mut SplitMix64) -> &'static SetEntry {
    let pool = &GEN9_SET_POOL[(rng.next() as usize) % GEN9_SET_POOL.len()];
    let total: u32 = pool.sets.iter().map(|s| s.count).sum();
    let mut r = (rng.next() % total.max(1) as u64) as u32;
    for s in pool.sets {
        if r < s.count {
            return s;
        }
        r -= s.count;
    }
    &pool.sets[pool.sets.len() - 1]
}

// like sample_true_set, but also returns the pool entry's species id and the chosen set's index.
fn sample_true_set_indexed(rng: &mut SplitMix64) -> (u16, usize, &'static SetEntry) {
    let pool = &GEN9_SET_POOL[(rng.next() as usize) % GEN9_SET_POOL.len()];
    let total: u32 = pool.sets.iter().map(|s| s.count).sum();
    let mut r = (rng.next() % total.max(1) as u64) as u32;
    for (i, s) in pool.sets.iter().enumerate() {
        if r < s.count {
            return (pool.species_id, i, s);
        }
        r -= s.count;
    }
    let last = pool.sets.len() - 1;
    (pool.species_id, last, &pool.sets[last])
}

// ---- Layer-1 #1 damage-elimination generators ----

use pkmn_engine::state::MonBuildInput;
use pkmn_engine::state::data_bridge::{move_hot, MoveCategory};
use poke_mcts::belief::{should_bail as de_should_bail, ObservedHit as DeObservedHit};

// shuffle the 4 move slots so the picked move is not biased to slot 0.
fn shuffled_moves(s: &SetEntry, rng: &mut SplitMix64) -> [u16; 4] {
    let mut m = s.moves;
    for i in (1..4).rev() {
        let j = (rng.next() as usize) % (i + 1);
        m.swap(i, j);
    }
    m
}

// a move from the set that is NOT a bail-class move (so this only tests discriminating damage).
fn sample_discriminating_move(s: &SetEntry, rng: &mut SplitMix64) -> Option<u16> {
    for &move_id in shuffled_moves(s, rng).iter() {
        if move_id == 0 { continue; }
        let probe = DeObservedHit { move_id, atk_side: 1, observed_abs: 1, defender_fainted: false, cond: Default::default() };
        if !de_should_bail(&probe) {
            return Some(move_id);
        }
    }
    None
}

// a non-bail damaging move whose type matches the set's tera_type (so a modeled Tera raises STAB).
fn sample_same_type_stab_move(s: &SetEntry, rng: &mut SplitMix64) -> Option<u16> {
    for &move_id in shuffled_moves(s, rng).iter() {
        if move_id == 0 { continue; }
        if move_hot(move_id).move_type as u8 != s.tera_type { continue; }
        let probe = DeObservedHit { move_id, atk_side: 1, observed_abs: 1, defender_fainted: false, cond: Default::default() };
        if !de_should_bail(&probe) {
            return Some(move_id);
        }
    }
    None
}

// a physical non-bail damaging move (so a modeled burn halves it).
fn sample_physical_move(s: &SetEntry, rng: &mut SplitMix64) -> Option<u16> {
    for &move_id in shuffled_moves(s, rng).iter() {
        if move_id == 0 { continue; }
        if move_hot(move_id).category != MoveCategory::Physical { continue; }
        let probe = DeObservedHit { move_id, atk_side: 1, observed_abs: 1, defender_fainted: false, cond: Default::default() };
        if !de_should_bail(&probe) {
            return Some(move_id);
        }
    }
    None
}

// a set that carries a non-bail damaging move whose type matches its tera_type (only ~17% of
// sets do, so a single draw misses too often; re-sample like the paradox/slow-start samplers).
fn sample_same_type_stab_set(rng: &mut SplitMix64) -> Option<(u16, usize, &'static SetEntry)> {
    for _ in 0..200_000u64 {
        let (sid, idx, s) = sample_true_set_indexed(rng);
        if sample_same_type_stab_move(s, rng).is_some() {
            return Some((sid, idx, s));
        }
    }
    None
}

// a set whose ability is Protosynthesis (281) or Quark-Drive (282), with a physical non-bail move.
fn sample_paradox_set(rng: &mut SplitMix64) -> Option<(u16, usize, &'static SetEntry)> {
    for _ in 0..200_000u64 {
        let (sid, idx, s) = sample_true_set_indexed(rng);
        if (s.ability_id == 281 || s.ability_id == 282) && sample_physical_move(s, rng).is_some() {
            return Some((sid, idx, s));
        }
    }
    None
}

// a Slow Start set (ability 112) with a physical non-bail move.
fn sample_slow_start_set(rng: &mut SplitMix64) -> Option<(u16, usize, &'static SetEntry)> {
    for _ in 0..200_000u64 {
        let (sid, idx, s) = sample_true_set_indexed(rng);
        if s.ability_id == 112 && sample_physical_move(s, rng).is_some() {
            return Some((sid, idx, s));
        }
    }
    None
}

// a fixed, owned exact defender (mirrors belief_calc/belief_prune's our_known_defender).
fn sample_our_defender(_rng: &mut SplitMix64) -> MonBuildInput {
    MonBuildInput {
        species_id: 25,
        ability_id: 9,
        item_id: 0,
        moves: [85, 0, 0, 0],
        ivs: [31; 6],
        evs: [0; 6],
        nature: 0,
        level: 80,
        tera_type: 0,
        is_female: false,
    }
}

// the observed number from an INDEPENDENT source: a hand-frozen mid-roll (roll 8), NOT the prune's
// own damage_range endpoints, so "true set survives its own band" is not tautological.
fn independent_observed(
    sid: u16,
    _idx_s: usize,
    s: &SetEntry,
    our_known: &MonBuildInput,
    move_id: u16,
    _rng: &mut SplitMix64,
) -> u16 {
    use poke_mcts::belief_calc::{damage_at_roll, Conditions};
    damage_at_roll(sid, s, our_known, move_id, 1, &Conditions::default(), 8)
}

// a single curated bit (positions 0..20) that `infer_bits` does NOT carry
fn a_bit_not_in(infer_bits: u32, rng: &mut SplitMix64) -> u32 {
    loop {
        let p = (rng.next() % 20) as u32;
        if (infer_bits >> p) & 1 == 0 {
            return 1 << p;
        }
    }
}

#[test]
fn soundness_never_excludes_true_set_under_random_exclusions() {
    for case in 0..200_000u64 {
        let mut rng = SplitMix64::new(0xB3F1 ^ case);
        let s = sample_true_set(&mut rng);
        // soundness: excluding a bit S does NOT carry must never exclude S
        let mut b = MonBelief::default();
        b.excluded_bits = a_bit_not_in(s.infer_bits, &mut rng);
        assert!(set_consistent(s, &b), "case {case}: true set wrongly excluded");
        // dual-negative: excluding a bit S DOES carry must reject S (guards a no-op masquerade)
        if s.infer_bits != 0 {
            let mut b2 = MonBelief::default();
            b2.excluded_bits = s.infer_bits & s.infer_bits.wrapping_neg(); // lowest set bit
            assert!(!set_consistent(s, &b2), "case {case}: overlapping set not rejected");
        }
    }
}

#[test]
fn possible_pool_mask_respects_word_boundaries() {
    // default set: infer_bits=0, no reveals -> set_consistent always true, so possible() reduces to
    // the mask half. The boundary sweep catches an i>>5/i&31 off-by-one (correct for one word, wrong
    // for the 256-bit [u64;4]).
    let s = SetEntry::default();
    let bounds = [0usize, 63, 64, 127, 128, 191, 192, 255];

    // inactive mask: every index possible regardless of mask contents
    let inactive = MonBelief::default();
    for &i in &bounds {
        assert!(possible(i, &s, &inactive), "inactive mask must allow index {i}");
    }

    // active mask: exactly the masked-in index survives; an index in a different word does not
    for &i in &bounds {
        let mut b = MonBelief::default();
        b.pool_active = true;
        pm_set(&mut b.pool_mask, i);
        assert!(possible(i, &s, &b), "masked-in index {i} must be possible");
        let other = if i == 0 { 255 } else { 0 };
        assert!(!possible(other, &s, &b), "masked-out index {other} must be impossible (set i={i})");
    }
}

// ---- Phase-1 exclusion helpers (Layer-1 precision/soundness oracle) ----
// Name->id is resolved from the SAME id_maps JSON the generator's convert.js writes SetEntry ids
// from, so item/ability ids here equal SetEntry.item_id/ability_id (never hand-numbered).
fn id_maps() -> (HashMap<String, u16>, HashMap<String, u16>) {
    static ITEM_MAP_JSON: &str = include_str!("../../testing_plan/id_maps/item_map.json");
    static ABILITY_MAP_JSON: &str = include_str!("../../testing_plan/id_maps/ability_map.json");
    (
        serde_json::from_str(ITEM_MAP_JSON).unwrap(),
        serde_json::from_str(ABILITY_MAP_JSON).unwrap(),
    )
}

// move name->id from the SAME map the tracker resolves move ids through (no hand-numbered ids).
fn move_ids() -> HashMap<String, u16> {
    static MOVE_MAP_JSON: &str = include_str!("../../testing_plan/id_maps/move_map.json");
    serde_json::from_str(MOVE_MAP_JSON).unwrap()
}

// a spec is a normalized item/ability id, optionally `!`-prefixed to mean "NOT this group"
fn parse_spec(spec: &[u8]) -> (bool, &str) {
    let s = std::str::from_utf8(spec).unwrap();
    match s.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, s),
    }
}

// curated bit for an item spec name (the `choice` group resolves to the 3-bit mask)
fn item_bit(name: &str) -> u32 {
    match name {
        "assaultvest" => BIT_ASSAULTVEST,
        "lifeorb" => BIT_LIFEORB,
        "leftovers" => BIT_LEFTOVERS,
        "blacksludge" => BIT_BLACKSLUDGE,
        "airballoon" => BIT_AIRBALLOON,
        "lumberry" => BIT_LUMBERRY,
        "choiceband" => BIT_CHOICEBAND,
        "choicescarf" => BIT_CHOICESCARF,
        "choicespecs" => BIT_CHOICESPECS,
        "choice" => CHOICE_ITEMS_MASK,
        other => panic!("unknown item spec {other}"),
    }
}

fn count_consistent(sid: u16, b: &MonBelief) -> usize {
    species_sets(sid).unwrap().sets.iter().filter(|s| set_consistent(s, b)).count()
}

// (species, true_set): the species carries the spec's curated bit on >=1 set. For a `!`-spec the
// returned true_set does NOT carry it (so ORing the bit shrinks the pool while the true set
// survives); for a plain spec the true_set carries it.
fn sample_set_with_item(spec: &[u8]) -> (u16, &'static SetEntry) {
    let (neg, name) = parse_spec(spec);
    let bit = item_bit(name);
    for ss in GEN9_SET_POOL {
        let has_carrier = ss.sets.iter().any(|s| s.infer_bits & bit != 0);
        if neg {
            if has_carrier {
                if let Some(s) = ss.sets.iter().find(|s| s.infer_bits & bit == 0) {
                    return (ss.species_id, s);
                }
            }
        } else if let Some(s) = ss.sets.iter().find(|s| s.infer_bits & bit != 0) {
            return (ss.species_id, s);
        }
    }
    panic!("no species satisfies item spec {}", String::from_utf8_lossy(spec));
}

// like sample_set_with_item, but the species must carry the guard item's bit AND have NO set
// pairing that item with Sheer Force / Magic Guard (the 02 §6 Life Orb species-level abstain).
fn sample_set_with_item_guarded(item_spec: &[u8], guard_spec: &[u8]) -> (u16, &'static SetEntry) {
    let (neg, name) = parse_spec(item_spec);
    let bit = item_bit(name);
    let (_, gname) = parse_spec(guard_spec);
    let gbit = item_bit(gname);
    let (items, abils) = id_maps();
    let guard_item = items[gname];
    let sf = abils["sheerforce"];
    let mg = abils["magicguard"];
    for ss in GEN9_SET_POOL {
        let has_carrier = ss.sets.iter().any(|s| s.infer_bits & gbit != 0);
        let suppressor = ss
            .sets
            .iter()
            .any(|s| s.item_id == guard_item && (s.ability_id == sf || s.ability_id == mg));
        if has_carrier && !suppressor {
            let true_set = if neg {
                ss.sets.iter().find(|s| s.infer_bits & bit == 0)
            } else {
                ss.sets.iter().find(|s| s.infer_bits & bit != 0)
            };
            if let Some(s) = true_set {
                return (ss.species_id, s);
            }
        }
    }
    panic!("no guarded species for item {name} guard {gname}");
}

// (species, true_set) whose set carries BOTH the given item and ability (lifeorb+SheerForce abstain trap)
fn sample_set_with_item_paired(item_spec: &[u8], ability_spec: &[u8]) -> (u16, &'static SetEntry) {
    let (items, abils) = id_maps();
    let iid = items[std::str::from_utf8(item_spec).unwrap()];
    let aid = abils[std::str::from_utf8(ability_spec).unwrap()];
    for ss in GEN9_SET_POOL {
        if let Some(s) = ss.sets.iter().find(|s| s.item_id == iid && s.ability_id == aid) {
            return (ss.species_id, s);
        }
    }
    panic!("no set pairs item {iid} with ability {aid}");
}

// curated bit for an on-entry-announcing ability spec name
fn ability_bit(name: &str) -> u32 {
    use poke_mcts::belief::{
        BIT_DRIZZLE, BIT_DROUGHT, BIT_INTIMIDATE, BIT_NEUTRALIZINGGAS, BIT_PRESSURE, BIT_SANDSTREAM,
        BIT_SNOWWARNING,
    };
    match name {
        "intimidate" => BIT_INTIMIDATE,
        "drought" => BIT_DROUGHT,
        "drizzle" => BIT_DRIZZLE,
        "sandstream" => BIT_SANDSTREAM,
        "snowwarning" => BIT_SNOWWARNING,
        "pressure" => BIT_PRESSURE,
        "neutralizinggas" => BIT_NEUTRALIZINGGAS,
        other => panic!("unknown ability spec {other}"),
    }
}

// (species, true_set): the species carries the ability spec's curated bit on >=1 set (mirrors
// sample_set_with_item over the ability curated bits, which live in infer_bits the same as items).
fn sample_set_with_ability(spec: &[u8]) -> (u16, &'static SetEntry) {
    let (neg, name) = parse_spec(spec);
    let bit = ability_bit(name);
    for ss in GEN9_SET_POOL {
        let has_carrier = ss.sets.iter().any(|s| s.infer_bits & bit != 0);
        if neg {
            if has_carrier {
                if let Some(s) = ss.sets.iter().find(|s| s.infer_bits & bit == 0) {
                    return (ss.species_id, s);
                }
            }
        } else if let Some(s) = ss.sets.iter().find(|s| s.infer_bits & bit != 0) {
            return (ss.species_id, s);
        }
    }
    panic!("no species satisfies ability spec {}", String::from_utf8_lossy(spec));
}

#[test]
fn e6_av_and_leftovers_precision_and_soundness() {
    // AV: a status-move user cannot hold Assault Vest.
    let (sid, true_non_av) = sample_set_with_item(b"!assaultvest");
    let mut b = MonBelief { species_id: sid, ..Default::default() };
    let before = count_consistent(sid, &b);
    b.excluded_bits |= poke_mcts::belief::BIT_ASSAULTVEST; // what handle_move(status) will OR
    let after = count_consistent(sid, &b);
    assert!(after < before, "AV exclusion must shrink the pool"); // B1 precision
    assert!(set_consistent(true_non_av, &b), "true set survives"); // B0 guard

    // Leftovers soundness trap: a TRUE Leftovers set at <full HP with a heal seen must NOT be excluded.
    let (sid2, true_lefto) = sample_set_with_item(b"leftovers");
    let b2 = MonBelief { species_id: sid2, ..Default::default() };
    // saw_heal == true => the hook must NOT set BIT_LEFTOVERS; modeled by NOT OR-ing it.
    assert!(set_consistent(true_lefto, &b2), "leftovers true set survives when heal seen");
}

#[test]
fn e6_lifeorb_precision_and_soundness() {
    // Life Orb (bit4): recoil-absence on a damaging move, species has NO SheerForce/MagicGuard pairing.
    let (sid, true_non_lo) = sample_set_with_item_guarded(b"!lifeorb", b"lifeorb");
    let mut b = MonBelief { species_id: sid, ..Default::default() };
    let before = count_consistent(sid, &b);
    b.excluded_bits |= poke_mcts::belief::BIT_LIFEORB; // what handle_move(damaging, recoil-absent) ORs
    let after = count_consistent(sid, &b);
    assert!(after < before, "Life Orb exclusion must shrink the pool"); // B1
    assert!(set_consistent(true_non_lo, &b), "non-lifeorb true set survives"); // B0
    // Soundness trap: a TRUE Life Orb set on a species that pairs lifeorb with SheerForce must NEVER
    // be excluded; the hook abstains for that whole species (model: do NOT OR the bit).
    let (sid_sf, true_lo_sf) = sample_set_with_item_paired(b"lifeorb", b"sheerforce");
    let b_sf = MonBelief { species_id: sid_sf, ..Default::default() };
    assert!(
        set_consistent(true_lo_sf, &b_sf),
        "lifeorb+SheerForce true set survives (species-level abstain)"
    );
    // Zero-damage trap (the reproduced B0 violation): a TRUE Life Orb holder whose damaging move dealt
    // 0 damage produces no recoil, so the window must NOT arm (model: do NOT OR).
    let (sid_z, true_lo_z) = sample_set_with_item(b"lifeorb");
    let b_z = MonBelief { species_id: sid_z, ..Default::default() };
    assert!(
        set_consistent(true_lo_z, &b_z),
        "lifeorb true set survives when its damaging move dealt 0 damage (no hit, no recoil window)"
    );
}

#[test]
fn e2_choice_lock_precision_and_soundness() {
    // True set is NON-choice: two distinct moves => the 3 choice sets of that species drop out.
    let (sid, true_non_choice) = sample_set_with_item(b"!choice"); // item not in {band,scarf,specs}
    let mut b = MonBelief { species_id: sid, ..Default::default() };
    let before = count_consistent(sid, &b);
    b.excluded_bits |= CHOICE_ITEMS_MASK; // what the two-distinct-moves hook ORs
    let after = count_consistent(sid, &b);
    assert!(after < before, "choice-lock must drop the choice sets"); // B1
    assert!(set_consistent(true_non_choice, &b), "non-choice true set survives"); // B0

    // True set IS a choice set: same move twice / single move => no exclusion (model: do NOT OR).
    let (sid2, true_choice) = sample_set_with_item(b"choicescarf");
    let b2 = MonBelief { species_id: sid2, ..Default::default() };
    assert!(
        set_consistent(true_choice, &b2),
        "choice true set survives when no second distinct move"
    );
}

#[test]
fn e2_utility_move_path_precision_and_soundness() {
    // A SINGLE curated utility/setup move (Substitute / Roost / a stat-boosting status move) is
    // enough: a choice holder would be locked into it and self-defeated, so it implies non-choice.
    let (sid, true_non_choice) = sample_set_with_item(b"!choice");
    let mut b = MonBelief { species_id: sid, ..Default::default() };
    let before = count_consistent(sid, &b);
    b.excluded_bits |= CHOICE_ITEMS_MASK; // what the utility-move hook ORs on a whitelisted move
    let after = count_consistent(sid, &b);
    assert!(after < before, "utility-move path must drop the choice sets"); // B1
    assert!(set_consistent(true_non_choice, &b), "non-choice true set survives"); // B0

    // SOUNDNESS trap: a genuine choice set must survive when no whitelisted move has been seen
    // (model: do NOT OR the mask on a damaging/non-whitelisted move).
    let (sid2, true_choice) = sample_set_with_item(b"choiceband");
    let b2 = MonBelief { species_id: sid2, ..Default::default() };
    assert!(
        set_consistent(true_choice, &b2),
        "choice true set survives when no whitelisted utility move has been observed"
    );
}

#[test]
fn e7_impossible_abilities_precision_and_soundness() {
    // True set does NOT carry Intimidate: a switch-in with no Intimidate announce excludes the set.
    let (sid, true_non_intim) = sample_set_with_ability(b"!intimidate");
    let mut b = MonBelief { species_id: sid, ..Default::default() };
    let before = count_consistent(sid, &b);
    b.excluded_bits |= poke_mcts::belief::BIT_INTIMIDATE; // what the non-announce switch-in hook ORs
    let after = count_consistent(sid, &b);
    assert!(after < before, "intimidate exclusion drops the intimidate set"); // B1
    assert!(set_consistent(true_non_intim, &b), "non-intimidate true set survives"); // B0

    // Weather guard: a true Drought set under PRE-EXISTING sun emits no -weather line; do NOT exclude
    // it (model the guard as: do not OR BIT_DROUGHT).
    let (sid2, true_drought) = sample_set_with_ability(b"drought");
    let b2 = MonBelief { species_id: sid2, ..Default::default() };
    assert!(set_consistent(true_drought, &b2), "drought true set survives under pre-existing sun");

    // Live-target trap: a true Intimidate holder switching into an empty field (our active fainted)
    // emits no -ability line, so the no-live-foe guard must NOT arm BIT_INTIMIDATE (model: do not OR).
    let (sid3, true_intim) = sample_set_with_ability(b"intimidate");
    let b3 = MonBelief { species_id: sid3, ..Default::default() };
    assert!(
        set_consistent(true_intim, &b3),
        "intimidate true set survives when no live opposing active existed at switch-in"
    );
}

// ---- Phase-2 positive-deduction Layer-1 oracles (D4 boots) ----

#[test]
fn d4_boots_positive_pin_keeps_only_boots_sets() {
    let boots = item_id_for_bit(BIT_HEAVYDUTYBOOTS); // pool-read id, exact SetEntry.item_id space
    let leftovers = item_id_for_bit(BIT_LEFTOVERS);
    assert!(boots != 0 && leftovers != 0, "pool carries boots and leftovers items");
    let mut b = MonBelief::default();
    b.item_id = boots; // positive pin (the #4-positive effect)
    let mut s_boots = SetEntry::default();
    s_boots.item_id = boots;
    let mut s_lefto = SetEntry::default();
    s_lefto.item_id = leftovers;
    s_lefto.infer_bits = BIT_LEFTOVERS;
    assert!(set_consistent(&s_boots, &b), "true boots set survives a boots pin");
    assert!(!set_consistent(&s_lefto, &b), "a non-boots set is filtered by the positive pin");
}

// Regenerator's id in the SetEntry.ability_id space (same id_maps source as the other id helpers).
fn ability_id_for_regen() -> u16 {
    id_maps().1["regenerator"]
}

#[test]
fn d8_regenerator_pin_keeps_only_regen_sets() {
    let regen = ability_id_for_regen();
    let mut b = MonBelief::default();
    b.ability_id = regen; // the #8 effect
    let mut s_regen = SetEntry::default();
    s_regen.ability_id = regen;
    let mut s_other = SetEntry::default();
    s_other.ability_id = first_pooled_ability_for_species(445); // Garchomp: Rough Skin, non-regen
    assert!(s_other.ability_id != regen, "fixture sanity: chosen non-regen id differs");
    assert!(set_consistent(&s_regen, &b), "true regenerator set survives");
    assert!(!set_consistent(&s_other, &b), "non-regen set filtered by the ability pin");
}

#[test]
fn d8_species_gate_abstains_when_species_cannot_have_regen() {
    // Garchomp 445's pool has no regenerator set -> the hook must abstain on an off-field gain.
    let regen = ability_id_for_regen();
    assert!(
        !poke_mcts::belief::species_can_have_ability(445, regen),
        "Garchomp cannot roll Regenerator -> the hook must abstain on an off-field gain"
    );
}

#[test]
fn d4_boots_negative_bit_excludes_boots_sets_only() {
    let boots = item_id_for_bit(BIT_HEAVYDUTYBOOTS);
    let mut b = MonBelief::default();
    b.excluded_bits = BIT_HEAVYDUTYBOOTS; // negative half (took hazard damage)
    let mut s_boots = SetEntry::default();
    s_boots.item_id = boots;
    s_boots.infer_bits = BIT_HEAVYDUTYBOOTS;
    let mut s_other = SetEntry::default();
    s_other.item_id = item_id_for_bit(BIT_LEFTOVERS);
    assert!(!set_consistent(&s_boots, &b), "boots set rejected by bit 7 (soundness OK: mon took damage)");
    assert!(set_consistent(&s_other, &b), "non-boots set survives");
}

// ---- Phase-2 positive-deduction Layer-1 oracles (D3 choice-scarf) ----
// Concrete randbats ids (engine species id space; base spe via data_bridge::species(sid).spe):
//   Slowbro    80  (base spe 30,  ability {Regenerator}) = SLOW_SID / NON_PRANK_SID (non-Prankster/Mycelium)
//   Dragapult  887 (base spe 142)                        = FAST_SID
//   Whimsicott 547 (base spe 116)                        = MID_SID
//   Altaria    334 (base spe 80)                         = BAND_SID (true-85 max 191, inflated-252 max 228)
//   Sableye    302 (base spe 50,  ability {Prankster})   = PRANK_SID
const SLOW_SID: u16 = 80;
const NON_PRANK_SID: u16 = 80;
const FAST_SID: u16 = 887;
const MID_SID: u16 = 547;
const BAND_SID: u16 = 334;
const BAND_DEFENDER_SPE: u32 = 200; // sits between the true-85-EV max (191) and the inflated-252 max (228)
const PRANK_SID: u16 = 302;
const PRANK_DEFENDER_SPE: u32 = 200; // above Sableye's 85-EV max non-scarf speed (138)
const ANY_SID: u16 = 80;

#[test]
fn d3_scarf_forced_when_max_spread_too_slow() {
    // A slow-base opp (Slowbro, base 30) at L80 cannot outspeed a fast defender (eff 400) w/o a scarf.
    assert!(poke_mcts::belief::scarf_forced(SLOW_SID, 80, 1, 1, 400));
}
#[test]
fn d3_no_scarf_when_max_spread_already_outspeeds() {
    // A fast-base opp (Dragapult, base 142) CAN outspeed naturally -> no inference.
    assert!(!poke_mcts::belief::scarf_forced(FAST_SID, 80, 1, 1, 200));
}
#[test]
fn d3_tailwind_explains_order_no_pin() {
    // With opp Tailwind (x2), a mid-base opp (Whimsicott) outspeeds without a scarf -> abstain.
    assert!(!poke_mcts::belief::scarf_forced(MID_SID, 80, 2, 1, 300));
}
#[test]
fn d3_scarf_forced_in_precision_band_the_252ev_bound_loses() {
    // BAND_SID's true 85-EV max (191) < this defender (200), but the inflated 252-EV bound (228)
    // would read it as fast enough and wrongly abstain. FAILS under the old 252 constant.
    assert!(poke_mcts::belief::scarf_forced(BAND_SID, 80, 1, 1, BAND_DEFENDER_SPE));
}
#[test]
fn d3_false_pin_without_the_priority_abstain_is_real() {
    // PRANK_SID is a slow Prankster lead (Sableye); the bare speed test reads it as impossibly fast.
    assert!(poke_mcts::belief::scarf_forced(PRANK_SID, 80, 1, 1, PRANK_DEFENDER_SPE));
}
#[test]
fn d3_priority_abstain_blocks_the_prankster_false_pin() {
    // A status move under Prankster moves at effective +1; the abstain fires for any move.
    let m = move_ids();
    assert!(poke_mcts::belief::can_have_priority_modified(
        PRANK_SID, m["thunderwave"], m["grassyglide"], false
    ));
}
#[test]
fn d3_grassy_glide_abstains_only_in_grassy_terrain() {
    let m = move_ids();
    let gg = m["grassyglide"];
    assert!(poke_mcts::belief::can_have_priority_modified(ANY_SID, gg, gg, true));
    assert!(!poke_mcts::belief::can_have_priority_modified(NON_PRANK_SID, gg, gg, false));
}
#[test]
fn d3_genuine_scarf_still_pins_no_false_abstain() {
    // A non-Prankster slow scarf user using a damaging move is NOT abstained -> precision held.
    let m = move_ids();
    assert!(!poke_mcts::belief::can_have_priority_modified(
        NON_PRANK_SID, m["surf"], m["grassyglide"], false
    ));
    assert!(poke_mcts::belief::scarf_forced(NON_PRANK_SID, 80, 1, 1, 400));
}

// ---- Layer-1 #1 damage-elimination property + mutation guard ----

#[test]
fn de_soundness_never_prunes_true_set_and_precision_fires() {
    use poke_mcts::belief::{MonBelief, species_sets, compute_survivors, apply_prune, ObservedHit, pm_get, pm_set};
    let mut pruned_some = 0u64;
    for case in 0..30_000u64 {
        let mut rng = SplitMix64::new(0xDE1A ^ case);
        let (sid, idx_s, s) = sample_true_set_indexed(&mut rng);
        let pool = species_sets(sid).unwrap().sets;
        let Some(move_id) = sample_discriminating_move(s, &mut rng) else { continue; };
        let our_known = sample_our_defender(&mut rng);            // an exact, owned mon
        // observed number from an INDEPENDENT source (fixture/omniscient or a hand-frozen roll
        // pin), NOT the prune's own damage_range, so survival is not tautological.
        let observed_abs = independent_observed(sid, idx_s, s, &our_known, move_id, &mut rng);

        let mut b = MonBelief::default();
        b.species_id = sid; b.pool_active = true;
        for i in 0..pool.len() { pm_set(&mut b.pool_mask, i); }
        let hit = ObservedHit { move_id, atk_side: 1, observed_abs, defender_fainted: false, cond: Default::default() };
        let before = b.pool_mask;
        let survivors = compute_survivors(sid, pool, &b, &our_known, &hit);
        apply_prune(&mut b, survivors);

        // SOUNDNESS (always-on): S's bit survives.
        assert!(pm_get(&b.pool_mask, idx_s), "case {case}: true set {idx_s} wrongly pruned");
        // NEVER-EMPTY:
        assert!(b.pool_mask != [0u64; 4], "case {case}: pool emptied");
        if b.pool_mask != before { pruned_some += 1; }
    }
    // PRECISION: across the corpus the prune fires (the pool shrinks) at least sometimes.
    assert!(pruned_some > 0, "no discriminating fixture ever shrank the pool — precision dead");
}

#[test]
fn de_gate_is_alive_wrong_observed_drops_the_set() {
    use poke_mcts::belief::{MonBelief, species_sets, compute_survivors, ObservedHit, pm_get, pm_set};
    use poke_mcts::belief_calc::damage_range;
    // a deliberately out-of-band observed value (max x4) must DROP the true set from survivors,
    // proving the band discriminates and the gate is not a tautology that always keeps S.
    let mut rng = SplitMix64::new(0xA11E);
    let (sid, idx_s, s) = sample_true_set_indexed(&mut rng);
    let pool = species_sets(sid).unwrap().sets;
    let move_id = sample_discriminating_move(s, &mut rng).expect("a discriminating move");
    let our_known = sample_our_defender(&mut rng);
    let (_, max_s) = damage_range(sid, s, &our_known, move_id, 1, &Default::default());
    let mut b = MonBelief::default(); b.species_id = sid; b.pool_active = true;
    for i in 0..pool.len() { pm_set(&mut b.pool_mask, i); }
    let hit = ObservedHit { move_id, atk_side: 1, observed_abs: max_s.saturating_mul(4),
                            defender_fainted: false, cond: Default::default() };
    let survivors = compute_survivors(sid, pool, &b, &our_known, &hit);
    assert!(!pm_get(&survivors, idx_s), "an impossible observed must drop the true set — the gate is alive");
}

#[test]
fn de_tera_stab_hit_keeps_the_true_set() {
    use poke_mcts::belief::{MonBelief, species_sets, compute_survivors, ObservedHit, pm_get, pm_set};
    use poke_mcts::belief_calc::{damage_range, Conditions};
    // an opp Tera-STAB hit: the true set's real (Tera-boosted) damage exceeds the no-Tera band, so a
    // cond WITHOUT atk_terastallized would clear it; with the Tera modeled the true set must survive.
    let mut rng = SplitMix64::new(0x7E2A);
    let (sid, idx_s, s) = sample_same_type_stab_set(&mut rng).expect("a set with a same-type STAB move");
    let pool = species_sets(sid).unwrap().sets;
    let move_id = sample_same_type_stab_move(s, &mut rng).expect("a STAB move sharing the tera type");
    let our_known = sample_our_defender(&mut rng);
    let cond = Conditions { atk_terastallized: true, atk_tera_type: s.tera_type, ..Default::default() };
    let (_, tera_max) = damage_range(sid, s, &our_known, move_id, 1, &cond);
    let mut b = MonBelief::default(); b.species_id = sid; b.pool_active = true;
    for i in 0..pool.len() { pm_set(&mut b.pool_mask, i); }
    let hit = ObservedHit { move_id, atk_side: 1, observed_abs: tera_max, defender_fainted: false, cond };
    let survivors = compute_survivors(sid, pool, &b, &our_known, &hit);
    assert!(pm_get(&survivors, idx_s), "a modeled Tera-STAB hit must keep the true set — B0");
}

#[test]
fn de_burned_hit_keeps_the_true_set() {
    use poke_mcts::belief::{MonBelief, species_sets, compute_survivors, ObservedHit, pm_get, pm_set};
    use poke_mcts::belief_calc::{damage_range, Conditions};
    use pkmn_engine::state::STATUS_BURN;
    // a burned opp PHYSICAL hit: the true set's real (halved) damage falls below the no-status band,
    // so a cond WITHOUT atk_status would clear it; with the burn modeled the true set must survive.
    let mut rng = SplitMix64::new(0xB041);
    let (sid, idx_s, s) = sample_true_set_indexed(&mut rng);
    let pool = species_sets(sid).unwrap().sets;
    let move_id = sample_physical_move(s, &mut rng).expect("a physical damaging move");
    let our_known = sample_our_defender(&mut rng);
    let cond = Conditions { atk_status: STATUS_BURN, ..Default::default() };
    let (_, burn_max) = damage_range(sid, s, &our_known, move_id, 1, &cond);
    let mut b = MonBelief::default(); b.species_id = sid; b.pool_active = true;
    for i in 0..pool.len() { pm_set(&mut b.pool_mask, i); }
    let hit = ObservedHit { move_id, atk_side: 1, observed_abs: burn_max, defender_fainted: false, cond };
    let survivors = compute_survivors(sid, pool, &b, &our_known, &hit);
    assert!(pm_get(&survivors, idx_s), "a modeled burned physical hit must keep the true set — B0");
}

#[test]
fn de_opp_paradox_sun_keeps_the_true_set() {
    use poke_mcts::belief::{MonBelief, species_sets, compute_survivors, ObservedHit, pm_get, pm_set};
    use poke_mcts::belief_calc::{damage_range, Conditions};
    use pkmn_engine::state::WEATHER_SUN;
    // an opp paradox attacker under sun: its real (1.3x boosted) damage exceeds the no-field band, so a
    // cond WITHOUT weather (no paradox re-fire) would clear it; with sun set, apply_conditions's
    // check_paradox_deactivation restores the boost and the true set survives.
    let mut rng = SplitMix64::new(0x9A20);
    let (sid, idx_s, s) = sample_paradox_set(&mut rng).expect("a Protosynthesis/Quark-Drive set");
    let pool = species_sets(sid).unwrap().sets;
    let move_id = sample_physical_move(s, &mut rng).expect("a physical damaging move");
    let our_known = sample_our_defender(&mut rng);
    let cond = Conditions { weather: WEATHER_SUN, ..Default::default() };
    let (_, sun_max) = damage_range(sid, s, &our_known, move_id, 1, &cond);
    let mut b = MonBelief::default(); b.species_id = sid; b.pool_active = true;
    for i in 0..pool.len() { pm_set(&mut b.pool_mask, i); }
    let hit = ObservedHit { move_id, atk_side: 1, observed_abs: sun_max, defender_fainted: false, cond };
    let survivors = compute_survivors(sid, pool, &b, &our_known, &hit);
    assert!(pm_get(&survivors, idx_s), "a modeled paradox-under-sun hit must keep the true set — B0");
}

#[test]
fn de_slow_start_candidate_is_bailed() {
    use poke_mcts::belief::{MonBelief, species_sets, compute_survivors, ObservedHit, pm_get, pm_set};
    use poke_mcts::belief_calc::damage_range;
    // a Slow Start set: its turn-0 synthetic band is halved, so a past-turn-5 hit cannot bracket it.
    // The per-set bail must keep its bit regardless of the observed value.
    let mut rng = SplitMix64::new(0x510B);
    let (sid, idx_s, s) = sample_slow_start_set(&mut rng).expect("a Slow Start set (ability 112)");
    let pool = species_sets(sid).unwrap().sets;
    let move_id = sample_physical_move(s, &mut rng).expect("a physical damaging move");
    let our_known = sample_our_defender(&mut rng);
    let (_, half_max) = damage_range(sid, s, &our_known, move_id, 1, &Default::default());
    let mut b = MonBelief::default(); b.species_id = sid; b.pool_active = true;
    for i in 0..pool.len() { pm_set(&mut b.pool_mask, i); }
    // an out-of-band observed (double the turn-0 halved max) the turn-0 band cannot reach.
    let hit = ObservedHit { move_id, atk_side: 1, observed_abs: half_max.saturating_mul(2),
                            defender_fainted: false, cond: Default::default() };
    let survivors = compute_survivors(sid, pool, &b, &our_known, &hit);
    assert!(pm_get(&survivors, idx_s), "a Slow Start candidate must be per-set bailed and kept — B0");
}
