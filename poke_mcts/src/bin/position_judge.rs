// A FINITE per-world iteration cap (not the wall clock) fixes each repeat's iteration count, so both
// arms share identical per-seed MCTS noise under the shared seed set and the paired-CRN pairing holds.
use poke_mcts::audit_snapshot::PositionSnapshot;
use poke_mcts::belief::Belief;
use poke_mcts::rng::splitmix64;
use poke_mcts::selfplay::{play_from_with_budget, reconstruct_opp_belief, JudgeBudget, Policy};

struct Args {
    snapshots: Option<String>,
    comparison: Option<String>,
    policy: Policy,
    judge_worlds: usize,
    judge_iters: u64,
    judge_ms: u64,
    n: usize,
    seed: u64,
    shard_i: usize,
    shard_m: usize,
    out: String,
    merge: bool,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            snapshots: None,
            comparison: None,
            policy: Policy::Mcts,
            judge_worlds: 8,
            judge_iters: 1000,
            judge_ms: 60_000,
            n: 200,
            seed: 1,
            shard_i: 0,
            shard_m: 1,
            out: "results".into(),
            merge: false,
        }
    }
}

#[derive(Clone)]
struct CostRow {
    game_id: String,
    turn: u32,
    our_pick: u8,
    a_cmp: u8,
    wr_ours: f64,
    wr_cmp: f64,
    cost: f64,
    ci_lo: f64,
    ci_hi: f64,
}

fn fnv1a(key: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// Deterministic stable index for a position; independent of M and filesystem enumeration order.
fn position_index(game_id: &str, turn: u32) -> u64 {
    fnv1a(&format!("{game_id}\t{turn}"))
}

fn repeat_seed(base: u64, r: usize) -> u64 {
    splitmix64(base ^ (r as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

fn parse_policy(s: &str) -> Policy {
    match s {
        "mcts" => Policy::Mcts,
        "greedy" => Policy::Greedy,
        other => {
            eprintln!("unknown --policy: {other} (expected mcts|greedy)");
            std::process::exit(2);
        }
    }
}

fn parse_shard(s: &str) -> (usize, usize) {
    let mut it = s.splitn(2, '/');
    let i: usize = it.next().and_then(|x| x.parse().ok()).unwrap_or_else(|| {
        eprintln!("--shard must be <i>/<M>");
        std::process::exit(2);
    });
    let m: usize = it.next().and_then(|x| x.parse().ok()).unwrap_or_else(|| {
        eprintln!("--shard must be <i>/<M>");
        std::process::exit(2);
    });
    if m == 0 || i >= m {
        eprintln!("--shard <i>/<M> requires M>0 and i<M");
        std::process::exit(2);
    }
    (i, m)
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    let need = |v: Option<String>, flag: &str| -> String {
        v.unwrap_or_else(|| {
            eprintln!("{flag} requires an argument");
            std::process::exit(2);
        })
    };
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--snapshots" => a.snapshots = Some(need(it.next(), "--snapshots")),
            "--comparison" => a.comparison = Some(need(it.next(), "--comparison")),
            "--policy" => a.policy = parse_policy(&need(it.next(), "--policy")),
            "--judge-worlds" => a.judge_worlds = need(it.next(), "--judge-worlds").parse().expect("--judge-worlds int"),
            "--judge-iters" => a.judge_iters = need(it.next(), "--judge-iters").parse().expect("--judge-iters int"),
            "--judge-ms" => a.judge_ms = need(it.next(), "--judge-ms").parse().expect("--judge-ms int"),
            "--n" => a.n = need(it.next(), "--n").parse().expect("--n int"),
            "--seed" => a.seed = need(it.next(), "--seed").parse().expect("--seed int"),
            "--shard" => {
                let (i, m) = parse_shard(&need(it.next(), "--shard"));
                a.shard_i = i;
                a.shard_m = m;
            }
            "--out" => a.out = need(it.next(), "--out"),
            "--merge" => a.merge = true,
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }
    if a.n == 0 {
        eprintln!("--n must be >= 1");
        std::process::exit(2);
    }
    a
}

fn read_comparison(path: &str) -> std::collections::HashMap<(String, u32), u8> {
    let data = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("read --comparison {path}: {e}");
        std::process::exit(1);
    });
    let mut map = std::collections::HashMap::new();
    let mut malformed = 0usize;
    for line in data.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut f = line.split('\t');
        let parsed = (|| {
            let game_id = f.next()?.to_string();
            let turn: u32 = f.next()?.parse().ok()?;
            let byte: u8 = f.next()?.parse().ok()?;
            Some((game_id, turn, byte))
        })();
        match parsed {
            Some((game_id, turn, byte)) => {
                map.insert((game_id, turn), byte);
            }
            None => malformed += 1,
        }
    }
    eprintln!("comparison: kept {} parsed {} malformed (dropped)", map.len(), malformed);
    map
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

// Seat-correct beliefs: index by our_side, never hardcode slot 0. beliefs[our_side] = our view of the
// opp (the snapshot's belief); beliefs[1-our_side] = the opp's reconstructed view of us.
fn build_beliefs(snap: &PositionSnapshot) -> [Belief; 2] {
    let mut beliefs = [Belief::default(), Belief::default()];
    beliefs[snap.our_side] = snap.belief.clone();
    beliefs[1 - snap.our_side] = reconstruct_opp_belief(&snap.state, snap.our_side);
    beliefs
}

fn score_position(snap: &PositionSnapshot, a_cmp: u8, policy: Policy, n: usize, seed: u64, budget: JudgeBudget) -> CostRow {
    let beliefs = build_beliefs(snap);
    let our_pick = snap.our_pick;
    let mut sum_ours = 0.0f64;
    let mut sum_cmp = 0.0f64;
    let mut diffs: Vec<f64> = Vec::with_capacity(n);
    for r in 0..n {
        let s = repeat_seed(seed, r);
        let w_ours = play_from_with_budget(&snap.state, &snap.teams, &beliefs, snap.our_side, our_pick, policy, s, budget);
        let w_cmp = play_from_with_budget(&snap.state, &snap.teams, &beliefs, snap.our_side, a_cmp, policy, s, budget);
        sum_ours += w_ours;
        sum_cmp += w_cmp;
        diffs.push(w_cmp - w_ours);
    }
    let nf = n.max(1) as f64;
    let wr_ours = sum_ours / nf;
    let wr_cmp = sum_cmp / nf;
    let cost = wr_cmp - wr_ours;
    let mean_d = diffs.iter().sum::<f64>() / nf;
    let (ci_lo, ci_hi) = if n >= 2 {
        let var = diffs.iter().map(|d| (d - mean_d) * (d - mean_d)).sum::<f64>() / (n as f64 - 1.0);
        let se = (var / nf).sqrt();
        (mean_d - 1.96 * se, mean_d + 1.96 * se)
    } else {
        (cost, cost)
    };
    CostRow {
        game_id: snap.game_id.clone(),
        turn: snap.turn,
        our_pick,
        a_cmp,
        wr_ours,
        wr_cmp,
        cost,
        ci_lo,
        ci_hi,
    }
}

const HEADER: &str = "game_id\tturn\tour_pick\ta_cmp\twr_ours\twr_cmp\tcost\tci_lo\tci_hi";

fn format_row(r: &CostRow) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.6}",
        r.game_id, r.turn, r.our_pick, r.a_cmp, r.wr_ours, r.wr_cmp, r.cost, r.ci_lo, r.ci_hi
    )
}

fn write_shard(out: &str, shard_i: usize, rows: &[CostRow]) -> std::io::Result<()> {
    std::fs::create_dir_all(out)?;
    let mut body = String::from(HEADER);
    body.push('\n');
    for r in rows {
        body.push_str(&format_row(r));
        body.push('\n');
    }
    std::fs::write(format!("{out}/costs.shard-{shard_i}.tsv"), body)
}

// Concat all shard files, drop their header lines, sort on (game_id asc, turn asc), write costs.tsv.
// Byte-identical regardless of how many shards produced the rows.
fn merge_shards(out: &str) -> std::io::Result<()> {
    let mut data_lines: Vec<String> = Vec::new();
    let mut entries: Vec<String> = std::fs::read_dir(out)?
        .filter_map(|e| e.ok())
        .map(|e| e.path().to_string_lossy().into_owned())
        .filter(|p| {
            let base = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            base.starts_with("costs.shard-") && base.ends_with(".tsv")
        })
        .collect();
    entries.sort();
    for path in &entries {
        let text = std::fs::read_to_string(path)?;
        for line in text.lines() {
            if line == HEADER || line.is_empty() {
                continue;
            }
            data_lines.push(line.to_string());
        }
    }
    data_lines.sort_by(|a, b| {
        let (ga, ta) = split_key(a);
        let (gb, tb) = split_key(b);
        ga.cmp(gb).then(ta.cmp(&tb))
    });
    let mut body = String::from(HEADER);
    body.push('\n');
    for line in &data_lines {
        body.push_str(line);
        body.push('\n');
    }
    std::fs::write(format!("{out}/costs.tsv"), body)
}

fn split_key(line: &str) -> (&str, u32) {
    let mut f = line.split('\t');
    let g = f.next().unwrap_or("");
    let t = f.next().and_then(|x| x.parse().ok()).unwrap_or(0u32);
    (g, t)
}

fn run(a: &Args) {
    let snapshots = a.snapshots.clone().unwrap_or_else(|| {
        eprintln!("--snapshots is required");
        std::process::exit(2);
    });
    let comparison = a.comparison.clone().unwrap_or_else(|| {
        eprintln!("--comparison is required");
        std::process::exit(2);
    });
    let cmp = read_comparison(&comparison);
    let paths = snapshot_paths(&snapshots).unwrap_or_else(|e| {
        eprintln!("read_dir {snapshots}: {e}");
        std::process::exit(1);
    });
    // judge-iters drives the per-world cap; judge-ms is only a non-tripping ceiling, never u64::MAX.
    let budget = JudgeBudget {
        num_worlds: a.judge_worlds,
        max_iters_per_world: a.judge_iters,
        time_ms_per_world: a.judge_ms,
    };

    let mut rows: Vec<CostRow> = Vec::new();
    for path in &paths {
        let snap = match PositionSnapshot::read(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("read {path}: {e}");
                continue;
            }
        };
        let a_cmp = match cmp.get(&(snap.game_id.clone(), snap.turn)) {
            Some(b) => *b,
            None => continue,
        };
        if a_cmp == snap.our_pick {
            continue; // non-divergent: cost is identically zero, skip.
        }
        let idx = position_index(&snap.game_id, snap.turn);
        if (idx % a.shard_m as u64) != a.shard_i as u64 {
            continue;
        }
        rows.push(score_position(&snap, a_cmp, a.policy, a.n, a.seed, budget));
    }
    write_shard(&a.out, a.shard_i, &rows).unwrap_or_else(|e| {
        eprintln!("write shard: {e}");
        std::process::exit(1);
    });
}

fn main() {
    let a = parse_args();
    if a.merge {
        merge_shards(&a.out).unwrap_or_else(|e| {
            eprintln!("merge: {e}");
            std::process::exit(1);
        });
        return;
    }
    run(&a);
}

#[cfg(test)]
mod tests {
    use super::*;
    use poke_mcts::testutil::{build_state, mon};

    fn iter_capped_budget(iters: u64) -> JudgeBudget {
        // A FINITE iteration cap (not the wall clock) fixes the per-world iteration count, so two runs
        // of the same position+seed are bit-identical; the huge time_ms is a non-tripping ceiling.
        JudgeBudget { num_worlds: 8, max_iters_per_world: iters, time_ms_per_world: 60_000 }
    }

    // Fast 1-HP Garchomp vs slow 100-HP Snorlax. our_pick=Swords Dance loses turn 1 (opp survives and
    // KOs our only mon); a_cmp=Earthquake one-shots the opp's only mon first and wins. Both terminal t1.
    fn forced_ko_vs_status() -> PositionSnapshot {
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0])],
            vec![mon(143, 47, [34, 0, 0, 0])],
        );
        state.sides[0].team[0].current_hp = 1;
        state.sides[1].team[0].current_hp = 100;
        let opp_active = state.active_mon(1);
        let mut our_belief = Belief::default();
        our_belief.note_species(opp_active.species_id, opp_active.level);
        PositionSnapshot {
            game_id: "ko-vs-status".into(),
            turn: 1,
            our_side: 0,
            our_pick: 1,
            live_budget: (8, 200, 1000),
            request_kind: "move".into(),
            state,
            teams,
            belief: our_belief,
            tags: vec![],
        }
    }

    fn divergent_snapshot(id: &str, our_pick: u8) -> PositionSnapshot {
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0])],
            vec![mon(143, 47, [34, 0, 0, 0])],
        );
        state.sides[1].team[0].current_hp = 1;
        let opp_active = state.active_mon(1);
        let mut our_belief = Belief::default();
        our_belief.note_species(opp_active.species_id, opp_active.level);
        PositionSnapshot {
            game_id: id.into(),
            turn: 1,
            our_side: 0,
            our_pick,
            live_budget: (8, 200, 1000),
            request_kind: "move".into(),
            state,
            teams,
            belief: our_belief,
            tags: vec![],
        }
    }

    #[test]
    fn cost_is_bit_identical_under_iteration_cap() {
        let snap = forced_ko_vs_status();
        let budget = iter_capped_budget(200);
        // The proof: assert the budget is genuinely iteration-capped and time-ceiling is non-tripping.
        assert_ne!(budget.max_iters_per_world, u64::MAX, "budget must be iteration-capped");
        assert!(budget.time_ms_per_world >= 60_000, "time ceiling must be large and non-tripping");
        let r1 = score_position(&snap, 0, Policy::Mcts, 4, 1, budget);
        let r2 = score_position(&snap, 0, Policy::Mcts, 4, 1, budget);
        assert_eq!(r1.cost.to_bits(), r2.cost.to_bits(), "cost must be bit-identical under a finite iteration cap");
        assert_eq!(r1.wr_ours.to_bits(), r2.wr_ours.to_bits());
        assert_eq!(r1.wr_cmp.to_bits(), r2.wr_cmp.to_bits());
    }

    #[test]
    fn forced_ko_vs_status_is_large_positive_cost() {
        let snap = forced_ko_vs_status();
        let budget = iter_capped_budget(200);
        // a_cmp = 0 (Earthquake, KO arm wins ~1.0); our_pick = 1 (Swords Dance, loses ~0.0).
        let r = score_position(&snap, 0, Policy::Mcts, 8, 1, budget);
        assert!(r.wr_cmp > 0.9, "KO arm should win, got wr_cmp={}", r.wr_cmp);
        assert!(r.wr_ours < 0.1, "status arm should lose, got wr_ours={}", r.wr_ours);
        assert!(r.cost > 0.8, "cost must be large positive, got {}", r.cost);
    }

    #[test]
    fn merge_is_byte_identical_across_shard_counts() {
        let budget = iter_capped_budget(150);
        let snaps = vec![
            divergent_snapshot("g-aaa", 1),
            divergent_snapshot("g-bbb", 1),
            divergent_snapshot("g-ccc", 1),
            divergent_snapshot("g-ddd", 1),
        ];
        // a_cmp = 0 (Earthquake) differs from our_pick = 1 (Swords Dance) for all of them.
        let a_cmp_for = |_id: &str| 0u8;
        let n = 4;
        let seed = 1u64;

        let base = std::env::temp_dir().join(format!("pj_shard_{}", std::process::id()));
        let dir_m1 = base.join("m1").to_string_lossy().into_owned();
        let dir_m2 = base.join("m2").to_string_lossy().into_owned();

        // M = 1: one shard takes everything.
        {
            let mut rows = Vec::new();
            for s in &snaps {
                let idx = position_index(&s.game_id, s.turn);
                if idx % 1 == 0 {
                    rows.push(score_position(s, a_cmp_for(&s.game_id), Policy::Mcts, n, seed, budget));
                }
            }
            write_shard(&dir_m1, 0, &rows).unwrap();
            merge_shards(&dir_m1).unwrap();
        }

        // M = 2: two shards partition by position_index % 2.
        {
            for shard_i in 0..2usize {
                let mut rows = Vec::new();
                for s in &snaps {
                    let idx = position_index(&s.game_id, s.turn);
                    if (idx % 2) as usize == shard_i {
                        rows.push(score_position(s, a_cmp_for(&s.game_id), Policy::Mcts, n, seed, budget));
                    }
                }
                write_shard(&dir_m2, shard_i, &rows).unwrap();
            }
            merge_shards(&dir_m2).unwrap();
        }

        let merged_m1 = std::fs::read(format!("{dir_m1}/costs.tsv")).unwrap();
        let merged_m2 = std::fs::read(format!("{dir_m2}/costs.tsv")).unwrap();
        assert_eq!(merged_m1, merged_m2, "merged costs.tsv must be byte-identical for M=1 vs M=2");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn beliefs_are_seat_correct_for_both_sides() {
        // our_side = 1: snapshot.belief must land in slot 1, reconstructed in slot 0.
        let (mut state, teams) = build_state(
            vec![mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 14, 0, 0])],
        );
        state.sides[0].team[0].current_hp = 1;
        let opp_active = state.active_mon(0);
        let mut our_belief = Belief::default();
        our_belief.note_species(opp_active.species_id, opp_active.level);
        let snap = PositionSnapshot {
            game_id: "seat-s1".into(),
            turn: 1,
            our_side: 1,
            our_pick: 0,
            live_budget: (8, 200, 1000),
            request_kind: "move".into(),
            state,
            teams,
            belief: our_belief.clone(),
            tags: vec![],
        };
        let beliefs = build_beliefs(&snap);
        // slot our_side (1) == our view of the opp; slot 1-our_side (0) == reconstructed view of us.
        assert_eq!(beliefs[1].mons, our_belief.mons, "snapshot belief must occupy slot our_side");
        let recon = reconstruct_opp_belief(&snap.state, snap.our_side);
        assert_eq!(beliefs[0].mons, recon.mons, "reconstructed belief must occupy slot 1-our_side");
    }
}
