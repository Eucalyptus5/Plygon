// Replay reconstructed blunder turns through the full production PIMC decision path and
// dump the internals the live log hides: per-world effective iterations + determinized
// opponent, the aggregated root arm table (visit fraction + mean avg_score), the 0.75
// survivors, and the pick. Partitions the live blunders into: pick-degeneracy (blunder
// clears the 0.75 filter on a flat distribution) vs determinization (the better move
// doesn't win in the sampled worlds) vs the search genuinely ranking the blunder higher.
//
// Run: cargo run --release --bin blunder_probe -- [case-name-substr]
// Env: PROBE_MS (per-world ms, default 4000 = live), PROBE_ITERS (default 100M),
//      PROBE_WORLDS (default 8), PROBE_SEED (default 0xB1U), CHANCE=open|closed|both.
use poke_mcts::belief::Belief;
use poke_mcts::determinize::{Determinizer, Observation, RandomBattle, World};
use poke_mcts::driver::{choose_action_traced, DecisionTrace, PickMode, PimcConfig};
use poke_mcts::rng::Lcg;
use poke_mcts::search::ChanceMode;
use pkmn_engine::state::*;

// Full-info control determinizer (E1 1-world control): one world that IS the fixture's actual
// opponent (single mon, no bench reconstruction), so a KO of the active empties the opp team and is
// terminal. Used to read the analytic root value against the true KO prob without belief sampling.
struct TrueState;
impl Determinizer for TrueState {
    fn sample_worlds(&self, obs: &Observation, _belief: &Belief, _n: usize, _rng: &mut Lcg) -> Vec<World> {
        vec![World { state: *obs.state, teams: obs.teams.clone(), weight: 1.0 }]
    }
}

// Last-mon determinizer: sample the normal 8-world belief opponent via RandomBattle, then faint the
// opp BENCH in each world so a root KO of the active empties the opp team (winner_value -> terminal).
// Used only for fixtures flagged `last_mon`; the opp-active set sampling is preserved across seeds.
// RandomBattle always fills the bench to 6 (determinize.rs), so the fixture's empty bench cannot
// encode "last mon" by itself — this wrapper is the poke_mcts-only way to get a terminal root KO
// under the belief-sampled path (FULLINFO=1 + TrueState is the alternative zero-code single-world path).
struct LastMonRandomBattle;
impl Determinizer for LastMonRandomBattle {
    fn sample_worlds(&self, obs: &Observation, belief: &Belief, n: usize, rng: &mut Lcg) -> Vec<World> {
        let mut worlds = RandomBattle.sample_worlds(obs, belief, n, rng);
        let opp = 1 - obs.our_side;
        for w in &mut worlds {
            let ai = w.state.sides[opp].active_index as usize;
            for slot in 0..6 {
                if slot != ai {
                    w.state.sides[opp].team[slot].current_hp = 0;
                }
            }
        }
        worlds
    }
}

#[derive(serde::Deserialize)]
struct MonSpec {
    species: u16,
    level: u8,
    #[serde(default)] ability: u16,
    #[serde(default)] item: u16,
    moves: [u16; 4],
    #[serde(default)] evs: [u8; 6],
}
#[derive(serde::Deserialize)]
struct OppSpec {
    species: u16,
    level: u8,
    #[serde(default)] ability: u16,
    #[serde(default)] item: u16,
    #[serde(default)] moves: [u16; 4],
    #[serde(default)] moves_revealed: Vec<u16>,
    hp_num: u32,
    hp_den: u32,
    #[serde(default)] status: String,
}
#[derive(serde::Deserialize)]
struct Case {
    name: String,
    #[serde(default)] note: String,
    our: Vec<MonSpec>,
    #[serde(default)] our_boosts: Option<[i8; 7]>,
    opp: OppSpec,
    blunder_action: u8,
    better_action: u8,
    #[serde(default)] last_mon: bool,
}
#[derive(serde::Deserialize)]
struct Suite { cases: Vec<Case> }

fn status_code(s: &str) -> u8 {
    match s {
        "brn" => STATUS_BURN,
        "par" => STATUS_PARALYSIS,
        "psn" => STATUS_POISON,
        "tox" => 4,
        "slp" => STATUS_SLEEP,
        "frz" => STATUS_FREEZE,
        _ => STATUS_NONE,
    }
}

fn input(species: u16, level: u8, ability: u16, item: u16, moves: [u16; 4], evs: [u8; 6]) -> MonBuildInput {
    MonBuildInput { species_id: species, ability_id: ability, item_id: item, moves,
        ivs: [31; 6], evs, nature: 0, level, tera_type: 0, is_female: false }
}
fn empty() -> MonBuildInput {
    MonBuildInput { species_id: 0, ability_id: 0, item_id: 0, moves: [0; 4],
        ivs: [0; 6], evs: [0; 6], nature: 0, level: 100, tera_type: 0, is_female: false }
}

fn build_case(c: &Case) -> (BattleState, TeamData, Belief) {
    let mut p1: [MonBuildInput; 6] = std::array::from_fn(|_| empty());
    for (i, m) in c.our.iter().take(6).enumerate() {
        p1[i] = input(m.species, m.level, m.ability, m.item, m.moves, m.evs);
    }
    let opp_moves = if c.opp.moves != [0; 4] { c.opp.moves } else {
        let mut mv = [0u16; 4];
        for (i, &x) in c.opp.moves_revealed.iter().take(4).enumerate() { mv[i] = x; }
        mv
    };
    let mut p2: [MonBuildInput; 6] = std::array::from_fn(|_| empty());
    p2[0] = input(c.opp.species, c.opp.level, c.opp.ability, c.opp.item, opp_moves, [85; 6]);

    let (t1, b1, l1) = build_team(&p1);
    let (t2, b2, l2) = build_team(&p2);
    let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
    let mut state = BattleState::default();
    state.sides[0].team = t1;
    state.sides[1].team = t2;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);

    // opponent active HP fraction + status (active HP lives in team[active_index])
    let oi = state.sides[1].active_index as usize;
    let mh = state.sides[1].team[oi].max_hp as u32;
    let hp = ((c.opp.hp_num.max(0) * mh + c.opp.hp_den / 2) / c.opp.hp_den.max(1)).clamp(1, mh) as u16;
    state.sides[1].team[oi].current_hp = hp;
    state.sides[1].team[oi].status = status_code(&c.opp.status);

    if let Some(b) = c.our_boosts {
        state.sides[0].active.boosts = b;
    }

    let mut belief = Belief::default();
    let slot = belief.note_species(c.opp.species, c.opp.level);
    for &mv in c.opp.moves_revealed.iter() { if mv != 0 { belief.note_move(slot, mv); } }
    (state, teams, belief)
}

fn label(byte: u8, our_active_moves: &[u16; 4]) -> String {
    match byte {
        0..=3 => format!("move[{}]=#{}", byte, our_active_moves[byte as usize]),
        4..=9 => format!("switch->slot{}", byte - 4),
        ACTION_TERA_0..=ACTION_TERA_3 => format!("tera+move[{}]=#{}", byte - ACTION_TERA_0, our_active_moves[(byte - ACTION_TERA_0) as usize]),
        255 => "struggle".to_string(),
        _ => format!("byte{}", byte),
    }
}

// visit-weighted mean avg_score for an action byte across worlds
fn mean_avg(tr: &DecisionTrace, byte: u8) -> f64 {
    let mut num = 0.0f64; let mut den = 0u64;
    for w in &tr.per_world {
        for a in &w.arms {
            if a.action == byte { num += a.avg_score * a.visits as f64; den += a.visits as u64; }
        }
    }
    if den == 0 { 0.0 } else { num / den as f64 }
}

fn run_one(c: &Case, mode: ChanceMode, worlds: usize, ms: u64, iters: u64, seed: u64, pick: &str, full_info: bool) {
    let (state, teams, belief) = build_case(c);
    let our_moves = state.sides[0].team[state.sides[0].active_index as usize].moves;
    let obs = Observation { state: &state, our_side: 0, teams: &teams };
    let pick_mode = match pick {
        "value" => PickMode::Value,
        "argmax" => PickMode::Argmax,
        _ => PickMode::Weighted,
    };
    let cfg = PimcConfig {
        num_worlds: worlds, time_ms_per_world: ms, max_iters_per_world: iters, seed,
        chance_mode: mode, pick_mode, filter_threshold: 0.75, raw_root: false,
    };
    let tr = if full_info {
        choose_action_traced(&obs, &belief, &TrueState, &cfg)
    } else if c.last_mon {
        choose_action_traced(&obs, &belief, &LastMonRandomBattle, &cfg)
    } else {
        choose_action_traced(&obs, &belief, &RandomBattle, &cfg)
    };

    let mode_s = match mode {
        ChanceMode::OpenLoop => "OPEN",
        ChanceMode::ClosedLoop => "CLOSED",
        ChanceMode::AnalyticRoot => "ANALYTIC",
    };
    let mean_iters: u64 = if tr.per_world.is_empty() { 0 } else { tr.per_world.iter().map(|w| w.iterations).sum::<u64>() / tr.per_world.len() as u64 };
    let mean_depth: f64 = if tr.per_world.is_empty() { 0.0 } else {
        tr.per_world.iter().map(|w| w.depth_sum as f64 / w.iterations.max(1) as f64).sum::<f64>() / tr.per_world.len() as f64 };
    let distinct_items: std::collections::HashSet<u16> = tr.per_world.iter().map(|w| w.opp_item).collect();
    let distinct_abil: std::collections::HashSet<u16> = tr.per_world.iter().map(|w| w.opp_ability).collect();

    println!("  [{mode_s}] worlds={} mean_iters/world={} mean_depth={:.1} | opp determinized: {} items {} abilities (hp {}/{} status {})",
        tr.per_world.len(), mean_iters, mean_depth, distinct_items.len(), distinct_abil.len(),
        tr.per_world.first().map_or(0, |w| w.opp_hp), tr.per_world.first().map_or(0, |w| w.opp_max_hp),
        tr.per_world.first().map_or(0, |w| w.opp_status));

    let survivor_set: std::collections::HashSet<u8> = tr.survivors.iter().map(|(a, _)| *a).collect();
    let best = tr.aggregate.first().map_or(0.0, |x| x.1);
    println!("    root arms (byte: frac | mean_avg_score | survivor?):");
    for (byte, frac) in &tr.aggregate {
        let tag = if *byte == c.blunder_action { " <== BLUNDER" }
                  else if *byte == c.better_action { " <== BETTER" } else { "" };
        let surv = if survivor_set.contains(byte) { "yes" } else { "FILTERED" };
        println!("      {:<16} {:.4} | {:.3} | {}{}",
            label(*byte, &our_moves), frac, mean_avg(&tr, *byte), surv, tag);
    }
    let bl_frac = tr.aggregate.iter().find(|(a, _)| *a == c.blunder_action).map_or(0.0, |x| x.1);
    let bt_frac = tr.aggregate.iter().find(|(a, _)| *a == c.better_action).map_or(0.0, |x| x.1);
    println!("    picked={} | blunder byte {} frac={:.4} ({}) | better byte {} frac={:.4} ({}) | best={:.4}",
        label(tr.picked, &our_moves), c.blunder_action, bl_frac,
        if survivor_set.contains(&c.blunder_action) { "CLEARS 0.75 -> pickable" } else { "filtered" },
        c.better_action, bt_frac,
        if survivor_set.contains(&c.better_action) { "survivor" } else { "FILTERED OUT" }, best);
    // counterfactual: what a VALUE-based selection (max mean avg_score among legal) would pick
    let val_pick = tr.legal.iter().copied()
        .max_by(|&a, &b| mean_avg(&tr, a).partial_cmp(&mean_avg(&tr, b)).unwrap());
    if let Some(vp) = val_pick {
        let tag = if vp == c.better_action { "== BETTER (value-based pick would FIX this blunder)" }
                  else if vp == c.blunder_action { "== BLUNDER (value agrees with the blunder)" } else { "" };
        println!("    value-argmax (max mean avg_score among legal) = {} avg={:.3} {}",
            label(vp, &our_moves), mean_avg(&tr, vp), tag);
    }
    let reproduced = tr.picked == c.blunder_action;
    println!("    => blunder {} (picked={}, blunder={})",
        if reproduced { "REPRODUCES" } else { "does NOT reproduce" }, tr.picked, c.blunder_action);
}

fn main() {
    let filter = std::env::args().nth(1);
    let worlds: usize = std::env::var("PROBE_WORLDS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let ms: u64 = std::env::var("PROBE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(4000);
    let iters: u64 = std::env::var("PROBE_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(100_000_000);
    let seed: u64 = std::env::var("PROBE_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0xB1);
    let chance = std::env::var("CHANCE").unwrap_or_else(|_| "open".into());
    let pick = std::env::var("PICK").unwrap_or_else(|_| "weighted".into());
    let full_info = std::env::var("FULLINFO").map(|v| v == "1").unwrap_or(false);

    let path = "data/blunder_cases.json";
    let suite: Suite = serde_json::from_str(
        &std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"))).unwrap();

    println!("blunder_probe: {} case(s), worlds={worlds} ms={ms} iters={iters} seed={seed} chance={chance} full_info={full_info}\n",
        suite.cases.len());
    for c in &suite.cases {
        if let Some(f) = &filter { if !c.name.contains(f.as_str()) { continue; } }
        println!("== {} ==\n   {}", c.name, c.note);
        let modes: Vec<ChanceMode> = match chance.as_str() {
            "closed" => vec![ChanceMode::ClosedLoop],
            "analytic" => vec![ChanceMode::AnalyticRoot],
            "both" => vec![ChanceMode::OpenLoop, ChanceMode::ClosedLoop],
            "openalytic" => vec![ChanceMode::OpenLoop, ChanceMode::AnalyticRoot],
            _ => vec![ChanceMode::OpenLoop],
        };
        for m in modes { run_one(c, m, worlds, ms, iters, seed, pick.as_str(), full_info); }
        println!();
    }
}
