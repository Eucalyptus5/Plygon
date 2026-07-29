use poke_mcts::action_features::{action_features_with, ActionDenseMode, ACTION_DENSE_DIM};
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
// visits (bytes 0-13) per record, gated by meta "visits", with u8-per-row
// visits_mask.bin (1 = decider-perspective labeled row) and decider.bin
// (the record's side_of_decider on both rows); move_ids.bin = 4 x u16 (the
// row's token-side-0 active mon's moveset ids) and legal_mask.bin = 14 x u8
// (legal_actions of the row's token-side-0 side, byte 255 outside the
// vector), both emitted with the visits sidecars; action_dense.bin = 14 x 8
// f32 per row (the row's token-side-0 side's per-action block, always computed
// on the unmirrored state), gated by meta "action_dense"; meta.json.

pub const VISIT_ACTIONS: usize = 14;

#[derive(Debug, PartialEq)]
pub struct ConvertStats {
    pub games_joined: u64,
    pub games_dropped: u64,
    pub records: u64,
    pub features: u64,
    pub labels_attached: u64,
    pub labels_invalid: u64,
    pub labels_legal_mismatch: u64,
    pub action_calls: u64,
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

// labels.jsonl rows keyed "<game_tag>.records.bin:<record_index>"; rows
// carrying "invalid" or "legal_match": false are skipped (their records stay
// mask-0 on both rows).
fn read_labels(path: &Path) -> io::Result<(HashMap<String, [f32; VISIT_ACTIONS]>, u64, u64)> {
    let bad = |m: String| io::Error::new(io::ErrorKind::InvalidData, m);
    let f = File::open(path)?;
    let mut map = HashMap::new();
    let mut invalid = 0u64;
    let mut legal_mismatch = 0u64;
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line)
            .map_err(|e| bad(format!("{}: {e}", path.display())))?;
        if v.get("invalid").is_some() {
            invalid += 1;
            continue;
        }
        let key = v["key"]
            .as_str()
            .ok_or_else(|| bad(format!("{}: label row missing key", path.display())))?;
        match v["legal_match"].as_bool() {
            Some(true) => {}
            Some(false) => {
                legal_mismatch += 1;
                continue;
            }
            None => return Err(bad(format!("{key}: label row missing legal_match"))),
        }
        let mdist = v["mdist"]
            .as_object()
            .ok_or_else(|| bad(format!("{key}: label row missing mdist")))?;
        let mut row = [0f32; VISIT_ACTIONS];
        for (k, val) in mdist {
            let b: usize = k
                .parse()
                .map_err(|_| bad(format!("{key}: non-numeric action byte {k}")))?;
            if b >= VISIT_ACTIONS {
                return Err(bad(format!("{key}: action byte {b} outside 0-13")));
            }
            row[b] = val
                .as_f64()
                .ok_or_else(|| bad(format!("{key}: non-numeric share for byte {b}")))?
                as f32;
        }
        map.insert(key.to_string(), row);
    }
    Ok((map, invalid, legal_mismatch))
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

pub fn convert(
    dir: &Path,
    out_dir: &Path,
    mirror_on: bool,
    visits: Option<&Path>,
    action_mode: Option<ActionDenseMode>,
) -> io::Result<ConvertStats> {
    let outcomes = read_outcomes(&dir.join("outcomes.jsonl"))?;
    let mut labels_invalid = 0u64;
    let mut labels_legal_mismatch = 0u64;
    let labels_map = match visits {
        Some(p) => {
            let (map, inv, mismatch) = read_labels(p)?;
            labels_invalid = inv;
            labels_legal_mismatch = mismatch;
            Some(map)
        }
        None => None,
    };
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
    let mut vis_out = if labels_map.is_some() {
        Some((
            BufWriter::new(File::create(out_dir.join("visits.bin"))?),
            BufWriter::new(File::create(out_dir.join("visits_mask.bin"))?),
            BufWriter::new(File::create(out_dir.join("decider.bin"))?),
            BufWriter::new(File::create(out_dir.join("move_ids.bin"))?),
            BufWriter::new(File::create(out_dir.join("legal_mask.bin"))?),
        ))
    } else {
        None
    };
    let mut ad_out = if action_mode == Some(ActionDenseMode::Full) {
        Some(BufWriter::new(File::create(out_dir.join("action_dense.bin"))?))
    } else {
        None
    };

    let rec_size = fixed_record_size();
    let mut stats = ConvertStats {
        games_joined: 0,
        games_dropped: 0,
        records: 0,
        features: 0,
        labels_attached: 0,
        labels_invalid,
        labels_legal_mismatch,
        action_calls: 0,
    };
    let mut action_calls = 0u64;
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
        for (rec_idx, rec) in train_dump::read_records(path.to_str().unwrap())?.into_iter().enumerate() {
            #[cfg(feature = "train_value")]
            if rec.record_version != 2 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: record_version {} != 2", path.display(), rec.record_version),
                ));
            }
            let visit_row: Option<[f32; VISIT_ACTIONS]> = labels_map
                .as_ref()
                .and_then(|m| m.get(&format!("{tag}.records.bin:{rec_idx}")).copied());
            // per row: the token-side-0 side's active moveset + legal set,
            // read from the source state (even row = side 0, mirror = side 1)
            let side_inputs = |side: usize| -> ([u16; 4], [u8; VISIT_ACTIONS]) {
                let s = &rec.state.sides[side];
                let moves = s.team[s.active_index as usize].moves;
                let mut mask = [0u8; VISIT_ACTIONS];
                for &a in pkmn_engine::state::legal_actions(&rec.state, side).as_slice() {
                    if (a as usize) < VISIT_ACTIONS {
                        mask[a as usize] = 1;
                    }
                }
                (moves, mask)
            };
            let mut emit = |state: &pkmn_engine::state::BattleState,
                            label: f32,
                            vrow: Option<&[f32; VISIT_ACTIONS]>,
                            side0: (
                &[u16; 4],
                &[u8; VISIT_ACTIONS],
                Option<&[[f32; ACTION_DENSE_DIM]; VISIT_ACTIONS]>,
            )|
             -> io::Result<()> {
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
                if let Some((vs, vm, dc, mv, lm)) = vis_out.as_mut() {
                    match vrow {
                        Some(row) => {
                            for v in row {
                                vs.write_all(&v.to_le_bytes())?;
                            }
                            vm.write_all(&[1])?;
                            stats.labels_attached += 1;
                        }
                        None => {
                            vs.write_all(&[0u8; VISIT_ACTIONS * 4])?;
                            vm.write_all(&[0])?;
                        }
                    }
                    dc.write_all(&[rec.side_of_decider])?;
                    for m in side0.0 {
                        mv.write_all(&m.to_le_bytes())?;
                    }
                    lm.write_all(side0.1)?;
                }
                if let (Some(ad), Some(block)) = (ad_out.as_mut(), side0.2) {
                    for row in block {
                        for v in row {
                            ad.write_all(&v.to_le_bytes())?;
                        }
                    }
                }
                Ok(())
            };
            let (moves0, legal0) = side_inputs(0);
            let (moves1, legal1) = side_inputs(1);
            // never on a mirrored state: mirror() swaps sides but leaves phase
            // and pending_actions side-specific
            let act0 = action_mode.map(|m| {
                action_calls += 1;
                action_features_with(&rec.state, 0, m)
            });
            if let Some(row) = visit_row.as_ref() {
                let legal_d = if rec.side_of_decider == 0 { &legal0 } else { &legal1 };
                for (b, &v) in row.iter().enumerate() {
                    if v > 0.0 && legal_d[b] == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "{tag}.records.bin:{rec_idx}: labeled visit share on illegal action byte {b}"
                            ),
                        ));
                    }
                }
            }
            // the label lands on the decider-perspective row only: the original
            // when side_of_decider==0, the mirror when ==1
            let orig_row = if rec.side_of_decider == 0 { visit_row.as_ref() } else { None };
            emit(&rec.state, z, orig_row, (&moves0, &legal0, act0.as_ref()))?;
            #[cfg(feature = "train_value")]
            va.write_all(&rec.root_value.to_le_bytes())?;
            if mirror_on {
                let mirror_row = if rec.side_of_decider == 1 { visit_row.as_ref() } else { None };
                let act1 = action_mode.map(|m| {
                    action_calls += 1;
                    action_features_with(&rec.state, 1, m)
                });
                emit(
                    &features::mirror(&rec.state),
                    1.0 - z,
                    mirror_row,
                    (&moves1, &legal1, act1.as_ref()),
                )?;
                #[cfg(feature = "train_value")]
                va.write_all(&(1.0 - rec.root_value).to_le_bytes())?;
            }
        }
        stats.games_joined += 1;
    }
    stats.action_calls = action_calls;
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
    if let Some((vs, vm, dc, mv, lm)) = vis_out.as_mut() {
        vs.flush()?;
        vm.flush()?;
        dc.flush()?;
        mv.flush()?;
        lm.flush()?;
    }
    if let Some(ad) = ad_out.as_mut() {
        ad.flush()?;
    }

    let mut meta = serde_json::json!({
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
        "visits": labels_map.is_some(),
        "move_ids": labels_map.is_some(),
        "legal_mask": labels_map.is_some(),
    });
    if ad_out.is_some() {
        let m = meta.as_object_mut().unwrap();
        m.insert("action_dense".to_string(), true.into());
        m.insert("action_dense_dim".to_string(), (ACTION_DENSE_DIM as u64).into());
    }
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
    let mut visits: Option<String> = None;
    let mut mirror_on = true;
    let mut action_mode = Some(ActionDenseMode::Full);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next(),
            "--visits" => visits = it.next(),
            "--no-mirror" => mirror_on = false,
            "--no-action-dense" => action_mode = None,
            "--action-dense-mode" => {
                let v = it.next().unwrap_or_default();
                action_mode = match v.as_str() {
                    "off" => None,
                    "priced" => Some(ActionDenseMode::Priced),
                    "full" => Some(ActionDenseMode::Full),
                    _ => {
                        eprintln!("unexpected --action-dense-mode value: {v}");
                        std::process::exit(2);
                    }
                };
            }
            _ if dir.is_none() => dir = Some(a),
            _ => {
                eprintln!("unexpected argument: {a}");
                std::process::exit(2);
            }
        }
    }
    let (Some(dir), Some(out)) = (dir, out) else {
        eprintln!("usage: dump_features <dir> --out <train_dir> [--no-mirror] [--visits <labels.jsonl>] [--action-dense-mode off|priced|full] | dump_features --spec");
        std::process::exit(2);
    };
    match convert(
        Path::new(&dir),
        Path::new(&out),
        mirror_on,
        visits.as_deref().map(Path::new),
        action_mode,
    ) {
        Ok(s) => println!(
            "games {} (dropped {})  records {}  features {}  labels {}  label-rows-invalid {}  label-rows-legal-mismatch {}",
            s.games_joined,
            s.games_dropped,
            s.records,
            s.features,
            s.labels_attached,
            s.labels_invalid,
            s.labels_legal_mismatch
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
    use poke_mcts::action_features::action_features;
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

        let stats = convert(&dir, &out, true, None, None).unwrap();
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
        assert!(!out.join("move_ids.bin").exists());
        assert!(!out.join("legal_mask.bin").exists());

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

        let stats = convert(&dir, &out, false, None, None).unwrap();
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
        let err = convert(&dir, &out, true, None, None).unwrap_err();
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
        let err = convert(&dir, &out, true, None, None).unwrap_err();
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
        let err = convert(&dir, &out, true, None, None).unwrap_err();
        assert!(err.to_string().contains("record_version"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    fn setup_visits_dump(dir: &Path) -> pkmn_engine::state::BattleState {
        let (state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0]), mon(130, 22, [57, 0, 0, 0])],
        );
        std::fs::create_dir_all(dir).unwrap();
        for (tag, side) in [(0u64, 0usize), (1, 1), (2, 0), (3, 0)] {
            let worlds = vec![World { state, teams: teams.clone(), weight: 1.0 }];
            #[cfg(not(feature = "train_value"))]
            train_dump::dump_worlds(dir.to_str().unwrap(), tag, &worlds, side, 5).unwrap();
            #[cfg(feature = "train_value")]
            train_dump::dump_worlds(dir.to_str().unwrap(), tag, &worlds, side, 5, 0.5).unwrap();
        }
        let outcomes = "{\"game_tag\":0,\"winner_side0\":1.0}\n{\"game_tag\":1,\"winner_side0\":0.0}\n{\"game_tag\":2,\"winner_side0\":1.0}\n{\"game_tag\":3,\"winner_side0\":0.0}\n";
        std::fs::write(dir.join("outcomes.jsonl"), outcomes).unwrap();
        state
    }

    fn legal_row(state: &pkmn_engine::state::BattleState, side: usize) -> [u8; VISIT_ACTIONS] {
        let mut mask = [0u8; VISIT_ACTIONS];
        for &a in pkmn_engine::state::legal_actions(state, side).as_slice() {
            if (a as usize) < VISIT_ACTIONS {
                mask[a as usize] = 1;
            }
        }
        mask
    }

    #[test]
    fn visits_label_lands_on_decider_perspective_row_only() {
        let dir = tmp_dir("visits");
        let out = tmp_dir("visits_out");
        let state = setup_visits_dump(&dir);
        // game 0: decider side 0; game 1: decider side 1; game 2: invalid
        // label row; game 3: legal_match false (excluded, mask 0)
        let labels = dir.join("labels.jsonl");
        std::fs::write(
            &labels,
            "{\"key\":\"0.records.bin:0\",\"mdist\":{\"0\":0.625,\"5\":0.375},\"iterations\":1000,\"elapsed\":0.2,\"legal_match\":true}\n\
             {\"key\":\"1.records.bin:0\",\"mdist\":{\"5\":1.0},\"iterations\":900,\"elapsed\":0.2,\"legal_match\":true}\n\
             {\"key\":\"2.records.bin:0\",\"invalid\":\"struggle\"}\n\
             {\"key\":\"3.records.bin:0\",\"mdist\":{\"0\":1.0},\"iterations\":800,\"elapsed\":0.2,\"legal_match\":false}\n",
        )
        .unwrap();

        let stats = convert(&dir, &out, true, Some(&labels), None).unwrap();
        assert_eq!(stats.records, 8);
        assert_eq!(stats.labels_attached, 2);
        assert_eq!(stats.labels_invalid, 1);
        assert_eq!(stats.labels_legal_mismatch, 1);

        let vis = read_f32s(&out.join("visits.bin"));
        assert_eq!(vis.len(), 8 * VISIT_ACTIONS);
        let row = |k: usize| &vis[k * VISIT_ACTIONS..(k + 1) * VISIT_ACTIONS];
        let mut want0 = [0f32; VISIT_ACTIONS];
        want0[0] = 0.625;
        want0[5] = 0.375;
        let mut want1 = [0f32; VISIT_ACTIONS];
        want1[5] = 1.0;
        assert_eq!(row(0), want0, "decider 0: label on the original row");
        assert_eq!(row(1), [0f32; VISIT_ACTIONS], "decider 0: mirror row all-zero");
        assert_eq!(row(2), [0f32; VISIT_ACTIONS], "decider 1: original row all-zero");
        assert_eq!(row(3), want1, "decider 1: label on the mirror row");
        assert_eq!(row(4), [0f32; VISIT_ACTIONS], "invalid label: no row");
        assert_eq!(row(5), [0f32; VISIT_ACTIONS], "invalid label: no row");
        assert_eq!(row(6), [0f32; VISIT_ACTIONS], "legal mismatch: no row");
        assert_eq!(row(7), [0f32; VISIT_ACTIONS], "legal mismatch: no row");

        let mask = std::fs::read(out.join("visits_mask.bin")).unwrap();
        assert_eq!(mask, vec![1, 0, 0, 1, 0, 0, 0, 0]);
        let decider = std::fs::read(out.join("decider.bin")).unwrap();
        assert_eq!(decider, vec![0, 0, 1, 1, 0, 0, 0, 0]);

        let mids = read_u16s(&out.join("move_ids.bin"));
        assert_eq!(mids.len(), 8 * 4);
        for r in 0..8 {
            let got = &mids[r * 4..(r + 1) * 4];
            let want: [u16; 4] = if r % 2 == 0 { [89, 14, 0, 0] } else { [89, 242, 0, 0] };
            assert_eq!(got, want, "row {r}: token-side-0 active moveset");
        }

        let lmask = std::fs::read(out.join("legal_mask.bin")).unwrap();
        assert_eq!(lmask.len(), 8 * VISIT_ACTIONS);
        let want_l0 = legal_row(&state, 0);
        let want_l1 = legal_row(&state, 1);
        for r in 0..8 {
            let got = &lmask[r * VISIT_ACTIONS..(r + 1) * VISIT_ACTIONS];
            let want = if r % 2 == 0 { &want_l0 } else { &want_l1 };
            assert_eq!(got, want, "row {r}: token-side-0 legal set");
        }
        // labeled rows carry visit mass only on legal bytes
        for r in [0usize, 3] {
            let lm = &lmask[r * VISIT_ACTIONS..(r + 1) * VISIT_ACTIONS];
            for b in 0..VISIT_ACTIONS {
                assert!(row(r)[b] == 0.0 || lm[b] == 1, "row {r} byte {b}: share on illegal");
            }
        }

        // the labeled row's tokens are the decider's: row 3 must equal the
        // mirrored state's emission, whose token side 0 is physical side 1
        let index = read_u64s(&out.join("index.bin"));
        let ids = read_u32s(&out.join("features.bin"));
        let row3: Vec<u32> = ids[index[3] as usize..index[4] as usize].to_vec();
        let mut buf: Vec<u32> = Vec::new();
        features::extract_segmented(&features::mirror(&state), &mut buf);
        assert_eq!(row3, buf, "labeled mirror row must carry the decider-as-side-0 tokens");

        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["visits"], true);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn visits_sidecars_absent_without_labels_present_with() {
        let dir = tmp_dir("vispair");
        let out_off = tmp_dir("vispair_off");
        let out_on = tmp_dir("vispair_on");
        setup_visits_dump(&dir);
        let labels = dir.join("labels.jsonl");
        std::fs::write(
            &labels,
            "{\"key\":\"0.records.bin:0\",\"mdist\":{\"0\":1.0},\"iterations\":1,\"elapsed\":0.1,\"legal_match\":true}\n",
        )
        .unwrap();

        convert(&dir, &out_off, true, None, None).unwrap();
        assert!(!out_off.join("visits.bin").exists());
        assert!(!out_off.join("visits_mask.bin").exists());
        assert!(!out_off.join("decider.bin").exists());
        assert!(!out_off.join("move_ids.bin").exists());
        assert!(!out_off.join("legal_mask.bin").exists());
        let meta_off: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out_off.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta_off["visits"], false);
        assert_eq!(meta_off["move_ids"], false);
        assert_eq!(meta_off["legal_mask"], false);

        convert(&dir, &out_on, true, Some(&labels), None).unwrap();
        assert!(out_on.join("visits.bin").exists());
        assert!(out_on.join("visits_mask.bin").exists());
        assert!(out_on.join("decider.bin").exists());
        assert_eq!(std::fs::metadata(out_on.join("visits_mask.bin")).unwrap().len(), 8);
        assert_eq!(std::fs::metadata(out_on.join("decider.bin")).unwrap().len(), 8);
        assert_eq!(
            std::fs::metadata(out_on.join("visits.bin")).unwrap().len(),
            8 * VISIT_ACTIONS as u64 * 4
        );
        assert_eq!(std::fs::metadata(out_on.join("move_ids.bin")).unwrap().len(), 8 * 4 * 2);
        assert_eq!(
            std::fs::metadata(out_on.join("legal_mask.bin")).unwrap().len(),
            8 * VISIT_ACTIONS as u64
        );
        let meta_on: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out_on.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta_on["visits"], true);
        assert_eq!(meta_on["move_ids"], true);
        assert_eq!(meta_on["legal_mask"], true);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out_off).ok();
        std::fs::remove_dir_all(&out_on).ok();
    }

    #[test]
    fn label_row_missing_legal_match_refused() {
        let dir = tmp_dir("nolm");
        let out = tmp_dir("nolm_out");
        setup_visits_dump(&dir);
        let labels = dir.join("labels.jsonl");
        std::fs::write(
            &labels,
            "{\"key\":\"0.records.bin:0\",\"mdist\":{\"0\":1.0},\"iterations\":1,\"elapsed\":0.1}\n",
        )
        .unwrap();
        let err = convert(&dir, &out, true, Some(&labels), None).unwrap_err();
        assert!(err.to_string().contains("legal_match"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn label_share_on_illegal_byte_refused() {
        let dir = tmp_dir("illb");
        let out = tmp_dir("illb_out");
        setup_visits_dump(&dir);
        // move slot 2 is empty (move id 0) so byte 2 is not legal for side 0
        let labels = dir.join("labels.jsonl");
        std::fs::write(
            &labels,
            "{\"key\":\"0.records.bin:0\",\"mdist\":{\"2\":1.0},\"iterations\":1,\"elapsed\":0.1,\"legal_match\":true}\n",
        )
        .unwrap();
        let err = convert(&dir, &out, true, Some(&labels), None).unwrap_err();
        assert!(err.to_string().contains("illegal action byte 2"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    const ACTION_ROW: usize = VISIT_ACTIONS * ACTION_DENSE_DIM;

    fn meta_of(out: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(out.join("meta.json")).unwrap()).unwrap()
    }

    fn assert_no_action_dense(out: &Path, what: &str) {
        assert!(!out.join("action_dense.bin").exists(), "{what}: action_dense.bin must not exist");
        let meta = meta_of(out);
        assert!(meta.get("action_dense").is_none(), "{what}: meta must omit action_dense, got {:?}", meta.get("action_dense"));
        assert!(
            meta.get("action_dense_dim").is_none(),
            "{what}: meta must omit action_dense_dim, got {:?}",
            meta.get("action_dense_dim")
        );
    }

    #[test]
    fn action_dense_priced_writes_no_sidecar_or_meta_keys() {
        let dir = tmp_dir("adpriced");
        let out = tmp_dir("adpriced_out");
        setup_dump(&dir);
        let stats = convert(&dir, &out, true, None, Some(ActionDenseMode::Priced)).unwrap();
        assert_eq!(stats.action_calls, stats.records, "priced must still price every row");
        assert_no_action_dense(&out, "priced");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn action_dense_full_writes_sidecar_and_meta_keys_without_visits() {
        let dir = tmp_dir("adfull");
        let out = tmp_dir("adfull_out");
        setup_dump(&dir);
        let stats = convert(&dir, &out, true, None, Some(ActionDenseMode::Full)).unwrap();
        assert_eq!(stats.records, 8);
        assert_eq!(
            std::fs::metadata(out.join("action_dense.bin")).unwrap().len(),
            stats.records * ACTION_ROW as u64 * 4,
            "action_dense.bin must be 14x8 f32 per row"
        );
        let meta = meta_of(&out);
        assert_eq!(meta["action_dense"], true);
        assert_eq!(meta["action_dense_dim"], ACTION_DENSE_DIM as u64);
        assert_eq!(meta["visits"], false, "the sidecar must not need --visits");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn action_dense_mode_leaves_the_shared_shards_byte_identical() {
        let dir = tmp_dir("adident");
        setup_visits_dump(&dir);
        let labels = dir.join("labels.jsonl");
        std::fs::write(
            &labels,
            "{\"key\":\"0.records.bin:0\",\"mdist\":{\"0\":0.625,\"5\":0.375},\"iterations\":1,\"elapsed\":0.1,\"legal_match\":true}\n",
        )
        .unwrap();
        let outs = [tmp_dir("adident_off"), tmp_dir("adident_priced"), tmp_dir("adident_full")];
        let modes = [None, Some(ActionDenseMode::Priced), Some(ActionDenseMode::Full)];
        for (o, m) in outs.iter().zip(modes) {
            convert(&dir, o, true, Some(&labels), m).unwrap();
        }
        for f in ["index.bin", "features.bin", "token_index.bin", "dense.bin", "legal_mask.bin"] {
            let want = std::fs::read(outs[0].join(f)).unwrap();
            assert!(!want.is_empty(), "{f} must be non-empty in the fixture");
            for o in &outs[1..] {
                assert_eq!(want, std::fs::read(o.join(f)).unwrap(), "{f} must not vary with the action-dense mode");
            }
        }
        for o in &outs {
            std::fs::remove_dir_all(o).ok();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn action_calls_are_one_per_emitted_row_in_priced_and_full() {
        let dir = tmp_dir("adcalls");
        setup_dump(&dir);
        for (mode, want) in [
            (None, 0),
            (Some(ActionDenseMode::Priced), 8),
            (Some(ActionDenseMode::Full), 8),
        ] {
            let out = tmp_dir("adcalls_out");
            let stats = convert(&dir, &out, true, None, mode).unwrap();
            assert_eq!(stats.records, 8);
            assert_eq!(stats.action_calls, want, "mode {mode:?} with mirror on");
            std::fs::remove_dir_all(&out).ok();
        }
        let out = tmp_dir("adcalls_nomirror");
        let stats = convert(&dir, &out, false, None, Some(ActionDenseMode::Priced)).unwrap();
        assert_eq!(stats.records, 4);
        assert_eq!(stats.action_calls, 4, "--no-mirror must cost one call per record");
        std::fs::remove_dir_all(&out).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn action_dense_mirror_row_holds_the_unmirrored_side_one_block() {
        let dir = tmp_dir("adorient");
        let out = tmp_dir("adorient_out");
        let (base, teams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0]), mon(130, 22, [57, 0, 0, 0])],
        );
        // under PHASE_ACTIONS mirror() IS a symmetry for this block, so the
        // orientation only shows up in the side-specific switch phases; P1
        // makes the original row non-degenerate, P2 the mirror row
        let mut p1 = base;
        p1.phase = pkmn_engine::state::PHASE_SWITCH_P1;
        let mut p2 = base;
        p2.phase = pkmn_engine::state::PHASE_SWITCH_P2;
        std::fs::create_dir_all(&dir).unwrap();
        let worlds = vec![
            World { state: p1, teams: teams.clone(), weight: 1.0 },
            World { state: p2, teams, weight: 1.0 },
        ];
        #[cfg(not(feature = "train_value"))]
        train_dump::dump_worlds(dir.to_str().unwrap(), 0, &worlds, 0, 5).unwrap();
        #[cfg(feature = "train_value")]
        train_dump::dump_worlds(dir.to_str().unwrap(), 0, &worlds, 0, 5, 0.5).unwrap();
        std::fs::write(dir.join("outcomes.jsonl"), "{\"game_tag\":0,\"winner_side0\":1.0}\n").unwrap();

        let stats = convert(&dir, &out, true, None, Some(ActionDenseMode::Full)).unwrap();
        assert_eq!(stats.records, 4);
        let stored = read_f32s(&out.join("action_dense.bin"));
        let block = |r: usize| stored[r * ACTION_ROW..(r + 1) * ACTION_ROW].to_vec();
        let flat = |b: [[f32; ACTION_DENSE_DIM]; VISIT_ACTIONS]| {
            b.iter().flatten().copied().collect::<Vec<f32>>()
        };
        let live = |v: &Vec<f32>| v.iter().any(|&x| x != 0.0);

        let p1_side0 = flat(action_features(&p1, 0));
        let p1_side1 = flat(action_features(&p1, 1));
        let p2_side0 = flat(action_features(&p2, 0));
        let p2_side1 = flat(action_features(&p2, 1));
        assert!(live(&p1_side0), "P1 record: the original row's block must not be all-zero");
        assert!(live(&p2_side1), "P2 record: the mirror row's block must not be all-zero");

        assert_eq!(block(0), p1_side0, "P1 original row must be the unmirrored side-0 block");
        assert_eq!(block(1), p1_side1, "P1 mirror row must be the unmirrored side-1 block");
        assert_eq!(block(2), p2_side0, "P2 original row must be the unmirrored side-0 block");
        assert_eq!(block(3), p2_side1, "P2 mirror row must be the unmirrored side-1 block");

        let mirrored_at_0 = flat(action_features(&features::mirror(&p2), 0));
        let mirrored_at_1 = flat(action_features(&features::mirror(&p2), 1));
        assert_ne!(block(3), mirrored_at_0, "mirror row must not be mirror(state) read at side 0");
        assert_ne!(block(3), mirrored_at_1, "mirror row must not be mirror(state) read at side 1");
        let p1_mirrored_at_0 = flat(action_features(&features::mirror(&p1), 0));
        let p1_mirrored_at_1 = flat(action_features(&features::mirror(&p1), 1));
        assert_ne!(block(0), p1_mirrored_at_0, "original row must not be read off mirror(state)");
        assert_ne!(block(0), p1_mirrored_at_1, "original row must not be read off mirror(state)");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

}
