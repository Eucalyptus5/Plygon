// Deep-search "reference pick" over captured position snapshots: for each snapshot, re-run OUR
// MCTS at a judge-grade budget (>=4x the live worlds/iters, with a high non-tripping time ceiling so
// the finite iteration cap is always the termination cause) under a fixed seed, so the same snapshot
// always yields the same reference best move. Prints one TSV line per snapshot: game_id\tturn\tref_byte.
//
// Fidelity caveat: this reference pick is produced by OUR engine, so it is blind to engine-fidelity
// divergence from Showdown. It answers only "where does our live pick lose value versus our own best
// move", which is a strict SUBSET of the reference question (it cannot see value lost because our engine
// models a position differently than the real game).
//
// FULLINFO: default OFF uses the belief-sampled RandomBattle determinizer with the snapshot's belief.
// Set FULLINFO=1 to instead search a single true-state world equal to the actual snapshot state
// (belief ignored). Judge-grade budget = live budget * 4 (worlds and iters), time ceiling 60s.
use poke_mcts::audit_snapshot::PositionSnapshot;
use poke_mcts::belief::Belief;
use poke_mcts::determinize::{Determinizer, Observation, RandomBattle, World};
use poke_mcts::driver::{choose_action, PickMode, PimcConfig};
use poke_mcts::rng::Lcg;
use poke_mcts::search::ChanceMode;

const JUDGE_MULT: u64 = 4;
const JUDGE_TIME_MS: u64 = 60_000;
const JUDGE_SEED: u64 = 0x5EED_5EED;

struct TrueState;
impl Determinizer for TrueState {
    fn sample_worlds(&self, obs: &Observation, _belief: &Belief, _n: usize, _rng: &mut Lcg) -> Vec<World> {
        vec![World { state: *obs.state, teams: obs.teams.clone(), weight: 1.0 }]
    }
}

fn judge_cfg(live_budget: (usize, u64, u64)) -> PimcConfig {
    let (live_worlds, _live_ms, live_iters) = live_budget;
    PimcConfig {
        num_worlds: live_worlds * JUDGE_MULT as usize,
        time_ms_per_world: JUDGE_TIME_MS,
        max_iters_per_world: live_iters.saturating_mul(JUDGE_MULT).max(JUDGE_MULT),
        seed: JUDGE_SEED,
        chance_mode: ChanceMode::OpenLoop,
        pick_mode: PickMode::Argmax,
        filter_threshold: 0.75,
        raw_root: false,
    }
}

fn reference_pick(snap: &PositionSnapshot, full_info: bool) -> u8 {
    let obs = Observation { state: &snap.state, teams: &snap.teams, our_side: snap.our_side };
    let cfg = judge_cfg(snap.live_budget);
    if full_info {
        choose_action(&obs, &snap.belief, &TrueState, &cfg)
    } else {
        choose_action(&obs, &snap.belief, &RandomBattle, &cfg)
    }
}

fn snapshot_paths(dir: &str) -> std::io::Result<Vec<String>> {
    let mut paths: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    paths.sort();
    Ok(paths)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<String> = None;
    while let Some(a) = args.next() {
        if a == "--snapshots" {
            match args.next() {
                Some(v) => dir = Some(v),
                None => {
                    eprintln!("--snapshots requires a directory argument");
                    std::process::exit(2);
                }
            }
        } else {
            eprintln!("unknown arg: {a}");
            std::process::exit(2);
        }
    }
    let dir = dir.unwrap_or_else(|| {
        eprintln!("usage: reference_pick --snapshots <dir>");
        std::process::exit(2);
    });
    let full_info = std::env::var("FULLINFO").map(|v| v == "1").unwrap_or(false);

    let paths = snapshot_paths(&dir).unwrap_or_else(|e| {
        eprintln!("read_dir {dir}: {e}");
        std::process::exit(1);
    });
    for path in &paths {
        let snap = match PositionSnapshot::read(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("read {path}: {e}");
                continue;
            }
        };
        let byte = reference_pick(&snap, full_info);
        println!("{}\t{}\t{}", snap.game_id, snap.turn, byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Forced-KO fixture: our Garchomp has Earthquake (#89) in move slot 0 and the much weaker
    // Tackle (#33) in slot 1, so two legal move actions exist and the search must choose. The
    // single opponent (Pikachu) is dropped to 1 HP, so under full-info (TrueState single world)
    // Earthquake is an unambiguous terminal KO and the reference pick must be move slot 0 = byte 0.
    fn ko_snapshot() -> PositionSnapshot {
        let (mut state, teams) = poke_mcts::testutil::build_state(
            vec![poke_mcts::testutil::mon(445, 24, [89, 33, 0, 0])],
            vec![poke_mcts::testutil::mon(25, 9, [85, 0, 0, 0])],
        );
        let ai = state.sides[1].active_index as usize;
        state.sides[1].team[ai].current_hp = 1;
        PositionSnapshot {
            game_id: "ko-fixture".into(),
            turn: 1,
            our_side: 0,
            our_pick: 0,
            live_budget: (8, 200, 1000),
            request_kind: "move".into(),
            state,
            teams,
            belief: Belief::default(),
            tags: vec![],
        }
    }

    #[test]
    fn forced_ko_reference_pick_is_move_slot_zero() {
        let snap = ko_snapshot();
        let byte = reference_pick(&snap, true);
        assert_eq!(byte, 0, "forced-KO reference pick must be the KO move in slot 0 (byte 0)");
    }
}
