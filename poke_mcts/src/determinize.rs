use crate::belief::{pm_get, set_consistent, species_sets, Belief, MonBelief};
use crate::gen_sets::{SetEntry, SpeciesSets, GEN9_SET_POOL, GEN9_SET_POOL_TOTAL};
use crate::rng::Lcg;
use pkmn_engine::data::types::Type;
use pkmn_engine::state::data_bridge;
use pkmn_engine::state::*;

pub struct World { pub state: BattleState, pub teams: TeamData, pub weight: f64 }

pub struct Observation<'a> {
    pub state: &'a BattleState,
    pub teams: &'a TeamData,
    pub our_side: usize,
}

pub trait Determinizer {
    fn sample_worlds(&self, obs: &Observation, belief: &Belief, n: usize, rng: &mut Lcg) -> Vec<World>;
}

pub struct RandomBattle;

// Single SetEntry->MonBuildInput source of truth, exposed crate-wide for the calc bridge (#1).
pub(crate) fn input_from_set_pub(species_id: u16, set: &SetEntry) -> MonBuildInput {
    input_from_set(species_id, set)
}

fn input_from_set(species_id: u16, set: &SetEntry) -> MonBuildInput {
    MonBuildInput {
        species_id,
        ability_id: set.ability_id,
        item_id: set.item_id,
        moves: set.moves,
        ivs: set.ivs,
        evs: set.evs,
        nature: 0,
        level: set.level,
        tera_type: set.tera_type,
        is_female: set.is_female,
    }
}

// battle formes (Minior-Meteor, Terapagos-Terastal, ...) are absent from the
// teambuilder-keyed set pool; fall back to the base species' sets
fn pool_for(species_id: u16) -> Option<&'static SpeciesSets> {
    species_sets(species_id).or_else(|| species_sets(data_bridge::base_species(species_id)))
}

pub fn sample_set(species_id: u16, b: &MonBelief, rng: &mut Lcg) -> MonBuildInput {
    let pool = pool_for(species_id)
        .unwrap_or_else(|| panic!("species {species_id} not in set pool — regenerate gen_sets (stale table)"));
    // live candidate pool (#1): when active, only sampled-in static indices survive (01 §2b)
    let consistent: Vec<&SetEntry> = pool.sets.iter().enumerate()
        .filter(|(i, _)| !(b.pool_active && !pm_get(&b.pool_mask, *i)))
        .map(|(_, s)| s)
        .filter(|s| set_consistent(s, b))
        .collect();
    let mut input = if !consistent.is_empty() {
        let total: u32 = consistent.iter().map(|s| s.count).sum();
        let mut r = rng.roll(total.max(1));
        let mut chosen = consistent[consistent.len() - 1];
        for s in &consistent { if r < s.count { chosen = s; break; } r -= s.count; }
        input_from_set(pool.species_id, chosen)
    } else {
        let mut input = input_from_set(pool.species_id, &pool.sets[0]);
        for &mv in b.moves[..b.n_moves as usize].iter() {
            if !input.moves.contains(&mv) {
                let slot = input.moves.iter().position(|m| !b.moves[..b.n_moves as usize].contains(m)).unwrap_or(0);
                input.moves[slot] = mv;
            }
        }
        if b.tera_revealed { input.tera_type = b.tera_type; }
        input
    };
    if b.ability_id != 0 { input.ability_id = b.ability_id; }
    if b.item_id != 0 { input.item_id = b.item_id; }
    if b.level != 0 { input.level = b.level; }
    input
}

fn types_of(species_id: u16) -> (Type, Type) {
    let sp = data_bridge::species(species_id);
    (sp.type1, sp.type2)
}

fn violates_typing(candidate: u16, drawn: &[u16]) -> bool {
    use pkmn_engine::data::types::{dual_type_effectiveness, NUM_TYPES};
    let team: Vec<(Type, Type)> = drawn.iter().chain([&candidate]).map(|&s| types_of(s)).collect();
    let (c1, c2) = types_of(candidate);
    let shares = |t: Type| team.iter().filter(|&&(a, b)| a == t || (b != a && b == t)).count();
    if shares(c1) > 2 || (c2 != c1 && shares(c2) > 2) { return true; }
    for atk_i in 0..NUM_TYPES {
        let atk = unsafe { std::mem::transmute::<u8, Type>(atk_i as u8) };
        if team.iter().filter(|&&(a, b)| dual_type_effectiveness(atk, a, b) > 4).count() > 3 { return true; }
        if team.iter().filter(|&&(a, b)| dual_type_effectiveness(atk, a, b) >= 16).count() > 1 { return true; }
    }
    false
}

pub fn sample_unrevealed_species(taken: &[u16], rng: &mut Lcg) -> u16 {
    let mut attempts = 0u32;
    loop {
        let mut r = (rng.roll(u32::MAX) as u64 | ((rng.roll(u32::MAX) as u64) << 32)) % GEN9_SET_POOL_TOTAL;
        let mut candidate = GEN9_SET_POOL[GEN9_SET_POOL.len() - 1].species_id;
        for sp in GEN9_SET_POOL {
            if r < sp.total_count as u64 { candidate = sp.species_id; break; }
            r -= sp.total_count as u64;
        }
        if taken.contains(&candidate) { attempts += 1; continue; }
        if attempts < 10 && violates_typing(candidate, taken) { attempts += 1; continue; }
        return candidate;
    }
}

fn install(
    state: &mut BattleState,
    teams: &mut TeamData,
    side: usize,
    slot: usize,
    input: &MonBuildInput,
    observed: Option<&MonSlot>,
) {
    let (mut mon, bd) = build_mon(input);
    // showdown_type_to_engine is 18-wide and clamps Stellar (18) to Normal; restore it post-build.
    if input.tera_type == 18 { mon.tera_type = 18; }
    if let Some(true_mon) = observed {
        if true_mon.current_hp == 0 {
            mon.current_hp = 0;
        } else {
            let hp = (true_mon.current_hp as u32 * mon.max_hp as u32 + true_mon.max_hp as u32 / 2)
                / true_mon.max_hp.max(1) as u32;
            mon.current_hp = (hp as u16).clamp(1, mon.max_hp);
        }
        mon.status = true_mon.status;
        mon.status_counter = true_mon.status_counter;
        if true_mon.is_terastallized() { mon.flags |= MON_FLAG_TERASTALLIZED; }
    }
    state.sides[side].team[slot] = mon;
    teams.mons[side][slot] = bd;
    teams.levels[side][slot] = input.level;
}

impl Determinizer for RandomBattle {
    fn sample_worlds(&self, obs: &Observation, belief: &Belief, n: usize, rng: &mut Lcg) -> Vec<World> {
        let opp = 1 - obs.our_side;
        let mut worlds = Vec::with_capacity(n);
        for _ in 0..n {
            let mut state = *obs.state;
            let mut teams = obs.teams.clone();

            let mut filled = [false; 6];
            let mut taken: Vec<u16> = Vec::with_capacity(6);
            for mb in belief.mons.iter().filter(|m| m.species_id != 0) {
                let slot = (0..6)
                    .find(|&i| {
                        let sid = obs.state.sides[opp].team[i].species_id;
                        sid != 0 && data_bridge::base_species(sid) == data_bridge::base_species(mb.species_id)
                    })
                    .expect("belief species must exist on the true opponent side");
                let input = sample_set(mb.species_id, mb, rng);
                install(&mut state, &mut teams, opp, slot, &input, Some(&obs.state.sides[opp].team[slot]));
                filled[slot] = true;
                taken.push(pool_for(mb.species_id).map_or(mb.species_id, |p| p.species_id));
            }
            for slot in 0..6 {
                if filled[slot] { continue; }
                let sp = sample_unrevealed_species(&taken, rng);
                taken.push(sp);
                let input = sample_set(sp, &MonBelief { species_id: sp, ..Default::default() }, rng);
                install(&mut state, &mut teams, opp, slot, &input, None);
            }
            worlds.push(World { state, teams, weight: 1.0 / n as f64 });
        }
        worlds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn sampled_set_respects_reveals() {
        let mut rng = Lcg::new(11);
        let sets = species_sets(445).unwrap();
        let probe = sets.sets[0].moves[0];
        let mut b = MonBelief { species_id: 445, ..Default::default() };
        b.moves[0] = probe; b.n_moves = 1;
        for _ in 0..50 {
            let input = sample_set(445, &b, &mut rng);
            assert!(input.moves.contains(&probe), "revealed move always present");
            assert_eq!(input.species_id, 445);
            assert_eq!(input.nature, 0);
        }
    }

    #[test]
    fn merge_fallback_when_no_set_matches() {
        let mut rng = Lcg::new(11);
        let mut b = MonBelief { species_id: 445, ..Default::default() };
        b.moves[0] = 150; b.n_moves = 1;
        let input = sample_set(445, &b, &mut rng);
        assert!(input.moves.contains(&150), "revealed move grafted onto nearest set");
    }

    #[test]
    fn species_dedup_is_hard() {
        let mut rng = Lcg::new(11);
        let taken = [445u16, 25, 130, 143, 248];
        for _ in 0..200 {
            let s = sample_unrevealed_species(&taken, &mut rng);
            assert!(!taken.contains(&s));
        }
    }

    #[test]
    fn worlds_preserve_public_state_and_vary_hidden() {
        let (mut s, t) = build_state(
            vec![mon(25, 9, [85, 150, 0, 0]), mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 14, 0, 0]), mon(130, 22, [57, 0, 0, 0])],
        );
        s.sides[1].team[0].current_hp /= 2;
        s.sides[1].team[0].status = STATUS_BURN;
        s.sides[0].side_conditions.spikes = 2;
        let mut belief = Belief::default();
        let slot = belief.note_species(445, s.sides[1].team[0].level);
        belief.note_move(slot, 89);

        let obs = Observation { state: &s, our_side: 0, teams: &t };
        let mut rng = Lcg::new(5);
        let worlds = RandomBattle.sample_worlds(&obs, &belief, 8, &mut rng);
        assert_eq!(worlds.len(), 8);
        let mut distinct_opp_teams = std::collections::HashSet::new();
        for w in &worlds {
            assert!((w.weight - 1.0 / 8.0).abs() < 1e-12);
            assert!(w.state.sides[0] == s.sides[0]);
            assert_eq!(w.state.sides[0].side_conditions.spikes, 2);
            let om = &w.state.sides[1].team[0];
            assert_eq!(om.species_id, 445);
            assert_eq!(om.status, STATUS_BURN);
            let f_true = s.sides[1].team[0].current_hp as f64 / s.sides[1].team[0].max_hp as f64;
            let f_w = om.current_hp as f64 / om.max_hp as f64;
            assert!((f_true - f_w).abs() < 0.02, "HP fraction preserved");
            assert!(w.state.sides[1].team.iter().filter(|m| m.species_id != 0).count() == 6,
                "unrevealed slots filled to a full team");
            distinct_opp_teams.insert((1..6).map(|i| w.state.sides[1].team[i].species_id).collect::<Vec<_>>());
        }
        assert!(distinct_opp_teams.len() > 1, "hidden slots actually vary across worlds");
    }

    #[test]
    fn battle_forme_falls_back_to_base_species_pool() {
        // Minior-Meteor (1291) is a build-time forme; its sets live under Minior (774)
        let mut rng = Lcg::new(11);
        let b = MonBelief { species_id: 1291, ..Default::default() };
        let input = sample_set(1291, &b, &mut rng);
        assert_eq!(input.species_id, 774, "input uses the teambuilder species");
    }

    #[test]
    fn forme_flipped_team_slot_still_found() {
        // belief recorded Meteor (1291); the true slot has since flipped to Core (774)
        let (mut s, t) = build_state(
            vec![mon(25, 9, [85, 150, 0, 0])],
            vec![mon(445, 24, [89, 14, 0, 0])],
        );
        s.sides[1].team[0].species_id = 774;
        let mut belief = Belief::default();
        belief.note_species(1291, s.sides[1].team[0].level);
        let obs = Observation { state: &s, our_side: 0, teams: &t };
        let mut rng = Lcg::new(5);
        let worlds = RandomBattle.sample_worlds(&obs, &belief, 2, &mut rng);
        assert_eq!(worlds.len(), 2, "no panic; slot resolved via base species");
    }

    #[test]
    fn typing_softcap_escape_valve_fires() {
        let mut rng = Lcg::new(99);
        for _ in 0..50 {
            let _ = sample_unrevealed_species(&[], &mut rng);
        }
    }
}
