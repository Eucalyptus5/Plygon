// Self-distillation labeler: run OUR MCTS on each policy-shard record as a
// single fully-known world at the LP1 baseline config (16,384 iterations,
// c = 0.7, hand eval, fixed seed), and emit the decider's normalized root
// visit distribution in label_pool JSONL format so the dump_features --visits
// converter consumes it unchanged. The teacher here is our own search instead
// of the external teacher; everything else mirrors that labeler so a self-distill corpus
// is scale- and key-matched to the external corpus.
//
// Source = the re-emitted policy shards (one <game_tag>.records.bin per game),
// so keys "<file>:<idx>" align with the external labels.jsonl for a clean join.
// Held-out games are skipped by shard-file game_tag. Restartable: keys already
// present in --out are skipped. Shards run in parallel (search_world is
// single-threaded; rayon parallelizes across shards).

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use rayon::prelude::*;

use poke_mcts::chance::OpenLoop;
use poke_mcts::eval::Handcrafted;
use poke_mcts::policy_label::{read_records_guarded, reconstruct_teams};
use poke_mcts::rng::splitmix64;
use poke_mcts::search::{search_world, SearchParams};

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

fn read_u64_set(path: &str) -> HashSet<u64> {
    let f = std::fs::File::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut set = HashSet::new();
    for line in BufReader::new(f).lines() {
        let line = line.unwrap();
        let t = line.trim();
        if !t.is_empty() {
            set.insert(t.parse::<u64>().unwrap_or_else(|e| panic!("{path}: {t}: {e}")));
        }
    }
    set
}

fn read_done_keys(path: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let Ok(f) = std::fs::File::open(path) else { return set };
    for line in BufReader::new(f).lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(k) = v.get("key").and_then(|k| k.as_str()) {
                set.insert(k.to_string());
            }
        }
    }
    set
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out_path: Option<String> = None;
    let mut heldout: Option<String> = None;
    let mut seed_base = BASELINE_SEED;
    let mut threads: Option<usize> = None;
    let mut emit_q = false;
    let mut inputs: Vec<String> = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out_path = it.next(),
            "--heldout-games" => heldout = it.next(),
            "--seed" => seed_base = it.next().and_then(|v| v.parse().ok()).unwrap_or(BASELINE_SEED),
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()),
            "--emit-q" => emit_q = true,
            _ => inputs.push(a),
        }
    }
    let Some(out_path) = out_path else {
        eprintln!("usage: policy_selfdistill --out <labels.jsonl> [--heldout-games f] [--seed S] [--threads T] [--emit-q] <shard-dir-or-file>...");
        std::process::exit(2);
    };
    if inputs.is_empty() {
        eprintln!("no input shards given");
        std::process::exit(2);
    }

    if let Some(t) = threads {
        rayon::ThreadPoolBuilder::new().num_threads(t).build_global().unwrap();
    }

    let heldout_set = heldout.as_deref().map(read_u64_set).unwrap_or_default();
    let done = read_done_keys(&out_path);
    eprintln!("restart: {} keys already present in {out_path}", done.len());

    let mut paths = shard_paths(&inputs);
    // Drop held-out shards up front (file stem == game_tag).
    let before = paths.len();
    if !heldout_set.is_empty() {
        paths.retain(|p| {
            let stem = p.file_name().unwrap().to_string_lossy();
            let tag: u64 = stem.trim_end_matches(".records.bin").parse().unwrap_or(u64::MAX);
            !heldout_set.contains(&tag)
        });
    }
    eprintln!("shards: {} total, {} after held-out filter", before, paths.len());

    let params = SearchParams {
        time_ms: TIME_CEILING_MS,
        max_iters: BASELINE_ITERS,
        explore_coeff: EXPLORE_COEFF,
        ..Default::default()
    };

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&out_path)
        .unwrap_or_else(|e| panic!("{out_path}: {e}"));
    let writer = Mutex::new(std::io::BufWriter::new(file));
    let n_labeled = AtomicU64::new(0);
    let n_invalid = AtomicU64::new(0);
    let n_mismatch = AtomicU64::new(0);
    let n_skipped = AtomicU64::new(0);
    let t_start = Instant::now();

    paths.par_iter().for_each(|path| {
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let recs = read_records_guarded(path.to_str().unwrap()).unwrap_or_else(|e| panic!("{e}"));
        let mut buf = String::new();
        for (idx, rec) in recs.iter().enumerate() {
            let key = format!("{file_name}:{idx}");
            if done.contains(&key) {
                n_skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let decider = rec.side_of_decider as usize;
            let legal = pkmn_engine::state::legal_actions(&rec.state, decider);
            let legal_set: HashSet<u8> = legal.as_slice().iter().copied().collect();
            if legal_set.is_empty() {
                buf.push_str(&format!("{{\"key\":\"{key}\",\"invalid\":\"no_legal\"}}\n"));
                n_invalid.fetch_add(1, Ordering::Relaxed);
                continue;
            }

            let seed = splitmix64(seed_base ^ rec.game_tag).wrapping_add(idx as u64);
            // Struggle (255) lives outside the 14-vector; exclude it from the
            // mdist and legal-match set so a struggle-in-legal record lands
            // legal_match=false and is skipped consistently.
            let strug = pkmn_engine::state::ACTION_STRUGGLE;
            let mut q = [0f32; 14];
            let mut nmax = 0u32;
            let (mdist, iterations, elapsed, arm_bytes): (BTreeMap<u8, f64>, u64, f64, HashSet<u8>) =
                if legal.count <= 1 {
                    let b = legal.as_slice()[0];
                    let mut m = BTreeMap::new();
                    let mut bytes = HashSet::new();
                    if b != strug {
                        m.insert(b, 1.0);
                        bytes.insert(b);
                    }
                    (m, 0, 0.0, bytes)
                } else {
                    let teams = reconstruct_teams(&rec.state);
                    let t0 = Instant::now();
                    let r = search_world(&rec.state, &teams, &Handcrafted, &OpenLoop, &params, seed, 0, None);
                    let el = t0.elapsed().as_secs_f64();
                    let stats = r.side(decider);
                    let total: u64 = stats.iter().filter(|s| s.action != strug).map(|s| s.visits as u64).sum();
                    let mut m = BTreeMap::new();
                    let mut bytes = HashSet::new();
                    for s in stats {
                        if s.action == strug {
                            continue;
                        }
                        bytes.insert(s.action);
                        if s.visits > 0 && total > 0 {
                            m.insert(s.action, s.visits as f64 / total as f64);
                            q[s.action as usize] = s.avg_score as f32;
                            nmax = nmax.max(s.visits);
                        }
                    }
                    (m, total, (el * 1000.0).round() / 1000.0, bytes)
                };

            if mdist.is_empty() {
                buf.push_str(&format!("{{\"key\":\"{key}\",\"invalid\":\"zero_visits\"}}\n"));
                n_invalid.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let legal_match = arm_bytes == legal_set;
            if !legal_match {
                n_mismatch.fetch_add(1, Ordering::Relaxed);
            }
            let mut md = String::from("{");
            for (i, (b, s)) in mdist.iter().enumerate() {
                if i > 0 {
                    md.push(',');
                }
                md.push_str(&format!("\"{b}\":{s}"));
            }
            md.push('}');
            let qs = if emit_q {
                let v: Vec<String> = q.iter().map(|x| x.to_string()).collect();
                format!(",\"q\":[{}],\"nmax\":{nmax}", v.join(","))
            } else {
                String::new()
            };
            buf.push_str(&format!(
                "{{\"key\":\"{key}\",\"mdist\":{md},\"iterations\":{iterations},\"elapsed\":{elapsed},\"legal_match\":{legal_match}{qs}}}\n"
            ));
            n_labeled.fetch_add(1, Ordering::Relaxed);
        }
        if !buf.is_empty() {
            let mut w = writer.lock().unwrap();
            w.write_all(buf.as_bytes()).unwrap();
            w.flush().unwrap();
        }
        let lab = n_labeled.load(Ordering::Relaxed);
        if lab % 20_000 < recs.len() as u64 {
            let dt = t_start.elapsed().as_secs_f64().max(1e-6);
            eprintln!(
                "progress labeled={lab} invalid={} mismatch={} skipped={} rate={:.1}/s",
                n_invalid.load(Ordering::Relaxed),
                n_mismatch.load(Ordering::Relaxed),
                n_skipped.load(Ordering::Relaxed),
                lab as f64 / dt
            );
        }
    });

    writer.lock().unwrap().flush().unwrap();
    let dt = t_start.elapsed().as_secs_f64();
    eprintln!(
        "DONE labeled={} invalid={} mismatch={} skipped={} wall_s={:.1} rate={:.1}/s",
        n_labeled.load(Ordering::Relaxed),
        n_invalid.load(Ordering::Relaxed),
        n_mismatch.load(Ordering::Relaxed),
        n_skipped.load(Ordering::Relaxed),
        dt,
        n_labeled.load(Ordering::Relaxed) as f64 / dt.max(1e-6),
    );
    println!(
        "{{\"labeled\":{},\"invalid\":{},\"legal_mismatch\":{},\"skipped\":{},\"wall_s\":{:.1}}}",
        n_labeled.load(Ordering::Relaxed),
        n_invalid.load(Ordering::Relaxed),
        n_mismatch.load(Ordering::Relaxed),
        n_skipped.load(Ordering::Relaxed),
        dt,
    );
}
