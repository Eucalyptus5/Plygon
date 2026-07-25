// Stage-1 extractor: sample decision records from retained dumps into policy
// shards (bincode TrainRecord re-emit, one <game_tag>.records.bin per game),
// plus work.jsonl {shard, index, state_string, decider, legal_bytes,
// option_map} for the labeler pool, outcomes.jsonl (joined for the sampled
// games), and a deterministic manifest.json. Sampling: uniform per game, cap
// --per-game records/game, cap --per-turn per (game,turn); struggle-only
// records (legal == {255}, outside the 14-vector) are skipped and counted.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use poke_mcts::policy_label::{option_map, read_records_guarded, serialize};
use poke_mcts::rng::splitmix64;
use poke_mcts::train_dump::TrainRecord;

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

fn species_check(rec: &TrainRecord, path: &Path, idx: usize) {
    for side in 0..2 {
        for k in 0..6 {
            let id = rec.state.sides[side].team[k].species_id;
            if id == 0 {
                continue;
            }
            assert!(
                serialize::species_token(id) != "NONE",
                "{}:{idx} side {side} slot {k}: species id {id} has no dex name",
                path.display()
            );
        }
    }
}

fn read_outcome_lines(dirs: &[PathBuf]) -> HashMap<u64, String> {
    let mut map = HashMap::new();
    for d in dirs {
        let p = d.join("outcomes.jsonl");
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let tag = v["game_tag"].as_u64().unwrap_or_else(|| panic!("{}: missing game_tag", p.display()));
            map.insert(tag, line.to_string());
        }
    }
    map
}

struct SampledGame {
    tag: u64,
    source: PathBuf,
    records_in: usize,
    indices: Vec<usize>,
    struggle_only: usize,
}

fn sample_game(
    recs: &[TrainRecord],
    tag: u64,
    seed: u64,
    per_game: usize,
    per_turn: usize,
) -> (Vec<usize>, usize) {
    let mut order: Vec<usize> = (0..recs.len()).collect();
    let mut s = splitmix64(seed ^ tag);
    for i in 0..order.len() {
        s = splitmix64(s);
        let j = i + (s as usize) % (order.len() - i);
        order.swap(i, j);
    }
    let mut per_turn_count: HashMap<u16, usize> = HashMap::new();
    let mut chosen: Vec<usize> = Vec::new();
    let mut struggle_only = 0usize;
    for &idx in &order {
        if chosen.len() >= per_game {
            break;
        }
        let rec = &recs[idx];
        let legal = pkmn_engine::state::legal_actions(&rec.state, rec.side_of_decider as usize);
        if legal.as_slice() == [pkmn_engine::state::ACTION_STRUGGLE] {
            struggle_only += 1;
            continue;
        }
        let n = per_turn_count.entry(rec.turn).or_insert(0);
        if *n >= per_turn {
            continue;
        }
        *n += 1;
        chosen.push(idx);
    }
    chosen.sort_unstable();
    (chosen, struggle_only)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out: Option<String> = None;
    let mut seed: Option<u64> = None;
    let mut per_game = 8usize;
    let mut per_turn = 2usize;
    let mut inputs: Vec<String> = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next(),
            "--seed" => seed = it.next().and_then(|v| v.parse().ok()),
            "--per-game" => per_game = it.next().and_then(|v| v.parse().ok()).expect("--per-game <n>"),
            "--per-turn" => per_turn = it.next().and_then(|v| v.parse().ok()).expect("--per-turn <n>"),
            _ => inputs.push(a),
        }
    }
    let (Some(out), Some(seed)) = (out, seed) else {
        eprintln!("usage: policy_sample --out <dir> --seed <u64> [--per-game 8] [--per-turn 2] <shard-dir-or-file>...");
        std::process::exit(2);
    };
    if inputs.is_empty() {
        eprintln!("no input shards given");
        std::process::exit(2);
    }
    let out_dir = PathBuf::from(&out);
    std::fs::create_dir_all(&out_dir).unwrap();

    let input_dirs: Vec<PathBuf> = inputs
        .iter()
        .map(PathBuf::from)
        .map(|p| if p.is_dir() { p } else { p.parent().map(Path::to_path_buf).unwrap_or_default() })
        .collect();
    let outcome_lines = read_outcome_lines(&input_dirs);

    let paths = shard_paths(&inputs);
    let mut games: Vec<SampledGame> = Vec::new();
    let mut records_in = 0u64;
    let mut work = std::io::BufWriter::new(std::fs::File::create(out_dir.join("work.jsonl")).unwrap());
    let mut outcomes = std::io::BufWriter::new(std::fs::File::create(out_dir.join("outcomes.jsonl")).unwrap());
    let mut records_sampled = 0u64;
    let mut struggle_skipped = 0u64;
    let mut outcomes_joined = 0u64;
    let mut seen_tags: HashMap<u64, PathBuf> = HashMap::new();

    for path in &paths {
        let recs = read_records_guarded(path.to_str().unwrap()).unwrap_or_else(|e| panic!("{e}"));
        if recs.is_empty() {
            continue;
        }
        let tag = recs[0].game_tag;
        for (idx, rec) in recs.iter().enumerate() {
            assert_eq!(rec.game_tag, tag, "{}:{idx}: mixed game_tag in one shard", path.display());
            species_check(rec, path, idx);
        }
        if let Some(prev) = seen_tags.insert(tag, path.clone()) {
            panic!("game_tag {tag} in both {} and {}", prev.display(), path.display());
        }
        records_in += recs.len() as u64;
        let (indices, struggle_only) = sample_game(&recs, tag, seed, per_game, per_turn);
        struggle_skipped += struggle_only as u64;

        if !indices.is_empty() {
            let shard_name = format!("{tag}.records.bin");
            let mut f = std::io::BufWriter::new(std::fs::File::create(out_dir.join(&shard_name)).unwrap());
            for (out_idx, &src_idx) in indices.iter().enumerate() {
                let rec = &recs[src_idx];
                f.write_all(&bincode::serialize(rec).unwrap()).unwrap();
                let decider = rec.side_of_decider as usize;
                let legal = pkmn_engine::state::legal_actions(&rec.state, decider);
                let mut omap = serde_json::Map::new();
                for (tok, byte) in option_map(&rec.state, decider) {
                    omap.insert(tok, byte.map_or(serde_json::Value::Null, |b| b.into()));
                }
                let row = serde_json::json!({
                    "shard": shard_name,
                    "index": out_idx,
                    "state_string": serialize::serialize_state(&rec.state, decider),
                    "decider": rec.side_of_decider,
                    "legal_bytes": legal.as_slice(),
                    "option_map": omap,
                });
                writeln!(work, "{row}").unwrap();
                records_sampled += 1;
            }
            if let Some(line) = outcome_lines.get(&tag) {
                writeln!(outcomes, "{line}").unwrap();
                outcomes_joined += 1;
            }
        }
        games.push(SampledGame {
            tag,
            source: path.clone(),
            records_in: recs.len(),
            indices,
            struggle_only,
        });
    }
    work.flush().unwrap();
    outcomes.flush().unwrap();

    games.sort_by_key(|g| g.tag);
    let manifest = serde_json::json!({
        "seed": seed,
        "per_game_cap": per_game,
        "per_turn_cap": per_turn,
        "games": games.len(),
        "records_in": records_in,
        "records_sampled": records_sampled,
        "struggle_only_skipped": struggle_skipped,
        "outcomes_joined": outcomes_joined,
        "shards": games.iter().map(|g| serde_json::json!({
            "shard": format!("{}.records.bin", g.tag),
            "source": g.source.to_string_lossy(),
            "records_in": g.records_in,
            "struggle_only": g.struggle_only,
            "sampled_indices": g.indices,
        })).collect::<Vec<_>>(),
    });
    std::fs::write(out_dir.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    println!(
        "games {}  records {} -> {}  struggle-only {}  outcomes {}",
        games.len(), records_in, records_sampled, struggle_skipped, outcomes_joined
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use poke_mcts::determinize::World;
    use poke_mcts::testutil::{build_state, mon};
    use poke_mcts::train_dump;

    fn tmp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("poke_mcts_psample_{}_{}", tag, std::process::id()))
    }

    fn dump(dir: &Path, tag: u64, worlds: &[World], side: usize, turn: u16) {
        #[cfg(feature = "train_value")]
        train_dump::dump_worlds(dir.to_str().unwrap(), tag, worlds, side, turn, 0.5).unwrap();
        #[cfg(not(feature = "train_value"))]
        train_dump::dump_worlds(dir.to_str().unwrap(), tag, worlds, side, turn).unwrap();
    }

    fn game_dump(dir: &Path, tag: u64, turns: u16) {
        let (state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0])],
        );
        let worlds = vec![
            World { state, teams: teams.clone(), weight: 1.0 },
            World { state, teams, weight: 1.0 },
        ];
        for t in 1..=turns {
            dump(dir, tag, &worlds, 0, t);
        }
    }

    #[test]
    fn sampler_respects_caps_and_is_deterministic() {
        let dir = tmp_dir("caps");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        game_dump(&dir, 11, 10);
        std::fs::write(
            dir.join("outcomes.jsonl"),
            "{\"game_tag\":11,\"winner_side0\":1.0}\n",
        )
        .unwrap();

        let recs = read_records_guarded(dir.join("11.records.bin").to_str().unwrap()).unwrap();
        assert_eq!(recs.len(), 20);
        let (a, _) = sample_game(&recs, 11, 7, 8, 2);
        let (b, _) = sample_game(&recs, 11, 7, 8, 2);
        assert_eq!(a, b, "same seed -> same sample");
        assert_eq!(a.len(), 8);
        let mut per_turn: HashMap<u16, usize> = HashMap::new();
        for &i in &a {
            *per_turn.entry(recs[i].turn).or_insert(0) += 1;
        }
        assert!(per_turn.values().all(|&n| n <= 2), "per-(game,turn) cap 2");
        assert!(a.windows(2).all(|w| w[0] < w[1]), "emitted in source order");
        let (c, _) = sample_game(&recs, 11, 8, 8, 2);
        assert_ne!(a, c, "different seed -> different sample");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn struggle_only_records_are_skipped_and_counted() {
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0])],
        );
        state.sides[0].team[0].pp = [0; 4];
        let legal = pkmn_engine::state::legal_actions(&state, 0);
        assert_eq!(legal.as_slice(), [pkmn_engine::state::ACTION_STRUGGLE]);

        let dir = tmp_dir("struggle");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let worlds = vec![World { state, teams, weight: 1.0 }];
        dump(&dir, 5, &worlds, 0, 3);
        let recs = read_records_guarded(dir.join("5.records.bin").to_str().unwrap()).unwrap();
        let (chosen, struggle_only) = sample_game(&recs, 5, 1, 8, 2);
        assert!(chosen.is_empty(), "byte 255 lives outside the 14-vector");
        assert_eq!(struggle_only, recs.len());
        std::fs::remove_dir_all(&dir).ok();
    }
}
