use crate::action_features::ACTION_DENSE_DIM;
use crate::eval::Evaluator;
use crate::features::{self, DENSE_DIM, FEATURE_SPEC_VERSION, NUM_SEGMENTS};
use pkmn_engine::data::{GEN_MOVES, TOTAL_SPECIES};
use pkmn_engine::state::BattleState;

const SIDE_BOTH: u8 = 2;

pub const NUM_ACTIONS: usize = 14;

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

impl Fc {
    fn apply(&self, x: &[f32], relu: bool) -> Vec<f32> {
        debug_assert_eq!(x.len(), self.inp);
        let mut y = vec![0.0f32; self.out];
        for (o, yv) in y.iter_mut().enumerate() {
            let w = &self.w[o * self.inp..(o + 1) * self.inp];
            let mut s = self.b[o];
            for k in 0..self.inp {
                s += x[k] * w[k];
            }
            *yv = if relu && s < 0.0 { 0.0 } else { s };
        }
        y
    }
}

fn run_head(layers: &[Fc], x: Vec<f32>) -> Vec<f32> {
    let last = layers.len() - 1;
    let mut cur = x;
    for (i, fc) in layers.iter().enumerate() {
        cur = fc.apply(&cur, i < last);
    }
    cur
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

pub struct LearnedPolicy {
    acc_width: usize,
    web_rank: usize,
    multiplier: f32,
    move_emb_dim: usize,
    emb: Vec<f32>,
    web_a: Vec<f32>,
    web_b: Vec<f32>,
    fc: Vec<Fc>,
    sw: Vec<Fc>,
    move_emb: Vec<f32>,
    mv: Vec<Fc>,
}

fn read_dims(c: &mut Cursor, n: usize) -> Result<Vec<(usize, usize)>, String> {
    let mut dims = Vec::with_capacity(n);
    for _ in 0..n {
        let inp = c.u32()? as usize;
        let out = c.u32()? as usize;
        dims.push((inp, out));
    }
    Ok(dims)
}

fn check_chain(dims: &[(usize, usize)], what: &str) -> Result<(), String> {
    for pair in dims.windows(2) {
        if pair[0].1 != pair[1].0 {
            return Err(format!("{what} dim chain break: {:?} -> {:?}", pair[0], pair[1]));
        }
    }
    Ok(())
}

fn read_layers(c: &mut Cursor, dims: &[(usize, usize)]) -> Result<Vec<Fc>, String> {
    let mut layers = Vec::with_capacity(dims.len());
    for &(inp, out) in dims {
        let w = c.f32_vec(inp * out)?;
        let b = c.f32_vec(out)?;
        layers.push(Fc { inp, out, w, b });
    }
    Ok(layers)
}

impl LearnedPolicy {
    pub fn load(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor { buf: bytes, pos: 0 };
        if c.take(4)? != b"LVP1" {
            return Err("bad magic (want LVP1)".into());
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
        let segments = c.u32()? as usize;
        if segments != NUM_SEGMENTS {
            return Err(format!("segment count mismatch: weights {segments}, extractor {NUM_SEGMENTS}"));
        }
        let web_rank = c.u32()? as usize;
        let tables = c.u32()?;
        let pair_rows = c.u32()? as usize;
        let type_rows = c.u32()? as usize;
        if tables != 0 {
            return Err(format!("tables arm not supported by the f32 port (tables flag {tables})"));
        }
        if pair_rows != 0 || type_rows != 0 {
            return Err(format!("table dims present with tables off: pair {pair_rows}, type {type_rows}"));
        }
        let n_fc = c.u32()? as usize;
        let fc_dims = read_dims(&mut c, n_fc)?;
        let multiplier = c.f32()?;
        if n_fc < 2 {
            return Err(format!("fc chain needs >= 2 layers (penultimate feeds the heads), got {n_fc}"));
        }
        let want_fc1 = 2 * acc_width + 2 * web_rank + DENSE_DIM;
        if fc_dims.first().map(|d| d.0) != Some(want_fc1) {
            return Err(format!("fc1 input dim: want {want_fc1}, got {:?}", fc_dims.first()));
        }
        if fc_dims.last().map(|d| d.1) != Some(1) {
            return Err(format!("last fc output dim: want 1, got {:?}", fc_dims.last()));
        }
        check_chain(&fc_dims, "fc")?;
        let g_dim = fc_dims[fc_dims.len() - 1].0;
        let sw_n = c.u32()? as usize;
        let sw_dims = read_dims(&mut c, sw_n)?;
        if sw_n == 0 {
            return Err("switch head needs >= 1 layer".into());
        }
        let want_sw = acc_width + web_rank + g_dim;
        if sw_dims.first().map(|d| d.0) != Some(want_sw) {
            return Err(format!("switch head input dim: want {want_sw}, got {:?}", sw_dims.first()));
        }
        if sw_dims.last().map(|d| d.1) != Some(1) {
            return Err(format!("switch head output dim: want 1, got {:?}", sw_dims.last()));
        }
        check_chain(&sw_dims, "switch head")?;
        let move_vocab = c.u32()? as usize;
        if move_vocab != GEN_MOVES.len() {
            return Err(format!("move vocab mismatch: weights {move_vocab}, engine {}", GEN_MOVES.len()));
        }
        let move_emb_dim = c.u32()? as usize;
        let mv_n = c.u32()? as usize;
        let mv_dims = read_dims(&mut c, mv_n)?;
        if mv_n == 0 {
            return Err("move head needs >= 1 layer".into());
        }
        let want_mv = acc_width + web_rank + move_emb_dim + g_dim;
        if mv_dims.first().map(|d| d.0) != Some(want_mv) {
            return Err(format!("move head input dim: want {want_mv}, got {:?}", mv_dims.first()));
        }
        if mv_dims.last().map(|d| d.1) != Some(2) {
            return Err(format!("move head output dim: want 2 (move, tera), got {:?}", mv_dims.last()));
        }
        check_chain(&mv_dims, "move head")?;
        let emb = c.f32_vec(vocab * acc_width)?;
        let web_a = c.f32_vec(web_rank * acc_width)?;
        let web_b = c.f32_vec(web_rank * acc_width)?;
        let fc = read_layers(&mut c, &fc_dims)?;
        let sw = read_layers(&mut c, &sw_dims)?;
        let move_emb = c.f32_vec(move_vocab * move_emb_dim)?;
        let mv = read_layers(&mut c, &mv_dims)?;
        if c.pos != bytes.len() {
            return Err(format!("{} trailing bytes after tensors", bytes.len() - c.pos));
        }
        Ok(Self {
            acc_width,
            web_rank,
            multiplier,
            move_emb_dim,
            emb,
            web_a,
            web_b,
            fc,
            sw,
            move_emb,
            mv,
        })
    }

    pub fn export_multiplier(&self) -> f32 {
        self.multiplier
    }

    pub fn policy_logits(
        &self,
        ids: &[u32],
        seg_lens: &[u16; NUM_SEGMENTS],
        dense: &[f32; DENSE_DIM],
        move_ids: &[u16; 4],
    ) -> [f32; NUM_ACTIONS] {
        self.forward(ids, seg_lens, dense, move_ids).0
    }

    // Raw natural logits over action bytes 0-13; legality mask + softmax live
    // outside (masked_softmax). Value tail is computed and returned untouched.
    pub fn forward(
        &self,
        ids: &[u32],
        seg_lens: &[u16; NUM_SEGMENTS],
        dense: &[f32; DENSE_DIM],
        move_ids: &[u16; 4],
    ) -> ([f32; NUM_ACTIONS], f32) {
        let aw = self.acc_width;
        let r = self.web_rank;
        let species = TOTAL_SPECIES;
        let f1_vocab = 4 * species;
        let mut tokens = vec![0.0f32; NUM_SEGMENTS * aw];
        let mut active0: i32 = -1;
        let mut active1: i32 = -1;
        let mut pos = 0usize;
        for (seg, &len) in seg_lens.iter().enumerate() {
            let tok_off = seg * aw;
            for &id in &ids[pos..pos + len as usize] {
                let idu = id as usize;
                let row = &self.emb[idu * aw..(idu + 1) * aw];
                for k in 0..aw {
                    tokens[tok_off + k] += row[k];
                }
                if seg < 12 && idu < f1_vocab && (idu / species) % 2 == 0 {
                    if seg < 6 {
                        active0 = seg as i32;
                    } else {
                        active1 = (seg - 6) as i32;
                    }
                }
            }
            pos += len as usize;
        }
        debug_assert_eq!(pos, ids.len(), "segment lengths must cover the id stream");
        let mut x = vec![0.0f32; 2 * aw + 2 * r + DENSE_DIM];
        for s in 0..6 {
            for k in 0..aw {
                x[k] += tokens[s * aw + k];
                x[aw + k] += tokens[(6 + s) * aw + k];
            }
        }
        for k in 0..aw {
            x[k] += tokens[12 * aw + k] + tokens[14 * aw + k];
            x[aw + k] += tokens[13 * aw + k] + tokens[14 * aw + k];
        }
        for v in &mut x[..2 * aw] {
            if *v < 0.0 {
                *v = 0.0;
            }
        }
        let mut a = vec![0.0f32; 6 * r];
        let mut b = vec![0.0f32; 6 * r];
        for i in 0..6 {
            for o in 0..r {
                let wa = &self.web_a[o * aw..(o + 1) * aw];
                let wb = &self.web_b[o * aw..(o + 1) * aw];
                let mut sa = 0.0f32;
                let mut sb = 0.0f32;
                for k in 0..aw {
                    sa += tokens[i * aw + k] * wa[k];
                    sb += tokens[(6 + i) * aw + k] * wb[k];
                }
                a[i * r + o] = sa;
                b[i * r + o] = sb;
            }
        }
        let mut h_i = vec![0.0f32; 6 * r];
        for i in 0..6 {
            for j in 0..6 {
                for o in 0..r {
                    let p = a[i * r + o] * b[j * r + o];
                    let p = if p < 0.0 { 0.0 } else { p };
                    h_i[i * r + o] += p;
                    x[2 * aw + o] += p;
                    if i as i32 == active0 && j as i32 == active1 {
                        x[2 * aw + r + o] = p;
                    }
                }
            }
        }
        x[2 * aw + 2 * r..].copy_from_slice(dense);
        let mut cur = x;
        for fc in &self.fc[..self.fc.len() - 1] {
            cur = fc.apply(&cur, true);
        }
        let g = cur;
        let value = self.fc[self.fc.len() - 1].apply(&g, false)[0];
        let mut logits = [0.0f32; NUM_ACTIONS];
        for k in 0..6 {
            let mut sin = Vec::with_capacity(aw + r + g.len());
            sin.extend_from_slice(&tokens[k * aw..(k + 1) * aw]);
            sin.extend_from_slice(&h_i[k * r..(k + 1) * r]);
            sin.extend_from_slice(&g);
            logits[4 + k] = run_head(&self.sw, sin)[0];
        }
        if active0 >= 0 {
            let ak = active0 as usize;
            let ed = self.move_emb_dim;
            for (j, &mid) in move_ids.iter().enumerate() {
                let mid = mid as usize;
                let mut min = Vec::with_capacity(aw + r + ed + g.len());
                min.extend_from_slice(&tokens[ak * aw..(ak + 1) * aw]);
                min.extend_from_slice(&h_i[ak * r..(ak + 1) * r]);
                min.extend_from_slice(&self.move_emb[mid * ed..(mid + 1) * ed]);
                min.extend_from_slice(&g);
                let out = run_head(&self.mv, min);
                logits[j] = out[0];
                logits[10 + j] = out[1];
            }
        }
        (logits, value)
    }
}

#[allow(dead_code)]
pub struct LearnedPolicyV2 {
    acc_width: usize,
    web_rank: usize,
    ctx_dim: usize,
    move_emb_dim: usize,
    pair_rows: usize,
    type_rows: usize,
    table_rank: usize,
    attn_dk: usize,
    action_dense_dim: usize,
    emb: Vec<f32>,
    web_a: Vec<f32>,
    web_b: Vec<f32>,
    pair_tab: Vec<f32>,
    type_tab: Vec<f32>,
    w_tab: Vec<f32>,
    attn_q: Vec<f32>,
    attn_k: Vec<f32>,
    attn_v: Vec<f32>,
    attn_out: Vec<f32>,
    ctx: Vec<Fc>,
    sw: Vec<Fc>,
    move_emb: Vec<f32>,
    mv: Vec<Fc>,
}

impl LearnedPolicyV2 {
    pub fn load(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor { buf: bytes, pos: 0 };
        if c.take(4)? != b"LVP2" {
            return Err("bad magic (want LVP2)".into());
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
        let segments = c.u32()? as usize;
        if segments != NUM_SEGMENTS {
            return Err(format!("segment count mismatch: weights {segments}, extractor {NUM_SEGMENTS}"));
        }
        let web_rank = c.u32()? as usize;
        let tables = c.u32()?;
        let pair_rows = c.u32()? as usize;
        let type_rows = c.u32()? as usize;
        let table_rank = c.u32()? as usize;
        if tables != 0 && (pair_rows == 0 || type_rows == 0 || table_rank == 0) {
            return Err(format!(
                "table dims incomplete with tables on: pair {pair_rows}, type {type_rows}, rank {table_rank}"
            ));
        }
        if tables == 0 && (pair_rows != 0 || type_rows != 0 || table_rank != 0) {
            return Err(format!(
                "table dims present with tables off: pair {pair_rows}, type {type_rows}, rank {table_rank}"
            ));
        }
        let attn = c.u32()?;
        let attn_dk = c.u32()? as usize;
        if attn != 0 && attn_dk == 0 {
            return Err("attn on with attn_dk 0".into());
        }
        if attn == 0 && attn_dk != 0 {
            return Err(format!("attn_dk present with attn off: {attn_dk}"));
        }
        let action_dense = c.u32()?;
        let action_dense_dim = c.u32()? as usize;
        if action_dense != 0 && action_dense_dim != ACTION_DENSE_DIM {
            return Err(format!(
                "action_dense_dim mismatch: weights {action_dense_dim}, engine {ACTION_DENSE_DIM}"
            ));
        }
        if action_dense == 0 && action_dense_dim != 0 {
            return Err(format!(
                "action_dense_dim present with action_dense off: {action_dense_dim}"
            ));
        }
        let ctx_dim = c.u32()? as usize;
        let n_ctx = c.u32()? as usize;
        let ctx_dims = read_dims(&mut c, n_ctx)?;
        let want_ctx = 2 * acc_width + 2 * web_rank + DENSE_DIM;
        if ctx_dims.first().map(|d| d.0) != Some(want_ctx) {
            return Err(format!("ctx input dim: want {want_ctx}, got {:?}", ctx_dims.first()));
        }
        check_chain(&ctx_dims, "ctx")?;
        if ctx_dims.last().map(|d| d.1) != Some(ctx_dim) {
            return Err(format!("ctx output dim: want ctx_dim {ctx_dim}, got {:?}", ctx_dims.last()));
        }
        let sw_n = c.u32()? as usize;
        let sw_dims = read_dims(&mut c, sw_n)?;
        let want_sw = 2 * acc_width + 3 * web_rank + ctx_dim + action_dense_dim;
        if sw_dims.first().map(|d| d.0) != Some(want_sw) {
            return Err(format!("switch head input dim: want {want_sw}, got {:?}", sw_dims.first()));
        }
        if sw_dims.last().map(|d| d.1) != Some(1) {
            return Err(format!("switch head output dim: want 1, got {:?}", sw_dims.last()));
        }
        check_chain(&sw_dims, "switch head")?;
        let move_vocab = c.u32()? as usize;
        if move_vocab != GEN_MOVES.len() {
            return Err(format!("move vocab mismatch: weights {move_vocab}, engine {}", GEN_MOVES.len()));
        }
        let move_emb_dim = c.u32()? as usize;
        let mv_n = c.u32()? as usize;
        let mv_dims = read_dims(&mut c, mv_n)?;
        let want_mv = acc_width + 2 * web_rank + move_emb_dim + ctx_dim + action_dense_dim;
        if mv_dims.first().map(|d| d.0) != Some(want_mv) {
            return Err(format!("move head input dim: want {want_mv}, got {:?}", mv_dims.first()));
        }
        // plain and tera are two evaluations of one output here, not LVP1's one evaluation with two
        if mv_dims.last().map(|d| d.1) != Some(1) {
            return Err(format!("move head output dim: want 1, got {:?}", mv_dims.last()));
        }
        check_chain(&mv_dims, "move head")?;
        let emb = c.f32_vec(vocab * acc_width)?;
        let web_a = c.f32_vec(web_rank * acc_width)?;
        let web_b = c.f32_vec(web_rank * acc_width)?;
        let (pair_tab, type_tab, w_tab) = if tables != 0 {
            (
                c.f32_vec(pair_rows * table_rank)?,
                c.f32_vec(type_rows * table_rank)?,
                c.f32_vec(web_rank * table_rank)?,
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        let (attn_q, attn_k, attn_v, attn_out) = if attn_dk != 0 {
            (
                c.f32_vec(attn_dk * acc_width)?,
                c.f32_vec(attn_dk * acc_width)?,
                c.f32_vec(attn_dk * acc_width)?,
                c.f32_vec(acc_width * attn_dk)?,
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new(), Vec::new())
        };
        let ctx = read_layers(&mut c, &ctx_dims)?;
        let sw = read_layers(&mut c, &sw_dims)?;
        let move_emb = c.f32_vec(move_vocab * move_emb_dim)?;
        let mv = read_layers(&mut c, &mv_dims)?;
        if c.pos != bytes.len() {
            return Err(format!("{} trailing bytes after tensors", bytes.len() - c.pos));
        }
        Ok(Self {
            acc_width,
            web_rank,
            ctx_dim,
            move_emb_dim,
            pair_rows,
            type_rows,
            table_rank,
            attn_dk,
            action_dense_dim,
            emb,
            web_a,
            web_b,
            pair_tab,
            type_tab,
            w_tab,
            attn_q,
            attn_k,
            attn_v,
            attn_out,
            ctx,
            sw,
            move_emb,
            mv,
        })
    }
}

// -inf on illegal actions then a stable softmax; all-illegal returns zeros.
pub fn masked_softmax(logits: &[f32; NUM_ACTIONS], legal: &[bool; NUM_ACTIONS]) -> [f32; NUM_ACTIONS] {
    let mut out = [0.0f32; NUM_ACTIONS];
    let mut mx = f32::NEG_INFINITY;
    for i in 0..NUM_ACTIONS {
        if legal[i] && logits[i] > mx {
            mx = logits[i];
        }
    }
    if mx == f32::NEG_INFINITY {
        return out;
    }
    let mut sum = 0.0f32;
    for i in 0..NUM_ACTIONS {
        if legal[i] {
            let e = (logits[i] - mx).exp();
            out[i] = e;
            sum += e;
        }
    }
    for v in &mut out {
        *v /= sum;
    }
    out
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
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lvp1-ff8e8651f6b5.bin");
    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../learned-eval/weights/lvp1-ff8e8651f6b5.fixtures.json"
    );
    #[allow(dead_code)]
    const WEIGHTS_V2: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lvp2-6f1e0facfcc4.bin");
    #[allow(dead_code)]
    const FIXTURES_V2: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../learned-eval/weights/lvp2-6f1e0facfcc4.fixtures.json"
    );
    const SPEC3_WEIGHTS: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lv1-db937c18c028.bin");

    // weights/ is a gitignored per-net artifact dir: absence skips, a
    // present-but-unloadable file fails
    fn artifacts_from(weights: &str, fixtures: &str) -> Option<(LearnedPolicy, serde_json::Value)> {
        let bin = std::fs::read(weights).ok()?;
        let fx = std::fs::read_to_string(fixtures).ok()?;
        Some((
            LearnedPolicy::from_bytes(&bin).expect("weights must load"),
            serde_json::from_str(&fx).expect("fixtures must parse"),
        ))
    }

    // LVP1_WEIGHTS/LVP1_FIXTURES point the parity gate at another export
    // (e.g. a freshly trained arm) without touching the pinned artifacts
    fn artifacts() -> Option<(LearnedPolicy, serde_json::Value)> {
        let (bin, fx) = resolve_artifact_paths(
            "LVP1",
            std::env::var("LVP1_WEIGHTS").ok(),
            std::env::var("LVP1_FIXTURES").ok(),
            WEIGHTS,
            FIXTURES,
        )?;
        Some((
            LearnedPolicy::from_bytes(&bin).expect("LVP1 weights must load"),
            serde_json::from_str(&fx).expect("LVP1 fixtures must parse"),
        ))
    }

    // either override set means an operator named a specific export, so a path
    // that will not read is a typo to surface, never an absent artifact to skip
    fn resolve_artifact_paths(
        magic: &str,
        w_override: Option<String>,
        f_override: Option<String>,
        w_default: &str,
        f_default: &str,
    ) -> Option<(Vec<u8>, String)> {
        let strict = w_override.is_some() || f_override.is_some();
        let w = w_override.unwrap_or_else(|| w_default.to_string());
        let f = f_override.unwrap_or_else(|| f_default.to_string());
        if !strict {
            return Some((std::fs::read(&w).ok()?, std::fs::read_to_string(&f).ok()?));
        }
        let bin =
            std::fs::read(&w).unwrap_or_else(|e| panic!("{magic}_WEIGHTS resolved to {w}: {e}"));
        let fx = std::fs::read_to_string(&f)
            .unwrap_or_else(|e| panic!("{magic}_FIXTURES resolved to {f}: {e}"));
        Some((bin, fx))
    }

    fn readable_tmp(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("poke_mcts_resolve_{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("present");
        std::fs::write(&p, b"present").unwrap();
        p.to_str().unwrap().to_string()
    }

    fn resolve_panic(
        magic: &'static str,
        w_override: Option<String>,
        f_override: Option<String>,
        w_default: String,
        f_default: String,
    ) -> String {
        let err = std::panic::catch_unwind(move || {
            resolve_artifact_paths(magic, w_override, f_override, &w_default, &f_default)
        })
        .expect_err("a set override pointing at a missing path must fail, not skip");
        err.downcast_ref::<String>().cloned().unwrap_or_default()
    }

    #[test]
    fn lvp1_weights_override_missing_fails() {
        let msg = resolve_panic(
            "LVP1",
            Some("/nonexistent/lvp1-w.bin".to_string()),
            None,
            readable_tmp("lvp1_w"),
            readable_tmp("lvp1_w"),
        );
        assert!(msg.contains("LVP1"), "failure must name its own magic: {msg}");
    }

    #[test]
    fn lvp1_fixtures_override_missing_fails_with_weights_unset() {
        let msg = resolve_panic(
            "LVP1",
            None,
            Some("/nonexistent/lvp1-f.json".to_string()),
            readable_tmp("lvp1_f"),
            readable_tmp("lvp1_f"),
        );
        assert!(msg.contains("LVP1"), "failure must name its own magic: {msg}");
    }

    #[test]
    fn lvp1_defaults_absent_skips() {
        assert!(resolve_artifact_paths(
            "LVP1",
            None,
            None,
            "/nonexistent/lvp1-w.bin",
            "/nonexistent/lvp1-f.json"
        )
        .is_none());
    }

    #[test]
    fn lvp2_weights_override_missing_fails() {
        let msg = resolve_panic(
            "LVP2",
            Some("/nonexistent/lvp2-w.bin".to_string()),
            None,
            readable_tmp("lvp2_w"),
            readable_tmp("lvp2_w"),
        );
        assert!(msg.contains("LVP2"), "failure must name its own magic: {msg}");
    }

    #[test]
    fn lvp2_fixtures_override_missing_fails_with_weights_unset() {
        let msg = resolve_panic(
            "LVP2",
            None,
            Some("/nonexistent/lvp2-f.json".to_string()),
            readable_tmp("lvp2_f"),
            readable_tmp("lvp2_f"),
        );
        assert!(msg.contains("LVP2"), "failure must name its own magic: {msg}");
    }

    #[test]
    fn lvp2_defaults_absent_skips() {
        assert!(resolve_artifact_paths(
            "LVP2",
            None,
            None,
            "/nonexistent/lvp2-w.bin",
            "/nonexistent/lvp2-f.json"
        )
        .is_none());
    }

    #[test]
    fn artifacts_absence_skips() {
        assert!(artifacts_from("/nonexistent/w.bin", "/nonexistent/f.json").is_none());
    }

    #[test]
    fn artifacts_present_but_unloadable_fails() {
        let dir = std::env::temp_dir().join("poke_mcts_artifacts_corrupt_test");
        std::fs::create_dir_all(&dir).unwrap();
        let w = dir.join("w.bin");
        let f = dir.join("f.json");
        std::fs::write(&w, b"not a weights file").unwrap();
        std::fs::write(&f, b"{}").unwrap();
        let bad_weights = std::panic::catch_unwind(|| {
            artifacts_from(w.to_str().unwrap(), f.to_str().unwrap())
        });
        assert!(bad_weights.is_err(), "corrupt weights must fail, not skip");
        let valid = PolicyFile::small().bytes();
        std::fs::write(&w, &valid).unwrap();
        std::fs::write(&f, b"not json").unwrap();
        let bad_fixtures = std::panic::catch_unwind(|| {
            artifacts_from(w.to_str().unwrap(), f.to_str().unwrap())
        });
        assert!(bad_fixtures.is_err(), "corrupt fixtures must fail, not skip");
    }

    #[test]
    fn spec3_weights_refused_at_current_spec() {
        let Ok(bin) = std::fs::read(SPEC3_WEIGHTS) else {
            eprintln!("SKIP spec3 refusal: weights artifacts not present");
            return;
        };
        let err = match LearnedEval::from_bytes(&bin) {
            Ok(_) => panic!("archived spec-3 weights must be refused at the current spec"),
            Err(e) => e,
        };
        let want =
            format!("FEATURE_SPEC_VERSION mismatch: weights 3, extractor {FEATURE_SPEC_VERSION}");
        assert_eq!(err, want);
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

    struct PolicyFile {
        magic: [u8; 4],
        spec: u32,
        vocab: u32,
        acc: u32,
        segments: u32,
        web_rank: u32,
        tables: u32,
        pair_rows: u32,
        type_rows: u32,
        fc: Vec<(u32, u32)>,
        multiplier: f32,
        sw: Vec<(u32, u32)>,
        move_vocab: u32,
        move_emb_dim: u32,
        mv: Vec<(u32, u32)>,
        truncate: usize,
        trailing: usize,
    }

    impl PolicyFile {
        fn small() -> Self {
            let acc = 1u32;
            let r = 1u32;
            let g = 2u32;
            PolicyFile {
                magic: *b"LVP1",
                spec: FEATURE_SPEC_VERSION,
                vocab: features::vocab_size(),
                acc,
                segments: NUM_SEGMENTS as u32,
                web_rank: r,
                tables: 0,
                pair_rows: 0,
                type_rows: 0,
                fc: vec![(2 * acc + 2 * r + DENSE_DIM as u32, g), (g, 1)],
                multiplier: 1.0,
                sw: vec![(acc + r + g, 2), (2, 1)],
                move_vocab: GEN_MOVES.len() as u32,
                move_emb_dim: 1,
                mv: vec![(acc + r + 1 + g, 2), (2, 2)],
                truncate: 0,
                trailing: 0,
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&self.magic);
            for u in [
                self.spec,
                self.vocab,
                self.acc,
                self.segments,
                self.web_rank,
                self.tables,
                self.pair_rows,
                self.type_rows,
                self.fc.len() as u32,
            ] {
                v.extend_from_slice(&u.to_le_bytes());
            }
            for &(i, o) in &self.fc {
                v.extend_from_slice(&i.to_le_bytes());
                v.extend_from_slice(&o.to_le_bytes());
            }
            v.extend_from_slice(&self.multiplier.to_le_bytes());
            v.extend_from_slice(&(self.sw.len() as u32).to_le_bytes());
            for &(i, o) in &self.sw {
                v.extend_from_slice(&i.to_le_bytes());
                v.extend_from_slice(&o.to_le_bytes());
            }
            v.extend_from_slice(&self.move_vocab.to_le_bytes());
            v.extend_from_slice(&self.move_emb_dim.to_le_bytes());
            v.extend_from_slice(&(self.mv.len() as u32).to_le_bytes());
            for &(i, o) in &self.mv {
                v.extend_from_slice(&i.to_le_bytes());
                v.extend_from_slice(&o.to_le_bytes());
            }
            let mut n_f32 = (self.vocab * self.acc + 2 * self.web_rank * self.acc) as usize;
            n_f32 += self.move_vocab as usize * self.move_emb_dim as usize;
            for dims in [&self.fc, &self.sw, &self.mv] {
                for &(i, o) in dims.iter() {
                    n_f32 += (i * o + o) as usize;
                }
            }
            for k in 0..n_f32 {
                let val = ((k % 13) as f32 - 6.0) * 0.01;
                v.extend_from_slice(&val.to_le_bytes());
            }
            if self.truncate > 0 {
                v.truncate(v.len() - self.truncate);
            }
            for _ in 0..self.trailing {
                v.push(0);
            }
            v
        }

        fn err(&self) -> String {
            match LearnedPolicy::from_bytes(&self.bytes()) {
                Ok(_) => panic!("malformed policy weights must be refused"),
                Err(e) => e,
            }
        }
    }

    #[test]
    fn policy_valid_synthetic_loads_and_forwards() {
        let net = LearnedPolicy::from_bytes(&PolicyFile::small().bytes()).expect("must load");
        let (s, _t) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0])],
        );
        let mut ids = Vec::new();
        let lens = features::extract_segmented(&s, &mut ids);
        let dense = features::extract_dense(&s);
        let move_ids = s.sides[0].team[s.sides[0].active_index as usize].moves;
        let (logits, value) = net.forward(&ids, &lens, &dense, &move_ids);
        assert!(logits.iter().all(|v| v.is_finite()));
        assert!(value.is_finite());
        let mut legal = [false; NUM_ACTIONS];
        legal[0] = true;
        legal[4] = true;
        legal[5] = true;
        let p = masked_softmax(&logits, &legal);
        let sum: f32 = p.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "legal mass must sum to 1, got {sum}");
        assert_eq!(p[1], 0.0);
        assert_eq!(p[10], 0.0);
    }

    #[test]
    fn masked_softmax_all_illegal_is_zero() {
        let p = masked_softmax(&[1.0; NUM_ACTIONS], &[false; NUM_ACTIONS]);
        assert!(p.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn policy_bad_magic_refused() {
        let mut f = PolicyFile::small();
        f.magic = *b"LVE1";
        assert_eq!(f.err(), "bad magic (want LVP1)");
    }

    #[test]
    fn policy_spec_mismatch_refused() {
        let mut f = PolicyFile::small();
        f.spec = 3;
        assert!(f.err().contains("FEATURE_SPEC_VERSION mismatch"), "{}", f.err());
    }

    #[test]
    fn policy_vocab_mismatch_refused() {
        let mut f = PolicyFile::small();
        f.vocab += 1;
        assert!(f.err().contains("vocab mismatch"), "{}", f.err());
    }

    #[test]
    fn policy_segment_count_mismatch_refused() {
        let mut f = PolicyFile::small();
        f.segments = 14;
        assert!(f.err().contains("segment count mismatch"), "{}", f.err());
    }

    #[test]
    fn policy_tables_arm_refused() {
        let mut f = PolicyFile::small();
        f.tables = 1;
        f.pair_rows = 1454 * 1454;
        f.type_rows = 18 * 18;
        assert!(f.err().contains("tables arm not supported"), "{}", f.err());
    }

    #[test]
    fn policy_table_dims_without_tables_refused() {
        let mut f = PolicyFile::small();
        f.pair_rows = 1454 * 1454;
        assert!(f.err().contains("table dims present with tables off"), "{}", f.err());
    }

    #[test]
    fn policy_fc1_input_dim_refused() {
        let mut f = PolicyFile::small();
        f.fc[0].0 += 1;
        assert!(f.err().contains("fc1 input dim"), "{}", f.err());
    }

    #[test]
    fn policy_last_fc_output_dim_refused() {
        let mut f = PolicyFile::small();
        f.fc.last_mut().unwrap().1 = 2;
        assert!(f.err().contains("last fc output dim"), "{}", f.err());
    }

    #[test]
    fn policy_fc_chain_break_refused() {
        let mut f = PolicyFile::small();
        f.fc[0].1 += 1;
        assert!(f.err().contains("fc dim chain break"), "{}", f.err());
    }

    #[test]
    fn policy_single_fc_layer_refused() {
        let mut f = PolicyFile::small();
        f.fc = vec![(2 + 2 + DENSE_DIM as u32, 1)];
        assert!(f.err().contains("fc chain needs >= 2 layers"), "{}", f.err());
    }

    #[test]
    fn policy_switch_head_input_dim_refused() {
        let mut f = PolicyFile::small();
        f.sw[0].0 += 1;
        assert!(f.err().contains("switch head input dim"), "{}", f.err());
    }

    #[test]
    fn policy_switch_head_output_dim_refused() {
        let mut f = PolicyFile::small();
        f.sw.last_mut().unwrap().1 = 2;
        assert!(f.err().contains("switch head output dim"), "{}", f.err());
    }

    #[test]
    fn policy_switch_head_chain_break_refused() {
        let mut f = PolicyFile::small();
        f.sw[0].1 += 1;
        assert!(f.err().contains("switch head dim chain break"), "{}", f.err());
    }

    #[test]
    fn policy_move_vocab_mismatch_refused() {
        let mut f = PolicyFile::small();
        f.move_vocab -= 1;
        assert!(f.err().contains("move vocab mismatch"), "{}", f.err());
    }

    #[test]
    fn policy_move_head_input_dim_refused() {
        let mut f = PolicyFile::small();
        f.mv[0].0 += 1;
        assert!(f.err().contains("move head input dim"), "{}", f.err());
    }

    #[test]
    fn policy_move_head_output_dim_refused() {
        let mut f = PolicyFile::small();
        f.mv.last_mut().unwrap().1 = 1;
        assert!(f.err().contains("move head output dim"), "{}", f.err());
    }

    #[test]
    fn policy_move_head_chain_break_refused() {
        let mut f = PolicyFile::small();
        f.mv[0].1 += 1;
        assert!(f.err().contains("move head dim chain break"), "{}", f.err());
    }

    #[test]
    fn policy_truncated_refused() {
        let mut f = PolicyFile::small();
        f.truncate = 4;
        assert!(f.err().contains("truncated weights file"), "{}", f.err());
    }

    #[test]
    fn policy_trailing_bytes_refused() {
        let mut f = PolicyFile::small();
        f.trailing = 4;
        assert!(f.err().contains("trailing bytes"), "{}", f.err());
    }

    struct PolicyFileV2 {
        magic: [u8; 4],
        spec: u32,
        vocab: u32,
        acc: u32,
        segments: u32,
        web_rank: u32,
        tables: u32,
        pair_rows: u32,
        type_rows: u32,
        table_rank: u32,
        attn: u32,
        attn_dk: u32,
        action_dense: u32,
        action_dense_dim: u32,
        ctx_dim: u32,
        ctx: Vec<(u32, u32)>,
        sw: Vec<(u32, u32)>,
        move_vocab: u32,
        move_emb_dim: u32,
        mv: Vec<(u32, u32)>,
        truncate: usize,
        trailing: usize,
    }

    impl PolicyFileV2 {
        fn small() -> Self {
            let acc = 1u32;
            let r = 1u32;
            let ctx_dim = 2u32;
            let ad = ACTION_DENSE_DIM as u32;
            let med = 1u32;
            PolicyFileV2 {
                magic: *b"LVP2",
                spec: FEATURE_SPEC_VERSION,
                vocab: features::vocab_size(),
                acc,
                segments: NUM_SEGMENTS as u32,
                web_rank: r,
                tables: 0,
                pair_rows: 0,
                type_rows: 0,
                table_rank: 0,
                attn: 0,
                attn_dk: 0,
                action_dense: 1,
                action_dense_dim: ad,
                ctx_dim,
                ctx: vec![(2 * acc + 2 * r + DENSE_DIM as u32, ctx_dim), (ctx_dim, ctx_dim)],
                sw: vec![(2 * acc + 3 * r + ctx_dim + ad, 2), (2, 1)],
                move_vocab: GEN_MOVES.len() as u32,
                move_emb_dim: med,
                mv: vec![(acc + 2 * r + med + ctx_dim + ad, 2), (2, 1)],
                truncate: 0,
                trailing: 0,
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&self.magic);
            for u in [
                self.spec,
                self.vocab,
                self.acc,
                self.segments,
                self.web_rank,
                self.tables,
                self.pair_rows,
                self.type_rows,
                self.table_rank,
                self.attn,
                self.attn_dk,
                self.action_dense,
                self.action_dense_dim,
                self.ctx_dim,
                self.ctx.len() as u32,
            ] {
                v.extend_from_slice(&u.to_le_bytes());
            }
            for &(i, o) in &self.ctx {
                v.extend_from_slice(&i.to_le_bytes());
                v.extend_from_slice(&o.to_le_bytes());
            }
            v.extend_from_slice(&(self.sw.len() as u32).to_le_bytes());
            for &(i, o) in &self.sw {
                v.extend_from_slice(&i.to_le_bytes());
                v.extend_from_slice(&o.to_le_bytes());
            }
            v.extend_from_slice(&self.move_vocab.to_le_bytes());
            v.extend_from_slice(&self.move_emb_dim.to_le_bytes());
            v.extend_from_slice(&(self.mv.len() as u32).to_le_bytes());
            for &(i, o) in &self.mv {
                v.extend_from_slice(&i.to_le_bytes());
                v.extend_from_slice(&o.to_le_bytes());
            }
            let mut n_f32 = (self.vocab * self.acc + 2 * self.web_rank * self.acc) as usize;
            if self.tables != 0 {
                n_f32 += ((self.pair_rows + self.type_rows + self.web_rank) * self.table_rank) as usize;
            }
            if self.attn != 0 {
                n_f32 += 4 * (self.attn_dk * self.acc) as usize;
            }
            n_f32 += self.move_vocab as usize * self.move_emb_dim as usize;
            for dims in [&self.ctx, &self.sw, &self.mv] {
                for &(i, o) in dims.iter() {
                    n_f32 += (i * o + o) as usize;
                }
            }
            for k in 0..n_f32 {
                let val = ((k % 13) as f32 - 6.0) * 0.01;
                v.extend_from_slice(&val.to_le_bytes());
            }
            if self.truncate > 0 {
                v.truncate(v.len() - self.truncate);
            }
            for _ in 0..self.trailing {
                v.push(0);
            }
            v
        }

        fn err(&self) -> String {
            match LearnedPolicyV2::from_bytes(&self.bytes()) {
                Ok(_) => panic!("malformed v2 policy weights must be refused"),
                Err(e) => e,
            }
        }
    }

    #[test]
    fn policy_v2_valid_synthetic_loads() {
        let f = PolicyFileV2::small();
        let net = LearnedPolicyV2::from_bytes(&f.bytes()).expect("synthetic LVP2 must load");
        assert_eq!(net.acc_width, f.acc as usize);
        assert_eq!(net.web_rank, f.web_rank as usize);
        assert_eq!(net.ctx_dim, f.ctx_dim as usize);
        assert_eq!(net.action_dense_dim, ACTION_DENSE_DIM);
        assert_eq!(net.ctx.len(), f.ctx.len());
        assert_eq!(net.sw.len(), f.sw.len());
        assert_eq!(net.mv.len(), f.mv.len());
    }

    #[test]
    fn policy_v2_bad_magic_refused() {
        let mut f = PolicyFileV2::small();
        f.magic = *b"LVP1";
        assert_eq!(f.err(), "bad magic (want LVP2)");
    }

    #[test]
    fn policy_v2_spec_mismatch_refused() {
        let mut f = PolicyFileV2::small();
        f.spec = 3;
        assert!(f.err().contains("FEATURE_SPEC_VERSION mismatch"), "{}", f.err());
    }

    #[test]
    fn policy_v2_vocab_mismatch_refused() {
        let mut f = PolicyFileV2::small();
        f.vocab += 1;
        assert!(f.err().contains("vocab mismatch"), "{}", f.err());
    }

    #[test]
    fn policy_v2_segment_count_mismatch_refused() {
        let mut f = PolicyFileV2::small();
        f.segments = NUM_SEGMENTS as u32 - 1;
        assert!(f.err().contains("segment count mismatch"), "{}", f.err());
    }

    #[test]
    fn policy_v2_tables_without_pair_rows_refused() {
        let mut f = PolicyFileV2::small();
        f.tables = 1;
        f.pair_rows = 0;
        f.type_rows = 3;
        f.table_rank = 2;
        assert!(f.err().contains("table dims incomplete with tables on"), "{}", f.err());
    }

    #[test]
    fn policy_v2_tables_without_type_rows_refused() {
        let mut f = PolicyFileV2::small();
        f.tables = 1;
        f.pair_rows = 4;
        f.type_rows = 0;
        f.table_rank = 2;
        assert!(f.err().contains("table dims incomplete with tables on"), "{}", f.err());
    }

    #[test]
    fn policy_v2_tables_without_table_rank_refused() {
        let mut f = PolicyFileV2::small();
        f.tables = 1;
        f.pair_rows = 4;
        f.type_rows = 3;
        f.table_rank = 0;
        assert!(f.err().contains("table dims incomplete with tables on"), "{}", f.err());
    }

    #[test]
    fn policy_v2_table_dims_without_tables_refused() {
        let mut f = PolicyFileV2::small();
        f.pair_rows = 4;
        assert!(f.err().contains("table dims present with tables off"), "{}", f.err());
    }

    #[test]
    fn policy_v2_attn_without_dk_refused() {
        let mut f = PolicyFileV2::small();
        f.attn = 1;
        assert!(f.err().contains("attn on with attn_dk 0"), "{}", f.err());
    }

    #[test]
    fn policy_v2_attn_dk_without_attn_refused() {
        let mut f = PolicyFileV2::small();
        f.attn_dk = 4;
        assert!(f.err().contains("attn_dk present with attn off"), "{}", f.err());
    }

    #[test]
    fn policy_v2_action_dense_dim_mismatch_refused() {
        let mut f = PolicyFileV2::small();
        f.action_dense_dim += 1;
        assert!(f.err().contains("action_dense_dim mismatch"), "{}", f.err());
    }

    #[test]
    fn policy_v2_action_dense_dim_without_flag_refused() {
        let mut f = PolicyFileV2::small();
        f.action_dense = 0;
        assert!(
            f.err().contains("action_dense_dim present with action_dense off"),
            "{}",
            f.err()
        );
    }

    #[test]
    fn policy_v2_ctx_input_dim_refused() {
        let mut f = PolicyFileV2::small();
        f.ctx[0].0 += 1;
        assert!(f.err().contains("ctx input dim"), "{}", f.err());
    }

    #[test]
    fn policy_v2_ctx_chain_break_refused() {
        let mut f = PolicyFileV2::small();
        f.ctx[0].1 += 1;
        assert!(f.err().contains("ctx dim chain break"), "{}", f.err());
    }

    #[test]
    fn policy_v2_ctx_output_dim_refused() {
        let mut f = PolicyFileV2::small();
        f.ctx.last_mut().unwrap().1 += 1;
        assert!(f.err().contains("ctx output dim"), "{}", f.err());
    }

    #[test]
    fn policy_v2_switch_head_input_dim_refused() {
        let mut f = PolicyFileV2::small();
        f.sw[0].0 += 1;
        assert!(f.err().contains("switch head input dim"), "{}", f.err());
    }

    #[test]
    fn policy_v2_switch_head_output_dim_refused() {
        let mut f = PolicyFileV2::small();
        f.sw.last_mut().unwrap().1 = 2;
        assert!(f.err().contains("switch head output dim"), "{}", f.err());
    }

    #[test]
    fn policy_v2_switch_head_chain_break_refused() {
        let mut f = PolicyFileV2::small();
        f.sw[0].1 += 1;
        assert!(f.err().contains("switch head dim chain break"), "{}", f.err());
    }

    #[test]
    fn policy_v2_move_vocab_mismatch_refused() {
        let mut f = PolicyFileV2::small();
        f.move_vocab -= 1;
        assert!(f.err().contains("move vocab mismatch"), "{}", f.err());
    }

    #[test]
    fn policy_v2_move_head_input_dim_refused() {
        let mut f = PolicyFileV2::small();
        f.mv[0].0 += 1;
        assert!(f.err().contains("move head input dim"), "{}", f.err());
    }

    #[test]
    fn policy_v2_move_head_output_dim_refused() {
        let mut f = PolicyFileV2::small();
        f.mv.last_mut().unwrap().1 = 2;
        assert!(f.err().contains("move head output dim"), "{}", f.err());
    }

    #[test]
    fn policy_v2_move_head_chain_break_refused() {
        let mut f = PolicyFileV2::small();
        f.mv[0].1 += 1;
        assert!(f.err().contains("move head dim chain break"), "{}", f.err());
    }

    #[test]
    fn policy_v2_truncated_refused() {
        let mut f = PolicyFileV2::small();
        f.truncate = 4;
        assert!(f.err().contains("truncated weights file"), "{}", f.err());
    }

    #[test]
    fn policy_v2_trailing_bytes_refused() {
        let mut f = PolicyFileV2::small();
        f.trailing = 4;
        assert!(f.err().contains("trailing bytes"), "{}", f.err());
    }

    // run id, web_rank, ctx_dim, head hidden, action_dense_dim, pair_rows,
    // type_rows, table_rank, attn_dk
    const V2_ARTIFACTS: [(&str, usize, usize, &[usize], usize, usize, usize, usize, usize); 6] = [
        ("6f1e0facfcc4", 128, 256, &[256, 128], 8, 0, 0, 0, 32),
        ("0beb036c5111", 128, 256, &[256, 128], 0, 0, 0, 0, 0),
        ("f397e133cee9", 32, 64, &[64], 8, 0, 0, 0, 0),
        ("bd3341316553", 128, 256, &[256, 128], 8, 0, 0, 0, 0),
        ("0b7a57e97325", 128, 256, &[256, 128], 8, 2114116, 324, 16, 0),
        ("0ccd9c8c3288", 128, 256, &[256, 128], 8, 0, 0, 0, 32),
    ];

    fn hidden(layers: &[Fc]) -> Vec<usize> {
        layers[..layers.len() - 1].iter().map(|l| l.out).collect()
    }

    #[test]
    fn policy_v2_real_artifacts_load() {
        for (id, web_rank, ctx_dim, head, ad, pair_rows, type_rows, table_rank, attn_dk) in
            V2_ARTIFACTS
        {
            let path = format!(
                "{}/../learned-eval/weights/lvp2-{id}.bin",
                env!("CARGO_MANIFEST_DIR")
            );
            let Ok(bin) = std::fs::read(&path) else {
                eprintln!("SKIP lvp2 load {id}: weights artifact not present");
                continue;
            };
            let net = LearnedPolicyV2::from_bytes(&bin)
                .unwrap_or_else(|e| panic!("lvp2-{id}.bin must load: {e}"));
            assert_eq!(net.web_rank, web_rank, "{id}: web_rank");
            assert_eq!(net.ctx_dim, ctx_dim, "{id}: ctx_dim");
            assert_eq!(hidden(&net.sw), head, "{id}: switch head hidden");
            assert_eq!(hidden(&net.mv), head, "{id}: move head hidden");
            assert_eq!(net.action_dense_dim, ad, "{id}: action_dense_dim");
            assert_eq!(net.pair_rows, pair_rows, "{id}: pair_rows");
            assert_eq!(net.type_rows, type_rows, "{id}: type_rows");
            assert_eq!(net.table_rank, table_rank, "{id}: table_rank");
            assert_eq!(net.attn_dk, attn_dk, "{id}: attn_dk");
            eprintln!(
                "lvp2 {id}: acc {} web_rank {} ctx_dim {} move_emb {} ad {} tables {}/{}/{} attn_dk {} ctx_in {} sw_in {} mv_in {}",
                net.acc_width,
                net.web_rank,
                net.ctx_dim,
                net.move_emb_dim,
                net.action_dense_dim,
                net.pair_rows,
                net.type_rows,
                net.table_rank,
                net.attn_dk,
                net.ctx[0].inp,
                net.sw[0].inp,
                net.mv[0].inp
            );
        }
    }

    fn fixture_inputs(f: &serde_json::Value) -> (Vec<u32>, [u16; NUM_SEGMENTS], [f32; DENSE_DIM], [u16; 4]) {
        let ids: Vec<u32> =
            f["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
        let mut lens = [0u16; NUM_SEGMENTS];
        for (k, v) in f["seg_lens"].as_array().unwrap().iter().enumerate() {
            lens[k] = v.as_u64().unwrap() as u16;
        }
        let mut dense = [0f32; DENSE_DIM];
        for (k, v) in f["dense"].as_array().unwrap().iter().enumerate() {
            dense[k] = v.as_f64().unwrap() as f32;
        }
        let mut move_ids = [0u16; 4];
        for (k, v) in f["move_ids"].as_array().unwrap().iter().enumerate() {
            move_ids[k] = v.as_u64().unwrap() as u16;
        }
        (ids, lens, dense, move_ids)
    }

    #[test]
    fn policy_parity_64_fixtures() {
        let Some((net, fx)) = artifacts() else {
            eprintln!("SKIP policy parity: weights artifacts not present");
            return;
        };
        let fixtures = fx["fixtures"].as_array().expect("fixtures array");
        assert_eq!(fixtures.len(), 64, "parity gate is 64 fixtures");
        let mut max_diff = 0f64;
        let mut max_vdiff = 0f64;
        for f in fixtures {
            let (ids, lens, dense, move_ids) = fixture_inputs(f);
            let (logits, value) = net.forward(&ids, &lens, &dense, &move_ids);
            let want = f["logits"].as_array().expect("14 logits");
            assert_eq!(want.len(), NUM_ACTIONS);
            for (k, w) in want.iter().enumerate() {
                let d = (logits[k] as f64 - w.as_f64().unwrap()).abs();
                if d > max_diff {
                    max_diff = d;
                }
            }
            let vd = (value as f64 - f["value_logit"].as_f64().unwrap()).abs();
            if vd > max_vdiff {
                max_vdiff = vd;
            }
        }
        eprintln!("policy parity: max abs diff logits {max_diff:.3e}, value {max_vdiff:.3e}");
        assert!(max_diff <= 1e-4, "logit parity {max_diff:e} exceeds 1e-4");
        assert!(max_vdiff <= 1e-4, "value parity {max_vdiff:e} exceeds 1e-4");
    }

    #[test]
    #[ignore]
    fn policy_forward_throughput() {
        let Some((net, fx)) = artifacts() else {
            eprintln!("SKIP throughput: weights artifacts not present");
            return;
        };
        let f = &fx["fixtures"].as_array().unwrap()[0];
        let (ids, lens, dense, move_ids) = fixture_inputs(f);
        let mut sink = 0f32;
        for _ in 0..1_000 {
            sink += net.forward(&ids, &lens, &dense, &move_ids).0[0];
        }
        let iters = 100_000u32;
        let t0 = std::time::Instant::now();
        for _ in 0..iters {
            let (l, _) = net.forward(&ids, &lens, &dense, &move_ids);
            sink += std::hint::black_box(l)[0];
        }
        let el = t0.elapsed();
        eprintln!("policy forward: {:.3} us/forward ({} iters, sink {sink})", el.as_secs_f64() * 1e6 / iters as f64, iters);
    }

    #[cfg(feature = "train_value")]
    #[test]
    #[ignore]
    fn dump_policy_fixture_inputs() {
        use crate::policy_label::read_records_guarded;
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../full_cp07_vlabel/s0");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .expect("full_cp07_vlabel/s0 must be present")
            .filter_map(|e| {
                let name = e.unwrap().file_name().into_string().unwrap();
                name.ends_with(".records.bin").then_some(name)
            })
            .collect();
        files.sort();
        files.truncate(8);
        let mut recs = Vec::new();
        for name in &files {
            let path = format!("{dir}/{name}");
            for (i, r) in read_records_guarded(&path).expect("guarded read").into_iter().enumerate() {
                recs.push((name.clone(), i, r));
            }
        }
        let total = recs.len();
        assert!(total >= 64, "need >= 64 records, got {total}");
        let mut fixtures = Vec::new();
        for k in 0..64 {
            let (name, idx, rec) = &recs[k * total / 64];
            let mut ids = Vec::new();
            let lens = features::extract_segmented(&rec.state, &mut ids);
            let dense = features::extract_dense(&rec.state);
            let side = &rec.state.sides[0];
            let move_ids = side.team[side.active_index as usize].moves;
            fixtures.push(serde_json::json!({
                "file": name,
                "index": idx,
                "ids": ids,
                "seg_lens": lens.to_vec(),
                "dense": dense.to_vec(),
                "move_ids": move_ids.to_vec(),
            }));
        }
        let doc = serde_json::json!({
            "source_dir": dir,
            "files": files,
            "selection": format!("stride 64 over {total} records"),
            "fixtures": fixtures,
        });
        let out = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../learned-eval/weights/policy-fixture-inputs.json"
        );
        std::fs::create_dir_all(std::path::Path::new(out).parent().unwrap()).unwrap();
        std::fs::write(out, serde_json::to_string(&doc).unwrap()).unwrap();
        eprintln!("wrote {out}: 64 fixtures from {total} records / {} files", files.len());
    }

    #[cfg(feature = "train_value")]
    #[test]
    #[ignore]
    fn dump_policy_fixture_inputs_v2() {
        use crate::action_features::{action_features, ACTION_DENSE_DIM};
        use crate::policy_label::read_records_guarded;
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../full_cp07_vlabel/s0");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .expect("full_cp07_vlabel/s0 must be present")
            .filter_map(|e| {
                let name = e.unwrap().file_name().into_string().unwrap();
                name.ends_with(".records.bin").then_some(name)
            })
            .collect();
        files.sort();
        files.truncate(8);
        let mut recs = Vec::new();
        for name in &files {
            let path = format!("{dir}/{name}");
            for (i, r) in read_records_guarded(&path).expect("guarded read").into_iter().enumerate() {
                recs.push((name.clone(), i, r));
            }
        }
        let total = recs.len();
        assert!(total >= 64, "need >= 64 records, got {total}");
        let mut fixtures = Vec::new();
        let mut nonzero = 0usize;
        for k in 0..64 {
            let (name, idx, rec) = &recs[k * total / 64];
            let mut ids = Vec::new();
            let lens = features::extract_segmented(&rec.state, &mut ids);
            let dense = features::extract_dense(&rec.state);
            let side = &rec.state.sides[0];
            let move_ids = side.team[side.active_index as usize].moves;
            let block = action_features(&rec.state, 0);
            if block.iter().flatten().any(|v| *v != 0.0) {
                nonzero += 1;
            }
            let action_dense: Vec<Vec<f32>> = block.iter().map(|row| row.to_vec()).collect();
            fixtures.push(serde_json::json!({
                "file": name,
                "index": idx,
                "ids": ids,
                "seg_lens": lens.to_vec(),
                "dense": dense.to_vec(),
                "move_ids": move_ids.to_vec(),
                "action_dense": action_dense,
            }));
        }
        assert_eq!(fixtures.len(), 64, "parity gate is 64 fixtures");
        for (k, f) in fixtures.iter().enumerate() {
            let rows = f["action_dense"].as_array().expect("action_dense array");
            assert_eq!(rows.len(), NUM_ACTIONS, "fixture {k}: action_dense rows");
            for (r, row) in rows.iter().enumerate() {
                let cols = row.as_array().expect("action_dense row");
                assert_eq!(cols.len(), ACTION_DENSE_DIM, "fixture {k} row {r}: width");
                for c in cols {
                    assert!(c.is_f64(), "fixture {k} row {r}: non-float entry {c}");
                }
            }
        }

        let v1_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../learned-eval/weights/policy-fixture-inputs.json"
        );
        let v1: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(v1_path).expect("v1 fixture inputs must be present"),
        )
        .expect("v1 fixture inputs must parse");
        let v1_fixtures = v1["fixtures"].as_array().expect("v1 fixtures array");
        assert_eq!(v1_fixtures.len(), 64, "v1 fixture inputs must carry 64 fixtures");
        for (k, (new, old)) in fixtures.iter().zip(v1_fixtures).enumerate() {
            for key in ["file", "index", "ids", "seg_lens", "move_ids"] {
                assert_eq!(
                    serde_json::to_string(&new[key]).unwrap(),
                    serde_json::to_string(&old[key]).unwrap(),
                    "fixture {k}: {key} diverges from the v1 selection"
                );
            }
            // serde_json's parser is not correctly rounded, so the f32 payload
            // is compared at f32 width rather than through the parsed f64.
            let want = old["dense"].as_array().expect("v1 dense array");
            let got = new["dense"].as_array().expect("dense array");
            assert_eq!(got.len(), want.len(), "fixture {k}: dense width");
            for (i, (g, w)) in got.iter().zip(want).enumerate() {
                assert_eq!(
                    g.as_f64().unwrap() as f32,
                    w.as_f64().unwrap() as f32,
                    "fixture {k}: dense[{i}] diverges from the v1 selection"
                );
            }
        }

        let doc = serde_json::json!({
            "source_dir": dir,
            "files": files,
            "selection": format!("stride 64 over {total} records"),
            "fixtures": fixtures,
        });
        let out = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../learned-eval/weights/policy-fixture-inputs-v2.json"
        );
        std::fs::create_dir_all(std::path::Path::new(out).parent().unwrap()).unwrap();
        std::fs::write(out, serde_json::to_string(&doc).unwrap()).unwrap();
        eprintln!("wrote {out}: 64 fixtures from {total} records / {} files", files.len());
        eprintln!("action_dense non-zero fraction: {nonzero}/64 = {:.4}", nonzero as f64 / 64.0);
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
        let vocab = features::vocab_size();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"LVE1");
        bytes.extend_from_slice(&FEATURE_SPEC_VERSION.to_le_bytes());
        bytes.extend_from_slice(&vocab.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(2 + DENSE_DIM as u32).to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&2.5f32.to_le_bytes());
        let n_f32 = vocab as usize + (2 + DENSE_DIM) + 1;
        for k in 0..n_f32 {
            let val = ((k % 13) as f32 - 6.0) * 0.01;
            bytes.extend_from_slice(&val.to_le_bytes());
        }
        let net = LearnedEval::from_bytes(&bytes).expect("synthetic LVE1 must load");
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
