// Our-search agreement baseline over policy shards: for each record, re-run
// OUR MCTS on the record state as a single fully-known world (the record IS a
// determinized world) at 16,384 iterations, c = 0.7, argmax pick, under a
// fixed seed. A bare TrainRecord carries no teams, so TeamData is
// reconstructed per record (neutral nature + back-solved EVs) — the same
// primitive the label serializer uses, keeping the form-change residual
// bias-symmetric between teacher and baseline.
//
// TSV per record: key, game_tag, turn, decider, baseline_pick[, teacher_argmax,
// q_margin]. With --labels, teacher_argmax is the label mdist argmax; q_margin
// is filled only when teacher_argmax is a switch byte (4-9): Q(baseline pick)
// - Q(teacher switch), both read from the pooled root stats the pick reads.
// Analysis-only column; no gate attaches to it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use poke_mcts::chance::OpenLoop;
use poke_mcts::driver::{aggregate, aggregate_value, pick_from, PickMode};
use poke_mcts::eval::Handcrafted;
use poke_mcts::policy_label::{read_records_guarded, reconstruct_teams};
use poke_mcts::rng::{splitmix64, Lcg};
use poke_mcts::search::{search_world, ArmStat, SearchParams};

const BASELINE_ITERS: u64 = 16_384;
// c = 0.7 => UCB sqrt coefficient c^2
const EXPLORE_COEFF: f64 = 0.49;
const TIME_CEILING_MS: u64 = 600_000;
const BASELINE_SEED: u64 = 0x5EED_BA5E;

fn shard_paths(args: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for a in args {
        let md = std::fs::metadata(a).unwrap_or_else(|e| panic!("{a}: {e}"));
        if md.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(a)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".records.bin"))
                .collect();
            files.sort();
            out.extend(files);
        } else {
            out.push(PathBuf::from(a));
        }
    }
    out
}

fn read_teacher_argmax(path: &str) -> HashMap<String, u8> {
    let f = std::fs::File::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut map = HashMap::new();
    for line in BufReader::new(f).lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        if v.get("invalid").is_some() {
            continue;
        }
        let key = v["key"].as_str().unwrap().to_string();
        let mdist = v["mdist"].as_object().unwrap();
        let mut best: Option<(u8, f64)> = None;
        for (k, val) in mdist {
            let b: u8 = k.parse().unwrap();
            let s = val.as_f64().unwrap();
            if best.map_or(true, |(bb, bs)| s > bs || (s == bs && b < bb)) {
                best = Some((b, s));
            }
        }
        if let Some((b, _)) = best {
            map.insert(key, b);
        }
    }
    map
}

fn baseline_pick(stats: &[ArmStat], legal: &pkmn_engine::state::ActionList, seed: u64) -> u8 {
    let per_world = vec![(stats.to_vec(), 1.0f64)];
    let agg = aggregate(&per_world);
    let mut rng = Lcg::new(seed);
    pick_from(&agg, legal, &mut rng, PickMode::Argmax, 0.75)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut labels: Option<String> = None;
    let mut limit: Option<usize> = None;
    let mut inputs: Vec<String> = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--labels" => labels = it.next(),
            "--limit" => limit = it.next().and_then(|v| v.parse().ok()),
            _ => inputs.push(a),
        }
    }
    if inputs.is_empty() {
        eprintln!("usage: policy_baseline [--labels labels.jsonl] [--limit N] <shard-dir-or-file>...");
        std::process::exit(2);
    }
    let teacher = labels.as_deref().map(read_teacher_argmax);

    let params = SearchParams {
        time_ms: TIME_CEILING_MS,
        max_iters: BASELINE_ITERS,
        explore_coeff: EXPLORE_COEFF,
        ..Default::default()
    };
    let mut done = 0usize;
    'outer: for path in shard_paths(&inputs) {
        let recs = read_records_guarded(path.to_str().unwrap()).unwrap_or_else(|e| panic!("{e}"));
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        for (idx, rec) in recs.iter().enumerate() {
            if let Some(n) = limit {
                if done >= n {
                    break 'outer;
                }
            }
            let key = format!("{file_name}:{idx}");
            let decider = rec.side_of_decider as usize;
            let legal = pkmn_engine::state::legal_actions(&rec.state, decider);
            let seed = splitmix64(BASELINE_SEED ^ rec.game_tag).wrapping_add(idx as u64);
            let (pick, valued) = if legal.count <= 1 {
                (legal.as_slice().first().copied().unwrap_or(pkmn_engine::state::ACTION_STRUGGLE), vec![])
            } else {
                let teams = reconstruct_teams(&rec.state);
                let r = search_world(&rec.state, &teams, &Handcrafted, &OpenLoop, &params, seed);
                let stats = r.side(decider);
                let pick = baseline_pick(stats, &legal, seed);
                (pick, aggregate_value(&[(stats.to_vec(), 1.0)]))
            };
            let mut line = format!(
                "{key}\t{}\t{}\t{}\t{pick}",
                rec.game_tag, rec.turn, rec.side_of_decider
            );
            if let Some(t) = teacher.as_ref() {
                match t.get(&key) {
                    Some(&ta) => {
                        let q = |b: u8| valued.iter().find(|(a, _, _)| *a == b).map(|(_, m, _)| *m);
                        let margin = if (4..=9).contains(&ta) {
                            match (q(pick), q(ta)) {
                                (Some(qp), Some(qt)) => format!("{:.6}", qp - qt),
                                _ => String::new(),
                            }
                        } else {
                            String::new()
                        };
                        line.push_str(&format!("\t{ta}\t{margin}"));
                    }
                    None => line.push_str("\t\t"),
                }
            }
            println!("{line}");
            done += 1;
        }
    }
    eprintln!("records {done}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use poke_mcts::testutil::{build_state, mon};

    // Forced-KO fixture (the reference_pick pattern): Earthquake in slot 0 KOs
    // the 1-HP opponent; the weaker slot-1 move exists so the search must
    // choose. The baseline must pick byte 0, reproducibly.
    #[test]
    fn forced_ko_baseline_pick_is_move_slot_zero() {
        let (mut state, _) = build_state(
            vec![mon(445, 24, [89, 33, 0, 0])],
            vec![mon(25, 9, [85, 0, 0, 0])],
        );
        let ai = state.sides[1].active_index as usize;
        state.sides[1].team[ai].current_hp = 1;

        let teams = reconstruct_teams(&state);
        let params = SearchParams {
            time_ms: TIME_CEILING_MS,
            max_iters: 2_000,
            explore_coeff: EXPLORE_COEFF,
            ..Default::default()
        };
        let legal = pkmn_engine::state::legal_actions(&state, 0);
        let r = search_world(&state, &teams, &Handcrafted, &OpenLoop, &params, 7);
        let pick = baseline_pick(r.side(0), &legal, 7);
        assert_eq!(pick, 0, "forced-KO baseline pick must be the KO move in slot 0");
        let r2 = search_world(&state, &teams, &Handcrafted, &OpenLoop, &params, 7);
        let pick2 = baseline_pick(r2.side(0), &legal, 7);
        assert_eq!(pick, pick2, "same seed -> same pick");
    }

    #[test]
    fn q_margin_reads_pooled_root_stats() {
        let stats = vec![
            ArmStat { action: 0, visits: 900, avg_score: 0.71, win_chance: 0.0 },
            ArmStat { action: 5, visits: 100, avg_score: 0.42, win_chance: 0.0 },
        ];
        let valued = aggregate_value(&[(stats, 1.0)]);
        let q = |b: u8| valued.iter().find(|(a, _, _)| *a == b).map(|(_, m, _)| *m).unwrap();
        assert!((q(0) - 0.71).abs() < 1e-9);
        assert!(((q(0) - q(5)) - 0.29).abs() < 1e-9);
    }
}
