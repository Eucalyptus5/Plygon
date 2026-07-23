use poke_mcts::eval::{Evaluator, Handcrafted};
use poke_mcts::{features, train_dump};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

// Output shard (little-endian): index.bin = u64 x (n+1) element offsets into
// features.bin (u32 ids); token_index.bin = 15 x u16 segment lengths per record;
// labels/hand_eval f32 + dense 7xf32 + held_out u8 + game_id u64 per record;
// values.bin f32 per record under train_value; visits.bin = 14 x f32 action
// visits (bytes 0-13) per record, gated by meta "visits"; meta.json.

#[derive(Debug, PartialEq)]
pub struct ConvertStats {
    pub games_joined: u64,
    pub games_dropped: u64,
    pub records: u64,
    pub features: u64,
}

fn read_outcomes(path: &Path) -> io::Result<HashMap<u64, f32>> {
    let f = File::open(path)?;
    let mut map = HashMap::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tag = v["game_tag"]
            .as_u64()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing game_tag"))?;
        let z = v["winner_side0"]
            .as_f64()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing winner_side0"))?;
        map.insert(tag, z as f32);
    }
    Ok(map)
}

fn fixed_record_size() -> u64 {
    let rec = train_dump::TrainRecord {
        #[cfg(feature = "train_value")]
        record_version: 2,
        game_tag: 0,
        turn: 0,
        side_of_decider: 0,
        world_idx: 0,
        #[cfg(feature = "train_value")]
        root_value: 0.0,
        state: pkmn_engine::state::BattleState::default(),
    };
    bincode::serialize(&rec).unwrap().len() as u64
}

fn shard_files(dir: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut shards = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if let Some(stem) = name.strip_suffix(".records.bin") {
            if let Ok(tag) = stem.parse::<u64>() {
                shards.push((tag, path));
            }
        }
    }
    shards.sort();
    Ok(shards)
}

pub fn convert(dir: &Path, out_dir: &Path, mirror_on: bool) -> io::Result<ConvertStats> {
    let outcomes = read_outcomes(&dir.join("outcomes.jsonl"))?;
    let shards = shard_files(dir)?;
    std::fs::create_dir_all(out_dir)?;
    let mut ix = BufWriter::new(File::create(out_dir.join("index.bin"))?);
    let mut fx = BufWriter::new(File::create(out_dir.join("features.bin"))?);
    let mut tx = BufWriter::new(File::create(out_dir.join("token_index.bin"))?);
    let mut lb = BufWriter::new(File::create(out_dir.join("labels.bin"))?);
    let mut he = BufWriter::new(File::create(out_dir.join("hand_eval.bin"))?);
    let mut ho = BufWriter::new(File::create(out_dir.join("held_out.bin"))?);
    let mut gi = BufWriter::new(File::create(out_dir.join("game_id.bin"))?);
    let mut de = BufWriter::new(File::create(out_dir.join("dense.bin"))?);
    #[cfg(feature = "train_value")]
    let mut va = BufWriter::new(File::create(out_dir.join("values.bin"))?);

    let rec_size = fixed_record_size();
    let mut stats = ConvertStats { games_joined: 0, games_dropped: 0, records: 0, features: 0 };
    ix.write_all(&0u64.to_le_bytes())?;
    let mut buf: Vec<u32> = Vec::with_capacity(256);
    for (tag, path) in shards {
        let Some(&z) = outcomes.get(&tag) else {
            stats.games_dropped += 1;
            continue;
        };
        let len = std::fs::metadata(&path)?.len();
        if len % rec_size != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {len} bytes not divisible by {rec_size}-byte records", path.display()),
            ));
        }
        // siblings of a CRN seat-swap pair share tag >> 1 (tag = game_index ^ C,
        // pair mates differ only in bit 0), so this keys the split on the PAIR
        let held = ((tag >> 1) % 10 == 0) as u8;
        for rec in train_dump::read_records(path.to_str().unwrap())? {
            #[cfg(feature = "train_value")]
            if rec.record_version != 2 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: record_version {} != 2", path.display(), rec.record_version),
                ));
            }
            let mut emit = |state: &pkmn_engine::state::BattleState, label: f32| -> io::Result<()> {
                buf.clear();
                let seg_lens = features::extract_segmented(state, &mut buf);
                for &id in &buf {
                    fx.write_all(&id.to_le_bytes())?;
                }
                for n in seg_lens {
                    tx.write_all(&n.to_le_bytes())?;
                }
                stats.features += buf.len() as u64;
                stats.records += 1;
                ix.write_all(&stats.features.to_le_bytes())?;
                lb.write_all(&label.to_le_bytes())?;
                he.write_all(&Handcrafted.eval(state).to_le_bytes())?;
                ho.write_all(&[held])?;
                gi.write_all(&tag.to_le_bytes())?;
                for v in features::extract_dense(state) {
                    de.write_all(&v.to_le_bytes())?;
                }
                Ok(())
            };
            emit(&rec.state, z)?;
            #[cfg(feature = "train_value")]
            va.write_all(&rec.root_value.to_le_bytes())?;
            if mirror_on {
                emit(&features::mirror(&rec.state), 1.0 - z)?;
                #[cfg(feature = "train_value")]
                va.write_all(&(1.0 - rec.root_value).to_le_bytes())?;
            }
        }
        stats.games_joined += 1;
    }
    ix.flush()?;
    fx.flush()?;
    tx.flush()?;
    lb.flush()?;
    he.flush()?;
    ho.flush()?;
    gi.flush()?;
    de.flush()?;
    #[cfg(feature = "train_value")]
    va.flush()?;

    let meta = serde_json::json!({
        "feature_spec_version": features::FEATURE_SPEC_VERSION,
        "vocab": features::vocab_size(),
        "dense_dim": features::DENSE_DIM,
        "records": stats.records,
        "features": stats.features,
        "games_joined": stats.games_joined,
        "games_dropped": stats.games_dropped,
        "mirror": mirror_on,
        "values": cfg!(feature = "train_value"),
        "token_segments": features::NUM_SEGMENTS,
        "visits": false,
    });
    std::fs::write(out_dir.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
    Ok(stats)
}

fn print_spec() {
    println!("FEATURE_SPEC_VERSION {}", features::FEATURE_SPEC_VERSION);
    for g in features::spec() {
        println!("{:<36} offset {:>6}  size {:>6}", g.name, g.offset, g.size);
    }
    println!("total vocab {}", features::vocab_size());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--spec") {
        print_spec();
        return;
    }
    let mut dir: Option<String> = None;
    let mut out: Option<String> = None;
    let mut mirror_on = true;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next(),
            "--no-mirror" => mirror_on = false,
            _ if dir.is_none() => dir = Some(a),
            _ => {
                eprintln!("unexpected argument: {a}");
                std::process::exit(2);
            }
        }
    }
    let (Some(dir), Some(out)) = (dir, out) else {
        eprintln!("usage: dump_features <dir> --out <train_dir> [--no-mirror] | dump_features --spec");
        std::process::exit(2);
    };
    match convert(Path::new(&dir), Path::new(&out), mirror_on) {
        Ok(s) => println!(
            "games {} (dropped {})  records {}  features {}",
            s.games_joined, s.games_dropped, s.records, s.features
        ),
        Err(e) => {
            eprintln!("dump_features: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poke_mcts::determinize::World;
    use poke_mcts::testutil::{build_state, duel, mon};

    fn tmp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("poke_mcts_dumpf_{}_{}", tag, std::process::id()))
    }

    fn read_u64s(p: &Path) -> Vec<u64> {
        std::fs::read(p).unwrap().chunks(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
    }
    fn read_u32s(p: &Path) -> Vec<u32> {
        std::fs::read(p).unwrap().chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
    }
    fn read_u16s(p: &Path) -> Vec<u16> {
        std::fs::read(p).unwrap().chunks(2).map(|c| u16::from_le_bytes(c.try_into().unwrap())).collect()
    }
    fn read_f32s(p: &Path) -> Vec<f32> {
        std::fs::read(p).unwrap().chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
    }

    fn setup_dump(dir: &Path) {
        let (mut normal, teams) = duel(mon(445, 24, [89, 14, 0, 0]), mon(248, 45, [89, 242, 0, 0]));
        normal.sides[1].team[0].current_hp /= 2;
        let (mut fainted, fteams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0])],
        );
        fainted.sides[0].team[0].current_hp = 0;
        fainted.sides[1].team[0].max_hp = 200;
        fainted.sides[1].team[0].current_hp = 100;
        let mut forced = fainted;
        forced.sides[1].team[0].current_hp = 50;
        forced.phase = pkmn_engine::state::PHASE_SWITCH_P1;
        std::fs::create_dir_all(dir).unwrap();
        // pair (0,1) held out (pair key 0); pair (2,3) not (pair key 1); 40 has no outcome
        for (tag, st, tm, _v) in [
            (0u64, normal, &teams, 0.75f32),
            (1, normal, &teams, 0.9),
            (2, fainted, &fteams, 0.6),
            (3, forced, &fteams, 0.8),
            (40, normal, &teams, 0.5),
        ] {
            let worlds = vec![World { state: st, teams: tm.clone(), weight: 1.0 }];
            #[cfg(not(feature = "train_value"))]
            train_dump::dump_worlds(dir.to_str().unwrap(), tag, &worlds, 0, 5).unwrap();
            #[cfg(feature = "train_value")]
            train_dump::dump_worlds(dir.to_str().unwrap(), tag, &worlds, 0, 5, _v).unwrap();
        }
        let outcomes = r#"{"game_tag":0,"winner_side0":1.0,"turns":30,"seed_quad":[1,2,3,4]}
{"game_tag":1,"winner_side0":0.0,"turns":31,"seed_quad":[1,2,3,4]}
{"game_tag":2,"winner_side0":0.5,"turns":12,"seed_quad":[5,6,7,8]}
{"game_tag":3,"winner_side0":1.0,"turns":40,"seed_quad":[5,6,7,8]}
"#;
        std::fs::write(dir.join("outcomes.jsonl"), outcomes).unwrap();
    }

    #[test]
    fn converts_joins_mirrors_and_keys_holdout_on_pair() {
        let dir = tmp_dir("full");
        let out = tmp_dir("full_out");
        setup_dump(&dir);

        let stats = convert(&dir, &out, true).unwrap();
        assert_eq!(stats.games_joined, 4);
        assert_eq!(stats.games_dropped, 1);
        assert_eq!(stats.records, 8);

        let labels = read_f32s(&out.join("labels.bin"));
        assert_eq!(labels, vec![1.0, 0.0, 0.0, 1.0, 0.5, 0.5, 1.0, 0.0]);

        let held = std::fs::read(out.join("held_out.bin")).unwrap();
        assert_eq!(held, vec![1, 1, 1, 1, 0, 0, 0, 0]);

        let gids = read_u64s(&out.join("game_id.bin"));
        assert_eq!(gids, vec![0, 0, 1, 1, 2, 2, 3, 3]);

        let hand = read_f32s(&out.join("hand_eval.bin"));
        assert!(hand[0] > 0.0, "side0 ahead in fixture");
        for k in (0..8).step_by(2) {
            assert_eq!(hand[k + 1], -hand[k], "mirrored hand_eval must negate");
        }

        let index = read_u64s(&out.join("index.bin"));
        assert_eq!(index.len(), 9);
        assert_eq!(index[0], 0);
        assert!(index.windows(2).all(|w| w[0] < w[1]));
        let ids = read_u32s(&out.join("features.bin"));
        assert_eq!(ids.len() as u64, *index.last().unwrap());
        assert_eq!(ids.len() as u64, stats.features);

        let tok = read_u16s(&out.join("token_index.bin"));
        assert_eq!(tok.len(), stats.records as usize * features::NUM_SEGMENTS);
        for r in 0..stats.records as usize {
            let lens = &tok[r * features::NUM_SEGMENTS..(r + 1) * features::NUM_SEGMENTS];
            let want = index[r + 1] - index[r];
            assert_eq!(lens.iter().map(|&n| n as u64).sum::<u64>(), want, "record {r} lens");
        }
        for r in (0..stats.records as usize).step_by(2) {
            let o = &tok[r * features::NUM_SEGMENTS..(r + 1) * features::NUM_SEGMENTS];
            let m = &tok[(r + 1) * features::NUM_SEGMENTS..(r + 2) * features::NUM_SEGMENTS];
            let mut want: Vec<u16> = Vec::with_capacity(features::NUM_SEGMENTS);
            want.extend_from_slice(&o[6..12]);
            want.extend_from_slice(&o[0..6]);
            want.push(o[13]);
            want.push(o[12]);
            want.push(o[14]);
            assert_eq!(m, want.as_slice(), "record {r} mirror lens must side-swap");
        }
        assert!(!out.join("visits.bin").exists());

        let orig: Vec<u32> = ids[index[0] as usize..index[1] as usize].to_vec();
        let mirrored: Vec<u32> = ids[index[1] as usize..index[2] as usize].to_vec();
        let mut flipped: Vec<u32> = orig.iter().map(|&i| features::flip_side(i)).collect();
        flipped.sort_unstable();
        let mut mirrored_sorted = mirrored.clone();
        mirrored_sorted.sort_unstable();
        assert_eq!(mirrored_sorted, flipped);

        let dense = read_f32s(&out.join("dense.bin"));
        assert_eq!(dense.len(), stats.records as usize * features::DENSE_DIM);
        assert_eq!(
            std::fs::metadata(out.join("dense.bin")).unwrap().len(),
            stats.records * features::DENSE_DIM as u64 * 4
        );
        let d0: Vec<f32> = dense[0..features::DENSE_DIM].to_vec();
        let expected0: Vec<f32> = vec![
            1.0,
            145.0f32 / 291.0,
            1.0,
            1.0,
            1.0 - 145.0f32 / 291.0,
            0.0,
            hand[0] * 0.0125,
        ];
        assert_eq!(d0, expected0);
        assert!(d0[6] > 0.0, "eval float must be live in the fixture");
        let d1: Vec<f32> = dense[features::DENSE_DIM..2 * features::DENSE_DIM].to_vec();
        assert_eq!(d1, vec![d0[1], d0[0], d0[3], d0[2], -d0[4], -d0[5], -d0[6]]);

        let row = |k: usize| -> Vec<f32> {
            dense[k * features::DENSE_DIM..(k + 1) * features::DENSE_DIM].to_vec()
        };
        assert_eq!(row(4), vec![1.0, 0.5, 1.0, 1.0, 0.5, 0.0, 0.625]);
        assert_eq!(row(5), vec![0.5, 1.0, 1.0, 1.0, -0.5, 0.0, -0.625]);
        assert_eq!(row(6), vec![1.0, 0.25, 1.0, 1.0, 0.75, 0.0, 0.9375]);
        assert_eq!(row(7), vec![0.25, 1.0, 1.0, 1.0, -0.75, 0.0, -0.9375]);

        #[cfg(feature = "train_value")]
        {
            let vals = read_f32s(&out.join("values.bin"));
            assert_eq!(
                vals,
                vec![0.75, 1.0 - 0.75, 0.9, 1.0 - 0.9f32, 0.6, 1.0 - 0.6f32, 0.8, 1.0 - 0.8f32]
            );
            for k in (0..8).step_by(2) {
                assert_eq!(vals[k + 1], 1.0 - vals[k], "mirror value must be the complement");
            }
        }
        #[cfg(not(feature = "train_value"))]
        assert!(!out.join("values.bin").exists());

        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["feature_spec_version"], features::FEATURE_SPEC_VERSION);
        assert_eq!(meta["records"], 8);
        assert_eq!(meta["games_dropped"], 1);
        assert_eq!(meta["vocab"], features::vocab_size());
        assert_eq!(meta["dense_dim"], features::DENSE_DIM);
        assert_eq!(meta["values"], cfg!(feature = "train_value"));
        assert_eq!(meta["token_segments"], features::NUM_SEGMENTS as u64);
        assert_eq!(meta["visits"], false);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn no_mirror_emits_one_record_per_dump() {
        let dir = tmp_dir("nomirror");
        let out = tmp_dir("nomirror_out");
        setup_dump(&dir);

        let stats = convert(&dir, &out, false).unwrap();
        assert_eq!(stats.records, 4);
        let labels = read_f32s(&out.join("labels.bin"));
        assert_eq!(labels, vec![1.0, 0.0, 0.5, 1.0]);

        let gids = read_u64s(&out.join("game_id.bin"));
        assert_eq!(gids, vec![0, 1, 2, 3]);

        #[cfg(feature = "train_value")]
        {
            let vals = read_f32s(&out.join("values.bin"));
            assert_eq!(vals, vec![0.75, 0.9, 0.6, 0.8]);
        }

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn rejects_length_mismatched_shard() {
        let dir = tmp_dir("badlen");
        let out = tmp_dir("badlen_out");
        setup_dump(&dir);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("0.records.bin"))
            .unwrap();
        f.write_all(&[0u8]).unwrap();
        drop(f);
        let err = convert(&dir, &out, true).unwrap_err();
        assert!(err.to_string().contains("not divisible"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[cfg(feature = "train_value")]
    #[test]
    fn rejects_old_format_records() {
        let dir = tmp_dir("oldfmt");
        let out = tmp_dir("oldfmt_out");
        std::fs::create_dir_all(&dir).unwrap();
        let (state, _teams) = duel(mon(445, 24, [89, 14, 0, 0]), mon(248, 45, [89, 242, 0, 0]));
        // pre-version record layout: no leading version byte, no root_value
        let old = bincode::serialize(&(0u64, 5u16, 0u8, 0u8, state)).unwrap();
        let mut bytes = old.clone();
        bytes.extend_from_slice(&old);
        std::fs::write(dir.join("0.records.bin"), &bytes).unwrap();
        std::fs::write(dir.join("outcomes.jsonl"), "{\"game_tag\":0,\"winner_side0\":1.0}\n")
            .unwrap();
        let err = convert(&dir, &out, true).unwrap_err();
        assert!(err.to_string().contains("not divisible"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[cfg(feature = "train_value")]
    #[test]
    fn rejects_wrong_record_version() {
        let dir = tmp_dir("badver");
        let out = tmp_dir("badver_out");
        setup_dump(&dir);
        let p = dir.join("0.records.bin");
        let mut bytes = std::fs::read(&p).unwrap();
        bytes[0] = 1; // leading field of the first record is record_version
        std::fs::write(&p, &bytes).unwrap();
        let err = convert(&dir, &out, true).unwrap_err();
        assert!(err.to_string().contains("record_version"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }
}
