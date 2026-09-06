use crate::determinize::input_from_set_pub;
use crate::gen_sets::SetEntry;
use pkmn_engine::state::calc::calc_damage;
use pkmn_engine::state::*;

// Battle conditions that change this hit's damage. This struct must carry EVERY modifier
// calc_damage reads; where the tracker knows a condition (weather is public, screens/Tera/our-HP
// are reconstructed), set it on the synthetic state; where it can't be reconstructed, the caller
// bails (belief_prune §4) rather than calling here with stale context.
#[derive(Clone, Copy, Default)]
pub struct Conditions {
    pub weather: u8,
    pub terrain: u8,
    pub def_boosts: [i8; 7],
    pub atk_boosts: [i8; 7],
    pub is_crit: bool,
    pub def_reflect: bool,
    pub def_light_screen: bool,
    pub def_aurora_veil: bool,
    // attacker Tera: stab_modifier raises a Tera-type move's STAB 1.5x -> 2.0x when the attacker
    // is_terastallized(). Reconstructed from opp_tera_used + belief tera.
    pub atk_terastallized: bool,
    pub atk_tera_type: u8,
    // defender current HP: Multiscale/Shadow Shield apply 0.5x ONLY at full HP. 0 = leave full.
    pub def_current_hp: u16,
    // attacker status: burn halves a PHYSICAL hit. 0 = none, 1 = burn.
    pub atk_status: u8,
}

// Force the damage roll to `dmg_roll` (0..16); pin no-crit unless crit, guaranteed-hit (100=>0).
// crit roll is 24-space at stage 0: only rng(24)==0 crits. no-crit picks 23; crit picks 0.
#[inline]
fn forced_roll(dmg_roll: u32, crit: bool) -> impl FnMut(u32) -> u32 {
    move |max: u32| match max {
        16 => dmg_roll,
        24 => {
            if crit {
                0
            } else {
                23
            }
        }
        100 => 0,
        _ => 0,
    }
}

// Build the 2-mon synthetic state with the candidate set as attacker on `atk_side` and our known
// mon as defender on `1-atk_side`; apply the public conditions.
fn synth_state(
    candidate_species: u16,
    candidate: &SetEntry,
    our_known: &MonBuildInput,
    atk_side: usize,
    cond: &Conditions,
) -> (BattleState, TeamData) {
    let attacker = input_from_set_pub(candidate_species, candidate);
    let mut a: [MonBuildInput; 6] = std::array::from_fn(|_| crate::testutil::empty_mon());
    let mut d: [MonBuildInput; 6] = std::array::from_fn(|_| crate::testutil::empty_mon());
    a[0] = if atk_side == 0 { attacker.clone() } else { our_known.clone() };
    d[0] = if atk_side == 0 { our_known.clone() } else { attacker };
    let (t0, b0, l0) = build_team(&a);
    let (t1, b1, l1) = build_team(&d);
    let teams = TeamData { mons: [b0, b1], levels: [l0, l1] };
    let mut state = BattleState::default();
    state.sides[0].team = t0;
    state.sides[1].team = t1;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);
    apply_conditions(&mut state, atk_side, cond);
    (state, teams)
}

#[inline]
fn apply_conditions(state: &mut BattleState, atk_side: usize, cond: &Conditions) {
    state.field.weather = cond.weather;
    state.field.terrain = cond.terrain;
    state.sides[atk_side].active.boosts = cond.atk_boosts;
    let def = 1 - atk_side;
    state.sides[def].active.boosts = cond.def_boosts;
    // screens live on the DEFENDER's side; calc_damage halves through screen_modifier.
    let sc = &mut state.sides[def].side_conditions;
    sc.reflect_turns = if cond.def_reflect { 5 } else { 0 };
    sc.light_screen_turns = if cond.def_light_screen { 5 } else { 0 };
    sc.aurora_veil_turns = if cond.def_aurora_veil { 5 } else { 0 };
    // attacker Tera: is_terastallized() needs the flag AND current_hp != 0; switch_in already
    // gave full HP, so the flag plus the engine-space tera type suffice (stab_modifier).
    if cond.atk_terastallized {
        let am = state.active_mon_mut(atk_side);
        am.flags |= MON_FLAG_TERASTALLIZED;
        am.tera_type = cond.atk_tera_type;
    }
    // defender current HP: only when below full, so Multiscale/Shadow Shield fires iff it did in the
    // real hit (defensive_ability_modifier full-HP gate). Clamp to max_hp.
    if cond.def_current_hp != 0 {
        let dm = state.active_mon_mut(def);
        dm.current_hp = cond.def_current_hp.min(dm.max_hp);
    }
    // attacker status: set AFTER both switch_ins (switch_in clears status). burn_modifier reads
    // atk_mon.status; a burned physical hit is halved.
    state.active_mon_mut(atk_side).status = cond.atk_status;
    // re-fire field-sourced paradox activation AFTER the field assignments; switch_in ran with an
    // empty field so a weather/terrain Protosynthesis/Quark-Drive boost is otherwise absent.
    switch::check_paradox_deactivation(state);
}

// THE bridge entry point. Two calc calls (roll 0 = 85%, roll 15 = 100%) give the exact roll range.
pub fn damage_range(
    candidate_species: u16,
    candidate: &SetEntry,
    our_known: &MonBuildInput,
    move_id: u16,
    atk_side: usize,
    cond: &Conditions,
) -> (u16, u16) {
    let (synth, _teams) = synth_state(candidate_species, candidate, our_known, atk_side, cond);
    let mut lo = forced_roll(0, false); // 85% roll, no crit
    let mut hi = forced_roll(15, cond.is_crit); // 100% roll; crit branch when the hit crit
    let min_dmg = calc_damage(&synth, atk_side, move_id, 100, &mut lo).damage;
    let max_dmg = calc_damage(&synth, atk_side, move_id, 100, &mut hi).damage;
    (min_dmg, max_dmg)
}

// Damage at a single forced roll (0..15), independent of the [min,max] endpoints damage_range pins.
// The cross-crate property harness uses this to source an observed number at a non-endpoint roll.
pub fn damage_at_roll(
    candidate_species: u16,
    candidate: &SetEntry,
    our_known: &MonBuildInput,
    move_id: u16,
    atk_side: usize,
    cond: &Conditions,
    roll: u32,
) -> u16 {
    let (synth, _teams) = synth_state(candidate_species, candidate, our_known, atk_side, cond);
    let mut r = forced_roll(roll, false);
    calc_damage(&synth, atk_side, move_id, 100, &mut r).damage
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_sets::SetEntry;
    use pkmn_engine::state::MonBuildInput;

    // Garchomp (445) Earthquake into a neutral target. SetEntry has no species_id/nature;
    // species is passed separately, nature is pinned 0 by input_from_set.
    fn known_attacker_set() -> SetEntry {
        let mut s = SetEntry::default();
        s.ability_id = 24; // Sand Veil (irrelevant to EQ damage)
        s.item_id = 0;
        s.moves = [89, 0, 0, 0]; // 89 = Earthquake
        s.ivs = [31; 6];
        s.evs = [0, 252, 0, 0, 4, 252];
        s.level = 80;
        s.tera_type = 0;
        s
    }
    fn our_known_defender() -> MonBuildInput {
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

    #[test]
    fn damage_range_is_ordered_and_85_to_100_wide() {
        let (min, max) = damage_range(
            445,
            &known_attacker_set(),
            &our_known_defender(),
            89,
            0,
            &Conditions::default(),
        );
        assert!(min > 0, "a damaging move must deal >0 at the low roll");
        assert!(max >= min, "max roll (100%) must be >= min roll (85%)");
        assert!(
            (max as u32) <= (min as u32) * 100 / 85 + 1,
            "range wider than the roll spread"
        );
    }

    #[test]
    fn reflect_halves_a_physical_hit() {
        let no_screen = Conditions::default();
        let with_reflect = Conditions {
            def_reflect: true,
            ..Default::default()
        };
        let (_, blind_max) = damage_range(445, &known_attacker_set(), &our_known_defender(), 89, 0, &no_screen);
        let (_, veil_max) = damage_range(445, &known_attacker_set(), &our_known_defender(), 89, 0, &with_reflect);
        assert!(
            veil_max < blind_max,
            "Reflect must reduce the physical band; a screen-blind band would over-estimate and wrongly prune the true set"
        );
    }

    #[test]
    fn attacker_tera_stab_widens_a_same_type_hit() {
        let no_tera = Conditions::default();
        let tera_ground = Conditions {
            atk_terastallized: true,
            atk_tera_type: 8, // engine-space Ground
            ..Default::default()
        };
        let (_, plain_max) = damage_range(445, &known_attacker_set(), &our_known_defender(), 89, 0, &no_tera);
        let (tera_min, _) = damage_range(445, &known_attacker_set(), &our_known_defender(), 89, 0, &tera_ground);
        assert!(
            tera_min > plain_max,
            "a Tera-Ground EQ minimum must exceed the no-Tera max; a no-Tera band would wrongly prune the true set on an opp Tera hit"
        );
    }

    #[test]
    fn multiscale_defender_below_full_hp_is_not_halved() {
        let mut def = our_known_defender();
        def.ability_id = data_bridge::ABILITY_MULTISCALE; // an our-mon carrying Multiscale
        let full_hp = Conditions::default(); // def_current_hp 0 -> switch_in's full HP -> halved
        let below_full = Conditions {
            def_current_hp: 1,
            ..Default::default()
        };
        let (_, halved_max) = damage_range(445, &known_attacker_set(), &def, 89, 0, &full_hp);
        let (below_min, _) = damage_range(445, &known_attacker_set(), &def, 89, 0, &below_full);
        assert!(
            below_min > halved_max,
            "below-full HP must drop the Multiscale halving; a full-HP band would wrongly prune the true set on a real below-full hit"
        );
    }

    #[test]
    fn burned_physical_hit_keeps_the_true_set() {
        let no_status = Conditions::default();
        let burned = Conditions {
            atk_status: STATUS_BURN,
            ..Default::default()
        };
        let (full_min, full_max) = damage_range(445, &known_attacker_set(), &our_known_defender(), 89, 0, &no_status);
        let (burn_min, burn_max) = damage_range(445, &known_attacker_set(), &our_known_defender(), 89, 0, &burned);
        let (full_lo, _full_hi) = ((full_min as f64) * 0.975 - 5.0, (full_max as f64) * 1.025 + 5.0);
        let burned_obs = burn_max as f64;
        assert!(
            burned_obs < full_lo,
            "a real burned hit falls below the full-Atk band; that band would wrongly prune the true set"
        );
        let (burn_lo, burn_hi) = ((burn_min as f64) * 0.975 - 5.0, (burn_max as f64) * 1.025 + 5.0);
        assert!(
            burned_obs >= burn_lo && burned_obs <= burn_hi,
            "the burn-modeled band must bracket the burned hit"
        );
        let full_hi = (full_max as f64) * 1.025 + 5.0;
        assert!(full_hi > burn_hi, "burn must halve the physical band");
    }

    #[test]
    fn paradox_under_sun_widens_the_band() {
        let mut s = known_attacker_set();
        s.ability_id = 281; // Protosynthesis
        let no_field = Conditions::default();
        let sun = Conditions {
            weather: WEATHER_SUN,
            ..Default::default()
        };
        let (_, plain_max) = damage_range(984, &s, &our_known_defender(), 89, 0, &no_field);
        let (sun_min, _) = damage_range(984, &s, &our_known_defender(), 89, 0, &sun);
        assert!(
            sun_min > plain_max,
            "a Protosynthesis-under-sun minimum must exceed the no-field max; without check_paradox_deactivation the band would wrongly prune the true set on a real boosted hit"
        );
    }
}
