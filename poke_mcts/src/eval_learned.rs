use crate::eval::Evaluator;
use crate::features::{self, DENSE_DIM, FEATURE_SPEC_VERSION};
use pkmn_engine::state::BattleState;

const SIDE_BOTH: u8 = 2;

pub struct LearnedEval {
    acc_width: usize,
    multiplier: f32,
    emb: Vec<f32>,
    layers: Vec<Fc>,
    side: Vec<u8>,
}

struct Fc {
    inp: usize,
    out: usize,
    w: Vec<f32>,
    b: Vec<f32>,
}

// Mirrors learned-eval/model.py build_side_table: F1..F10 half-split on the
// side axis, F11 = used-bit pair then two 19-type blocks, F12/F13 feed both
// accumulators.
fn side_table(vocab: usize) -> Vec<u8> {
    let mut t = vec![u8::MAX; vocab];
    for g in features::spec() {
        let off = g.offset as usize;
        let size = g.size as usize;
        match g.name.split(' ').next().unwrap_or("") {
            "F12" | "F13" => t[off..off + size].fill(SIDE_BOTH),
            "F11" => {
                t[off] = 0;
                t[off + 1] = 1;
                for k in 0..size - 2 {
                    t[off + 2 + k] = (k / 19) as u8;
                }
            }
            _ => {
                let half = size / 2;
                t[off..off + half].fill(0);
                t[off + half..off + size].fill(1);
            }
        }
    }
    debug_assert!(t.iter().all(|&v| v != u8::MAX));
    t
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos + n;
        if end > self.buf.len() {
            return Err(format!("truncated weights file at byte {}", self.pos));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32_vec(&mut self, n: usize) -> Result<Vec<f32>, String> {
        let bytes = self.take(n * 4)?;
        Ok(bytes.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
    }
}

impl LearnedEval {
    pub fn from_env() -> Self {
        let path = std::env::var("BRIDGE_EVAL_WEIGHTS")
            .expect("BRIDGE_EVAL_WEIGHTS must point at a learned-eval weights file");
        match Self::load(&path) {
            Ok(e) => e,
            Err(m) => panic!("bad weights file {path}: {m}"),
        }
    }

    pub fn load(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor { buf: bytes, pos: 0 };
        if c.take(4)? != b"LVE1" {
            return Err("bad magic (want LVE1)".into());
        }
        let spec = c.u32()?;
        if spec != FEATURE_SPEC_VERSION {
            return Err(format!(
                "FEATURE_SPEC_VERSION mismatch: weights {spec}, extractor {FEATURE_SPEC_VERSION}"
            ));
        }
        let vocab = c.u32()? as usize;
        if vocab != features::vocab_size() as usize {
            return Err(format!("vocab mismatch: weights {vocab}, extractor {}", features::vocab_size()));
        }
        let acc_width = c.u32()? as usize;
        let n_fc = c.u32()? as usize;
        let mut dims = Vec::with_capacity(n_fc);
        for _ in 0..n_fc {
            let inp = c.u32()? as usize;
            let out = c.u32()? as usize;
            dims.push((inp, out));
        }
        let multiplier = c.f32()?;
        if dims.first().map(|d| d.0) != Some(2 * acc_width + DENSE_DIM) {
            return Err(format!("fc1 input dim: want {}, got {:?}", 2 * acc_width + DENSE_DIM, dims.first()));
        }
        if dims.last().map(|d| d.1) != Some(1) {
            return Err(format!("last fc output dim: want 1, got {:?}", dims.last()));
        }
        for pair in dims.windows(2) {
            if pair[0].1 != pair[1].0 {
                return Err(format!("fc dim chain break: {:?} -> {:?}", pair[0], pair[1]));
            }
        }
        let emb = c.f32_vec(vocab * acc_width)?;
        let mut layers = Vec::with_capacity(n_fc);
        for (inp, out) in dims {
            let w = c.f32_vec(inp * out)?;
            let b = c.f32_vec(out)?;
            layers.push(Fc { inp, out, w, b });
        }
        if c.pos != bytes.len() {
            return Err(format!("{} trailing bytes after tensors", bytes.len() - c.pos));
        }
        Ok(Self { acc_width, multiplier, emb, layers, side: side_table(vocab) })
    }

    pub fn export_multiplier(&self) -> f32 {
        self.multiplier
    }

    pub fn natural_logit(&self, ids: &[u32], dense: &[f32; DENSE_DIM]) -> f32 {
        let aw = self.acc_width;
        let mut x = vec![0.0f32; 2 * aw + DENSE_DIM];
        for &id in ids {
            let id = id as usize;
            let row = &self.emb[id * aw..(id + 1) * aw];
            let side = self.side[id];
            if side != 1 {
                for k in 0..aw {
                    x[k] += row[k];
                }
            }
            if side != 0 {
                for k in 0..aw {
                    x[aw + k] += row[k];
                }
            }
        }
        // ReLU covers the accumulators only; the dense channel enters raw
        for v in &mut x[..2 * aw] {
            if *v < 0.0 {
                *v = 0.0;
            }
        }
        x[2 * aw..].copy_from_slice(dense);
        let mut cur = x;
        for (li, fc) in self.layers.iter().enumerate() {
            let mut next = vec![0.0f32; fc.out];
            for (o, nv) in next.iter_mut().enumerate() {
                let w = &fc.w[o * fc.inp..(o + 1) * fc.inp];
                let mut s = fc.b[o];
                for k in 0..fc.inp {
                    s += cur[k] * w[k];
                }
                *nv = if li + 1 < self.layers.len() && s < 0.0 { 0.0 } else { s };
            }
            cur = next;
        }
        cur[0]
    }
}

impl Evaluator for LearnedEval {
    fn eval(&self, state: &BattleState) -> f32 {
        let mut ids = Vec::with_capacity(192);
        features::extract(state, &mut ids);
        let dense = features::extract_dense(state);
        self.natural_logit(&ids, &dense) * self.multiplier
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{build_state, mon};

    const WEIGHTS: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lv1-db937c18c028.bin");
    const FIXTURES: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lv1-db937c18c028.fixtures.json");

    // weights/ is a gitignored per-net artifact dir; skip when absent
    fn artifacts() -> Option<(LearnedEval, serde_json::Value)> {
        let bin = std::fs::read(WEIGHTS).ok()?;
        let fx = std::fs::read_to_string(FIXTURES).ok()?;
        Some((
            LearnedEval::from_bytes(&bin).expect("weights must load"),
            serde_json::from_str(&fx).expect("fixtures must parse"),
        ))
    }

    #[test]
    fn parity_natural_logit_within_1e4() {
        let Some((net, doc)) = artifacts() else {
            eprintln!("SKIP parity: weights artifacts not present");
            return;
        };
        assert_eq!(doc["feature_spec_version"].as_u64().unwrap() as u32, FEATURE_SPEC_VERSION);
        let fixtures = doc["fixtures"].as_array().unwrap();
        assert_eq!(fixtures.len(), 64);
        let mut max_diff = 0f64;
        for f in fixtures {
            let ids: Vec<u32> =
                f["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
            let dv = f["dense"].as_array().unwrap();
            assert_eq!(dv.len(), DENSE_DIM);
            let mut dense = [0f32; DENSE_DIM];
            for (k, v) in dv.iter().enumerate() {
                dense[k] = v.as_f64().unwrap() as f32;
            }
            let want = f["natural_logit"].as_f64().unwrap();
            let got = net.natural_logit(&ids, &dense) as f64;
            max_diff = max_diff.max((got - want).abs());
        }
        eprintln!("parity max abs diff = {max_diff:.3e}");
        assert!(max_diff <= 1e-4, "parity max abs diff {max_diff:.3e} > 1e-4");
    }

    #[test]
    fn spec_version_mismatch_refused() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"LVE1");
        bytes.extend_from_slice(&(FEATURE_SPEC_VERSION + 1).to_le_bytes());
        bytes.extend_from_slice(&features::vocab_size().to_le_bytes());
        bytes.extend_from_slice(&128u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1.0f32.to_le_bytes());
        let err = match LearnedEval::from_bytes(&bytes) {
            Ok(_) => panic!("mismatched spec version must be refused"),
            Err(e) => e,
        };
        assert!(err.contains("FEATURE_SPEC_VERSION"), "{err}");
    }

    #[test]
    fn side_table_matches_frozen_v1_routing() {
        let vocab = features::vocab_size() as usize;
        let t = side_table(vocab);
        let half_split: [(usize, usize); 11] = [
            (0, 5816),
            (5816, 49436),
            (55252, 17448),
            (72700, 3684),
            (76384, 3048),
            (79432, 1280),
            (80712, 120),
            (80832, 52),
            (80884, 2),
            (80886, 14),
            (80900, 12),
        ];
        for (off, size) in half_split {
            for k in 0..size {
                assert_eq!(t[off + k], (k >= size / 2) as u8, "id {}", off + k);
            }
        }
        assert_eq!(t[80912], 0);
        assert_eq!(t[80913], 1);
        for k in 0..38 {
            assert_eq!(t[80914 + k], (k / 19) as u8, "F11 type id {}", 80914 + k);
        }
        for id in 80952..vocab {
            assert_eq!(t[id], SIDE_BOTH, "id {id}");
        }
    }

    #[test]
    fn eval_is_scaled_forward_of_extracted_features() {
        let Some((net, _)) = artifacts() else {
            eprintln!("SKIP eval wiring: weights artifacts not present");
            return;
        };
        let (s, _t) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0])],
        );
        let mut ids = Vec::new();
        features::extract(&s, &mut ids);
        let dense = features::extract_dense(&s);
        let want = net.natural_logit(&ids, &dense) * net.export_multiplier();
        assert!(want.is_finite());
        assert_ne!(want, 0.0, "forward must produce a real logit on a live state");
        assert_eq!(net.eval(&s), want);
    }
}
