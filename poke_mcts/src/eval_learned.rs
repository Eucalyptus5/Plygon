use crate::action_features::ACTION_DENSE_DIM;
use crate::eval::Evaluator;
use crate::features::{self, DENSE_DIM, FEATURE_SPEC_VERSION, NUM_SEGMENTS};
use pkmn_engine::data::base_stats::species as species_row;
use pkmn_engine::data::{GEN_MOVES, TOTAL_SPECIES};
use pkmn_engine::state::BattleState;

const SIDE_BOTH: u8 = 2;

const NUM_TYPES: usize = 18;

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

fn transpose(src: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    debug_assert_eq!(src.len(), rows * cols);
    let mut out = vec![0.0f32; rows * cols];
    for r in 0..rows {
        for c in 0..cols {
            out[c * rows + r] = src[r * cols + c];
        }
    }
    out
}

// One weight row serves the whole batch before the next is touched, and the batch
// index is the innermost axis so the lanes are independent accumulators. Each
// element still sums over k in `Fc::apply`'s order, so it is bit-identical.
fn apply_batch_t(fc: &Fc, xt: &[f32], n: usize, relu: bool) -> Vec<f32> {
    let inp = fc.inp;
    debug_assert_eq!(xt.len(), n * inp);
    let mut y = vec![0.0f32; fc.out * n];
    for o in 0..fc.out {
        let w = &fc.w[o * inp..(o + 1) * inp];
        let yo = &mut y[o * n..(o + 1) * n];
        yo.fill(fc.b[o]);
        for k in 0..inp {
            let wk = w[k];
            let xk = &xt[k * n..(k + 1) * n];
            for m in 0..n {
                yo[m] += xk[m] * wk;
            }
        }
        if relu {
            for v in yo.iter_mut() {
                if *v < 0.0 {
                    *v = 0.0;
                }
            }
        }
    }
    y
}

fn head_scalar_batch(layers: &[Fc], xs: &[f32], n: usize, inp: usize) -> Vec<f32> {
    let last = layers.len() - 1;
    let mut cur = transpose(xs, n, inp);
    for (i, fc) in layers.iter().enumerate() {
        cur = apply_batch_t(fc, &cur, n, i < last);
    }
    debug_assert_eq!(cur.len(), n);
    cur
}

// single-output head chain over a borrowed input, so one buffer serves every slot
fn head_scalar(layers: &[Fc], x: &[f32]) -> f32 {
    let last = layers.len() - 1;
    let mut cur = layers[0].apply(x, last > 0);
    for (i, fc) in layers.iter().enumerate().skip(1) {
        cur = fc.apply(&cur, i < last);
    }
    cur[0]
}

#[inline(always)]
fn dot(w: &[f32], x: &[f32]) -> f32 {
    debug_assert_eq!(w.len(), x.len());
    w.iter().zip(x).map(|(a, b)| a * b).sum()
}

#[inline(always)]
fn species_types(id: usize) -> (usize, usize) {
    let s = species_row(id);
    (s.type1 as usize, s.type2 as usize)
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

struct Prep {
    tokens: Vec<f32>,
    p: Vec<f32>,
    h_sum: Vec<f32>,
    h_max: Vec<f32>,
    ctx_in: Vec<f32>,
    active0: i32,
    active1: i32,
}

/// One member of a batched policy forward; the fields are `forward`'s arguments.
pub struct ForwardInput<'a> {
    pub ids: &'a [u32],
    pub seg_lens: &'a [u16; NUM_SEGMENTS],
    pub dense: &'a [f32; DENSE_DIM],
    pub move_ids: &'a [u16; 4],
    pub action_dense: Option<&'a [[f32; ACTION_DENSE_DIM]; NUM_ACTIONS]>,
}

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

    pub fn action_dense_dim(&self) -> usize {
        self.action_dense_dim
    }

    fn attend(&self, tokens: &[f32]) -> Vec<f32> {
        let aw = self.acc_width;
        let dk = self.attn_dk;
        let mut present = [false; NUM_SEGMENTS];
        for (s, flag) in present.iter_mut().enumerate() {
            *flag = tokens[s * aw..(s + 1) * aw].iter().map(|v| v.abs()).sum::<f32>() > 0.0;
        }
        let mut q = vec![0.0f32; NUM_SEGMENTS * dk];
        let mut key = vec![0.0f32; NUM_SEGMENTS * dk];
        let mut val = vec![0.0f32; NUM_SEGMENTS * dk];
        for s in 0..NUM_SEGMENTS {
            let t = &tokens[s * aw..(s + 1) * aw];
            for o in 0..dk {
                let w = o * aw..(o + 1) * aw;
                q[s * dk + o] = dot(&self.attn_q[w.clone()], t);
                key[s * dk + o] = dot(&self.attn_k[w.clone()], t);
                val[s * dk + o] = dot(&self.attn_v[w], t);
            }
        }
        let scale = (dk as f32).sqrt();
        let mut out = tokens.to_vec();
        let mut w = [0.0f32; NUM_SEGMENTS];
        let mut mix = vec![0.0f32; dk];
        for i in 0..NUM_SEGMENTS {
            if !present[i] {
                continue;
            }
            let qi = &q[i * dk..(i + 1) * dk];
            let mut mx = f32::NEG_INFINITY;
            for j in 0..NUM_SEGMENTS {
                if present[j] {
                    w[j] = dot(qi, &key[j * dk..(j + 1) * dk]) / scale;
                    if w[j] > mx {
                        mx = w[j];
                    }
                }
            }
            let mut sum = 0.0f32;
            for j in 0..NUM_SEGMENTS {
                w[j] = if present[j] { (w[j] - mx).exp() } else { 0.0 };
                sum += w[j];
            }
            for v in mix.iter_mut() {
                *v = 0.0;
            }
            for j in 0..NUM_SEGMENTS {
                if !present[j] {
                    continue;
                }
                let wj = w[j] / sum;
                for o in 0..dk {
                    mix[o] += wj * val[j * dk + o];
                }
            }
            for o in 0..aw {
                out[i * aw + o] += dot(&self.attn_out[o * dk..(o + 1) * dk], &mix);
            }
        }
        out
    }

    fn cell_bias(&self, si: i32, sj: i32, low: &mut [f32], out: &mut [f32]) {
        if si < 0 || sj < 0 {
            out.fill(0.0);
            return;
        }
        let tr = self.table_rank;
        let (si, sj) = (si as usize, sj as usize);
        let pair = si * TOTAL_SPECIES + sj;
        debug_assert!(pair < self.pair_rows, "pair row {pair} beyond {}", self.pair_rows);
        let pv = &self.pair_tab[pair * tr..(pair + 1) * tr];
        let (t1a, t2a) = species_types(si);
        let (t1b, t2b) = species_types(sj);
        let rows = [
            t1a * NUM_TYPES + t1b,
            t1a * NUM_TYPES + t2b,
            t2a * NUM_TYPES + t1b,
            t2a * NUM_TYPES + t2b,
        ];
        debug_assert!(rows.iter().all(|&t| t < self.type_rows), "type row beyond {}", self.type_rows);
        for o in 0..tr {
            let mut s = 0.0f32;
            for &row in &rows {
                s += self.type_tab[row * tr + o];
            }
            low[o] = pv[o] + s / 4.0;
        }
        for o in 0..self.web_rank {
            out[o] = dot(&self.w_tab[o * tr..(o + 1) * tr], low);
        }
    }

    // Batched sibling of `forward`: the ctx trunk and both heads stream their weights
    // once across the whole batch while every output element keeps the single-forward
    // accumulation order, so each row is bit-identical to `forward` on that input.
    pub fn forward_batch(&self, batch: &[ForwardInput<'_>]) -> Vec<[f32; NUM_ACTIONS]> {
        let ad = self.action_dense_dim;
        let aw = self.acc_width;
        let r = self.web_rank;
        let n = batch.len();
        let mut out = vec![[0.0f32; NUM_ACTIONS]; n];
        if n == 0 {
            return out;
        }
        let preps: Vec<Prep> = batch
            .iter()
            .map(|it| {
                assert_eq!(
                    ad != 0,
                    it.action_dense.is_some(),
                    "the per-action block must be given exactly when the header sets action_dense"
                );
                self.prep(it.ids, it.seg_lens, it.dense)
            })
            .collect();

        let mut rows = Vec::with_capacity(n * self.ctx[0].inp);
        for pr in &preps {
            rows.extend_from_slice(&pr.ctx_in);
        }
        let mut cur = transpose(&rows, n, self.ctx[0].inp);
        for fc in &self.ctx {
            cur = apply_batch_t(fc, &cur, n, true);
        }
        let gd = self.ctx_dim;
        let g = transpose(&cur, gd, n);

        let sw_in = self.sw[0].inp;
        let mut srows = Vec::with_capacity(n * 6 * sw_in);
        for (m, pr) in preps.iter().enumerate() {
            for k in 0..6 {
                srows.extend_from_slice(&pr.tokens[k * aw..(k + 1) * aw]);
                srows.extend_from_slice(&pr.h_sum[k * r..(k + 1) * r]);
                srows.extend_from_slice(&pr.h_max[k * r..(k + 1) * r]);
                if pr.active1 >= 0 {
                    let a1 = pr.active1 as usize;
                    let idx = (k * 6 + a1) * r;
                    srows.extend_from_slice(&pr.p[idx..idx + r]);
                    srows.extend_from_slice(&pr.tokens[(6 + a1) * aw..(7 + a1) * aw]);
                } else {
                    srows.resize(srows.len() + r + aw, 0.0);
                }
                srows.extend_from_slice(&g[m * gd..(m + 1) * gd]);
                if ad != 0 {
                    srows.extend_from_slice(&batch[m].action_dense.unwrap()[4 + k]);
                }
            }
        }
        let sout = head_scalar_batch(&self.sw, &srows, n * 6, sw_in);
        for (m, row) in out.iter_mut().enumerate() {
            for k in 0..6 {
                row[4 + k] = sout[m * 6 + k];
            }
        }

        let mv_in = self.mv[0].inp;
        let mut mrows: Vec<f32> = Vec::new();
        let mut slots: Vec<(usize, usize)> = Vec::new();
        for (m, pr) in preps.iter().enumerate() {
            if pr.active0 < 0 {
                continue;
            }
            for (j, &mid) in batch[m].move_ids.iter().enumerate() {
                let mid = mid as usize;
                match batch[m].action_dense {
                    None => {
                        self.push_move_row(&mut mrows, pr, mid, &g[m * gd..(m + 1) * gd]);
                        slots.push((m, j));
                    }
                    Some(block) => {
                        self.push_move_row(&mut mrows, pr, mid, &g[m * gd..(m + 1) * gd]);
                        mrows.extend_from_slice(&block[j]);
                        slots.push((m, j));
                        self.push_move_row(&mut mrows, pr, mid, &g[m * gd..(m + 1) * gd]);
                        mrows.extend_from_slice(&block[10 + j]);
                        slots.push((m, 10 + j));
                    }
                }
            }
        }
        if !slots.is_empty() {
            let mout = head_scalar_batch(&self.mv, &mrows, slots.len(), mv_in);
            for (i, &(m, slot)) in slots.iter().enumerate() {
                out[m][slot] = mout[i];
                if ad == 0 {
                    out[m][10 + slot] = mout[i];
                }
            }
        }
        out
    }

    fn push_move_row(&self, rows: &mut Vec<f32>, pr: &Prep, mid: usize, g_row: &[f32]) {
        let aw = self.acc_width;
        let r = self.web_rank;
        let ed = self.move_emb_dim;
        let a0 = pr.active0 as usize;
        rows.extend_from_slice(&pr.tokens[a0 * aw..(a0 + 1) * aw]);
        rows.extend_from_slice(&pr.h_sum[a0 * r..(a0 + 1) * r]);
        if pr.active1 >= 0 {
            let idx = (a0 * 6 + pr.active1 as usize) * r;
            rows.extend_from_slice(&pr.p[idx..idx + r]);
        } else {
            rows.resize(rows.len() + r, 0.0);
        }
        rows.extend_from_slice(&self.move_emb[mid * ed..(mid + 1) * ed]);
        rows.extend_from_slice(g_row);
    }

    // Everything both forward paths share, up to the context MLP's input.
    fn prep(
        &self,
        ids: &[u32],
        seg_lens: &[u16; NUM_SEGMENTS],
        dense: &[f32; DENSE_DIM],
    ) -> Prep {
        let aw = self.acc_width;
        let r = self.web_rank;
        let mut tokens = vec![0.0f32; NUM_SEGMENTS * aw];
        let mut species = [-1i32; 12];
        let mut active0: i32 = -1;
        let mut active1: i32 = -1;
        let f1_vocab = 4 * TOTAL_SPECIES;
        let mut pos = 0usize;
        for (seg, &len) in seg_lens.iter().enumerate() {
            let tok_off = seg * aw;
            for &id in &ids[pos..pos + len as usize] {
                let idu = id as usize;
                let row = &self.emb[idu * aw..(idu + 1) * aw];
                for k in 0..aw {
                    tokens[tok_off + k] += row[k];
                }
                if seg < 12 && idu < f1_vocab {
                    species[seg] = (idu % TOTAL_SPECIES) as i32;
                    if (idu / TOTAL_SPECIES) % 2 == 0 {
                        if seg < 6 {
                            active0 = seg as i32;
                        } else {
                            active1 = (seg - 6) as i32;
                        }
                    }
                }
            }
            pos += len as usize;
        }
        debug_assert_eq!(pos, ids.len(), "segment lengths must cover the id stream");
        if self.attn_dk != 0 {
            tokens = self.attend(&tokens);
        }

        // the v2 accumulators reach the context MLP raw, unlike the v1 tail's ReLU
        let mut ctx_in = vec![0.0f32; 2 * aw + 2 * r + DENSE_DIM];
        for s in 0..6 {
            for k in 0..aw {
                ctx_in[k] += tokens[s * aw + k];
                ctx_in[aw + k] += tokens[(6 + s) * aw + k];
            }
        }
        for k in 0..aw {
            ctx_in[k] += tokens[12 * aw + k] + tokens[14 * aw + k];
            ctx_in[aw + k] += tokens[13 * aw + k] + tokens[14 * aw + k];
        }

        let mut a = vec![0.0f32; 6 * r];
        let mut b = vec![0.0f32; 6 * r];
        for i in 0..6 {
            let ti = &tokens[i * aw..(i + 1) * aw];
            let tj = &tokens[(6 + i) * aw..(7 + i) * aw];
            for o in 0..r {
                a[i * r + o] = dot(&self.web_a[o * aw..(o + 1) * aw], ti);
                b[i * r + o] = dot(&self.web_b[o * aw..(o + 1) * aw], tj);
            }
        }
        let mut p = vec![0.0f32; 36 * r];
        let mut bias = vec![0.0f32; if self.table_rank != 0 { r } else { 0 }];
        let mut low = vec![0.0f32; self.table_rank];
        for i in 0..6 {
            for j in 0..6 {
                let cell = &mut p[(i * 6 + j) * r..(i * 6 + j + 1) * r];
                for o in 0..r {
                    cell[o] = a[i * r + o] * b[j * r + o];
                }
                if self.table_rank != 0 {
                    self.cell_bias(species[i], species[6 + j], &mut low, &mut bias);
                    for o in 0..r {
                        cell[o] += bias[o];
                    }
                }
                for v in cell.iter_mut() {
                    if *v < 0.0 {
                        *v = 0.0;
                    }
                }
            }
        }
        let mut h_sum = vec![0.0f32; 6 * r];
        let mut h_max = vec![0.0f32; 6 * r];
        for i in 0..6 {
            for j in 0..6 {
                let cell = &p[(i * 6 + j) * r..(i * 6 + j + 1) * r];
                for o in 0..r {
                    h_sum[i * r + o] += cell[o];
                    ctx_in[2 * aw + o] += cell[o];
                    if cell[o] > h_max[i * r + o] {
                        h_max[i * r + o] = cell[o];
                    }
                }
            }
        }
        if active0 >= 0 && active1 >= 0 {
            let idx = (active0 as usize * 6 + active1 as usize) * r;
            ctx_in[2 * aw + r..2 * aw + 2 * r].copy_from_slice(&p[idx..idx + r]);
        }
        ctx_in[2 * aw + 2 * r..].copy_from_slice(dense);
        Prep { tokens, p, h_sum, h_max, ctx_in, active0, active1 }
    }

    // Raw natural logits over bytes 0-13 as [plain, switch, tera]; masking and
    // softmax live outside. The block is an argument because at seat 1 the
    // caller's ids are mirrored while the block must come from the unmirrored root.
    pub fn forward(
        &self,
        ids: &[u32],
        seg_lens: &[u16; NUM_SEGMENTS],
        dense: &[f32; DENSE_DIM],
        move_ids: &[u16; 4],
        action_dense: Option<&[[f32; ACTION_DENSE_DIM]; NUM_ACTIONS]>,
    ) -> [f32; NUM_ACTIONS] {
        let ad = self.action_dense_dim;
        assert_eq!(
            ad != 0,
            action_dense.is_some(),
            "the per-action block must be given exactly when the header sets action_dense"
        );
        let aw = self.acc_width;
        let r = self.web_rank;
        let Prep { tokens, p, h_sum, h_max, ctx_in, active0, active1 } =
            self.prep(ids, seg_lens, dense);
        let mut g = ctx_in;
        for fc in &self.ctx {
            g = fc.apply(&g, true);
        }
        let mut logits = [0.0f32; NUM_ACTIONS];
        let mut sin = Vec::with_capacity(self.sw[0].inp);
        for k in 0..6 {
            sin.clear();
            sin.extend_from_slice(&tokens[k * aw..(k + 1) * aw]);
            sin.extend_from_slice(&h_sum[k * r..(k + 1) * r]);
            sin.extend_from_slice(&h_max[k * r..(k + 1) * r]);
            if active1 >= 0 {
                let a1 = active1 as usize;
                let idx = (k * 6 + a1) * r;
                sin.extend_from_slice(&p[idx..idx + r]);
                sin.extend_from_slice(&tokens[(6 + a1) * aw..(7 + a1) * aw]);
            } else {
                sin.resize(sin.len() + r + aw, 0.0);
            }
            sin.extend_from_slice(&g);
            if ad != 0 {
                sin.extend_from_slice(&action_dense.unwrap()[4 + k]);
            }
            logits[4 + k] = head_scalar(&self.sw, &sin);
        }

        if active0 >= 0 {
            let a0 = active0 as usize;
            let ed = self.move_emb_dim;
            let base_len = aw + 2 * r + ed + self.ctx_dim;
            let mut min = Vec::with_capacity(base_len + ad);
            for (j, &mid) in move_ids.iter().enumerate() {
                let mid = mid as usize;
                min.clear();
                min.extend_from_slice(&tokens[a0 * aw..(a0 + 1) * aw]);
                min.extend_from_slice(&h_sum[a0 * r..(a0 + 1) * r]);
                if active1 >= 0 {
                    let idx = (a0 * 6 + active1 as usize) * r;
                    min.extend_from_slice(&p[idx..idx + r]);
                } else {
                    min.resize(min.len() + r, 0.0);
                }
                min.extend_from_slice(&self.move_emb[mid * ed..(mid + 1) * ed]);
                min.extend_from_slice(&g);
                match action_dense {
                    None => {
                        let plain = head_scalar(&self.mv, &min);
                        logits[j] = plain;
                        logits[10 + j] = plain;
                    }
                    Some(block) => {
                        min.extend_from_slice(&block[j]);
                        logits[j] = head_scalar(&self.mv, &min);
                        min.truncate(base_len);
                        min.extend_from_slice(&block[10 + j]);
                        logits[10 + j] = head_scalar(&self.mv, &min);
                    }
                }
            }
        }
        logits
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

pub fn value_v2_input_len(acc_width: usize, web_rank: usize) -> usize {
    2 * acc_width + 2 * web_rank + DENSE_DIM
}

pub struct LearnedValueV2 {
    acc_width: usize,
    web_rank: usize,
    attn_dk: usize,
    multiplier: f32,
    emb: Vec<f32>,
    web_a: Vec<f32>,
    web_b: Vec<f32>,
    attn_q: Vec<f32>,
    attn_k: Vec<f32>,
    attn_v: Vec<f32>,
    attn_out: Vec<f32>,
    fc: Vec<Fc>,
}

impl LearnedValueV2 {
    pub fn from_env() -> Self {
        let path = std::env::var("BRIDGE_EVAL_WEIGHTS_V2")
            .expect("BRIDGE_EVAL_WEIGHTS_V2 must point at a learned-value-v2 weights file");
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
        if c.take(4)? != b"LVV2" {
            return Err("bad magic: not an LVV2 file".into());
        }
        let spec = c.u32()?;
        if spec != FEATURE_SPEC_VERSION {
            return Err(format!("LVV2 spec {spec} != compiled {FEATURE_SPEC_VERSION}"));
        }
        let vocab = c.u32()? as usize;
        if vocab != features::vocab_size() as usize {
            return Err(format!("vocab mismatch: weights {vocab}, extractor {}", features::vocab_size()));
        }
        let (aw, seg, r) = (c.u32()? as usize, c.u32()? as usize, c.u32()? as usize);
        if seg != NUM_SEGMENTS {
            return Err(format!("LVV2 token_segments {seg} != {NUM_SEGMENTS}"));
        }
        let tables = c.u32()?;
        let (prow, trow, trank) = (c.u32()?, c.u32()?, c.u32()?);
        if tables != 0 {
            return Err("LVV2 carries no pair table; tables flag must be 0".into());
        }
        if prow != 0 || trow != 0 || trank != 0 {
            return Err(format!(
                "LVV2 table dims present with tables off: pair {prow}, type {trow}, rank {trank}"
            ));
        }
        let attn = c.u32()?;
        let dk = c.u32()? as usize;
        if (attn != 0) != (dk != 0) {
            return Err("LVV2 attn flag and attn_dk disagree".into());
        }
        let vdim = c.u32()? as usize;
        // no serving-side extractor exists for the sidecar width
        if vdim != 0 {
            return Err(format!("LVV2 value_dense_dim {vdim} != 0; the value forward has no sidecar input"));
        }
        let n_fc = c.u32()? as usize;
        if n_fc < 2 {
            return Err(format!("LVV2 fc chain needs >= 2 layers, got {n_fc}"));
        }
        let dims = read_dims(&mut c, n_fc)?;
        check_chain(&dims, "LVV2 fc")?;
        let want = value_v2_input_len(aw, r);
        if dims[0].0 != want {
            return Err(format!("LVV2 fc1 in {} != {want}", dims[0].0));
        }
        if dims[dims.len() - 1].1 != 1 {
            return Err("LVV2 fc chain must end in 1 output".into());
        }
        let multiplier = c.f32()?;
        let emb = c.f32_vec(vocab * aw)?;
        let web_a = c.f32_vec(r * aw)?;
        let web_b = c.f32_vec(r * aw)?;
        let (mut q, mut k, mut v, mut o) = (vec![], vec![], vec![], vec![]);
        if attn != 0 {
            q = c.f32_vec(dk * aw)?;
            k = c.f32_vec(dk * aw)?;
            v = c.f32_vec(dk * aw)?;
            o = c.f32_vec(aw * dk)?;
        }
        let fc = read_layers(&mut c, &dims)?;
        if c.pos != bytes.len() {
            return Err("LVV2 trailing bytes after the last tensor".into());
        }
        Ok(Self {
            acc_width: aw,
            web_rank: r,
            attn_dk: dk,
            multiplier,
            emb,
            web_a,
            web_b,
            attn_q: q,
            attn_k: k,
            attn_v: v,
            attn_out: o,
            fc,
        })
    }

    pub fn fc1_input(&self) -> usize {
        self.fc[0].inp
    }

    fn attend(&self, tokens: &[f32]) -> Vec<f32> {
        let aw = self.acc_width;
        let dk = self.attn_dk;
        let mut present = [false; NUM_SEGMENTS];
        for (s, flag) in present.iter_mut().enumerate() {
            *flag = tokens[s * aw..(s + 1) * aw].iter().map(|v| v.abs()).sum::<f32>() > 0.0;
        }
        let mut q = vec![0.0f32; NUM_SEGMENTS * dk];
        let mut key = vec![0.0f32; NUM_SEGMENTS * dk];
        let mut val = vec![0.0f32; NUM_SEGMENTS * dk];
        for s in 0..NUM_SEGMENTS {
            let t = &tokens[s * aw..(s + 1) * aw];
            for o in 0..dk {
                let w = o * aw..(o + 1) * aw;
                q[s * dk + o] = dot(&self.attn_q[w.clone()], t);
                key[s * dk + o] = dot(&self.attn_k[w.clone()], t);
                val[s * dk + o] = dot(&self.attn_v[w], t);
            }
        }
        let scale = (dk as f32).sqrt();
        let mut out = tokens.to_vec();
        let mut w = [0.0f32; NUM_SEGMENTS];
        let mut mix = vec![0.0f32; dk];
        for i in 0..NUM_SEGMENTS {
            if !present[i] {
                continue;
            }
            let qi = &q[i * dk..(i + 1) * dk];
            let mut mx = f32::NEG_INFINITY;
            for j in 0..NUM_SEGMENTS {
                if present[j] {
                    w[j] = dot(qi, &key[j * dk..(j + 1) * dk]) / scale;
                    if w[j] > mx {
                        mx = w[j];
                    }
                }
            }
            let mut sum = 0.0f32;
            for j in 0..NUM_SEGMENTS {
                w[j] = if present[j] { (w[j] - mx).exp() } else { 0.0 };
                sum += w[j];
            }
            for v in mix.iter_mut() {
                *v = 0.0;
            }
            for j in 0..NUM_SEGMENTS {
                if !present[j] {
                    continue;
                }
                let wj = w[j] / sum;
                for o in 0..dk {
                    mix[o] += wj * val[j * dk + o];
                }
            }
            for o in 0..aw {
                out[i * aw + o] += dot(&self.attn_out[o * dk..(o + 1) * dk], &mix);
            }
        }
        out
    }

    fn value_input(
        &self,
        ids: &[u32],
        seg_lens: &[u16; NUM_SEGMENTS],
        dense: &[f32; DENSE_DIM],
    ) -> (Vec<f32>, i32, i32) {
        let aw = self.acc_width;
        let r = self.web_rank;
        let mut tokens = vec![0.0f32; NUM_SEGMENTS * aw];
        let mut active0: i32 = -1;
        let mut active1: i32 = -1;
        let f1_vocab = 4 * TOTAL_SPECIES;
        let mut pos = 0usize;
        for (seg, &len) in seg_lens.iter().enumerate() {
            let tok_off = seg * aw;
            for &id in &ids[pos..pos + len as usize] {
                let idu = id as usize;
                let row = &self.emb[idu * aw..(idu + 1) * aw];
                for k in 0..aw {
                    tokens[tok_off + k] += row[k];
                }
                if seg < 12 && idu < f1_vocab && (idu / TOTAL_SPECIES) % 2 == 0 {
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
        if self.attn_dk != 0 {
            tokens = self.attend(&tokens);
        }

        let mut acc = vec![0.0f32; 2 * aw];
        for s in 0..6 {
            for k in 0..aw {
                acc[k] += tokens[s * aw + k];
                acc[aw + k] += tokens[(6 + s) * aw + k];
            }
        }
        for k in 0..aw {
            acc[k] += tokens[12 * aw + k] + tokens[14 * aw + k];
            acc[aw + k] += tokens[13 * aw + k] + tokens[14 * aw + k];
        }

        let mut a = vec![0.0f32; 6 * r];
        let mut b = vec![0.0f32; 6 * r];
        for i in 0..6 {
            let ti = &tokens[i * aw..(i + 1) * aw];
            let tj = &tokens[(6 + i) * aw..(7 + i) * aw];
            for o in 0..r {
                a[i * r + o] = dot(&self.web_a[o * aw..(o + 1) * aw], ti);
                b[i * r + o] = dot(&self.web_b[o * aw..(o + 1) * aw], tj);
            }
        }
        let mut web_total = vec![0.0f32; r];
        let mut active_cell = vec![0.0f32; r];
        let mut cell = vec![0.0f32; r];
        for i in 0..6 {
            for j in 0..6 {
                for o in 0..r {
                    cell[o] = (a[i * r + o] * b[j * r + o]).max(0.0);
                    web_total[o] += cell[o];
                }
                if active0 == i as i32 && active1 == j as i32 {
                    active_cell.copy_from_slice(&cell);
                }
            }
        }

        // the value tail ReLUs the accumulators; the v2 policy trunk feeds them raw
        let mut x = vec![0.0f32; value_v2_input_len(aw, r)];
        for k in 0..2 * aw {
            x[k] = acc[k].max(0.0);
        }
        x[2 * aw..2 * aw + r].copy_from_slice(&web_total);
        x[2 * aw + r..2 * aw + 2 * r].copy_from_slice(&active_cell);
        x[2 * aw + 2 * r..].copy_from_slice(dense);
        (x, active0, active1)
    }

    pub fn natural_logit(
        &self,
        ids: &[u32],
        seg_lens: &[u16; NUM_SEGMENTS],
        dense: &[f32; DENSE_DIM],
    ) -> f32 {
        let (x, _, _) = self.value_input(ids, seg_lens, dense);
        head_scalar(&self.fc, &x)
    }
}

impl Evaluator for LearnedValueV2 {
    fn eval(&self, state: &BattleState) -> f32 {
        let mut ids = Vec::with_capacity(192);
        let seg_lens = features::extract_segmented(state, &mut ids);
        let dense = features::extract_dense(state);
        self.natural_logit(&ids, &seg_lens, &dense) * self.multiplier
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
    const WEIGHTS_V2: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lvp2-6f1e0facfcc4.bin");
    const FIXTURES_V2: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../learned-eval/weights/lvp2-6f1e0facfcc4.fixtures.json"
    );
    const SPEC3_WEIGHTS: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lv1-db937c18c028.bin");
    const LVV2_WEIGHTS_PATH: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lvv2-phase0.bin");
    const LVV2_FIXTURES_PATH: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lvv2-phase0.fixtures.json");
    const LVV2_ATTN_WEIGHTS_PATH: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../learned-eval/weights/lvv2-phase0-attn.bin");
    const LVV2_ATTN_FIXTURES_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../learned-eval/weights/lvv2-phase0-attn.fixtures.json"
    );

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

    // the resolved paths ride along so the ledger row shows which export was
    // read, including on the skip path
    fn artifacts_v2() -> (Option<(LearnedPolicyV2, serde_json::Value)>, String, String) {
        let w_env = std::env::var("LVP2_WEIGHTS").ok();
        let f_env = std::env::var("LVP2_FIXTURES").ok();
        let wp = w_env.clone().unwrap_or_else(|| WEIGHTS_V2.to_string());
        let fp = f_env.clone().unwrap_or_else(|| FIXTURES_V2.to_string());
        let loaded = resolve_artifact_paths("LVP2", w_env, f_env, WEIGHTS_V2, FIXTURES_V2).map(
            |(bin, fx)| {
                (
                    LearnedPolicyV2::from_bytes(&bin).expect("LVP2 weights must load"),
                    serde_json::from_str(&fx).expect("LVP2 fixtures must parse"),
                )
            },
        );
        (loaded, wp, fp)
    }

    // sibling of artifacts_v2 for the value net; the magic doubles as the env
    // prefix so the bare and attention arms share one resolver
    fn artifacts_v2_value(
        magic: &str,
        w_default: &str,
        f_default: &str,
    ) -> (Option<(LearnedValueV2, serde_json::Value)>, String, String) {
        let w_env = std::env::var(format!("{magic}_WEIGHTS")).ok();
        let f_env = std::env::var(format!("{magic}_FIXTURES")).ok();
        let wp = w_env.clone().unwrap_or_else(|| w_default.to_string());
        let fp = f_env.clone().unwrap_or_else(|| f_default.to_string());
        let loaded =
            resolve_artifact_paths(magic, w_env, f_env, w_default, f_default).map(|(bin, fx)| {
                (
                    LearnedValueV2::from_bytes(&bin).expect("LVV2 weights must load"),
                    serde_json::from_str(&fx).expect("LVV2 fixtures must parse"),
                )
            });
        (loaded, wp, fp)
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

    type BlockOf = Option<[[f32; ACTION_DENSE_DIM]; NUM_ACTIONS]>;

    // sibling of fixture_inputs: same four inputs plus the per-action block,
    // which LVP1's pinned gate has no reader for
    fn fixture_inputs_v2(
        f: &serde_json::Value,
    ) -> (Vec<u32>, [u16; NUM_SEGMENTS], [f32; DENSE_DIM], [u16; 4], BlockOf) {
        let (ids, lens, dense, move_ids) = fixture_inputs(f);
        let block = f.get("action_dense").map(|rows| {
            let rows = rows.as_array().expect("action_dense array");
            assert_eq!(rows.len(), NUM_ACTIONS, "action_dense rows");
            let mut out = [[0.0f32; ACTION_DENSE_DIM]; NUM_ACTIONS];
            for (r, row) in rows.iter().enumerate() {
                let cols = row.as_array().expect("action_dense row");
                assert_eq!(cols.len(), ACTION_DENSE_DIM, "action_dense row {r} width");
                for (c, v) in cols.iter().enumerate() {
                    out[r][c] = v.as_f64().expect("action_dense float") as f32;
                }
            }
            out
        });
        (ids, lens, dense, move_ids, block)
    }

    #[test]
    fn policy_v2_parity_64_fixtures() {
        let (loaded, wpath, fpath) = artifacts_v2();
        let Some((net, fx)) = loaded else {
            eprintln!(
                "SKIP policy v2 parity: fixtures_compared=0 max_abs_diff=inf \
                 action_dense_field=0/0 action_dense_nonzero=0/0 weights={wpath} fixtures={fpath}"
            );
            return;
        };
        let fixtures = fx["fixtures"].as_array().expect("fixtures array");
        assert_eq!(fixtures.len(), 64, "parity gate is 64 fixtures");
        let mut max_diff = 0f64;
        let mut compared = 0usize;
        let mut present = 0usize;
        let mut nonzero = 0usize;
        for (k, f) in fixtures.iter().enumerate() {
            let (ids, lens, dense, move_ids, block) = fixture_inputs_v2(f);
            assert!(
                net.action_dense_dim == 0 || block.is_some(),
                "fixture {k}: the header consumes a per-action block but the fixtures carry none"
            );
            if let Some(b) = &block {
                present += 1;
                if b.iter().flatten().any(|v| *v != 0.0) {
                    nonzero += 1;
                }
            }
            let arg = if net.action_dense_dim == 0 { None } else { block.as_ref() };
            let logits = net.forward(&ids, &lens, &dense, &move_ids, arg);
            let want = f["logits"].as_array().expect("14 logits");
            assert_eq!(want.len(), NUM_ACTIONS);
            for (i, w) in want.iter().enumerate() {
                let d = (logits[i] as f64 - w.as_f64().unwrap()).abs();
                if d > max_diff {
                    max_diff = d;
                }
            }
            compared += 1;
        }
        eprintln!(
            "policy v2 parity: fixtures_compared={compared} max_abs_diff={max_diff:.3e} \
             action_dense_field={present}/{compared} action_dense_nonzero={nonzero}/{compared}={:.4} \
             weights={wpath} fixtures={fpath}",
            nonzero as f64 / compared as f64
        );
        assert!(max_diff <= 1e-4, "v2 logit parity {max_diff:e} exceeds 1e-4");
    }

    fn value_v2_parity(label: &str, magic: &str, w_default: &str, f_default: &str) {
        let (loaded, wpath, fpath) = artifacts_v2_value(magic, w_default, f_default);
        let Some((net, fx)) = loaded else {
            eprintln!(
                "SKIP {label} parity: fixtures_compared=0 max_abs_diff=inf distinct=0 \
                 active0_ok=0 active1_ok=0 web_total_nz=0 active_cell_nz=0 \
                 weights={wpath} fixtures={fpath}"
            );
            return;
        };
        let fixtures = fx["fixtures"].as_array().expect("fixtures array");
        assert_eq!(fixtures.len(), 64, "parity gate is 64 fixtures");
        let aw = net.acc_width;
        let r = net.web_rank;
        let mut seen: Vec<Vec<u32>> = Vec::new();
        let mut active0_ok = 0usize;
        let mut active1_ok = 0usize;
        let mut web_total_nz = 0usize;
        let mut active_cell_nz = 0usize;
        let mut max_diff = 0f64;
        let mut compared = 0usize;
        for f in fixtures {
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
            let (x, active0, active1) = net.value_input(&ids, &seg, &dense);
            if !seen.contains(&ids) {
                seen.push(ids.clone());
            }
            active0_ok += usize::from(active0 >= 0);
            active1_ok += usize::from(active1 >= 0);
            web_total_nz += usize::from(x[2 * aw..2 * aw + r].iter().any(|v| *v != 0.0));
            active_cell_nz += usize::from(x[2 * aw + r..2 * aw + 2 * r].iter().any(|v| *v != 0.0));
            let got = net.natural_logit(&ids, &seg, &dense);
            let d = (got as f64 - f["logit"].as_f64().unwrap()).abs();
            if d > max_diff {
                max_diff = d;
            }
            compared += 1;
        }
        let distinct = seen.len();
        eprintln!(
            "{label} parity: fixtures_compared={compared} max_abs_diff={max_diff:.3e} \
             distinct={distinct} active0_ok={active0_ok} active1_ok={active1_ok} \
             web_total_nz={web_total_nz} active_cell_nz={active_cell_nz} \
             weights={wpath} fixtures={fpath}"
        );
        assert_eq!(distinct, 64, "{label}: fixtures must be 64 distinct positions");
        assert_eq!(active0_ok, 64, "{label}: every fixture needs an active seat 0");
        assert_eq!(active1_ok, 64, "{label}: every fixture needs an active seat 1");
        assert_eq!(web_total_nz, 64, "{label}: every fixture needs a nonzero web total");
        assert!(active_cell_nz >= 32, "{label}: active cell nonzero on {active_cell_nz} < 32");
        assert_eq!(compared, 64, "{label}: 64 fixtures compared");
        assert!(max_diff <= 1e-4, "{label} parity {max_diff:e} > 1e-4");
    }

    #[test]
    fn value_v2_parity_64_fixtures() {
        value_v2_parity("value v2", "LVV2", LVV2_WEIGHTS_PATH, LVV2_FIXTURES_PATH);
    }

    #[test]
    fn value_v2_parity_64_fixtures_attn() {
        value_v2_parity(
            "value v2 attn",
            "LVV2_ATTN",
            LVV2_ATTN_WEIGHTS_PATH,
            LVV2_ATTN_FIXTURES_PATH,
        );
    }

    // eight varied roots, both orientations, so the batch exercises the
    // active0/active1 branches rather than one repeated position
    fn batch_states() -> Vec<BattleState> {
        let specs: [(u16, u16, [u16; 4]); 4] = [
            (25, 9, [85, 150, 33, 34]),
            (445, 24, [89, 14, 33, 0]),
            (130, 22, [57, 85, 0, 0]),
            (143, 47, [34, 89, 0, 0]),
        ];
        let mut out = Vec::new();
        for k in 0..4 {
            let (a, b, c) = specs[k];
            let (d, e, f) = specs[(k + 1) % 4];
            let (s, _t) = build_state(vec![mon(a, b, c)], vec![mon(d, e, f)]);
            out.push(s);
            out.push(features::mirror(&s));
        }
        out
    }

    fn single_inputs(
        state: &BattleState,
    ) -> (Vec<u32>, [u16; NUM_SEGMENTS], [f32; DENSE_DIM], [u16; 4], [[f32; ACTION_DENSE_DIM]; NUM_ACTIONS])
    {
        let mut ids = Vec::new();
        let lens = features::extract_segmented(state, &mut ids);
        let dense = features::extract_dense(state);
        let side = &state.sides[0];
        let move_ids = side.team[side.active_index as usize].moves;
        let block = crate::action_features::action_features(state, 0);
        (ids, lens, dense, move_ids, block)
    }

    fn assert_batch_matches_single(
        net: &LearnedPolicyV2,
        rows: &[(Vec<u32>, [u16; NUM_SEGMENTS], [f32; DENSE_DIM], [u16; 4], BlockOf)],
        tag: &str,
    ) {
        let items: Vec<ForwardInput> = rows
            .iter()
            .map(|(ids, lens, dense, move_ids, block)| ForwardInput {
                ids,
                seg_lens: lens,
                dense,
                move_ids,
                action_dense: if net.action_dense_dim == 0 { None } else { block.as_ref() },
            })
            .collect();
        let batched = net.forward_batch(&items);
        assert_eq!(batched.len(), items.len(), "{tag}: one output row per input");
        for (k, it) in items.iter().enumerate() {
            let single =
                net.forward(it.ids, it.seg_lens, it.dense, it.move_ids, it.action_dense);
            for i in 0..NUM_ACTIONS {
                assert_eq!(
                    batched[k][i].to_bits(),
                    single[i].to_bits(),
                    "{tag}: batch row {k} logit {i} is not bit-identical to the single forward"
                );
            }
        }
    }

    #[test]
    fn policy_v2_batched_forward_matches_single_synthetic() {
        let f = PolicyFileV2::small();
        let net = LearnedPolicyV2::from_bytes(&f.bytes()).expect("synthetic LVP2 must load");
        let states = batch_states();
        let rows: Vec<_> = states
            .iter()
            .map(|s| {
                let (ids, lens, dense, move_ids, block) = single_inputs(s);
                (ids, lens, dense, move_ids, Some(block))
            })
            .collect();
        assert_eq!(rows.len(), 8, "batch is the eight-world serve shape");
        assert_batch_matches_single(&net, &rows, "synthetic");
    }

    #[test]
    fn policy_v2_batched_forward_matches_single_on_fixtures() {
        let (loaded, wpath, fpath) = artifacts_v2();
        let Some((net, fx)) = loaded else {
            eprintln!("SKIP policy v2 batched forward: weights={wpath} fixtures={fpath}");
            return;
        };
        let fixtures = fx["fixtures"].as_array().expect("fixtures array");
        assert_eq!(fixtures.len(), 64, "parity gate is 64 fixtures");
        for (c, chunk) in fixtures.chunks(8).enumerate() {
            let rows: Vec<_> = chunk.iter().map(fixture_inputs_v2).collect();
            assert_batch_matches_single(&net, &rows, &format!("fixtures chunk {c}"));
        }
    }

    // Measurement only: the eight-world per-turn cost as one batched call against
    // eight single calls. Release build; the ratio is a reported fact, not a gate.
    #[test]
    #[ignore]
    fn policy_v2_batch_speedup() {
        let (loaded, wpath, fpath) = artifacts_v2();
        let Some((net, fx)) = loaded else {
            eprintln!("SKIP policy v2 batch speedup: weights={wpath} fixtures={fpath}");
            return;
        };
        let fixtures = fx["fixtures"].as_array().expect("fixtures array");
        let rows: Vec<_> = fixtures[..8].iter().map(fixture_inputs_v2).collect();
        let items: Vec<ForwardInput> = rows
            .iter()
            .map(|(ids, lens, dense, move_ids, block)| ForwardInput {
                ids,
                seg_lens: lens,
                dense,
                move_ids,
                action_dense: if net.action_dense_dim == 0 { None } else { block.as_ref() },
            })
            .collect();
        let reps = 5usize;
        let iters = 100u32;
        let mut single = Vec::with_capacity(reps);
        let mut batched = Vec::with_capacity(reps);
        for _ in 0..5 {
            std::hint::black_box(net.forward_batch(&items));
        }
        for _ in 0..reps {
            let t0 = std::time::Instant::now();
            for _ in 0..iters {
                for it in &items {
                    std::hint::black_box(net.forward(
                        it.ids,
                        it.seg_lens,
                        it.dense,
                        it.move_ids,
                        it.action_dense,
                    ));
                }
            }
            single.push(t0.elapsed().as_secs_f64() * 1e6 / iters as f64);
            let t1 = std::time::Instant::now();
            for _ in 0..iters {
                std::hint::black_box(net.forward_batch(&items));
            }
            batched.push(t1.elapsed().as_secs_f64() * 1e6 / iters as f64);
        }
        let raw = |v: &[f64]| v.iter().map(|t| format!("{t:.1}")).collect::<Vec<_>>().join(", ");
        let (sr, br) = (raw(&single), raw(&batched));
        let s = median_us(single);
        let b = median_us(batched);
        eprintln!(
            "policy v2 batch: worlds=8 single={s:.1} us batched={b:.1} us factor={:.3}x \
             single_raw=[{sr}] batched_raw=[{br}] weights={wpath}",
            s / b
        );
    }

    // Measurement only: the per-action block's two denominators, over fixtures
    // and over legal action bytes, plus the cause of every zero block.
    #[cfg(feature = "train_value")]
    #[test]
    #[ignore]
    fn action_dense_block_census() {
        use crate::policy_label::read_records_guarded;
        use pkmn_engine::state::legal_actions;
        let (loaded, wpath, fpath) = artifacts_v2();
        let Some((_net, fx)) = loaded else {
            eprintln!("SKIP action_dense census: weights={wpath} fixtures={fpath}");
            return;
        };
        let dir = fx["source"]["source_dir"].as_str().expect("source_dir");
        let fixtures = fx["fixtures"].as_array().expect("fixtures array");
        let (mut legal_bytes, mut legal_nonzero, mut fixture_nonzero) = (0usize, 0usize, 0usize);
        for (k, f) in fixtures.iter().enumerate() {
            let block = fixture_inputs_v2(f).4.expect("census needs the per-action block");
            let name = f["file"].as_str().unwrap();
            let idx = f["index"].as_u64().unwrap() as usize;
            let recs = read_records_guarded(&format!("{dir}/{name}")).expect("guarded read");
            let state = &recs[idx].state;
            let legal: Vec<usize> = legal_actions(state, 0)
                .as_slice()
                .iter()
                .map(|&a| a as usize)
                .filter(|&a| a < NUM_ACTIONS)
                .collect();
            let mut zero_legal = Vec::new();
            for &a in &legal {
                legal_bytes += 1;
                if block[a].iter().any(|v| *v != 0.0) {
                    legal_nonzero += 1;
                } else {
                    zero_legal.push(a);
                }
            }
            if block.iter().flatten().any(|v| *v != 0.0) {
                fixture_nonzero += 1;
            } else {
                eprintln!("census zero block: fixture {k} {name} #{idx} phase {} legal {legal:?}", state.phase);
            }
            if !zero_legal.is_empty() {
                eprintln!("census zero legal bytes: fixture {k} {zero_legal:?}");
            }
        }
        let n = fixtures.len();
        eprintln!(
            "action_dense census: fixture_level={fixture_nonzero}/{n}={:.4} legal_byte_level={legal_nonzero}/{legal_bytes}={:.4} fixtures={fpath}",
            fixture_nonzero as f64 / n as f64,
            legal_nonzero as f64 / legal_bytes as f64
        );
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

    fn median_us(mut v: Vec<f64>) -> f64 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    }

    #[cfg(feature = "train_value")]
    fn timed_us(label: &str, reps: usize, iters: u32, mut body: impl FnMut()) -> f64 {
        let mut takes = Vec::with_capacity(reps);
        for _ in 0..reps {
            let t0 = std::time::Instant::now();
            for _ in 0..iters {
                body();
            }
            takes.push(t0.elapsed().as_secs_f64() * 1e6 / iters as f64);
        }
        let raw: Vec<String> = takes.iter().map(|t| format!("{t:.3}")).collect();
        let med = median_us(takes);
        eprintln!("  {label}: median={med:.3} us raw=[{}]", raw.join(", "));
        med
    }

    // The wall-fairness quantity is everything the serve path runs per world,
    // so the extractors are inside the measured region, not beside it.
    #[cfg(feature = "train_value")]
    #[test]
    #[ignore]
    fn policy_v2_serve_path_per_forward() {
        use crate::action_features::action_features;
        use crate::policy_label::read_records_guarded;
        let (loaded, wpath, fpath) = artifacts_v2();
        let Some((net, fx)) = loaded else {
            eprintln!("SKIP policy v2 serve path: weights={wpath} fixtures={fpath}");
            return;
        };
        let dir = fx["source"]["source_dir"].as_str().expect("source_dir");
        let f = &fx["fixtures"].as_array().expect("fixtures array")[0];
        let name = f["file"].as_str().unwrap();
        let idx = f["index"].as_u64().unwrap() as usize;
        let recs = read_records_guarded(&format!("{dir}/{name}")).expect("guarded read");
        let state = recs[idx].state;

        let reps = 5usize;
        let iters = 100_000u32;
        let mut ids = Vec::with_capacity(512);
        let mut sink = 0f32;

        let serve = |ids: &mut Vec<u32>| -> f32 {
            ids.clear();
            let lens = features::extract_segmented(&state, ids);
            let dense = features::extract_dense(&state);
            let block = action_features(&state, 0);
            let side = &state.sides[0];
            let move_ids = side.team[side.active_index as usize].moves;
            let arg = if net.action_dense_dim == 0 { None } else { Some(&block) };
            let logits = net.forward(ids, &lens, &dense, &move_ids, arg);
            std::hint::black_box(logits)[0]
        };
        for _ in 0..1_000 {
            sink += serve(&mut ids);
        }

        eprintln!("policy v2 serve path: reps={reps} iters={iters} weights={wpath}");
        let mut whole = Vec::with_capacity(reps);
        for _ in 0..reps {
            let t0 = std::time::Instant::now();
            for _ in 0..iters {
                sink += serve(&mut ids);
            }
            let us = t0.elapsed().as_secs_f64() * 1e6 / iters as f64;
            eprintln!("  serve rep: {us:.3} us");
            whole.push(us);
        }
        let raw: Vec<String> = whole.iter().map(|t| format!("{t:.3}")).collect();
        let whole_med = median_us(whole);

        let seg = timed_us("extract_segmented", reps, iters, || {
            ids.clear();
            std::hint::black_box(features::extract_segmented(&state, &mut ids));
        });
        let dn = timed_us("extract_dense", reps, iters, || {
            std::hint::black_box(features::extract_dense(&state));
        });
        let af = timed_us("action_features", reps, iters, || {
            std::hint::black_box(action_features(&state, 0));
        });

        ids.clear();
        let lens = features::extract_segmented(&state, &mut ids);
        let dense = features::extract_dense(&state);
        let block = action_features(&state, 0);
        let side = &state.sides[0];
        let move_ids = side.team[side.active_index as usize].moves;
        let arg = if net.action_dense_dim == 0 { None } else { Some(&block) };
        let fwd = timed_us("net_forward", reps, iters, || {
            std::hint::black_box(net.forward(&ids, &lens, &dense, &move_ids, arg));
        });

        eprintln!(
            "policy v2 serve path: median={whole_med:.3} us raw=[{}] \
             extract_segmented={seg:.3} extract_dense={dn:.3} action_features={af:.3} \
             net_forward={fwd:.3} parts_sum={:.3} sink={sink} weights={wpath} fixtures={fpath}",
            raw.join(", "),
            seg + dn + af + fwd
        );
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

    struct ValueFileV2 {
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
        value_dense_dim: u32,
        fc: Vec<(u32, u32)>,
        multiplier: f32,
        truncate: usize,
        trailing: usize,
    }

    impl ValueFileV2 {
        fn small() -> Self {
            let acc = 3u32;
            let r = 2u32;
            let want = 2 * acc + 2 * r + DENSE_DIM as u32;
            ValueFileV2 {
                magic: *b"LVV2",
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
                value_dense_dim: 0,
                fc: vec![(want, 4), (4, 1)],
                multiplier: 2.5,
                truncate: 0,
                trailing: 0,
            }
        }

        // acc_width and attn_dk differ so a wrongly squared attn_out size cannot pass
        fn attention() -> Self {
            let acc = 6u32;
            let r = 2u32;
            let want = 2 * acc + 2 * r + DENSE_DIM as u32;
            ValueFileV2 {
                acc,
                attn: 1,
                attn_dk: 5,
                fc: vec![(want, 3), (3, 2), (2, 1)],
                ..ValueFileV2::small()
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
                self.value_dense_dim,
                self.fc.len() as u32,
            ] {
                v.extend_from_slice(&u.to_le_bytes());
            }
            for &(i, o) in &self.fc {
                v.extend_from_slice(&i.to_le_bytes());
                v.extend_from_slice(&o.to_le_bytes());
            }
            v.extend_from_slice(&self.multiplier.to_le_bytes());
            let mut n_f32 = (self.vocab * self.acc + 2 * self.web_rank * self.acc) as usize;
            if self.attn != 0 {
                n_f32 += 4 * (self.attn_dk * self.acc) as usize;
            }
            for &(i, o) in &self.fc {
                n_f32 += (i * o + o) as usize;
            }
            for k in 0..n_f32 {
                v.extend_from_slice(&lvv2_payload(k).to_le_bytes());
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
            match LearnedValueV2::from_bytes(&self.bytes()) {
                Ok(_) => panic!("malformed value weights must be refused"),
                Err(e) => e,
            }
        }
    }

    fn lvv2_payload(k: usize) -> f32 {
        ((k % 13) as f32 - 6.0) * 0.01
    }

    fn lvv2_slice(start: usize, len: usize) -> Vec<f32> {
        (start..start + len).map(lvv2_payload).collect()
    }

    fn lvv2_header_bytes(acc: u32, web_rank: u32, fc0_in: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"LVV2");
        for u in [
            FEATURE_SPEC_VERSION,
            features::vocab_size(),
            acc,
            NUM_SEGMENTS as u32,
            web_rank,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            2,
        ] {
            v.extend_from_slice(&u.to_le_bytes());
        }
        for (i, o) in [(fc0_in, 4u32), (4, 1)] {
            v.extend_from_slice(&i.to_le_bytes());
            v.extend_from_slice(&o.to_le_bytes());
        }
        v
    }

    fn lvv2_header_err(acc: u32, web_rank: u32, fc0_in: u32) -> String {
        match LearnedValueV2::from_bytes(&lvv2_header_bytes(acc, web_rank, fc0_in)) {
            Ok(_) => panic!("a tensorless header must be refused"),
            Err(e) => e,
        }
    }

    #[test]
    fn value_v2_input_len_matches_exported_widths() {
        assert_eq!(value_v2_input_len(8, 2), 27);
        assert_eq!(value_v2_input_len(128, 32), 327);
        assert_eq!(value_v2_input_len(128, 64), 391);
    }

    #[test]
    fn value_v2_loader_wants_input_len_at_exported_widths() {
        for (acc, r, want) in [(8u32, 2u32, 27u32), (128, 32, 327), (128, 64, 391)] {
            let short = lvv2_header_err(acc, r, want - 1);
            assert_eq!(short, format!("LVV2 fc1 in {} != {want}", want - 1));
            let exact = lvv2_header_err(acc, r, want);
            assert!(exact.starts_with("truncated weights file at byte"), "{exact}");
        }
    }

    #[test]
    fn value_v2_bare_arm_loads_tensors_in_order() {
        let f = ValueFileV2::small();
        let net = LearnedValueV2::from_bytes(&f.bytes()).expect("synthetic LVV2 must load");
        let (vocab, acc, r) = (f.vocab as usize, f.acc as usize, f.web_rank as usize);
        assert_eq!(net.acc_width, acc);
        assert_eq!(net.web_rank, r);
        assert_eq!(net.attn_dk, 0);
        assert_eq!(net.multiplier, f.multiplier);
        assert!(net.attn_q.is_empty(), "bare arm must carry no attn_q");
        assert!(net.attn_k.is_empty(), "bare arm must carry no attn_k");
        assert!(net.attn_v.is_empty(), "bare arm must carry no attn_v");
        assert!(net.attn_out.is_empty(), "bare arm must carry no attn_out");
        let mut off = 0;
        assert_eq!(net.emb, lvv2_slice(off, vocab * acc));
        off += vocab * acc;
        assert_eq!(net.web_a, lvv2_slice(off, r * acc));
        off += r * acc;
        assert_eq!(net.web_b, lvv2_slice(off, r * acc));
        off += r * acc;
        assert_eq!(net.fc.len(), f.fc.len());
        for (li, &(i, o)) in f.fc.iter().enumerate() {
            let (i, o) = (i as usize, o as usize);
            assert_eq!(net.fc[li].w, lvv2_slice(off, i * o), "fc{li} weight");
            off += i * o;
            assert_eq!(net.fc[li].b, lvv2_slice(off, o), "fc{li} bias");
            off += o;
        }
        assert_eq!(net.fc1_input(), value_v2_input_len(net.acc_width, net.web_rank));
    }

    #[test]
    fn value_v2_attention_arm_loads_tensors_in_order() {
        let f = ValueFileV2::attention();
        let net = LearnedValueV2::from_bytes(&f.bytes()).expect("synthetic attention LVV2 must load");
        let (vocab, acc, r, dk) =
            (f.vocab as usize, f.acc as usize, f.web_rank as usize, f.attn_dk as usize);
        assert_ne!(acc, dk, "a square attn block would hide a wrong attn_out size");
        assert_eq!(net.acc_width, acc);
        assert_eq!(net.attn_dk, dk);
        let mut off = vocab * acc + 2 * r * acc;
        assert_eq!(net.attn_q, lvv2_slice(off, dk * acc));
        off += dk * acc;
        assert_eq!(net.attn_k, lvv2_slice(off, dk * acc));
        off += dk * acc;
        assert_eq!(net.attn_v, lvv2_slice(off, dk * acc));
        off += dk * acc;
        assert_eq!(net.attn_out, lvv2_slice(off, acc * dk));
        assert_eq!(net.attn_out.len(), acc * dk);
        off += acc * dk;
        assert_ne!(net.attn_q, net.attn_k, "the fixture must separate q from k");
        assert_ne!(net.attn_k, net.attn_v, "the fixture must separate k from v");
        assert_ne!(net.attn_q, net.attn_v, "the fixture must separate q from v");
        assert_eq!(net.fc.len(), f.fc.len());
        for (li, &(i, o)) in f.fc.iter().enumerate() {
            let (i, o) = (i as usize, o as usize);
            assert_eq!(net.fc[li].w, lvv2_slice(off, i * o), "fc{li} weight");
            off += i * o;
            assert_eq!(net.fc[li].b, lvv2_slice(off, o), "fc{li} bias");
            off += o;
        }
        assert_eq!(net.fc1_input(), value_v2_input_len(net.acc_width, net.web_rank));
    }

    #[test]
    fn value_v2_eval_is_scaled_forward_of_extracted_features() {
        let f = ValueFileV2 { multiplier: 7.5, ..ValueFileV2::small() };
        let net = LearnedValueV2::from_bytes(&f.bytes()).expect("synthetic LVV2 must load");
        let (s, _t) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0])],
        );
        let mut ids = Vec::new();
        let seg_lens = features::extract_segmented(&s, &mut ids);
        let dense = features::extract_dense(&s);
        let raw = net.natural_logit(&ids, &seg_lens, &dense);
        assert!(raw.is_finite());
        assert_ne!(raw, 0.0, "forward must produce a real logit on a live state");
        assert_eq!(net.eval(&s), raw * 7.5, "eval must apply the export multiplier");
        assert_eq!(crate::driver::EvalKind::LearnedV2(&net).name(), "learned_v2");
    }

    #[test]
    fn value_v2_bad_magic_refused() {
        let mut f = ValueFileV2::small();
        f.magic = *b"LVP2";
        assert_eq!(f.err(), "bad magic: not an LVV2 file");
    }

    #[test]
    fn value_v2_spec_mismatch_refused() {
        let mut f = ValueFileV2::small();
        f.spec = FEATURE_SPEC_VERSION - 1;
        assert_eq!(f.err(), format!("LVV2 spec {} != compiled {FEATURE_SPEC_VERSION}", f.spec));
    }

    #[test]
    fn value_v2_vocab_mismatch_refused() {
        let mut f = ValueFileV2::small();
        f.vocab += 1;
        assert_eq!(
            f.err(),
            format!("vocab mismatch: weights {}, extractor {}", f.vocab, features::vocab_size())
        );
    }

    #[test]
    fn value_v2_segment_count_mismatch_refused() {
        let mut f = ValueFileV2::small();
        f.segments = NUM_SEGMENTS as u32 - 1;
        assert_eq!(f.err(), format!("LVV2 token_segments {} != {NUM_SEGMENTS}", f.segments));
    }

    #[test]
    fn value_v2_tables_flag_refused() {
        let mut f = ValueFileV2::small();
        f.tables = 1;
        assert_eq!(f.err(), "LVV2 carries no pair table; tables flag must be 0");
    }

    #[test]
    fn value_v2_pair_rows_without_tables_refused() {
        let mut f = ValueFileV2::small();
        f.pair_rows = 4;
        assert_eq!(f.err(), "LVV2 table dims present with tables off: pair 4, type 0, rank 0");
    }

    #[test]
    fn value_v2_type_rows_without_tables_refused() {
        let mut f = ValueFileV2::small();
        f.type_rows = 3;
        assert_eq!(f.err(), "LVV2 table dims present with tables off: pair 0, type 3, rank 0");
    }

    #[test]
    fn value_v2_table_rank_without_tables_refused() {
        let mut f = ValueFileV2::small();
        f.table_rank = 2;
        assert_eq!(f.err(), "LVV2 table dims present with tables off: pair 0, type 0, rank 2");
    }

    #[test]
    fn value_v2_attn_without_dk_refused() {
        let mut f = ValueFileV2::small();
        f.attn = 1;
        assert_eq!(f.err(), "LVV2 attn flag and attn_dk disagree");
    }

    #[test]
    fn value_v2_attn_dk_without_attn_refused() {
        let mut f = ValueFileV2::small();
        f.attn_dk = 4;
        assert_eq!(f.err(), "LVV2 attn flag and attn_dk disagree");
    }

    #[test]
    fn value_v2_value_dense_dim_refused() {
        let mut f = ValueFileV2::small();
        f.value_dense_dim = DENSE_DIM as u32;
        assert_eq!(
            f.err(),
            format!(
                "LVV2 value_dense_dim {DENSE_DIM} != 0; the value forward has no sidecar input"
            )
        );
    }

    #[test]
    fn value_v2_single_fc_layer_refused() {
        let mut f = ValueFileV2::small();
        f.fc = vec![(f.fc[0].0, 1)];
        assert_eq!(f.err(), "LVV2 fc chain needs >= 2 layers, got 1");
    }

    #[test]
    fn value_v2_zero_fc_layers_refused() {
        let mut f = ValueFileV2::small();
        f.fc = vec![];
        assert_eq!(f.err(), "LVV2 fc chain needs >= 2 layers, got 0");
    }

    #[test]
    fn value_v2_fc1_input_dim_refused() {
        let mut f = ValueFileV2::small();
        let want = f.fc[0].0;
        f.fc[0].0 += 1;
        assert_eq!(f.err(), format!("LVV2 fc1 in {} != {want}", want + 1));
    }

    #[test]
    fn value_v2_fc_chain_break_refused() {
        let mut f = ValueFileV2::small();
        f.fc[0].1 += 1;
        assert!(f.err().starts_with("LVV2 fc dim chain break"), "{}", f.err());
    }

    #[test]
    fn value_v2_fc_output_dim_refused() {
        let mut f = ValueFileV2::small();
        f.fc.last_mut().unwrap().1 = 2;
        assert_eq!(f.err(), "LVV2 fc chain must end in 1 output");
    }

    #[test]
    fn value_v2_trailing_bytes_refused() {
        let mut f = ValueFileV2::small();
        f.trailing = 4;
        assert_eq!(f.err(), "LVV2 trailing bytes after the last tensor");
    }

    #[test]
    fn value_v2_truncated_refused() {
        let mut f = ValueFileV2::small();
        f.truncate = 4;
        assert!(f.err().starts_with("truncated weights file at byte"), "{}", f.err());
    }
}
