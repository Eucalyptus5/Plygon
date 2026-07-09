use poke_mcts::belief::{pm_set, possible, set_consistent, species_sets, MonBelief};
use poke_mcts::belief::{
    BIT_AIRBALLOON, BIT_ASSAULTVEST, BIT_BLACKSLUDGE, BIT_CHOICEBAND, BIT_CHOICESCARF,
    BIT_CHOICESPECS, BIT_LEFTOVERS, BIT_LIFEORB, BIT_LUMBERRY, CHOICE_ITEMS_MASK,
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
