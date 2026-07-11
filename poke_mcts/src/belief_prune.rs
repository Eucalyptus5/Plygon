use crate::belief::{pm_get, pm_set, MonBelief};
use crate::belief_calc::{damage_range, Conditions};
use crate::gen_sets::SetEntry;
use pkmn_engine::state::data_bridge::ABILITY_SLOW_START;
use pkmn_engine::state::MonBuildInput;

// One observed damaging hit, assembled by the tracker hook.
pub struct ObservedHit {
    pub move_id: u16,
    pub atk_side: usize,         // side that used the move in the synthetic state
    pub observed_abs: u16,       // absolute HP lost this event
    pub defender_fainted: bool,  // KO this hit -> lower bound is clamped
    pub cond: Conditions,
}

// Default-bail: run the prune ONLY for a positively-classified spread-dependent move (returns
// true => BAIL). An unknown/forgotten move bails (precision loss), never reaching calc to
// false-eliminate the true set (soundness).
pub fn should_bail(hit: &ObservedHit) -> bool {
    use pkmn_engine::data::moves::VarPower; // NOT re-exported via data_bridge
    use pkmn_engine::state::data_bridge::{move_hot, MoveCategory};
    if hit.observed_abs == 0 {
        return true; // no constraint
    }
    if hit.move_id == 0 {
        return true; // unmapped move: unknown true category/power
    }
    let md = move_hot(hit.move_id);
    if md.category == MoveCategory::Status {
        return true; // status category
    }
    const FUTURE_SIGHT: u16 = 248;
    const HIDDEN_POWER: u16 = 237;
    let spread_dependent = md.base_power > 0           // kills Sonic Boom/Dragon Rage/Psywave (bp 0)
        && md.var_power == VarPower::None              // variable-power is not a clean spread function
        && md.multihit == 0                           // multi-hit: per-hit count unknown
        && !is_charge_effect(md.effect)               // +stat charge state makes damage non-standard
        && !matches_bail_effect(md.effect)            // fixed/level/hp-relative/counter-class
        && hit.move_id != HIDDEN_POWER                // type/effectiveness randomized
        && hit.move_id != FUTURE_SIGHT                // bp 120 effect:None slips the bp check
        && !is_transformed_or_forme_active(hit);      // ditto/forme: candidate build invalid
    !spread_dependent
}

// MoveEffect variants whose damage does not depend on the attacker's EV/item spread.
#[inline]
fn matches_bail_effect(e: pkmn_engine::state::data_bridge::MoveEffect) -> bool {
    use pkmn_engine::state::data_bridge::MoveEffect::*;
    matches!(
        e,
        SeismicToss                       // level (Night Shade is tagged SeismicToss too)
            | Ohko
            | Endeavor
            | FinalGambit
            | PainSplit
            | SuperFang
            | SpitUp                      // hp-relative / stockpile
            | Counter
            | MirrorCoat
            | MetalBurst
            | FoulPlay                    // counter-class
            | ChargeMeteorBeam
            | ChargeElectroShot           // +SpA charge
    )
}

// Charge effects: the +stat charge turn makes the executed-turn damage non-standard.
#[inline]
fn is_charge_effect(e: pkmn_engine::state::data_bridge::MoveEffect) -> bool {
    use pkmn_engine::state::data_bridge::MoveEffect::*;
    matches!(
        e,
        ChargeFly
            | ChargeDig
            | ChargeDive
            | ChargePhantom
            | ChargeSkyAttack
            | ChargeSkullBash
            | ChargeMeteorBeam
            | ChargeElectroShot
            | ChargeGeomancy
    )
}

// Transform/forme is not reconstructable from the hit alone; it is bailed upstream in the tracker
// (it knows the opp's live transform/forme state). Kept here so the move-keyed predicate is total.
#[inline]
fn is_transformed_or_forme_active(_hit: &ObservedHit) -> bool {
    false
}

// §3 tolerance band. KEEP a set iff observed_abs is within
// [min*0.975 - 5, max*1.025 + 5]. On a faint (lower bound clamped) drop the lower bound.
#[inline]
fn keeps(observed_abs: u16, min_dmg: u16, max_dmg: u16, defender_fainted: bool) -> bool {
    let lower = (min_dmg as f64) * 0.975 - 5.0;
    let upper = (max_dmg as f64) * 1.025 + 5.0;
    let o = observed_abs as f64;
    let lower_ok = defender_fainted || o >= lower; // KO clamps observed to remaining HP
    let upper_ok = o <= upper;
    lower_ok && upper_ok
}

// Word-wise intersection of two 256-bit masks.
#[inline]
pub fn mask_and(a: [u64; 4], b: [u64; 4]) -> [u64; 4] {
    [a[0] & b[0], a[1] & b[1], a[2] & b[2], a[3] & b[3]]
}
#[inline]
pub fn mask_is_empty(m: &[u64; 4]) -> bool {
    m[0] | m[1] | m[2] | m[3] == 0
}

// Compute the survivors mask over the LIVE pool only (off hot path; <= ~30 sets). Bails => the
// current pool_mask unchanged. Does NOT write pool_mask (DE3's apply_prune owns the never-empty
// write). Loops the FULL 0..pool.len() (never breaks at 64); addresses bits via pm_get/pm_set.
pub fn compute_survivors(
    species_id: u16,
    pool: &[SetEntry],
    b: &MonBelief,
    our_known: &MonBuildInput,
    hit: &ObservedHit,
) -> [u64; 4] {
    if should_bail(hit) {
        return b.pool_mask; // prune nothing
    }
    let mut survivors = [0u64; 4];
    for (i, set) in pool.iter().enumerate() {
        if b.pool_active && !pm_get(&b.pool_mask, i) {
            continue; // already dead
        }
        // Slow Start halves Atk while turns_active < 5; the synthetic is permanently turns_active==0
        // and turns_active is not plumbed, so keep this candidate unconditionally (never a clear).
        if set.ability_id == ABILITY_SLOW_START {
            pm_set(&mut survivors, i);
            continue;
        }
        let (min_dmg, max_dmg) = damage_range(species_id, set, our_known, hit.move_id, hit.atk_side, &hit.cond);
        if keeps(hit.observed_abs, min_dmg, max_dmg, hit.defender_fainted) {
            pm_set(&mut survivors, i);
        }
    }
    survivors
}

// DE2 stub: unconditional word-wise intersection (can empty the pool). DE3 hardens this with the
// never-empty escape + calc-divergence logging. Returns false (no divergence logged yet).
pub fn apply_prune(b: &mut MonBelief, survivors: [u64; 4]) -> bool {
    b.pool_mask = mask_and(b.pool_mask, survivors);
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::belief::MonBelief;
    use crate::gen_sets::SetEntry;
    use pkmn_engine::state::MonBuildInput;

    const GARCHOMP: u16 = 445;
    const SWORDS_DANCE: u16 = 14; // status category

    fn base_eq_set() -> SetEntry {
        let mut s = SetEntry::default();
        s.ability_id = 24;
        s.item_id = 0;
        s.moves = [89, 0, 0, 0]; // Earthquake
        s.ivs = [31; 6];
        s.evs = [0, 252, 0, 0, 4, 252];
        s.level = 80;
        s
    }
    fn band_set() -> SetEntry {
        let mut s = base_eq_set();
        s.item_id = 68; // Choice Band
        s
    }
    fn plain_set() -> SetEntry {
        base_eq_set()
    }
    fn defender() -> MonBuildInput {
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
    fn live_mask(n: usize) -> [u64; 4] {
        let mut m = [0u64; 4];
        for i in 0..n {
            pm_set(&mut m, i);
        }
        m
    }

    #[test]
    fn band_observed_prunes_the_no_item_set() {
        let pool = [plain_set(), band_set()]; // idx 0 = plain, idx 1 = band
        let mut b = MonBelief::default();
        b.species_id = GARCHOMP;
        b.pool_active = true;
        b.pool_mask = live_mask(2);
        let (_, band_max) = damage_range(GARCHOMP, &band_set(), &defender(), 89, 1, &Default::default());
        let hit = ObservedHit {
            move_id: 89,
            atk_side: 1,
            observed_abs: band_max,
            defender_fainted: false,
            cond: Default::default(),
        };
        let survivors = compute_survivors(GARCHOMP, &pool, &b, &defender(), &hit);
        assert!(pm_get(&survivors, 1), "the true (band) set must survive — B0 soundness");
        assert!(!pm_get(&survivors, 0), "the no-item set must be pruned — B1 precision");
    }

    #[test]
    fn status_move_bails_prunes_nothing() {
        let pool = [plain_set(), band_set()];
        let mut b = MonBelief::default();
        b.species_id = GARCHOMP;
        b.pool_active = true;
        b.pool_mask = live_mask(2);
        let hit = ObservedHit {
            move_id: SWORDS_DANCE,
            atk_side: 1,
            observed_abs: 0,
            defender_fainted: false,
            cond: Default::default(),
        };
        let survivors = compute_survivors(GARCHOMP, &pool, &b, &defender(), &hit);
        assert_eq!(survivors, b.pool_mask, "a status/0-damage move must bail — prune NOTHING");
    }

    #[test]
    fn slow_start_candidate_is_per_set_bailed() {
        let mut slow = base_eq_set();
        slow.ability_id = ABILITY_SLOW_START; // 112
        let pool = [slow, plain_set()]; // idx 0 = Slow Start, idx 1 = plain
        let mut b = MonBelief::default();
        b.species_id = GARCHOMP;
        b.pool_active = true;
        b.pool_mask = live_mask(2);
        let (_, plain_max) = damage_range(GARCHOMP, &plain_set(), &defender(), 89, 1, &Default::default());
        let hit = ObservedHit {
            move_id: 89,
            atk_side: 1,
            observed_abs: plain_max,
            defender_fainted: false,
            cond: Default::default(),
        };
        let survivors = compute_survivors(GARCHOMP, &pool, &b, &defender(), &hit);
        assert!(pm_get(&survivors, 0), "a Slow Start candidate must be kept by the per-set bail — B0");
    }

    #[test]
    fn high_index_set_is_tested_not_skipped() {
        // a 70-set pool with only bit 69 live; the prune must evaluate index 69 (word 1, bit 5)
        // and survive its own observed damage, proving the loop never breaks at 64.
        let pool: Vec<SetEntry> = (0..70).map(|_| plain_set()).collect();
        let mut b = MonBelief::default();
        b.species_id = GARCHOMP;
        b.pool_active = true;
        pm_set(&mut b.pool_mask, 69);
        let (_, max69) = damage_range(GARCHOMP, &pool[69], &defender(), 89, 1, &Default::default());
        let hit = ObservedHit {
            move_id: 89,
            atk_side: 1,
            observed_abs: max69,
            defender_fainted: false,
            cond: Default::default(),
        };
        let survivors = compute_survivors(GARCHOMP, &pool, &b, &defender(), &hit);
        assert!(pm_get(&survivors, 69), "the high-index live set must be tested and survive");
    }
}
