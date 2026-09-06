use poke_mcts::eval_learned::{artifacts_required, LearnedValueV2, INT8_SCOPE_ENV, LVV2_WEIGHTS_PATH};
use poke_mcts::features::{DENSE_DIM, NUM_SEGMENTS};

const FIXTURES_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/lvv2-ext.fixtures.json");

struct Row {
    ids: Vec<u32>,
    seg: [u16; NUM_SEGMENTS],
    dense: [f32; DENSE_DIM],
}

fn load_rows() -> Vec<Row> {
    let text = std::fs::read_to_string(FIXTURES_PATH)
        .unwrap_or_else(|e| panic!("tracked fixture corpus {FIXTURES_PATH}: {e}"));
    let fx: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{FIXTURES_PATH} must parse: {e}"));
    let rows: Vec<Row> = fx["fixtures"]
        .as_array()
        .expect("fixtures array")
        .iter()
        .map(|f| {
            let ids: Vec<u32> =
                f["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
            let mut seg = [0u16; NUM_SEGMENTS];
            for (i, v) in f["seg_lens"].as_array().unwrap().iter().enumerate() {
                seg[i] = v.as_u64().unwrap() as u16;
            }
            let mut dense = [0f32; DENSE_DIM];
            for (i, v) in f["dense"].as_array().unwrap().iter().enumerate() {
                dense[i] = v.as_f64().unwrap() as f32;
            }
            Row { ids, seg, dense }
        })
        .collect();
    assert!(rows.len() >= 256, "corpus holds {} rows, need 256", rows.len());
    rows
}

fn span(seg: &[u16; NUM_SEGMENTS], s: usize) -> (usize, usize) {
    let start: usize = seg[..s].iter().map(|&l| l as usize).sum();
    (start, seg[s] as usize)
}

fn logit(net: &LearnedValueV2, r: &Row) -> u32 {
    net.natural_logit(&r.ids, &r.seg, &r.dense).to_bits()
}

// same-side slots are permutation-invariant, so only a cross-side swap can move the logit
fn cross_side_swap(r: &Row) -> Option<Row> {
    for s in 0..6 {
        let (sa, la) = span(&r.seg, s);
        let (sb, lb) = span(&r.seg, s + 6);
        if r.ids[sa..sa + la] == r.ids[sb..sb + lb] {
            continue;
        }
        let mut ids = Vec::with_capacity(r.ids.len());
        ids.extend_from_slice(&r.ids[..sa]);
        ids.extend_from_slice(&r.ids[sb..sb + lb]);
        ids.extend_from_slice(&r.ids[sa + la..sb]);
        ids.extend_from_slice(&r.ids[sa..sa + la]);
        ids.extend_from_slice(&r.ids[sb + lb..]);
        let mut seg = r.seg;
        seg.swap(s, s + 6);
        return Some(Row { ids, seg, dense: r.dense });
    }
    None
}

fn nudged_id(r: &Row) -> Row {
    let mut ids = r.ids.clone();
    ids[0] = if ids[0] > 0 { ids[0] - 1 } else { 1 };
    Row { ids, seg: r.seg, dense: r.dense }
}

fn dropped_id(r: &Row) -> Option<Row> {
    let s = (0..NUM_SEGMENTS).find(|&s| r.seg[s] >= 2)?;
    let (start, len) = span(&r.seg, s);
    let mut ids = r.ids.clone();
    ids.remove(start + len - 1);
    let mut seg = r.seg;
    seg[s] -= 1;
    Some(Row { ids, seg, dense: r.dense })
}

fn nudged_dense(r: &Row) -> Option<Row> {
    let j = (0..DENSE_DIM).find(|&j| r.dense[j] != 0.0)?;
    let mut dense = r.dense;
    dense[j] = f32::from_bits(dense[j].to_bits() + 1);
    Some(Row { ids: r.ids.clone(), seg: r.seg, dense })
}

#[test]
fn served_value_forward_is_stateless_and_input_sensitive() {
    let require = artifacts_required();
    let named = std::env::var("LVV2_WEIGHTS").ok().filter(|s| !s.is_empty());
    assert!(
        !(require && named.is_none()),
        "LVV2_REQUIRE is set but LVV2_WEIGHTS names no export; the tracked default is never an armed subject"
    );
    let not_subject = named.is_none();
    let path = named.unwrap_or_else(|| LVV2_WEIGHTS_PATH.to_string());

    let rows = load_rows();
    let scope = std::env::var(INT8_SCOPE_ENV).unwrap_or_default();
    let net = match LearnedValueV2::load_quantized(&path, &scope) {
        Ok(n) => n,
        Err(e) => {
            if require {
                panic!("LVV2_WEIGHTS resolved to {path}: {e}");
            }
            println!(
                "SKIP mutation battery: rows_compared=0 reference_bits_differing=0 \
                 order_compared=0 order_differing=0 swap_compared=0 swap_unmoved=0 \
                 id_compared=0 id_unmoved=0 drop_compared=0 drop_unmoved=0 \
                 dense_ulp_compared=0 dense_ulp_moved=0 int8={scope} weights={path}"
            );
            return;
        }
    };

    let mut reference_differing = 0usize;
    for r in &rows {
        let served = net.natural_logit(&r.ids, &r.seg, &r.dense);
        let reference = net.natural_logit_reference(&r.ids, &r.seg, &r.dense);
        if served.to_bits() != reference.to_bits() {
            reference_differing += 1;
        }
    }

    // a per-thread cache is only visible when a row is scored after a different predecessor
    let forward: Vec<u32> = rows.iter().map(|r| logit(&net, r)).collect();
    let mut order_compared = 0usize;
    let mut order_differing = 0usize;
    let poison = &rows[rows.len() - 1];
    for i in (0..rows.len()).rev() {
        if logit(&net, &rows[i]) != forward[i] {
            order_differing += 1;
        }
        order_compared += 1;
    }
    for (i, r) in rows.iter().enumerate() {
        logit(&net, poison);
        if logit(&net, r) != forward[i] {
            order_differing += 1;
        }
        order_compared += 1;
    }

    let mut swap_compared = 0usize;
    let mut swap_unmoved = 0usize;
    let mut id_unmoved = 0usize;
    let mut drop_compared = 0usize;
    let mut drop_unmoved = 0usize;
    let mut dense_compared = 0usize;
    let mut dense_moved = 0usize;
    for (i, r) in rows.iter().enumerate() {
        let base = forward[i];
        if let Some(m) = cross_side_swap(r) {
            swap_unmoved += usize::from(logit(&net, &m) == base);
            swap_compared += 1;
        }
        id_unmoved += usize::from(logit(&net, &nudged_id(r)) == base);
        if let Some(m) = dropped_id(r) {
            drop_unmoved += usize::from(logit(&net, &m) == base);
            drop_compared += 1;
        }
        if let Some(m) = nudged_dense(r) {
            dense_moved += usize::from(logit(&net, &m) != base);
            dense_compared += 1;
        }
    }

    let n = rows.len();
    let tag = if not_subject { " NOT-SUBJECT" } else { "" };
    println!(
        "mutation battery: rows_compared={n} reference_bits_differing={reference_differing} \
         order_compared={order_compared} order_differing={order_differing} \
         swap_compared={swap_compared} swap_unmoved={swap_unmoved} \
         id_compared={n} id_unmoved={id_unmoved} \
         drop_compared={drop_compared} drop_unmoved={drop_unmoved} \
         dense_ulp_compared={dense_compared} dense_ulp_moved={dense_moved} int8={} weights={path}{tag}",
        net.int8_scope()
    );

    assert_eq!(
        reference_differing, 0,
        "served left the frozen reference on {reference_differing} rows"
    );
    assert_eq!(order_compared, 2 * n, "every row replays under both re-orderings");
    assert_eq!(order_differing, 0, "served depends on call history on {order_differing} replays");
    assert_eq!(swap_compared, n, "every row must carry a cross-side pair to swap");
    assert_eq!(swap_unmoved, 0, "a cross-side swap held the logit bits on {swap_unmoved} rows");
    assert_eq!(id_unmoved, 0, "a one-step id change held the logit bits on {id_unmoved} rows");
    assert_eq!(drop_compared, n, "every row must carry a multi-id segment");
    assert_eq!(drop_unmoved, 0, "a dropped trailing id held the logit bits on {drop_unmoved} rows");
    assert_eq!(dense_compared, n, "every row must carry a nonzero dense value");
    // one ulp of dense sits under the logit's resolution on most rows: a floor, not a bar
    assert!(dense_moved > 0, "a one-ulp dense change never reached the logit");
}
