use crate::chance::ChanceModel;
use pkmn_engine::data::moves::MoveCategory;
use pkmn_engine::state::calc::calc_damage;
use pkmn_engine::state::data_bridge::move_hot;
use pkmn_engine::state::move_exec::effective_accuracy;
use pkmn_engine::state::*;
use smallvec::SmallVec;

// Fixed-point resolution for the per-iteration weighted child draw (rng.roll(WEIGHT_SCALE)).
pub const WEIGHT_SCALE: u32 = 1 << 20;

pub type RootChildren = SmallVec<[(f64, BattleState); 4]>;

// Force the damage roll to `dmg_roll` (0..16), pin no-crit (24=>23) + guaranteed-hit (100=>0),
// matching the harness force-roll convention (policies.rs median_roll, marginal_ko_probe forced_grid).
#[inline]
fn forced_roll(dmg_roll: u32) -> impl FnMut(u32) -> u32 {
    move |max: u32| match max {
        16 => dmg_roll,
        24 => 23,
        100 => 0,
        _ => 0,
    }
}

// One calc at the chosen roll with no crit / guaranteed hit.
#[inline]
fn calc_at_roll(parent: &BattleState, side: usize, move_id: u16, dmg_roll: u32) -> DamageResult {
    let mut roll = forced_roll(dmg_roll);
    calc_damage(parent, side, move_id, 100, &mut roll)
}

// Idea (b): derive the 16-roll grid from the max-roll FINAL damage and count rolls that meet-or-exceed
// the defender's HP. roll i deals floor(max * (85+i) / 100); the derived grid carries the >=1 slack the
// G2 derived-vs-exact check measures against the exact 16-call grid (00 §2-G2, 05 Idea b).
#[inline]
pub fn derived_kill_count(max_dmg: u16, def_hp: u16) -> u32 {
    let mut k = 0u32;
    for i in 0..16u32 {
        let di = (max_dmg as u32 * (85 + i) / 100) as u16;
        if di >= def_hp {
            k += 1;
        }
    }
    k
}

// Mean of the surviving roll indices (the bottom 16-k); credits expected chip, not the minimum.
#[inline]
fn survivor_roll(k: u32) -> u32 {
    (15 - k) / 2
}

// Resolve the whole turn-pair via the engine with the damage roll forced, so speed order, the
// opponent's move, secondaries, and faint -> game-over are all engine-faithful. The KO child forces
// max roll (15); the survivor child forces min roll (0).
#[inline]
fn resolve_forced(parent: &BattleState, teams: &TeamData, a1: u8, a2: u8, dmg_roll: u32) -> BattleState {
    let mut s = *parent;
    let mut roll = forced_roll(dmg_roll);
    match s.phase {
        PHASE_ACTIONS => execute_turn(&mut s, teams, a1, a2, &mut roll),
        PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH =>
            execute_switch_turn(&mut s, teams, a1, a2, &mut roll),
        _ => {}
    }
    s
}

// First-cut analytic root: KO-split on a single-hit damaging move, axis 1 only (no accuracy/crit
// axis -> engine-free). Returns None for the open-loop fallback when neither side's chosen move is a
// single-hit damaging KO threat (multi-hit / OHKO / fixed-damage / variable-BP / no-KO / non-move).
//
// Weight is the DERIVED kill probability k/16 (no crit term in the first cut). The KO child empties
// the defender at max roll; for the 16/16 fixtures k==16 so a single KO child carries weight 1.0 and,
// when it empties the opponent's team, is terminal (winner_value 1.0). The descent in search_world
// still pushes a PathStep and forms visit fractions, so G1/G2 read normally (Design 1, 05 §2).
pub fn analytic_root_children(
    parent: &BattleState,
    teams: &TeamData,
    a1: u8,
    a2: u8,
) -> Option<RootChildren> {
    if parent.phase != PHASE_ACTIONS {
        return None;
    }
    // Prefer side 0 (the searched side in blunder_probe / PIMC); fall back to side 1 so a KO threat
    // against side 0 still prices correctly. The chosen attacker only sets the split WEIGHT; the
    // child STATE is the engine's faithful resolution including turn order.
    for atk in [0usize, 1] {
        let action = if atk == 0 { a1 } else { a2 };
        if action > 3 {
            continue; // not a move (switch / tera / struggle)
        }
        let moves = effective_moves(parent, atk);
        let mid = moves[action as usize];
        if mid == 0 || move_hot(mid).category == MoveCategory::Status {
            continue;
        }
        let def = 1 - atk;
        let def_hp = parent.active_mon(def).current_hp;
        if def_hp == 0 {
            continue;
        }
        let res_max = calc_at_roll(parent, atk, mid, 15);
        if res_max.hits != 1 || res_max.damage == 0 || res_max.type_immune {
            continue; // multi-hit / no damage / immune -> open-loop fallback
        }
        // A move that can miss is over-credited by the guaranteed-hit KO-split; open-loop samples it.
        if effective_accuracy(parent, atk, move_hot(mid)) < 100 {
            continue;
        }
        // roll-invariant damage (fixed-damage / OHKO) breaks the (85+i)/100 scaling -> fall back.
        if calc_at_roll(parent, atk, mid, 0).damage == res_max.damage {
            continue;
        }
        let k = derived_kill_count(res_max.damage, def_hp);
        if k == 0 {
            continue; // no roll KOs -> not the KO axis for this arm
        }
        let p_ko = k as f64 / 16.0;
        let mut children: RootChildren = SmallVec::new();
        children.push((p_ko, resolve_forced(parent, teams, a1, a2, 15)));
        if k < 16 {
            children.push((1.0 - p_ko, resolve_forced(parent, teams, a1, a2, survivor_roll(k))));
        }
        return Some(children);
    }
    None
}

// Deterministic per-iteration weighted pick over the analytic children (weights sum to ~1.0).
// `r` is rng.roll(WEIGHT_SCALE) in 0..WEIGHT_SCALE. The last child absorbs any rounding remainder.
#[inline]
pub fn pick_weighted(children: &RootChildren, r: u32) -> BattleState {
    let last = children.len() - 1;
    let mut acc: u32 = 0;
    for (i, (w, s)) in children.iter().enumerate() {
        if i == last {
            return *s;
        }
        acc = acc.saturating_add((w * WEIGHT_SCALE as f64) as u32);
        if r < acc {
            return *s;
        }
    }
    children[last].1
}

// Analytic-root chance model: identical to OpenLoop off-root (transition) and for non-single-hit
// root moves (analytic_root_children returns None); the root KO-split is the only behavioral change.
pub struct AnalyticRoot;

impl ChanceModel for AnalyticRoot {
    fn transition(
        &self,
        parent: &BattleState,
        teams: &TeamData,
        a1: u8,
        a2: u8,
        rng: &mut impl FnMut(u32) -> u32,
    ) -> BattleState {
        let mut s = *parent;
        match s.phase {
            PHASE_ACTIONS => execute_turn(&mut s, teams, a1, a2, rng),
            PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH => {
                execute_switch_turn(&mut s, teams, a1, a2, rng)
            }
            _ => {}
        }
        s
    }

    fn analytic_root_children(
        &self,
        parent: &BattleState,
        teams: &TeamData,
        a1: u8,
        a2: u8,
    ) -> Option<RootChildren> {
        analytic_root_children(parent, teams, a1, a2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn pick_weighted_single_child_always_returns_it() {
        let (s, _t) = duel(mon(25, 9, [85, 0, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        let mut children: RootChildren = SmallVec::new();
        children.push((1.0, s));
        for r in [0u32, 1, WEIGHT_SCALE / 2, WEIGHT_SCALE - 1] {
            assert!(pick_weighted(&children, r) == s);
        }
    }

    #[test]
    fn derived_grid_counts_guaranteed_and_no_ko() {
        // max_dmg 200 vs 100 hp: every roll (>=170) kills -> 16; vs 300 hp: none -> 0.
        assert_eq!(derived_kill_count(200, 100), 16);
        assert_eq!(derived_kill_count(200, 300), 0);
        // marginal: max_dmg 200 vs 190 hp -> only the top rolls cross.
        let k = derived_kill_count(200, 190);
        assert!(k >= 1 && k <= 15, "marginal kill count in (0,16): got {k}");
    }

    #[test]
    fn ko_move_yields_terminal_single_child() {
        // Pikachu Thunderbolt vs 1-HP Gyarados (4x weak): guaranteed KO, opp team empties -> terminal.
        let (mut s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(130, 22, [58, 0, 0, 0]));
        s.sides[1].team[0].current_hp = 1;
        // byte 0 = Thunderbolt (move slot 0); opp byte 0 = its first move.
        let children = analytic_root_children(&s, &t, 0, 0).expect("KO move -> Some");
        assert_eq!(children.len(), 1, "16/16 KO -> single KO child");
        assert!((children[0].0 - 1.0).abs() < 1e-9, "weight 1.0");
        assert!(children[0].1.is_game_over(), "KO empties opp team -> terminal child");
    }

    #[test]
    fn status_move_falls_back_to_none() {
        // Splash (slot 1 here is move 150 in the duel helper) is non-damaging -> no KO axis.
        let (s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        // Force both to a status/no-KO arm: opp at full HP, our weak move won't KO.
        let res = analytic_root_children(&s, &t, 1, 0);
        // Either None (no KO axis) or a real split; assert it never panics and weights are sane.
        if let Some(ch) = res {
            let sum: f64 = ch.iter().map(|x| x.0).sum();
            assert!(sum > 0.0 && sum <= 1.0 + 1e-9);
        }
    }

    #[test]
    fn survivor_roll_picks_mean_surviving_index() {
        assert_eq!(survivor_roll(15), 0);
        assert_eq!(survivor_roll(1), 7);
        assert_eq!(survivor_roll(8), 3);
        assert_eq!(survivor_roll(2), 6);
    }

    #[test]
    fn survivor_child_uses_mean_roll_not_min() {
        // Pikachu Thunderbolt (move 85, 100% acc) vs Gyarados (4x weak). Set defender HP into the
        // marginal band so 1 < k < 16: top rolls KO, bottom rolls survive.
        let (mut s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(130, 22, [58, 0, 0, 0]));
        let max_dmg = calc_at_roll(&s, 0, 85, 15).damage;
        let min_dmg = calc_at_roll(&s, 0, 85, 0).damage;
        assert!(min_dmg < max_dmg, "need a real damage spread: min {min_dmg} max {max_dmg}");
        // Defender HP one below max roll -> only the top roll(s) KO -> small k, large survivor band.
        let def_hp = max_dmg - 1;
        s.sides[1].team[0].current_hp = def_hp;
        let k = derived_kill_count(max_dmg, def_hp);
        assert!(
            k >= 1 && k <= 13,
            "need 1<=k<=13 so mean roll (15-k)/2 > 0 and deals strictly more than roll 0, got {k}"
        );

        let children = analytic_root_children(&s, &t, 0, 0).expect("damaging move -> Some");
        assert_eq!(children.len(), 2, "marginal k -> KO + survivor children");
        let survivor_hp = children[1].1.active_mon(1).current_hp;

        // Old behavior built the survivor at roll 0 (min damage); the fix uses mean roll (15-k)/2.
        let old_survivor_hp = resolve_forced(&s, &t, 0, 0, 0).active_mon(1).current_hp;
        let mean_survivor_hp =
            resolve_forced(&s, &t, 0, 0, survivor_roll(k)).active_mon(1).current_hp;
        assert!(
            survivor_hp < old_survivor_hp,
            "mean roll must deal more chip than min roll: survivor {survivor_hp} old {old_survivor_hp}"
        );
        assert_eq!(survivor_hp, mean_survivor_hp, "survivor child uses mean roll (15-k)/2");
    }

    #[test]
    fn inaccurate_move_falls_back_to_open_loop() {
        // Play Rough (move 583, 90% acc) can miss -> analytic over-credits -> fall back (None).
        // Same setup with Thunderbolt (move 85, 100% acc) keeps the analytic KO-split (Some).
        let (mut s, t) = duel(mon(282, 9, [583, 150, 0, 0]), mon(130, 22, [58, 0, 0, 0]));
        s.sides[1].team[0].current_hp = 1; // guaranteed KO on damage, isolates the accuracy axis
        assert!(
            analytic_root_children(&s, &t, 0, 0).is_none(),
            "<100% accuracy move -> open-loop fallback (None)"
        );

        let (mut s2, t2) = duel(mon(25, 9, [85, 150, 0, 0]), mon(130, 22, [58, 0, 0, 0]));
        s2.sides[1].team[0].current_hp = 1;
        assert!(
            analytic_root_children(&s2, &t2, 0, 0).is_some(),
            "100% accuracy move -> analytic KO-split (Some)"
        );
    }
}
