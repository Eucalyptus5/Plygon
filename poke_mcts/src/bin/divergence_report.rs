use pkmn_engine::state::calc::calc_damage;
use pkmn_engine::state::*;
use poke_mcts::audit_snapshot::PositionSnapshot;

#[allow(dead_code)]
pub struct TaggedDivergence {
    pub game_id: String,
    pub turn: u32,
    pub cost: f64,
    pub ci_lo: f64,
    pub decision_type: String,
    pub phase: String,
    pub mechanic: String,
    pub feature: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ActionCat {
    Move,
    Switch,
    Tera,
}

fn cat(byte: u8) -> ActionCat {
    match byte {
        ACTION_SWITCH_0..=ACTION_SWITCH_5 => ActionCat::Switch,
        ACTION_TERA_0..=ACTION_TERA_3 => ActionCat::Tera,
        _ => ActionCat::Move,
    }
}

fn cat_label(c: ActionCat) -> &'static str {
    match c {
        ActionCat::Move => "move",
        ActionCat::Switch => "switch",
        ActionCat::Tera => "tera",
    }
}

fn decision_type(our_pick: u8, a_cmp: u8, request_kind: &str) -> String {
    let co = cat(our_pick);
    let cc = cat(a_cmp);
    if co == ActionCat::Tera || cc == ActionCat::Tera {
        return "tera-flip".into();
    }
    // A force_switch request can only be answered with a switch; a raw move byte there is degenerate.
    let origin = if request_kind == "force_switch" { ActionCat::Switch } else { co };
    format!("{}->{}", cat_label(origin), cat_label(cc))
}

fn alive_count(state: &BattleState, side: usize) -> usize {
    (0..6)
        .filter(|&i| state.sides[side].team[i].species_id != 0 && state.sides[side].team[i].current_hp > 0)
        .count()
}

fn revealed_slots(state: &BattleState, side: usize) -> usize {
    (0..6).filter(|&i| state.sides[side].team[i].species_id != 0).count()
}

fn phase(turn: u32, revealed_total: usize) -> String {
    if turn <= 6 {
        return "early".into();
    }
    if turn >= 16 || revealed_total >= 9 {
        return "late".into();
    }
    "mid".into()
}

fn any_screen(sc: &SideConditions) -> bool {
    sc.reflect_turns > 0 || sc.light_screen_turns > 0 || sc.aurora_veil_turns > 0
}

fn any_hazard(sc: &SideConditions) -> bool {
    sc.spikes != 0 || sc.toxic_spikes != 0 || sc.hazard_flags != 0
}

fn mechanic(state: &BattleState) -> String {
    if state.field.trick_room_turns > 0 {
        return "trick-room".into();
    }
    if state.field.weather != WEATHER_NONE {
        return "weather".into();
    }
    if state.field.terrain != TERRAIN_NONE {
        return "terrain".into();
    }
    if any_screen(&state.sides[0].side_conditions) || any_screen(&state.sides[1].side_conditions) {
        return "screen".into();
    }
    if any_hazard(&state.sides[0].side_conditions) || any_hazard(&state.sides[1].side_conditions) {
        return "hazard".into();
    }
    if state.sides[0].active.choice_locked_move != 0 || state.sides[1].active.choice_locked_move != 0 {
        return "choice-lock".into();
    }
    "none".into()
}

fn ko_range_present(state: &BattleState, our_side: usize) -> bool {
    let opp = state.active_mon(1 - our_side);
    let opp_hp = opp.current_hp;
    if opp_hp == 0 {
        return false;
    }
    let moves = effective_moves(state, our_side);
    let mut max_roll = |max: u32| max.saturating_sub(1);
    for &mid in moves.iter() {
        if mid == 0 {
            continue;
        }
        let dmg = calc_damage(state, our_side, mid, 100, &mut max_roll).damage;
        if dmg >= opp_hp {
            return true;
        }
    }
    false
}

fn hp_frac(m: &MonSlot) -> f64 {
    if m.max_hp == 0 {
        return 0.0;
    }
    m.current_hp as f64 / m.max_hp as f64
}

// Diagnostic buckets: high>0.66, mid>0.33, low>0.10, else critical.
fn hp_band(frac: f64) -> &'static str {
    if frac > 0.66 {
        "high"
    } else if frac > 0.33 {
        "mid"
    } else if frac > 0.10 {
        "low"
    } else {
        "critical"
    }
}

fn band_rank(frac: f64) -> u8 {
    match hp_band(frac) {
        "critical" => 0,
        "low" => 1,
        "mid" => 2,
        _ => 3,
    }
}

// Diagnostic posture buckets: lead by material -> ahead/behind; on a tie, active-HP band rank breaks it.
fn who_leads(state: &BattleState, our_side: usize) -> &'static str {
    let opp_side = 1 - our_side;
    let our_mat = alive_count(state, our_side);
    let opp_mat = alive_count(state, opp_side);
    if our_mat > opp_mat {
        return "ahead";
    }
    if our_mat < opp_mat {
        return "behind";
    }
    let our_rank = band_rank(hp_frac(state.active_mon(our_side)));
    let opp_rank = band_rank(hp_frac(state.active_mon(opp_side)));
    if our_rank > opp_rank {
        "ahead"
    } else if our_rank < opp_rank {
        "behind"
    } else {
        "even"
    }
}

fn feature(state: &BattleState, our_side: usize) -> String {
    let posture = who_leads(state, our_side);
    let ko = if ko_range_present(state, our_side) { "ko" } else { "noko" };
    format!("{}-{}", posture, ko)
}

fn tag(snap: &PositionSnapshot, a_cmp: u8, cost: f64, ci_lo: f64) -> TaggedDivergence {
    let revealed_total = revealed_slots(&snap.state, 0) + revealed_slots(&snap.state, 1);
    TaggedDivergence {
        game_id: snap.game_id.clone(),
        turn: snap.turn,
        cost,
        ci_lo,
        decision_type: decision_type(snap.our_pick, a_cmp, snap.request_kind.as_str()),
        phase: phase(snap.turn, revealed_total),
        mechanic: mechanic(&snap.state),
        feature: feature(&snap.state, snap.our_side),
    }
}

struct CostRow {
    game_id: String,
    turn: u32,
    a_cmp: u8,
    cost: f64,
    ci_lo: f64,
}

const COSTS_HEADER: &str = "game_id\tturn\tour_pick\ta_cmp\twr_ours\twr_cmp\tcost\tci_lo\tci_hi";

fn parse_cost_row(line: &str) -> Option<CostRow> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() != 9 {
        return None;
    }
    Some(CostRow {
        game_id: f[0].to_string(),
        turn: f[1].parse().ok()?,
        a_cmp: f[3].parse().ok()?,
        cost: f[6].parse().ok()?,
        ci_lo: f[7].parse().ok()?,
    })
}

fn load_and_tag(snapshots_dir: &str, costs_path: &str) -> Vec<TaggedDivergence> {
    let raw = match std::fs::read_to_string(costs_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not read costs file {}: {}", costs_path, e);
            return vec![];
        }
    };
    let mut out = Vec::new();
    let mut dropped = 0usize;
    for line in raw.lines() {
        if line.is_empty() || line == COSTS_HEADER {
            continue;
        }
        let row = match parse_cost_row(line) {
            Some(r) => r,
            None => {
                dropped += 1;
                continue;
            }
        };
        let path = format!("{}/{}-t{}.json", snapshots_dir, row.game_id, row.turn);
        match PositionSnapshot::read(&path) {
            Ok(snap) => out.push(tag(&snap, row.a_cmp, row.cost, row.ci_lo)),
            Err(_) => dropped += 1,
        }
    }
    if dropped > 0 {
        eprintln!("dropped {} cost rows (unreadable snapshot or malformed line)", dropped);
    }
    out
}

use poke_mcts::rng::Lcg;

const COST_KEEP_MIN: f64 = 0.03;
const FLOOR_N: usize = 100;
const TOP_K: usize = 3;
const MIN_ELIGIBLE_BUCKETS: usize = 3;

const DIM_NAMES: [&str; 4] = ["decision-type", "phase", "mechanic", "feature"];

struct AnalyzeConfig {
    alpha: f64,
    perms: usize,
    seed: u64,
    floor_trials: usize,
    floor_perms: usize,
    floor_grid: Vec<f64>,
}

impl Default for AnalyzeConfig {
    fn default() -> Self {
        AnalyzeConfig {
            alpha: 0.05,
            perms: 2000,
            seed: 0x5EED_1234_ABCD_0001,
            floor_trials: 100,
            floor_perms: 300,
            floor_grid: vec![0.10, 0.15, 0.20, 0.25, 0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60],
        }
    }
}

struct DimResult {
    name: String,
    k: usize,
    eligible: bool,
    observed_top3: f64,
    crit_bonferroni: f64,
    crit_uncorrected: f64,
    fires: bool,
    buckets: Vec<(String, f64)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    Cluster,
    WeakCluster,
    Diffuse,
    Underpowered,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Cluster => "CLUSTER",
            Verdict::WeakCluster => "WEAK-CLUSTER",
            Verdict::Diffuse => "DIFFUSE",
            Verdict::Underpowered => "UNDERPOWERED",
        }
    }
}

enum FloorReport {
    Detected(f64),
    AboveGrid(f64),
    NoEligible,
}

impl FloorReport {
    fn describe(&self) -> String {
        match self {
            FloorReport::Detected(s) => format!("{:.0}%", s * 100.0),
            FloorReport::AboveGrid(g) => format!("> {:.0}%", g * 100.0),
            FloorReport::NoEligible => "n/a (no eligible dimension)".to_string(),
        }
    }
}

struct AnalysisResult {
    n_scored: usize,
    total_positive_cost: f64,
    dims: Vec<DimResult>,
    m: usize,
    most_concentrated: Option<String>,
    verdict: Verdict,
    detectable_share_floor: FloorReport,
}

fn dim_label(d: &TaggedDivergence, dim: usize) -> &str {
    match dim {
        0 => &d.decision_type,
        1 => &d.phase,
        2 => &d.mechanic,
        _ => &d.feature,
    }
}

// Sum costs per bucket (given item->bucket-index and item costs), return the top-K share.
fn top_share(bucket_of: &[usize], costs: &[f64], n_buckets: usize, total: f64) -> f64 {
    if total <= 0.0 {
        return 0.0;
    }
    let mut sums = vec![0.0f64; n_buckets];
    for (i, &b) in bucket_of.iter().enumerate() {
        sums[b] += costs[i];
    }
    sums.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let take = TOP_K.min(sums.len());
    sums[..take].iter().sum::<f64>() / total
}

fn fisher_yates(v: &mut [f64], lcg: &mut Lcg) {
    let n = v.len();
    for i in (1..n).rev() {
        let j = lcg.roll((i + 1) as u32) as usize;
        v.swap(i, j);
    }
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = (q * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// Null: hold bucket labels fixed, shuffle costs across items, recompute top-K share.
fn null_distribution(
    bucket_of: &[usize],
    costs: &[f64],
    n_buckets: usize,
    total: f64,
    perms: usize,
    lcg: &mut Lcg,
) -> Vec<f64> {
    let mut scratch = costs.to_vec();
    let mut out = Vec::with_capacity(perms);
    for _ in 0..perms {
        fisher_yates(&mut scratch, lcg);
        out.push(top_share(bucket_of, &scratch, n_buckets, total));
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

// Bucketize one dimension over the filtered set: labels in first-seen order.
fn bucketize<'a>(divs: &'a [TaggedDivergence], dim: usize) -> (Vec<usize>, usize) {
    let mut labels: Vec<&'a str> = Vec::new();
    let mut idx = Vec::with_capacity(divs.len());
    for d in divs {
        let l = dim_label(d, dim);
        let b = match labels.iter().position(|&x| x == l) {
            Some(p) => p,
            None => {
                labels.push(l);
                labels.len() - 1
            }
        };
        idx.push(b);
    }
    (idx, labels.len())
}

// Group the filtered set by one dimension's label, summing cost per label; sort descending.
fn ranked_buckets(divs: &[TaggedDivergence], dim: usize) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for d in divs {
        let l = dim_label(d, dim);
        match out.iter_mut().find(|(label, _)| label == l) {
            Some(entry) => entry.1 += d.cost,
            None => out.push((l.to_string(), d.cost)),
        }
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn analyze(divs: &[TaggedDivergence], cfg: &AnalyzeConfig) -> AnalysisResult {
    let kept: Vec<&TaggedDivergence> = divs
        .iter()
        .filter(|d| d.cost > COST_KEEP_MIN && d.ci_lo > 0.0)
        .collect();
    let n_scored = kept.len();
    let total_positive_cost: f64 = kept.iter().map(|d| d.cost).sum();
    let costs: Vec<f64> = kept.iter().map(|d| d.cost).collect();
    let owned: Vec<TaggedDivergence> = kept
        .iter()
        .map(|d| TaggedDivergence {
            game_id: d.game_id.clone(),
            turn: d.turn,
            cost: d.cost,
            ci_lo: d.ci_lo,
            decision_type: d.decision_type.clone(),
            phase: d.phase.clone(),
            mechanic: d.mechanic.clone(),
            feature: d.feature.clone(),
        })
        .collect();

    // Bucket counts per dimension; m = eligible-after-drop count.
    let mut ks = [0usize; 4];
    for dim in 0..4 {
        ks[dim] = bucketize(&owned, dim).1;
    }
    let m = ks.iter().filter(|&&k| k > MIN_ELIGIBLE_BUCKETS).count();

    // Floor gate: BEFORE any null read.
    let underpowered = n_scored < FLOOR_N;

    let mut dims = Vec::with_capacity(4);
    let alpha = cfg.alpha;
    let mut lcg = Lcg::new(cfg.seed);
    for dim in 0..4 {
        let (bucket_of, k) = bucketize(&owned, dim);
        let eligible = k > MIN_ELIGIBLE_BUCKETS;
        let (observed, crit_bonf, crit_unc, fires) = if eligible && !underpowered && m > 0 {
            let observed = top_share(&bucket_of, &costs, k, total_positive_cost);
            let null = null_distribution(
                &bucket_of,
                &costs,
                k,
                total_positive_cost,
                cfg.perms,
                &mut lcg,
            );
            let crit_bonf = quantile(&null, 1.0 - alpha / m as f64);
            let crit_unc = quantile(&null, 1.0 - alpha);
            let fires = observed > crit_bonf;
            (observed, crit_bonf, crit_unc, fires)
        } else if eligible {
            // Eligible but under the floor: still report observed, no null read.
            let observed = top_share(&bucket_of, &costs, k, total_positive_cost);
            (observed, 0.0, 0.0, false)
        } else {
            (0.0, 0.0, 0.0, false)
        };
        dims.push(DimResult {
            name: DIM_NAMES[dim].to_string(),
            k,
            eligible,
            observed_top3: observed,
            crit_bonferroni: crit_bonf,
            crit_uncorrected: crit_unc,
            fires,
            buckets: ranked_buckets(&owned, dim),
        });
    }

    // Most-concentrated eligible dim (by observed top-3 share).
    let most_concentrated = dims
        .iter()
        .filter(|d| d.eligible)
        .max_by(|a, b| {
            a.observed_top3
                .partial_cmp(&b.observed_top3)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|d| d.name.clone());

    let verdict = if underpowered {
        Verdict::Underpowered
    } else if dims.iter().any(|d| d.eligible && d.fires) {
        Verdict::Cluster
    } else {
        // Weak-cluster: most-concentrated eligible clears the uncorrected but not Bonferroni.
        let weak = dims
            .iter()
            .filter(|d| d.eligible)
            .any(|d| d.observed_top3 > d.crit_uncorrected && !d.fires);
        if weak {
            Verdict::WeakCluster
        } else {
            Verdict::Diffuse
        }
    };

    let detectable_share_floor = detectable_floor(&owned, &dims, m, cfg);

    AnalysisResult {
        n_scored,
        total_positive_cost,
        dims,
        m,
        most_concentrated,
        verdict,
        detectable_share_floor,
    }
}

// Smallest planted top-3 share the null detects at this N, for the most-concentrated eligible dim.
fn detectable_floor(
    divs: &[TaggedDivergence],
    dims: &[DimResult],
    m: usize,
    cfg: &AnalyzeConfig,
) -> FloorReport {
    let target = dims
        .iter()
        .filter(|d| d.eligible)
        .max_by(|a, b| {
            a.observed_top3
                .partial_cmp(&b.observed_top3)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let target = match target {
        Some(d) if m > 0 => d,
        _ => return FloorReport::NoEligible,
    };
    let n = divs.len();
    let k = target.k;
    if n == 0 || k <= MIN_ELIGIBLE_BUCKETS {
        return FloorReport::NoEligible;
    }
    let alpha = cfg.alpha;
    let mut lcg = Lcg::new(cfg.seed ^ 0xF100_0000_0000_0001);
    let grid = &cfg.floor_grid;
    let last_grid = grid.last().copied().unwrap_or(0.60);

    for &s in grid {
        let mut fired = 0usize;
        for _ in 0..cfg.floor_trials {
            // Synthesize N costs: one bucket carries fraction s of total, rest ~uniform.
            let total = 1.0f64;
            let heavy = total * s;
            let rest = total - heavy;
            let per_rest = if k > 1 { rest / (k - 1) as f64 } else { 0.0 };
            // Assign items to buckets in ~equal sizes.
            let mut bucket_of = Vec::with_capacity(n);
            for i in 0..n {
                bucket_of.push(i % k);
            }
            // Per-bucket target mass -> per-item cost = bucket mass / bucket size.
            let mut sizes = vec![0usize; k];
            for &b in &bucket_of {
                sizes[b] += 1;
            }
            let mut costs = Vec::with_capacity(n);
            for &b in &bucket_of {
                let mass = if b == 0 { heavy } else { per_rest };
                let sz = sizes[b].max(1);
                costs.push(mass / sz as f64);
            }
            let tot: f64 = costs.iter().sum();
            let observed = top_share(&bucket_of, &costs, k, tot);
            let null = null_distribution(&bucket_of, &costs, k, tot, cfg.floor_perms, &mut lcg);
            let crit = quantile(&null, 1.0 - alpha / m as f64);
            if observed > crit {
                fired += 1;
            }
        }
        let rate = fired as f64 / cfg.floor_trials.max(1) as f64;
        if rate >= 0.8 {
            return FloorReport::Detected(s);
        }
    }
    FloorReport::AboveGrid(last_grid)
}

fn pearson(xs: &[f64], ys: &[f64]) -> Option<f64> {
    let n = xs.len();
    if n < 2 || ys.len() != n {
        return None;
    }
    let mx: f64 = xs.iter().sum::<f64>() / n as f64;
    let my: f64 = ys.iter().sum::<f64>() / n as f64;
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for i in 0..n {
        let dx = xs[i] - mx;
        let dy = ys[i] - my;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    let denom = (sxx * syy).sqrt();
    if denom <= 0.0 {
        return None;
    }
    Some(sxy / denom)
}

fn greedy_correlation(divs: &[TaggedDivergence], _cfg: &AnalyzeConfig, path: &str) -> Option<f64> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut greedy: std::collections::HashMap<(String, u32), f64> = std::collections::HashMap::new();
    for line in raw.lines() {
        if line.is_empty() || line == COSTS_HEADER {
            continue;
        }
        if let Some(r) = parse_cost_row(line) {
            greedy.insert((r.game_id, r.turn), r.cost);
        }
    }
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for d in divs {
        if d.cost > COST_KEEP_MIN && d.ci_lo > 0.0 {
            if let Some(&g) = greedy.get(&(d.game_id.clone(), d.turn)) {
                xs.push(d.cost);
                ys.push(g);
            }
        }
    }
    pearson(&xs, &ys)
}

// Attribute a bucket to the likely lever: tera slots our search collapses -> action-space;
// switch-involving decisions could be either; both-representable buckets -> eval.
fn attribute(dim_name: &str, bucket_label: &str) -> &'static str {
    if dim_name == "decision-type" {
        if bucket_label == "tera-flip" {
            return "action-space (a tera slot our search collapses)";
        }
        if bucket_label.starts_with("switch->") || bucket_label.ends_with("->switch") {
            return "action-space-or-eval (either possible)";
        }
    }
    "eval (both moves representable)"
}

fn write_report(
    res: &AnalysisResult,
    greedy_corr: Option<f64>,
    out_dir: &str,
) -> std::io::Result<()> {
    use std::fmt::Write as _;
    std::fs::create_dir_all(out_dir)?;
    let mut s = String::new();

    writeln!(s, "# Move-divergence concentration report").ok();
    writeln!(s).ok();
    writeln!(s, "N_scored: {}", res.n_scored).ok();
    writeln!(
        s,
        "total_positive_cost: {:.4}",
        res.total_positive_cost
    )
    .ok();
    writeln!(s, "eligible dimensions (m, Bonferroni divisor): {}", res.m).ok();
    writeln!(s).ok();

    writeln!(
        s,
        "| dimension | #buckets k | eligible? | observed top-3 share | null Bonferroni critical | fires? |"
    )
    .ok();
    writeln!(s, "|---|---|---|---|---|---|").ok();
    for d in &res.dims {
        if d.eligible {
            writeln!(
                s,
                "| {} | {} | yes | {:.4} | {:.4} | {} |",
                d.name,
                d.k,
                d.observed_top3,
                d.crit_bonferroni,
                if d.fires { "yes" } else { "no" }
            )
            .ok();
        } else {
            writeln!(
                s,
                "| {} | {} | dropped (k<=3) | - | - | - |",
                d.name, d.k
            )
            .ok();
        }
    }
    writeln!(s).ok();

    let most = res
        .most_concentrated
        .clone()
        .unwrap_or_else(|| "none".to_string());
    writeln!(s, "Most-concentrated eligible dimension: {}", most).ok();
    writeln!(s).ok();

    writeln!(
        s,
        "VERDICT: {}  (N_scored={}, detectable-share floor={})",
        res.verdict.label(),
        res.n_scored,
        res.detectable_share_floor.describe()
    )
    .ok();
    writeln!(s).ok();

    match greedy_corr {
        Some(c) => {
            writeln!(s, "Mcts-vs-Greedy cost correlation (Pearson): {:.4}", c).ok();
            writeln!(
                s,
                "A low correlation means the judge is fidelity-blind: any bucket may in fact be an engine-fidelity gap."
            )
            .ok();
        }
        None => {
            writeln!(s, "Mcts-vs-Greedy cost correlation: not available").ok();
        }
    }
    writeln!(s).ok();

    if res.verdict == Verdict::Cluster {
        writeln!(s, "## Fix list (ranked top buckets of firing dimensions)").ok();
        writeln!(s).ok();
        let total = res.total_positive_cost;
        for dim in res.dims.iter().filter(|d| d.eligible && d.fires) {
            writeln!(s, "### Dimension `{}`", dim.name).ok();
            writeln!(s).ok();
            writeln!(s, "| rank | bucket | summed cost | share | attribution |").ok();
            writeln!(s, "|---|---|---|---|---|").ok();
            let take = dim.buckets.len().min(TOP_K.max(3));
            for (rank, (label, sum)) in dim.buckets.iter().take(take).enumerate() {
                let share = if total > 0.0 { sum / total } else { 0.0 };
                writeln!(
                    s,
                    "| {} | {} | {:.4} | {:.4} | {} |",
                    rank + 1,
                    label,
                    sum,
                    share,
                    attribute(&dim.name, label)
                )
                .ok();
            }
            writeln!(s).ok();
        }
        writeln!(
            s,
            "Caveat: a low Mcts-vs-Greedy correlation means the judge is fidelity-blind, so any bucket may in fact be an engine-fidelity gap."
        )
        .ok();
        writeln!(s).ok();
    }

    writeln!(s, "## Verdict-trust rule").ok();
    match res.verdict {
        Verdict::Cluster | Verdict::WeakCluster => {
            writeln!(
                s,
                "A CLUSTER result is conservative: the judge is our own MCTS + engine and the bias works against firing, so a fired cluster is trustworthy."
            )
            .ok();
        }
        Verdict::Diffuse | Verdict::Underpowered => {
            writeln!(
                s,
                "The cost judge IS our own MCTS + engine (the suspected-blind component). This read is unresolved — cannot exclude a cluster the judge is itself blind to. It does NOT prove the absence of a cluster, does NOT confirm a ceiling, and does NOT green-light learned-eval or accepting ~32%."
            )
            .ok();
        }
    }
    writeln!(s).ok();

    writeln!(s, "## Recommendation").ok();
    match res.verdict {
        Verdict::Cluster => {
            writeln!(
                s,
                "Act on the ranked fix list above (cheaper than a learned eval): address the firing dimension's top buckets in cost order."
            )
            .ok();
        }
        Verdict::Diffuse => {
            writeln!(
                s,
                "Unresolved; cannot exclude a cluster the judge is blind to. This does NOT green-light learned-eval or accepting ~32%. Run the fidelity-independent reference oracle (2A) before any ceiling decision."
            )
            .ok();
        }
        Verdict::Underpowered => {
            writeln!(
                s,
                "No verdict: N_scored is below the floor. Re-run with a corpus sized up (size from the Wilson lower bound of the pilot rate)."
            )
            .ok();
        }
        Verdict::WeakCluster => {
            writeln!(
                s,
                "Run a second corpus pass, sized up, to resolve the weak signal."
            )
            .ok();
        }
    }

    std::fs::write(format!("{}/report.md", out_dir), s)
}

fn main() {
    let mut snapshots: Option<String> = None;
    let mut costs: Option<String> = None;
    let mut greedy_costs: Option<String> = None;
    let mut out: Option<String> = None;
    let mut cfg = AnalyzeConfig::default();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--snapshots" => snapshots = args.next(),
            "--costs" => costs = args.next(),
            "--greedy-costs" => greedy_costs = args.next(),
            "--out" => out = args.next(),
            "--alpha" => {
                cfg.alpha = args.next().and_then(|s| s.parse().ok()).unwrap_or(cfg.alpha)
            }
            "--perms" => {
                cfg.perms = args.next().and_then(|s| s.parse().ok()).unwrap_or(cfg.perms)
            }
            "--seed" => cfg.seed = args.next().and_then(|s| s.parse().ok()).unwrap_or(cfg.seed),
            other => {
                eprintln!("unknown argument: {}", other);
                std::process::exit(2);
            }
        }
    }

    let snapshots = snapshots.unwrap_or_else(|| {
        eprintln!("--snapshots <dir> is required");
        std::process::exit(2);
    });
    let costs = costs.unwrap_or_else(|| {
        eprintln!("--costs <tsv> is required");
        std::process::exit(2);
    });
    let out = out.unwrap_or_else(|| ".decompose/move-divergence-audit/results".to_string());

    let tagged = load_and_tag(&snapshots, &costs);
    let res = analyze(&tagged, &cfg);

    let greedy_corr = greedy_costs
        .as_deref()
        .and_then(|p| greedy_correlation(&tagged, &cfg, p));

    if let Err(e) = write_report(&res, greedy_corr, &out) {
        eprintln!("could not write report: {}", e);
        std::process::exit(1);
    }
    eprintln!(
        "{} (N_scored={}, detectable-share floor={})",
        res.verdict.label(),
        res.n_scored,
        res.detectable_share_floor.describe()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use poke_mcts::belief::Belief;
    use poke_mcts::testutil::{build_state, mon};

    fn snap_with(
        state: BattleState,
        teams: TeamData,
        our_pick: u8,
        request_kind: &str,
    ) -> PositionSnapshot {
        let mut belief = Belief::default();
        belief.note_species(6, 80);
        PositionSnapshot {
            game_id: "g".into(),
            turn: 3,
            our_side: 0,
            our_pick,
            live_budget: (8, 200, 1000),
            request_kind: request_kind.into(),
            state,
            teams,
            belief,
            tags: vec![],
        }
    }

    #[test]
    fn cat_and_decision_type_bytes() {
        assert_eq!(cat(0), ActionCat::Move);
        assert_eq!(cat(3), ActionCat::Move);
        assert_eq!(cat(255), ActionCat::Move);
        assert_eq!(cat(4), ActionCat::Switch);
        assert_eq!(cat(9), ActionCat::Switch);
        assert_eq!(cat(10), ActionCat::Tera);
        assert_eq!(cat(13), ActionCat::Tera);

        assert_eq!(decision_type(0, 1, "move"), "move->move");
        assert_eq!(decision_type(0, 4, "move"), "move->switch");
        assert_eq!(decision_type(4, 0, "move"), "switch->move");
        assert_eq!(decision_type(4, 5, "move"), "switch->switch");
        assert_eq!(decision_type(0, 10, "move"), "tera-flip");
        assert_eq!(decision_type(10, 0, "move"), "tera-flip");
        assert_eq!(decision_type(255, 4, "move"), "move->switch");
        assert_eq!(decision_type(4, 5, "force_switch"), "switch->switch");
    }

    #[test]
    fn force_switch_degenerate_move_byte_is_switch_origin() {
        assert_eq!(decision_type(0, 4, "force_switch"), "switch->switch");
    }

    #[test]
    fn move_move_weather_ko() {
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328])],
            vec![mon(6, 66, [53, 394, 0, 0])],
        );
        state.active_mon_mut(1).current_hp = 1;
        state.field.weather = WEATHER_SUN;

        let snap = snap_with(state, teams, 0, "move");
        let td = tag(&snap, 1, 2.0, 1.0);

        assert_eq!(td.decision_type, "move->move");
        assert_eq!(td.mechanic, "weather");
        assert!(td.feature.ends_with("-ko"), "feature was {}", td.feature);
    }

    #[test]
    fn switch_switch_force_switch_trick_room() {
        let (mut state, teams) = build_state(
            vec![
                mon(445, 24, [89, 14, 200, 328]),
                mon(25, 9, [85, 150, 0, 0]),
            ],
            vec![mon(6, 66, [53, 394, 0, 0])],
        );
        state.field.trick_room_turns = 5;

        let snap = snap_with(state, teams, 4, "force_switch");
        let td = tag(&snap, 5, 1.5, 0.5);

        assert_eq!(td.decision_type, "switch->switch");
        assert_eq!(td.mechanic, "trick-room");
    }

    #[test]
    fn tera_flip_hazard() {
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328])],
            vec![mon(6, 66, [53, 394, 0, 0])],
        );
        state.sides[0].side_conditions.hazard_flags = HAZARD_STEALTH_ROCK;

        let snap = snap_with(state, teams, 0, "move");
        let td = tag(&snap, 10, 1.0, 0.2);

        assert_eq!(td.decision_type, "tera-flip");
        assert_eq!(td.mechanic, "hazard");
    }

    #[test]
    fn posture_band_tiebreak_ahead() {
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328])],
            vec![mon(6, 66, [53, 394, 0, 0])],
        );
        state.active_mon_mut(0).max_hp = 100;
        state.active_mon_mut(0).current_hp = 67;
        state.active_mon_mut(1).max_hp = 100;
        state.active_mon_mut(1).current_hp = 66;

        let snap = snap_with(state, teams, 0, "move");
        let td = tag(&snap, 1, 1.0, 0.5);
        assert!(td.feature.starts_with("ahead-"), "feature was {}", td.feature);
    }

    #[test]
    fn posture_band_tiebreak_behind() {
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328])],
            vec![mon(6, 66, [53, 394, 0, 0])],
        );
        state.active_mon_mut(0).max_hp = 100;
        state.active_mon_mut(0).current_hp = 66;
        state.active_mon_mut(1).max_hp = 100;
        state.active_mon_mut(1).current_hp = 67;

        let snap = snap_with(state, teams, 0, "move");
        let td = tag(&snap, 1, 1.0, 0.5);
        assert!(td.feature.starts_with("behind-"), "feature was {}", td.feature);
    }

    #[test]
    fn posture_band_tiebreak_even() {
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328])],
            vec![mon(6, 66, [53, 394, 0, 0])],
        );
        state.active_mon_mut(0).max_hp = 100;
        state.active_mon_mut(0).current_hp = 99;
        state.active_mon_mut(1).max_hp = 100;
        state.active_mon_mut(1).current_hp = 68;

        let snap = snap_with(state, teams, 0, "move");
        let td = tag(&snap, 1, 1.0, 0.5);
        assert!(td.feature.starts_with("even-"), "feature was {}", td.feature);
    }

    #[test]
    fn loader_round_trip() {
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328])],
            vec![mon(6, 66, [53, 394, 0, 0])],
        );
        state.field.weather = WEATHER_SUN;
        state.active_mon_mut(1).current_hp = 1;

        let mut belief = Belief::default();
        belief.note_species(6, 80);
        let game_id = format!("rt-{}", std::process::id());
        let snap = PositionSnapshot {
            game_id: game_id.clone(),
            turn: 4,
            our_side: 0,
            our_pick: 0,
            live_budget: (8, 200, 1000),
            request_kind: "move".into(),
            state,
            teams,
            belief,
            tags: vec![],
        };

        let dir = std::env::temp_dir()
            .join(format!("poke_mcts_dr_{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        snap.write(&dir).expect("write snapshot");

        let costs = format!("{}/costs.tsv", dir);
        std::fs::write(
            &costs,
            format!(
                "game_id\tturn\tour_pick\ta_cmp\twr_ours\twr_cmp\tcost\tci_lo\tci_hi\n{}\t4\t0\t1\t0.5\t0.4\t2.0\t1.0\t3.0\n",
                game_id
            ),
        )
        .expect("write costs");

        let v = load_and_tag(&dir, &costs);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].game_id, game_id);
        assert_eq!(v[0].turn, 4);
        assert_eq!(v[0].cost, 2.0);
        assert_eq!(v[0].ci_lo, 1.0);
        assert_eq!(v[0].decision_type, "move->move");
        assert_eq!(v[0].mechanic, "weather");
        assert!(v[0].feature.ends_with("-ko"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn td(dt: &str, ph: &str, me: &str, fe: &str, cost: f64) -> TaggedDivergence {
        TaggedDivergence {
            game_id: "g".into(),
            turn: 1,
            cost,
            ci_lo: 0.5,
            decision_type: dt.into(),
            phase: ph.into(),
            mechanic: me.into(),
            feature: fe.into(),
        }
    }

    fn fast_cfg() -> AnalyzeConfig {
        AnalyzeConfig {
            alpha: 0.05,
            perms: 400,
            seed: 0xABCD_1234,
            floor_trials: 20,
            floor_perms: 60,
            floor_grid: vec![0.10, 0.20, 0.30, 0.40, 0.50, 0.60],
        }
    }

    // 5 distinct labels per dimension, evenly cycled with equal costs.
    fn flat_set(n: usize) -> Vec<TaggedDivergence> {
        let dts = [
            "move->move",
            "move->switch",
            "switch->move",
            "switch->switch",
            "tera-flip",
        ];
        let phs = ["early", "mid", "late", "early", "mid"];
        let mes = ["weather", "terrain", "screen", "hazard", "trick-room"];
        let fes = [
            "ahead-ko",
            "ahead-noko",
            "even-ko",
            "behind-ko",
            "behind-noko",
        ];
        (0..n)
            .map(|i| {
                let j = i % 5;
                td(dts[j], phs[j], mes[j], fes[j], 1.0)
            })
            .collect()
    }

    #[test]
    fn flat_input_no_cluster() {
        let divs = flat_set(150);
        let res = analyze(&divs, &fast_cfg());
        assert_ne!(res.verdict, Verdict::Cluster, "flat input must not CLUSTER");
        for d in &res.dims {
            assert!(
                !d.fires,
                "flat dim {} fired (observed {} > crit {})",
                d.name, d.observed_top3, d.crit_bonferroni
            );
        }
    }

    #[test]
    fn planted_cluster_does_fire() {
        // decision-type: 5 distinct buckets, nearly all cost in 3 of them; other dims flat.
        let dts = [
            "move->move",
            "move->switch",
            "switch->move",
            "switch->switch",
            "tera-flip",
        ];
        let phs = ["early", "mid", "late", "early", "mid"];
        let mes = ["weather", "terrain", "screen", "hazard", "trick-room"];
        let fes = [
            "ahead-ko",
            "ahead-noko",
            "even-ko",
            "behind-ko",
            "behind-noko",
        ];
        let mut divs = Vec::new();
        for i in 0..150usize {
            let j = i % 5;
            // 3 heavy decision-type buckets carry ~all the cost.
            let cost = if j < 3 { 20.0 } else { 0.05 };
            divs.push(td(dts[j], phs[j], mes[j], fes[j], cost));
        }
        let res = analyze(&divs, &fast_cfg());
        assert_eq!(res.verdict, Verdict::Cluster, "planted cluster must CLUSTER");
        let dt_dim = res
            .dims
            .iter()
            .find(|d| d.name == "decision-type")
            .expect("decision-type dim present");
        assert!(dt_dim.fires, "decision-type dim must fire");
    }

    #[test]
    fn m_equals_eligible_count_after_drop() {
        // decision-type=5, mechanic=5, feature=5 distinct; phase=3 distinct -> phase dropped, m==3.
        let dts = [
            "move->move",
            "move->switch",
            "switch->move",
            "switch->switch",
            "tera-flip",
        ];
        let mes = ["weather", "terrain", "screen", "hazard", "trick-room"];
        let fes = [
            "ahead-ko",
            "ahead-noko",
            "even-ko",
            "behind-ko",
            "behind-noko",
        ];
        let phs3 = ["early", "mid", "late"];
        let mut divs = Vec::new();
        for i in 0..150usize {
            let j = i % 5;
            divs.push(td(dts[j], phs3[i % 3], mes[j], fes[j], 1.0));
        }
        let res = analyze(&divs, &fast_cfg());
        assert_eq!(res.m, 3, "phase (k=3) must be dropped -> m==3");

        // Second variant: feature also has only 3 distinct -> m==2.
        let fes3 = ["ahead-ko", "even-ko", "behind-noko"];
        let mut divs2 = Vec::new();
        for i in 0..150usize {
            let j = i % 5;
            divs2.push(td(dts[j], phs3[i % 3], mes[j], fes3[i % 3], 1.0));
        }
        let res2 = analyze(&divs2, &fast_cfg());
        assert_eq!(res2.m, 2, "phase and feature (k=3) dropped -> m==2");
    }

    #[test]
    fn underpowered_floor_forces_verdict() {
        // 40 filtered divergences, heavily concentrated -> still UNDERPOWERED.
        let dts = [
            "move->move",
            "move->switch",
            "switch->move",
            "switch->switch",
            "tera-flip",
        ];
        let mes = ["weather", "terrain", "screen", "hazard", "trick-room"];
        let fes = [
            "ahead-ko",
            "ahead-noko",
            "even-ko",
            "behind-ko",
            "behind-noko",
        ];
        let phs = ["early", "mid", "late", "early", "mid"];
        let mut divs = Vec::new();
        for i in 0..40usize {
            let j = i % 5;
            let cost = if j < 3 { 20.0 } else { 0.05 };
            divs.push(td(dts[j], phs[j], mes[j], fes[j], cost));
        }
        let res = analyze(&divs, &fast_cfg());
        assert_eq!(res.verdict, Verdict::Underpowered);
        assert_eq!(res.n_scored, 40);
        // Floor is still computed regardless of the floor gate.
        match res.detectable_share_floor {
            FloorReport::Detected(_) | FloorReport::AboveGrid(_) | FloorReport::NoEligible => {}
        }
    }

    #[test]
    fn report_content_diffuse_and_cluster() {
        let dir = std::env::temp_dir()
            .join(format!("poke_mcts_dr_report_{}", std::process::id()))
            .to_string_lossy()
            .into_owned();

        // DIFFUSE report.
        let diffuse = analyze(&flat_set(150), &fast_cfg());
        assert_eq!(diffuse.verdict, Verdict::Diffuse);
        let diffuse_dir = format!("{}/diffuse", dir);
        write_report(&diffuse, None, &diffuse_dir).expect("write diffuse report");
        let body = std::fs::read_to_string(format!("{}/report.md", diffuse_dir))
            .expect("read diffuse report");
        assert!(body.contains("N_scored"), "must print N_scored");
        assert!(
            body.contains("detectable-share floor"),
            "must print detectable-share floor next to verdict"
        );
        // Floor + N_scored must sit next to the verdict on the same line.
        let verdict_line = body
            .lines()
            .find(|l| l.contains("VERDICT"))
            .expect("verdict line present");
        assert!(verdict_line.contains("N_scored"));
        assert!(verdict_line.contains("detectable-share floor"));
        assert!(
            body.contains("unresolved") && body.contains("cannot exclude a cluster the judge is"),
            "DIFFUSE must use the unresolved phrasing"
        );
        assert!(
            !body.contains("no cluster exists"),
            "must not claim no cluster exists"
        );

        // CLUSTER report -> ranked fix list enumerating the firing dim's ACTUAL buckets.
        // decision-type clusters on 3 known buckets with distinct, ordered heavy costs so we
        // can assert the report lists them by LABEL, prints their summed costs, and ranks
        // them descending. Costs: move->move=30, move->switch=20, switch->move=10, rest ~0.
        let dts = [
            "move->move",
            "move->switch",
            "switch->move",
            "switch->switch",
            "tera-flip",
        ];
        let phs = ["early", "mid", "late", "early", "mid"];
        let mes = ["weather", "terrain", "screen", "hazard", "trick-room"];
        let fes = [
            "ahead-ko",
            "ahead-noko",
            "even-ko",
            "behind-ko",
            "behind-noko",
        ];
        let heavy = [30.0f64, 20.0, 10.0, 0.05, 0.05];
        let mut cdivs = Vec::new();
        for i in 0..150usize {
            let j = i % 5;
            cdivs.push(td(dts[j], phs[j], mes[j], fes[j], heavy[j]));
        }
        let cluster = analyze(&cdivs, &fast_cfg());
        assert_eq!(cluster.verdict, Verdict::Cluster);
        let cluster_dir = format!("{}/cluster", dir);
        write_report(&cluster, Some(0.2), &cluster_dir).expect("write cluster report");
        let cbody = std::fs::read_to_string(format!("{}/report.md", cluster_dir))
            .expect("read cluster report");
        assert!(cbody.contains("Fix list") || cbody.contains("fix list"));

        // The fix list must ENUMERATE the firing dimension's actual heavy bucket labels.
        let p_mm = cbody
            .find("move->move")
            .expect("fix list must name the move->move bucket");
        let p_ms = cbody
            .find("move->switch")
            .expect("fix list must name the move->switch bucket");
        let p_sm = cbody
            .find("switch->move")
            .expect("fix list must name the switch->move bucket");

        // Ranking is descending by summed cost: move->move (30) > move->switch (20) > switch->move (10).
        assert!(
            p_mm < p_ms && p_ms < p_sm,
            "buckets must be listed in descending cost order (mm@{} ms@{} sm@{})",
            p_mm,
            p_ms,
            p_sm
        );

        // The per-bucket summed costs must appear. Each bucket has 30 filtered items at its
        // per-item cost, so summed cost = 30*per-item: 900, 600, 300.
        assert!(
            cbody.contains("900"),
            "move->move summed cost (900) must appear in the fix list"
        );
        assert!(
            cbody.contains("600"),
            "move->switch summed cost (600) must appear in the fix list"
        );
        assert!(
            cbody.contains("300"),
            "switch->move summed cost (300) must appear in the fix list"
        );

        // Per-bucket attribution to action-space / eval must be present.
        assert!(
            cbody.contains("action-space") || cbody.contains("eval"),
            "cluster report must attribute buckets to action-space / eval"
        );
        // The fidelity-blind caveat must remain.
        assert!(
            cbody.contains("engine-fidelity"),
            "cluster report must keep the engine-fidelity caveat"
        );
        assert!(cbody.contains("Recommendation"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
