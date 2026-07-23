use std::collections::HashMap;
use std::io::Write;

use poke_mcts::policy_label::{fixed_record_size, read_records_guarded, serialize};
use poke_mcts::train_dump::TrainRecord;

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

fn shard_paths(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in args {
        let md = std::fs::metadata(a).unwrap_or_else(|e| panic!("{a}: {e}"));
        if md.is_dir() {
            let mut files: Vec<String> = std::fs::read_dir(a)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.path().to_string_lossy().into_owned())
                .filter(|p| p.ends_with(".records.bin"))
                .collect();
            files.sort();
            out.extend(files);
        } else {
            out.push(a.clone());
        }
    }
    out
}

fn species_check(rec: &TrainRecord, path: &str, idx: usize) {
    for side in 0..2 {
        for k in 0..6 {
            let id = rec.state.sides[side].team[k].species_id;
            if id == 0 {
                continue;
            }
            let tok = serialize::species_token(id);
            assert!(
                tok != "NONE",
                "{path}:{idx} side {side} slot {k}: species id {id} has no dex name"
            );
        }
        let active_id =
            rec.state.sides[side].team[rec.state.sides[side].active_index as usize].species_id;
        assert!(active_id != 0, "{path}:{idx} side {side}: active slot empty");
    }
}

fn mon_json(mon: &pkmn_engine::state::MonSlot) -> serde_json::Value {
    let sp = pkmn_engine::state::data_bridge::species(mon.species_id);
    serde_json::json!({
        "species": serialize::species_token(mon.species_id).to_lowercase(),
        "species_id": mon.species_id,
        "level": mon.level,
        "current_hp": mon.current_hp,
        "max_hp": mon.max_hp,
        "stats": mon.stats,
        "base": [sp.hp, sp.atk, sp.def, sp.spa, sp.spd, sp.spe],
        "evs_backsolved": serialize::back_solve_evs(mon),
        "moves": (0..4).map(|i| serialize::move_token(mon.moves[i]).to_lowercase()).collect::<Vec<_>>(),
        "status": mon.status,
        "terastallized": mon.is_terastallized(),
    })
}

fn record_json(rec: &TrainRecord, key: &str) -> serde_json::Value {
    let decider = rec.side_of_decider as usize;
    let ds = &rec.state.sides[decider];
    let active = &ds.team[ds.active_index as usize];
    let legal = pkmn_engine::state::legal_actions(&rec.state, decider);
    serde_json::json!({
        "key": key,
        "game_tag": rec.game_tag,
        "turn": rec.turn,
        "decider": rec.side_of_decider,
        "world_idx": rec.world_idx,
        "phase": rec.state.phase,
        "active_index": ds.active_index,
        "move_tokens": (0..4).map(|i| serialize::move_token(active.moves[i]).to_lowercase()).collect::<Vec<_>>(),
        "team_tokens": (0..6).map(|i| serialize::species_token(ds.team[i].species_id).to_lowercase()).collect::<Vec<_>>(),
        "legal": legal.as_slice(),
        "state": serialize::serialize_state(&rec.state, decider),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("scan") => {
            let targets: [(u16, &str); 7] = [
                (964, "palafin"),
                (1311, "palafinhero"),
                (875, "eiscue"),
                (1191, "eiscuenoice"),
                (774, "minior"),
                (1291, "miniormeteor"),
                (1437, "wishiwashischool"),
            ];
            let mut total = 0u64;
            let mut phase_counts: HashMap<u8, u64> = HashMap::new();
            let mut hits: HashMap<&str, Vec<String>> = HashMap::new();
            let paths = shard_paths(&args[2..]);
            eprintln!("rec_size={} shards={}", fixed_record_size(), paths.len());
            for path in &paths {
                let recs = read_records_guarded(path).unwrap_or_else(|e| panic!("{e}"));
                for (idx, rec) in recs.iter().enumerate() {
                    total += 1;
                    *phase_counts.entry(rec.state.phase).or_default() += 1;
                    for side in 0..2 {
                        for k in 0..6 {
                            let id = rec.state.sides[side].team[k].species_id;
                            if let Some((_, name)) = targets.iter().find(|(t, _)| *t == id) {
                                let v = hits.entry(name).or_default();
                                if v.len() < 8 {
                                    v.push(format!("{path}:{idx}:side{side}:slot{k}"));
                                }
                            }
                        }
                    }
                }
            }
            println!(
                "{}",
                serde_json::json!({
                    "records": total,
                    "phases": phase_counts.iter().map(|(k, v)| (k.to_string(), v)).collect::<HashMap<_, _>>(),
                    "form_changer_hits": hits,
                })
            );
        }
        Some("sample") => {
            let n: usize = args[2].parse().unwrap();
            let seed: u64 = args[3].parse().unwrap();
            let out_path = &args[4];
            let paths = shard_paths(&args[5..]);
            let mut pool: Vec<(usize, usize)> = Vec::new();
            let mut shards: Vec<Vec<TrainRecord>> = Vec::new();
            for (si, path) in paths.iter().enumerate() {
                let recs = read_records_guarded(path).unwrap_or_else(|e| panic!("{e}"));
                for idx in 0..recs.len() {
                    pool.push((si, idx));
                }
                shards.push(recs);
            }
            assert!(pool.len() >= n, "pool {} < requested {n}", pool.len());
            let mut s = seed;
            for i in 0..n {
                s = splitmix64(s);
                let j = i + (s as usize) % (pool.len() - i);
                pool.swap(i, j);
            }
            let mut out = std::io::BufWriter::new(std::fs::File::create(out_path).unwrap());
            for &(si, idx) in pool.iter().take(n) {
                let rec = &shards[si][idx];
                species_check(rec, &paths[si], idx);
                let key = format!("{}:{}", paths[si], idx);
                writeln!(out, "{}", record_json(rec, &key)).unwrap();
            }
            eprintln!("sampled {n} of {} records from {} shards", pool.len(), paths.len());
        }
        Some("detail") => {
            let path = &args[2];
            let idx: usize = args[3].parse().unwrap();
            let recs = read_records_guarded(path).unwrap_or_else(|e| panic!("{e}"));
            let rec = &recs[idx];
            let key = format!("{path}:{idx}");
            let mut v = record_json(rec, &key);
            for side in 0..2 {
                v[format!("side{side}_mons")] = serde_json::Value::Array(
                    (0..6).map(|k| mon_json(&rec.state.sides[side].team[k])).collect(),
                );
                v[format!("side{side}_active_index")] =
                    serde_json::json!(rec.state.sides[side].active_index);
            }
            v["state_flipped"] = serde_json::json!(serialize::serialize_state(
                &rec.state,
                1 - rec.side_of_decider as usize
            ));
            println!("{v}");
        }
        Some("naturecheck") => {
            let paths = shard_paths(&args[2..]);
            let mut mons = 0u64;
            let mut inexact_mons = 0u64;
            let mut inexact_stats = 0u64;
            let mut max_dev = 0u32;
            let mut examples: Vec<String> = Vec::new();
            let form_ids = [964u16, 1311, 875, 1191, 774, 1291, 1437];
            let mut form_mons = 0u64;
            let mut form_inexact = 0u64;
            let mut form_examples: Vec<String> = Vec::new();
            for path in &paths {
                let recs = read_records_guarded(path).unwrap_or_else(|e| panic!("{e}"));
                for (idx, rec) in recs.iter().enumerate() {
                    for side in 0..2 {
                        for k in 0..6 {
                            let mon = &rec.state.sides[side].team[k];
                            if mon.species_id == 0 {
                                continue;
                            }
                            mons += 1;
                            let sp = pkmn_engine::state::data_bridge::species(mon.species_id);
                            let bases = [sp.hp, sp.atk, sp.def, sp.spa, sp.spd, sp.spe];
                            let e = serialize::back_solve_evs(mon);
                            let l = mon.level as u32;
                            let mut mon_exact = true;
                            for i in 0..6usize {
                                let target =
                                    if i == 0 { mon.max_hp } else { mon.stats[i - 1] } as u32;
                                let raw = (2 * bases[i] as u32 + 31 + e[i] as u32 / 4) * l / 100;
                                let s = if i == 0 { raw + l + 10 } else { raw + 5 };
                                let d = s.abs_diff(target);
                                if d > 0 {
                                    mon_exact = false;
                                    inexact_stats += 1;
                                    if d > max_dev {
                                        max_dev = d;
                                    }
                                    if examples.len() < 6 {
                                        examples.push(format!(
                                            "{path}:{idx} side{side} slot{k} {} stat{i} recorded {target} reachable {s}",
                                            serialize::species_token(mon.species_id)
                                        ));
                                    }
                                }
                            }
                            if !mon_exact {
                                inexact_mons += 1;
                            }
                            if form_ids.contains(&mon.species_id) {
                                form_mons += 1;
                                if !mon_exact {
                                    form_inexact += 1;
                                    if form_examples.len() < 12 {
                                        form_examples.push(format!(
                                            "{path}:{idx} side{side} slot{k} {}",
                                            serialize::species_token(mon.species_id)
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            println!(
                "{}",
                serde_json::json!({
                    "mons": mons,
                    "inexact_mons": inexact_mons,
                    "inexact_stats": inexact_stats,
                    "max_deviation": max_dev,
                    "examples": examples,
                    "form_changer_mons": form_mons,
                    "form_changer_inexact": form_inexact,
                    "form_changer_examples": form_examples,
                })
            );
        }
        Some("basestats") => {
            let by_name: HashMap<String, u16> = serde_json::from_str(include_str!(
                "../../../testing_plan/id_maps/species_map.json"
            ))
            .unwrap();
            for name in &args[2..] {
                let id = by_name[name];
                let sp = pkmn_engine::state::data_bridge::species(id);
                println!(
                    "{}",
                    serde_json::json!({
                        "name": name,
                        "id": id,
                        "base": [sp.hp, sp.atk, sp.def, sp.spa, sp.spd, sp.spe],
                    })
                );
            }
        }
        _ => {
            eprintln!("usage: policy_spike scan|sample|detail|basestats ...");
            std::process::exit(2);
        }
    }
}
