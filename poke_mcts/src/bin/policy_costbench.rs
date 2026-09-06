use pkmn_engine::data::moves::MoveCategory;
use pkmn_engine::data::GEN_MOVES;
use pkmn_engine::state::calc::calc_damage;
use pkmn_engine::state::data_bridge::move_hot;
use pkmn_engine::state::{effective_moves, BattleState, PHASE_ACTIONS};
use poke_mcts::eval_learned::NUM_ACTIONS;
use poke_mcts::features::{DENSE_DIM, NUM_SEGMENTS};
use poke_mcts::policies::median_roll;
use std::hint::black_box;
use std::io::Read;
use std::time::Instant;

const ACC: usize = 128;
const VOCAB: usize = 80973;
const MOVE_EMB_DIM: usize = 16;
const DK: usize = 32;
const TAB_DIM: usize = 16;
const PAIR_ROWS: usize = 1454 * 1454;
const TYPE_ROWS: usize = 18 * 18;
const ACTION_DENSE_DIM: usize = 8;
const BATCH: usize = 8;
const SLOTS: usize = 6;
const MOVE_SLOTS: usize = 4;
const SWITCH_BASE: usize = 4;
const TERA_BASE: usize = 10;
const MOVE_IDS_ROW_BYTES: usize = MOVE_SLOTS * 2;
const ACT0: usize = 0;
const ACT1: usize = 0;
const SEG_LENS: [u16; NUM_SEGMENTS] = [8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 0, 0, 1];
const N_IDS: usize = {
    let mut s = 0;
    let mut i = 0;
    while i < NUM_SEGMENTS {
        s += SEG_LENS[i] as usize;
        i += 1;
    }
    s
};
const WEIGHT_SEED: u64 = 0xC057_BE0C_1234_5678;
const INPUT_SEED: u64 = 0xC057_BE0C_A5A5_0F0F;
const TAB_INDEX_SEED: u64 = 0xC057_BE0C_7E57_1D0F;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn weight(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.1
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn fill(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.weight()).collect()
    }
}

struct Fc {
    inp: usize,
    out: usize,
    w: Vec<f32>,
    b: Vec<f32>,
}

impl Fc {
    fn new(inp: usize, out: usize, rng: &mut Rng) -> Self {
        Fc { inp, out, w: rng.fill(inp * out), b: rng.fill(out) }
    }

    fn apply(&self, x: &[f32], relu: bool) -> Vec<f32> {
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

    fn apply_batch(&self, xt: &[[f32; BATCH]], relu: bool) -> Vec<[f32; BATCH]> {
        let mut ys = vec![[0.0f32; BATCH]; self.out];
        for (o, y) in ys.iter_mut().enumerate() {
            let w = &self.w[o * self.inp..(o + 1) * self.inp];
            let mut acc = [self.b[o]; BATCH];
            for k in 0..self.inp {
                let wk = w[k];
                let col = &xt[k];
                for bi in 0..BATCH {
                    acc[bi] += col[bi] * wk;
                }
            }
            if relu {
                for a in acc.iter_mut() {
                    if *a < 0.0 {
                        *a = 0.0;
                    }
                }
            }
            *y = acc;
        }
        ys
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

fn run_head_batch(layers: &[Fc], xt: Vec<[f32; BATCH]>) -> Vec<[f32; BATCH]> {
    let last = layers.len() - 1;
    let mut cur = xt;
    for (i, fc) in layers.iter().enumerate() {
        cur = fc.apply_batch(&cur, i < last);
    }
    cur
}

fn interleave(xs: &[Vec<f32>]) -> Vec<[f32; BATCH]> {
    let mut out = vec![[0.0f32; BATCH]; xs[0].len()];
    for (bi, x) in xs.iter().enumerate() {
        for (k, &v) in x.iter().enumerate() {
            out[k][bi] = v;
        }
    }
    out
}

fn deinterleave(xt: &[[f32; BATCH]]) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0f32; xt.len()]; BATCH];
    for (k, col) in xt.iter().enumerate() {
        for bi in 0..BATCH {
            out[bi][k] = col[bi];
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Topo {
    V1,
    V2,
}

struct Config {
    name: String,
    topo: Topo,
    web_rank: usize,
    ctx_dim: usize,
    head_hidden: Vec<usize>,
    action_dense: bool,
    tables: bool,
    attn: bool,
}

impl Config {
    fn ctx_in_len(&self) -> usize {
        2 * ACC + 2 * self.web_rank + DENSE_DIM
    }

    fn switch_in(&self) -> usize {
        match self.topo {
            Topo::V1 => ACC + self.web_rank + self.ctx_dim,
            Topo::V2 => {
                ACC + 3 * self.web_rank + ACC + self.ctx_dim
                    + if self.action_dense { ACTION_DENSE_DIM } else { 0 }
            }
        }
    }

    fn move_in(&self) -> usize {
        match self.topo {
            Topo::V1 => ACC + self.web_rank + MOVE_EMB_DIM + self.ctx_dim,
            Topo::V2 => {
                ACC + 2 * self.web_rank + MOVE_EMB_DIM + self.ctx_dim
                    + if self.action_dense { ACTION_DENSE_DIM } else { 0 }
            }
        }
    }

    fn move_out(&self) -> usize {
        match self.topo {
            Topo::V1 => 2,
            Topo::V2 => 1,
        }
    }

    fn move_evals(&self) -> usize {
        match self.topo {
            Topo::V1 => MOVE_SLOTS,
            Topo::V2 => 2 * MOVE_SLOTS,
        }
    }

    fn head_dims(&self, in_w: usize, out: usize) -> Vec<(usize, usize)> {
        let mut dims = Vec::with_capacity(self.head_hidden.len() + 1);
        let mut cur = in_w;
        for &h in &self.head_hidden {
            dims.push((cur, h));
            cur = h;
        }
        dims.push((cur, out));
        dims
    }

    fn macs(&self) -> u64 {
        let r = self.web_rank;
        let c = self.ctx_dim;
        let mut m = (N_IDS * ACC) as u64;
        m += (2 * SLOTS * r * ACC) as u64;
        m += (SLOTS * SLOTS * r) as u64;
        if self.tables {
            m += (SLOTS * SLOTS * TAB_DIM * r) as u64;
        }
        if self.attn {
            let s = NUM_SEGMENTS;
            m += (3 * s * DK * ACC + 2 * s * s * DK + s * ACC * DK) as u64;
        }
        m += (self.ctx_in_len() * c + c * c) as u64;
        if self.topo == Topo::V1 {
            m += c as u64;
        }
        let chain = |dims: Vec<(usize, usize)>| -> u64 {
            dims.iter().map(|&(i, o)| (i * o) as u64).sum()
        };
        m += SLOTS as u64 * chain(self.head_dims(self.switch_in(), 1));
        m += self.move_evals() as u64 * chain(self.head_dims(self.move_in(), self.move_out()));
        m
    }
}

fn matrix() -> Vec<Config> {
    let shapes: [(&str, usize, usize, &[usize]); 3] = [
        ("S1", 128, 256, &[512, 256]),
        ("S2", 128, 256, &[256, 128]),
        ("S3", 64, 128, &[256, 128]),
    ];
    let arms: [(&str, bool, bool, bool); 4] = [
        ("A1", false, false, false),
        ("A3", true, false, false),
        ("A4", true, true, false),
        ("A5", true, false, true),
    ];
    let mut out = Vec::new();
    for (sn, r, c, hh) in shapes {
        for (an, action_dense, tables, attn) in arms {
            out.push(Config {
                name: format!("{sn}-{an}"),
                topo: Topo::V2,
                web_rank: r,
                ctx_dim: c,
                head_hidden: hh.to_vec(),
                action_dense,
                tables,
                attn,
            });
        }
    }
    // A2 is pinned to its own shape, so it is not part of the shape x arm grid above.
    out.push(Config {
        name: "S0-A2".into(),
        topo: Topo::V2,
        web_rank: 32,
        ctx_dim: 64,
        head_hidden: vec![64],
        action_dense: true,
        tables: false,
        attn: false,
    });
    out.push(Config {
        name: "LVP1".into(),
        topo: Topo::V1,
        web_rank: 32,
        ctx_dim: 64,
        head_hidden: vec![64],
        action_dense: false,
        tables: false,
        attn: false,
    });
    out
}

struct Input {
    ids: Vec<u32>,
    seg_lens: [u16; NUM_SEGMENTS],
    dense: [f32; DENSE_DIM],
    move_ids: [u16; 4],
    sw_extra: [[f32; ACTION_DENSE_DIM]; 6],
    mv_extra: [[f32; ACTION_DENSE_DIM]; 8],
}

fn build_input(seed: u64, vocab: usize) -> Input {
    let mut rng = Rng::new(seed);
    let ids: Vec<u32> = (0..N_IDS).map(|_| rng.below(vocab) as u32).collect();
    let mut sw_extra = [[0.0f32; ACTION_DENSE_DIM]; 6];
    let mut mv_extra = [[0.0f32; ACTION_DENSE_DIM]; 8];
    for row in sw_extra.iter_mut() {
        for v in row.iter_mut() {
            *v = rng.weight() * 10.0;
        }
    }
    for row in mv_extra.iter_mut() {
        for v in row.iter_mut() {
            *v = rng.weight() * 10.0;
        }
    }
    let mv = GEN_MOVES.len();
    Input {
        ids,
        seg_lens: SEG_LENS,
        dense: [0.5, -0.25, 1.0, 0.125, -0.75, 0.375, 0.875],
        move_ids: [(33 % mv) as u16, (85 % mv) as u16, (247 % mv) as u16, (412 % mv) as u16],
        sw_extra,
        mv_extra,
    }
}

struct Cells {
    h_sum: Vec<f32>,
    h_max: Vec<f32>,
    p_col: Vec<f32>,
    p_act: Vec<f32>,
    web_total: Vec<f32>,
}

struct Net {
    cfg: Config,
    emb: Vec<f32>,
    web_a: Vec<f32>,
    web_b: Vec<f32>,
    wq: Vec<f32>,
    wk: Vec<f32>,
    wv: Vec<f32>,
    wo: Vec<f32>,
    pair: Vec<f32>,
    types: Vec<f32>,
    w_tab: Vec<f32>,
    pair_idx: Vec<u32>,
    type_idx: Vec<u32>,
    ctx: Vec<Fc>,
    value: Option<Fc>,
    sw: Vec<Fc>,
    move_emb: Vec<f32>,
    mv: Vec<Fc>,
}

impl Net {
    fn new(cfg: Config) -> Self {
        let r = cfg.web_rank;
        let c = cfg.ctx_dim;
        let mut rng = Rng::new(WEIGHT_SEED);
        let emb = rng.fill(VOCAB * ACC);
        let web_a = rng.fill(r * ACC);
        let web_b = rng.fill(r * ACC);
        let (wq, wk, wv, wo) = if cfg.attn {
            (rng.fill(DK * ACC), rng.fill(DK * ACC), rng.fill(DK * ACC), rng.fill(ACC * DK))
        } else {
            (Vec::new(), Vec::new(), Vec::new(), Vec::new())
        };
        let (pair, types, w_tab, pair_idx, type_idx) = if cfg.tables {
            let pair = rng.fill(PAIR_ROWS * TAB_DIM);
            let types = rng.fill(TYPE_ROWS * TAB_DIM);
            let w_tab = rng.fill(r * TAB_DIM);
            let mut irng = Rng::new(TAB_INDEX_SEED);
            let pair_idx = (0..36).map(|_| irng.below(PAIR_ROWS) as u32).collect();
            let type_idx = (0..36 * 4).map(|_| irng.below(TYPE_ROWS) as u32).collect();
            (pair, types, w_tab, pair_idx, type_idx)
        } else {
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())
        };
        let ctx = vec![Fc::new(cfg.ctx_in_len(), c, &mut rng), Fc::new(c, c, &mut rng)];
        let value = match cfg.topo {
            Topo::V1 => Some(Fc::new(c, 1, &mut rng)),
            Topo::V2 => None,
        };
        let sw = cfg
            .head_dims(cfg.switch_in(), 1)
            .into_iter()
            .map(|(i, o)| Fc::new(i, o, &mut rng))
            .collect();
        let move_emb = rng.fill(GEN_MOVES.len() * MOVE_EMB_DIM);
        let mv = cfg
            .head_dims(cfg.move_in(), cfg.move_out())
            .into_iter()
            .map(|(i, o)| Fc::new(i, o, &mut rng))
            .collect();
        Net {
            cfg,
            emb,
            web_a,
            web_b,
            wq,
            wk,
            wv,
            wo,
            pair,
            types,
            w_tab,
            pair_idx,
            type_idx,
            ctx,
            value,
            sw,
            move_emb,
            mv,
        }
    }

    fn tokens(&self, inp: &Input) -> Vec<f32> {
        let mut tokens = vec![0.0f32; NUM_SEGMENTS * ACC];
        let mut pos = 0usize;
        for (seg, &len) in inp.seg_lens.iter().enumerate() {
            let off = seg * ACC;
            for &id in &inp.ids[pos..pos + len as usize] {
                let row = &self.emb[id as usize * ACC..(id as usize + 1) * ACC];
                for k in 0..ACC {
                    tokens[off + k] += row[k];
                }
            }
            pos += len as usize;
        }
        if self.cfg.attn {
            self.attend(&mut tokens);
        }
        tokens
    }

    fn attend(&self, tokens: &mut [f32]) {
        let mut q = vec![0.0f32; NUM_SEGMENTS * DK];
        let mut k = vec![0.0f32; NUM_SEGMENTS * DK];
        let mut v = vec![0.0f32; NUM_SEGMENTS * DK];
        for t in 0..NUM_SEGMENTS {
            let tok = &tokens[t * ACC..(t + 1) * ACC];
            for o in 0..DK {
                let wq = &self.wq[o * ACC..(o + 1) * ACC];
                let wk = &self.wk[o * ACC..(o + 1) * ACC];
                let wv = &self.wv[o * ACC..(o + 1) * ACC];
                let mut sq = 0.0f32;
                let mut sk = 0.0f32;
                let mut sv = 0.0f32;
                for i in 0..ACC {
                    sq += tok[i] * wq[i];
                    sk += tok[i] * wk[i];
                    sv += tok[i] * wv[i];
                }
                q[t * DK + o] = sq;
                k[t * DK + o] = sk;
                v[t * DK + o] = sv;
            }
        }
        let scale = 1.0 / (DK as f32).sqrt();
        let mut ctxv = vec![0.0f32; NUM_SEGMENTS * DK];
        let mut score = [0.0f32; NUM_SEGMENTS];
        for t in 0..NUM_SEGMENTS {
            let mut mx = f32::NEG_INFINITY;
            for u in 0..NUM_SEGMENTS {
                let mut s = 0.0f32;
                for o in 0..DK {
                    s += q[t * DK + o] * k[u * DK + o];
                }
                score[u] = s * scale;
                if score[u] > mx {
                    mx = score[u];
                }
            }
            let mut sum = 0.0f32;
            for s in score.iter_mut() {
                *s = (*s - mx).exp();
                sum += *s;
            }
            for u in 0..NUM_SEGMENTS {
                let w = score[u] / sum;
                for o in 0..DK {
                    ctxv[t * DK + o] += w * v[u * DK + o];
                }
            }
        }
        for t in 0..NUM_SEGMENTS {
            let cv = &ctxv[t * DK..(t + 1) * DK];
            for i in 0..ACC {
                let wo = &self.wo[i * DK..(i + 1) * DK];
                let mut s = 0.0f32;
                for o in 0..DK {
                    s += cv[o] * wo[o];
                }
                tokens[t * ACC + i] = s;
            }
        }
    }

    fn accs(&self, tokens: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut a0 = vec![0.0f32; ACC];
        let mut a1 = vec![0.0f32; ACC];
        for s in 0..6 {
            for k in 0..ACC {
                a0[k] += tokens[s * ACC + k];
                a1[k] += tokens[(6 + s) * ACC + k];
            }
        }
        for k in 0..ACC {
            a0[k] += tokens[12 * ACC + k] + tokens[14 * ACC + k];
            a1[k] += tokens[13 * ACC + k] + tokens[14 * ACC + k];
        }
        for v in a0.iter_mut().chain(a1.iter_mut()) {
            if *v < 0.0 {
                *v = 0.0;
            }
        }
        (a0, a1)
    }

    fn web(&self, tokens: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let r = self.cfg.web_rank;
        let mut a = vec![0.0f32; 6 * r];
        let mut b = vec![0.0f32; 6 * r];
        for i in 0..6 {
            for o in 0..r {
                let wa = &self.web_a[o * ACC..(o + 1) * ACC];
                let wb = &self.web_b[o * ACC..(o + 1) * ACC];
                let mut sa = 0.0f32;
                let mut sb = 0.0f32;
                for k in 0..ACC {
                    sa += tokens[i * ACC + k] * wa[k];
                    sb += tokens[(6 + i) * ACC + k] * wb[k];
                }
                a[i * r + o] = sa;
                b[i * r + o] = sb;
            }
        }
        (a, b)
    }

    fn web_batch(&self, tokens: &[Vec<f32>]) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let r = self.cfg.web_rank;
        let tt = interleave(tokens);
        let mut a = vec![vec![0.0f32; 6 * r]; BATCH];
        let mut b = vec![vec![0.0f32; 6 * r]; BATCH];
        for o in 0..r {
            let wa = &self.web_a[o * ACC..(o + 1) * ACC];
            let wb = &self.web_b[o * ACC..(o + 1) * ACC];
            for i in 0..6 {
                let mut sa = [0.0f32; BATCH];
                let mut sb = [0.0f32; BATCH];
                for k in 0..ACC {
                    let wak = wa[k];
                    let wbk = wb[k];
                    let ca = &tt[i * ACC + k];
                    let cb = &tt[(6 + i) * ACC + k];
                    for bi in 0..BATCH {
                        sa[bi] += ca[bi] * wak;
                        sb[bi] += cb[bi] * wbk;
                    }
                }
                for bi in 0..BATCH {
                    a[bi][i * r + o] = sa[bi];
                    b[bi][i * r + o] = sb[bi];
                }
            }
        }
        (a, b)
    }

    fn tabs(&self) -> Vec<f32> {
        let r = self.cfg.web_rank;
        let mut out = vec![0.0f32; 36 * r];
        for cell in 0..36 {
            let pr = self.pair_idx[cell] as usize;
            let mut e = [0.0f32; TAB_DIM];
            e.copy_from_slice(&self.pair[pr * TAB_DIM..(pr + 1) * TAB_DIM]);
            for t in 0..4 {
                let ti = self.type_idx[cell * 4 + t] as usize;
                let row = &self.types[ti * TAB_DIM..(ti + 1) * TAB_DIM];
                for o in 0..TAB_DIM {
                    e[o] += row[o] * 0.25;
                }
            }
            for o in 0..r {
                let w = &self.w_tab[o * TAB_DIM..(o + 1) * TAB_DIM];
                let mut s = 0.0f32;
                for i in 0..TAB_DIM {
                    s += e[i] * w[i];
                }
                out[cell * r + o] = s;
            }
        }
        out
    }

    fn cells(&self, a: &[f32], b: &[f32], tabs: Option<&[f32]>) -> Cells {
        let r = self.cfg.web_rank;
        let v2 = self.cfg.topo == Topo::V2;
        let mut c = Cells {
            h_sum: vec![0.0f32; 6 * r],
            h_max: if v2 { vec![0.0f32; 6 * r] } else { Vec::new() },
            p_col: if v2 { vec![0.0f32; 6 * r] } else { Vec::new() },
            p_act: vec![0.0f32; r],
            web_total: vec![0.0f32; r],
        };
        let mut p = vec![0.0f32; r];
        for i in 0..6 {
            for j in 0..6 {
                let tab = tabs.map(|t| &t[(i * 6 + j) * r..(i * 6 + j + 1) * r]);
                for o in 0..r {
                    let mut val = a[i * r + o] * b[j * r + o];
                    if let Some(t) = tab {
                        val += t[o];
                    }
                    if val < 0.0 {
                        val = 0.0;
                    }
                    p[o] = val;
                    c.h_sum[i * r + o] += val;
                    c.web_total[o] += val;
                }
                if v2 {
                    for o in 0..r {
                        if p[o] > c.h_max[i * r + o] {
                            c.h_max[i * r + o] = p[o];
                        }
                    }
                    if j == ACT1 {
                        c.p_col[i * r..(i + 1) * r].copy_from_slice(&p);
                    }
                }
                if i == ACT0 && j == ACT1 {
                    c.p_act.copy_from_slice(&p);
                }
            }
        }
        c
    }

    fn ctx_in(&self, a0: &[f32], a1: &[f32], c: &Cells, dense: &[f32; DENSE_DIM]) -> Vec<f32> {
        let mut x = Vec::with_capacity(self.cfg.ctx_in_len());
        x.extend_from_slice(a0);
        x.extend_from_slice(a1);
        x.extend_from_slice(&c.web_total);
        x.extend_from_slice(&c.p_act);
        x.extend_from_slice(dense);
        x
    }

    fn switch_input(&self, tokens: &[f32], c: &Cells, g: &[f32], k: usize, inp: &Input) -> Vec<f32> {
        let r = self.cfg.web_rank;
        let mut x = Vec::with_capacity(self.cfg.switch_in());
        x.extend_from_slice(&tokens[k * ACC..(k + 1) * ACC]);
        x.extend_from_slice(&c.h_sum[k * r..(k + 1) * r]);
        if self.cfg.topo == Topo::V2 {
            x.extend_from_slice(&c.h_max[k * r..(k + 1) * r]);
            x.extend_from_slice(&c.p_col[k * r..(k + 1) * r]);
            x.extend_from_slice(&tokens[(6 + ACT1) * ACC..(7 + ACT1) * ACC]);
        }
        x.extend_from_slice(g);
        if self.cfg.action_dense {
            x.extend_from_slice(&inp.sw_extra[k]);
        }
        x
    }

    fn move_input(
        &self,
        tokens: &[f32],
        c: &Cells,
        g: &[f32],
        j: usize,
        tera: bool,
        inp: &Input,
    ) -> Vec<f32> {
        let r = self.cfg.web_rank;
        let mid = inp.move_ids[j] as usize;
        let mut x = Vec::with_capacity(self.cfg.move_in());
        x.extend_from_slice(&tokens[ACT0 * ACC..(ACT0 + 1) * ACC]);
        x.extend_from_slice(&c.h_sum[ACT0 * r..(ACT0 + 1) * r]);
        if self.cfg.topo == Topo::V2 {
            x.extend_from_slice(&c.p_act);
        }
        x.extend_from_slice(&self.move_emb[mid * MOVE_EMB_DIM..(mid + 1) * MOVE_EMB_DIM]);
        x.extend_from_slice(g);
        if self.cfg.action_dense {
            x.extend_from_slice(&inp.mv_extra[if tera { 4 + j } else { j }]);
        }
        x
    }

    fn forward(&self, inp: &Input) -> ([f32; NUM_ACTIONS], f32) {
        let tokens = self.tokens(inp);
        let (a0, a1) = self.accs(&tokens);
        let (a, b) = self.web(&tokens);
        let tabs = if self.cfg.tables { Some(self.tabs()) } else { None };
        let cells = self.cells(&a, &b, tabs.as_deref());
        let mut cur = self.ctx_in(&a0, &a1, &cells, &inp.dense);
        for fc in &self.ctx {
            cur = fc.apply(&cur, true);
        }
        let g = cur;
        let value = self.value.as_ref().map_or(0.0, |fc| fc.apply(&g, false)[0]);
        let mut logits = [0.0f32; NUM_ACTIONS];
        for k in 0..SLOTS {
            let x = self.switch_input(&tokens, &cells, &g, k, inp);
            logits[SWITCH_BASE + k] = run_head(&self.sw, x)[0];
        }
        match self.cfg.topo {
            Topo::V1 => {
                for j in 0..MOVE_SLOTS {
                    let x = self.move_input(&tokens, &cells, &g, j, false, inp);
                    let out = run_head(&self.mv, x);
                    logits[j] = out[0];
                    logits[TERA_BASE + j] = out[1];
                }
            }
            Topo::V2 => {
                for j in 0..MOVE_SLOTS {
                    for tera in [false, true] {
                        let x = self.move_input(&tokens, &cells, &g, j, tera, inp);
                        let out = run_head(&self.mv, x)[0];
                        logits[if tera { TERA_BASE + j } else { j }] = out;
                    }
                }
            }
        }
        (logits, value)
    }

    fn forward_batch8(&self, inps: &[Input]) -> Vec<([f32; NUM_ACTIONS], f32)> {
        assert_eq!(inps.len(), BATCH);
        let tokens: Vec<Vec<f32>> = inps.iter().map(|i| self.tokens(i)).collect();
        let accs: Vec<(Vec<f32>, Vec<f32>)> = tokens.iter().map(|t| self.accs(t)).collect();
        let (a, b) = self.web_batch(&tokens);
        let tabs = if self.cfg.tables { Some(self.tabs()) } else { None };
        let cells: Vec<Cells> =
            (0..BATCH).map(|i| self.cells(&a[i], &b[i], tabs.as_deref())).collect();
        let ctx_ins: Vec<Vec<f32>> = (0..BATCH)
            .map(|i| self.ctx_in(&accs[i].0, &accs[i].1, &cells[i], &inps[i].dense))
            .collect();
        let mut cur = interleave(&ctx_ins);
        for fc in &self.ctx {
            cur = fc.apply_batch(&cur, true);
        }
        let values: Vec<f32> = match &self.value {
            Some(fc) => fc.apply_batch(&cur, false)[0].to_vec(),
            None => vec![0.0f32; BATCH],
        };
        let g = deinterleave(&cur);
        let mut logits = vec![[0.0f32; NUM_ACTIONS]; BATCH];
        for k in 0..SLOTS {
            let xs: Vec<Vec<f32>> = (0..BATCH)
                .map(|i| self.switch_input(&tokens[i], &cells[i], &g[i], k, &inps[i]))
                .collect();
            let out = run_head_batch(&self.sw, interleave(&xs));
            for i in 0..BATCH {
                logits[i][SWITCH_BASE + k] = out[0][i];
            }
        }
        match self.cfg.topo {
            Topo::V1 => {
                for j in 0..MOVE_SLOTS {
                    let xs: Vec<Vec<f32>> = (0..BATCH)
                        .map(|i| self.move_input(&tokens[i], &cells[i], &g[i], j, false, &inps[i]))
                        .collect();
                    let out = run_head_batch(&self.mv, interleave(&xs));
                    for i in 0..BATCH {
                        logits[i][j] = out[0][i];
                        logits[i][TERA_BASE + j] = out[1][i];
                    }
                }
            }
            Topo::V2 => {
                for j in 0..MOVE_SLOTS {
                    for tera in [false, true] {
                        let xs: Vec<Vec<f32>> = (0..BATCH)
                            .map(|i| {
                                self.move_input(&tokens[i], &cells[i], &g[i], j, tera, &inps[i])
                            })
                            .collect();
                        let out = run_head_batch(&self.mv, interleave(&xs));
                        for i in 0..BATCH {
                            logits[i][if tera { TERA_BASE + j } else { j }] = out[0][i];
                        }
                    }
                }
            }
        }
        logits.into_iter().zip(values).collect()
    }
}

const DEFAULT_ITERS: usize = 100_000;
const DEFAULT_REPS: usize = 5;
const DEFAULT_WARMUP: usize = 1000;

struct Args {
    iters: Option<usize>,
    reps: Option<usize>,
    warmup: Option<usize>,
    only: Vec<String>,
    batch: bool,
    per_action: bool,
    records_dir: Option<String>,
    converted_dir: Option<String>,
}

impl Args {
    fn reps(&self) -> usize {
        self.reps.unwrap_or(DEFAULT_REPS)
    }

    fn warmup(&self) -> usize {
        self.warmup.unwrap_or(DEFAULT_WARMUP)
    }

    fn iters(&self) -> usize {
        self.iters.unwrap_or(DEFAULT_ITERS)
    }
}

fn usage() {
    println!("policy_costbench: forward-cost and per-action-cost measurement instrument");
    println!();
    println!("modes (pick one; default is the shape-only table)");
    println!("  <none>          shape-only forward cost over the {} config rows", matrix().len());
    println!("  --batch         batched-{BATCH} vs {BATCH}x single for one row");
    println!("  --per-action    calc_damage / clone primitives + corpus profile (needs a train_value build)");
    println!();
    println!("shape-only and --batch");
    println!("  --iters <N>     forwards per timed rep (default {DEFAULT_ITERS}, must be >= 1)");
    println!("  --reps <N>      timed reps, median reported (default {DEFAULT_REPS}, must be >= 1)");
    println!("  --warmup <N>    untimed forwards before timing (default {DEFAULT_WARMUP})");
    println!("  --only <name>   restrict to a named row; repeatable in shape-only, at most one with --batch");
    println!();
    println!("--per-action");
    println!("  --iters <N>     iterations per timed primitive (default 200000)");
    println!("  --records-dir <path>    shard dir (default <crate>/../full_cp07_vlabel/s0)");
    println!("  --converted-dir <path>  corpus dir (default <crate>/data/corpus/converted)");
    println!();
    println!("  --help          this text");
}

fn arg_fail(msg: String) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}

fn parse_count(flag: &str, v: &str) -> usize {
    v.parse().unwrap_or_else(|_| arg_fail(format!("{flag}: expected a non-negative integer, got \"{v}\"")))
}

fn parse_args() -> Args {
    let mut a = Args {
        iters: None,
        reps: None,
        warmup: None,
        only: Vec::new(),
        batch: false,
        per_action: false,
        records_dir: None,
        converted_dir: None,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let flag = argv[i].clone();
        let next = || -> String {
            argv.get(i + 1)
                .unwrap_or_else(|| arg_fail(format!("{flag} needs a value")))
                .clone()
        };
        match argv[i].as_str() {
            "--iters" => {
                a.iters = Some(parse_count("--iters", &next()));
                i += 2;
            }
            "--reps" => {
                a.reps = Some(parse_count("--reps", &next()));
                i += 2;
            }
            "--warmup" => {
                a.warmup = Some(parse_count("--warmup", &next()));
                i += 2;
            }
            "--only" => {
                a.only.push(next());
                i += 2;
            }
            "--records-dir" => {
                a.records_dir = Some(next());
                i += 2;
            }
            "--converted-dir" => {
                a.converted_dir = Some(next());
                i += 2;
            }
            "--batch" => {
                a.batch = true;
                i += 1;
            }
            "--per-action" => {
                a.per_action = true;
                i += 1;
            }
            "--help" | "-h" => {
                usage();
                std::process::exit(0);
            }
            other => arg_fail(format!("unknown argument {other} (try --help)")),
        }
    }
    validate_args(&a);
    a
}

fn validate_args(a: &Args) {
    if a.iters == Some(0) {
        arg_fail("--iters must be at least 1".into());
    }
    if a.reps == Some(0) {
        arg_fail("--reps must be at least 1".into());
    }
    if a.batch && a.per_action {
        arg_fail("--batch and --per-action are different modes; pick one".into());
    }
    if a.per_action {
        for (set, flag) in [
            (a.reps.is_some(), "--reps"),
            (a.warmup.is_some(), "--warmup"),
            (!a.only.is_empty(), "--only"),
        ] {
            if set {
                arg_fail(format!("{flag} is not read in --per-action mode"));
            }
        }
    } else {
        for (set, flag) in
            [(a.records_dir.is_some(), "--records-dir"), (a.converted_dir.is_some(), "--converted-dir")]
        {
            if set {
                arg_fail(format!("{flag} is only read in --per-action mode"));
            }
        }
        if a.batch && a.only.len() > 1 {
            arg_fail(format!("--batch measures one row; got {} --only values", a.only.len()));
        }
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|x, y| x.partial_cmp(y).unwrap());
    v[v.len() / 2]
}

fn time_forwards(net: &Net, inp: &Input, iters: usize, reps: usize, warmup: usize) -> (Vec<f64>, f64) {
    let mut sink = 0.0f64;
    for _ in 0..warmup {
        let out = black_box(black_box(net).forward(black_box(inp)));
        sink += out.0[0] as f64 + out.1 as f64;
    }
    let mut times = Vec::with_capacity(reps);
    for _ in 0..reps {
        let t = Instant::now();
        for _ in 0..iters {
            let out = black_box(black_box(net).forward(black_box(inp)));
            sink += out.0[0] as f64 + out.1 as f64;
        }
        times.push(t.elapsed().as_secs_f64() * 1e6 / iters as f64);
    }
    (times, sink)
}

fn head_str(cfg: &Config) -> String {
    cfg.head_hidden.iter().map(|h| h.to_string()).collect::<Vec<_>>().join("x")
}

fn shape_mode(args: &Args) {
    let iters = args.iters();
    let configs: Vec<Config> = matrix()
        .into_iter()
        .filter(|c| args.only.is_empty() || args.only.iter().any(|o| *o == c.name))
        .collect();
    if configs.is_empty() {
        eprintln!("no configs matched --only");
        std::process::exit(1);
    }
    println!(
        "policy_costbench shape-only  os={} arch={}  iters={} reps={} warmup={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        iters,
        args.reps(),
        args.warmup()
    );
    println!(
        "fixed: acc_width={ACC} vocab={VOCAB} n_ids={N_IDS} move_vocab={} move_emb_dim={MOVE_EMB_DIM} dk={DK} act0={ACT0} act1={ACT1}",
        GEN_MOVES.len()
    );
    println!(
        "macs convention: emb n_ids*acc + web {}*r*acc + cells {}*r + tables {}*{TAB_DIM}*r + attn (3*{NUM_SEGMENTS}*dk*acc + 2*{NUM_SEGMENTS}*{NUM_SEGMENTS}*dk + {NUM_SEGMENTS}*acc*dk) + ctx (ctx_in*c + c*c, v1 adds c for the value tail) + heads sum(in*out) over {SLOTS} switch and {} (v1) / {} (v2) move evals",
        2 * SLOTS,
        SLOTS * SLOTS,
        SLOTS * SLOTS,
        MOVE_SLOTS,
        2 * MOVE_SLOTS
    );
    println!(
        "{:<7} {:>5} {:>5} {:>9} {:>6} {:>6} {:>10} {:>11} {:>8} {:>11} {:>11}",
        "name", "rank", "ctx", "head", "sw_in", "mv_in", "macs", "us/fwd", "gmac/s", "min_us", "max_us"
    );
    let mut sink = 0.0f64;
    for cfg in configs {
        let macs = cfg.macs();
        let sw_in = cfg.switch_in();
        let mv_in = cfg.move_in();
        let name = cfg.name.clone();
        let hs = head_str(&cfg);
        let rank = cfg.web_rank;
        let ctx = cfg.ctx_dim;
        let net = Net::new(cfg);
        let inp = build_input(INPUT_SEED, VOCAB);
        let (times, s) = time_forwards(&net, &inp, iters, args.reps(), args.warmup());
        sink += s;
        let lo = times.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = times.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let med = median(times);
        println!(
            "{:<7} {:>5} {:>5} {:>9} {:>6} {:>6} {:>10} {:>11.4} {:>8.3} {:>11.4} {:>11.4}",
            name,
            rank,
            ctx,
            hs,
            sw_in,
            mv_in,
            macs,
            med,
            macs as f64 / med * 1e-3,
            lo,
            hi
        );
    }
    println!("sink {sink:.6e}");
}

fn batch_mode(args: &Args) {
    let iters = args.iters();
    let want = args.only.first().cloned().unwrap_or_else(|| "S2-A3".into());
    let cfg = matrix()
        .into_iter()
        .find(|c| c.name == want)
        .unwrap_or_else(|| panic!("no config named {want}"));
    let name = cfg.name.clone();
    let macs = cfg.macs();
    let net = Net::new(cfg);
    let inps: Vec<Input> =
        (0..BATCH).map(|i| build_input(INPUT_SEED ^ (i as u64 * 0x1000_0001), VOCAB)).collect();
    let calls = iters / BATCH;
    if calls == 0 {
        eprintln!("--iters must be at least {BATCH} in batch mode");
        std::process::exit(1);
    }
    let forwards = calls * BATCH;
    println!(
        "policy_costbench batch  os={} arch={}  row={name} macs={macs} forwards_per_rep={forwards} reps={} warmup={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        args.reps(),
        args.warmup()
    );
    let run_batched = || -> (f64, f64) {
        let mut s = 0.0f64;
        let t = Instant::now();
        for _ in 0..calls {
            let out = black_box(black_box(&net).forward_batch8(black_box(&inps[..])));
            s += out[0].0[0] as f64;
        }
        (t.elapsed().as_secs_f64() * 1e6, s)
    };
    let run_single = || -> (f64, f64) {
        let mut s = 0.0f64;
        let t = Instant::now();
        for _ in 0..calls {
            for inp in &inps {
                let out = black_box(black_box(&net).forward(black_box(inp)));
                s += out.0[0] as f64;
            }
        }
        (t.elapsed().as_secs_f64() * 1e6, s)
    };
    let mut sink = 0.0f64;
    for _ in 0..args.warmup() / BATCH + 1 {
        let out = black_box(black_box(&net).forward_batch8(black_box(&inps[..])));
        sink += out[0].0[0] as f64;
    }
    for _ in 0..args.warmup() {
        let out = black_box(black_box(&net).forward(black_box(&inps[0])));
        sink += out.0[0] as f64;
    }
    let mut btimes = Vec::with_capacity(args.reps());
    let mut stimes = Vec::with_capacity(args.reps());
    for rep in 0..args.reps() {
        let (bt, st) = if rep % 2 == 0 {
            let b = run_batched();
            let s = run_single();
            (b, s)
        } else {
            let s = run_single();
            let b = run_batched();
            (b, s)
        };
        btimes.push(bt.0);
        stimes.push(st.0);
        sink += bt.1 + st.1;
    }
    let bt = median(btimes);
    let st = median(stimes);
    let factor = st / bt;
    println!("single   total_us {st:.1}  us/fwd {:.4}", st / forwards as f64);
    println!("batched8 total_us {bt:.1}  us/fwd {:.4}", bt / forwards as f64);
    println!(
        "factor single/batched {factor:.3}  clears 1.5x: {}",
        if factor >= 1.5 { "yes" } else { "no" }
    );
    println!("sink {sink:.6e}");
}

fn cat_name(c: MoveCategory) -> &'static str {
    match c {
        MoveCategory::Physical => "Physical",
        MoveCategory::Special => "Special",
        MoveCategory::Status => "Status",
    }
}

fn category_mix(counts: &[u64; 3]) -> String {
    let total = counts.iter().sum::<u64>() as f64;
    [MoveCategory::Status, MoveCategory::Physical, MoveCategory::Special]
        .iter()
        .map(|&c| {
            let n = counts[c as usize];
            format!("{} {} ({:.2}%)", cat_name(c), n, n as f64 / total * 100.0)
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn uptime_line() -> String {
    match std::process::Command::new("uptime").output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "NOT AVAILABLE".into(),
    }
}

fn load_states(dir: &str) -> Vec<BattleState> {
    let mut shards: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().ends_with(".records.bin"))
            .collect(),
        Err(e) => {
            eprintln!("records dir {dir}: {e}");
            std::process::exit(1);
        }
    };
    shards.sort();
    let mut out = Vec::new();
    for path in shards {
        if out.len() >= 3 {
            break;
        }
        let recs = match poke_mcts::policy_label::read_records_guarded(&path.to_string_lossy()) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                std::process::exit(1);
            }
        };
        for rec in recs {
            if out.len() >= 3 {
                break;
            }
            if rec.state.phase != PHASE_ACTIONS {
                continue;
            }
            if effective_moves(&rec.state, 0).iter().all(|&m| m == 0) {
                continue;
            }
            out.push(rec.state);
        }
    }
    out
}

fn primitive_costs(args: &Args) {
    let iters = args.iters.unwrap_or(200_000);
    let dir = args
        .records_dir
        .clone()
        .unwrap_or_else(|| format!("{}/../full_cp07_vlabel/s0", env!("CARGO_MANIFEST_DIR")));
    println!("records dir: {dir}");
    let states = load_states(&dir);
    if states.len() < 3 {
        eprintln!("found only {} usable states in {dir}", states.len());
        std::process::exit(1);
    }
    let mut sink = 0u64;
    let mut per_state = Vec::new();
    let mut cats = [0u64; 3];
    for (si, state) in states.iter().enumerate() {
        let moves = effective_moves(state, 0);
        let mut total = 0.0f64;
        let mut slots = 0usize;
        let mut breakdown = String::new();
        for slot in 0..4 {
            let mid = moves[slot];
            if mid == 0 {
                continue;
            }
            let t = Instant::now();
            for _ in 0..iters {
                let d = calc_damage(
                    black_box(state),
                    black_box(0),
                    black_box(mid),
                    black_box(100),
                    &mut median_roll,
                );
                sink += black_box(d).damage as u64;
            }
            let ns = t.elapsed().as_secs_f64() * 1e9 / iters as f64;
            total += ns;
            slots += 1;
            let cat = move_hot(mid).category;
            cats[cat as usize] += 1;
            breakdown.push_str(&format!(" [slot {slot} {} {ns:.2} ns]", cat_name(cat)));
        }
        let mean = total / slots as f64;
        println!("state {si}: {slots} nonzero move slots,{breakdown} mean {mean:.2} ns");
        per_state.push(mean);
    }
    let calc_ns = per_state.iter().sum::<f64>() / per_state.len() as f64;
    println!("calc_damage mean across {} states: {calc_ns:.2} ns", per_state.len());
    println!(
        "timed slot category mix over {} slots: {}",
        cats.iter().sum::<u64>(),
        category_mix(&cats)
    );

    let state = &states[0];
    let t = Instant::now();
    for _ in 0..iters {
        let c = black_box(black_box(state).clone());
        sink += c.phase as u64;
    }
    let clone_ns = t.elapsed().as_secs_f64() * 1e9 / iters as f64;
    println!("BattleState clone: {clone_ns:.2} ns  size_of {} bytes", std::mem::size_of::<BattleState>());
    println!(
        "predicted action_features cost = 56*calc + 6*clone = {:.3} us (prediction from the two primitives above, not a measurement of action_features)",
        (56.0 * calc_ns + 6.0 * clone_ns) / 1000.0
    );
    println!("sink {sink}");
}

fn corpus_profile(args: &Args) {
    let dir = args
        .converted_dir
        .clone()
        .unwrap_or_else(|| format!("{}/data/corpus/converted", env!("CARGO_MANIFEST_DIR")));
    println!("converted dir: {dir}");
    let meta_path = format!("{dir}/meta.json");
    let meta_txt = match std::fs::read_to_string(&meta_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FALLBACK: {meta_path} unreadable ({e})");
            eprintln!("FALLBACK ids per row 99.1961");
            eprintln!("FALLBACK mean legal move bytes per row 3.3464");
            eprintln!("FALLBACK mean legal switch bytes per row 3.1617");
            eprintln!("FALLBACK mean legal tera bytes per row 2.0632");
            eprintln!("FALLBACK total legal bytes per row 8.5713");
            eprintln!("FALLBACK category mix Status 31.60% Physical 40.22% Special 28.18%");
            eprintln!("DEVIATION: figures above are the campaign reference values, not measured on this box");
            std::process::exit(3);
        }
    };
    let meta: serde_json::Value = serde_json::from_str(&meta_txt).expect("meta.json");
    let records = meta["records"].as_u64().expect("meta.records") as usize;
    let features = meta["features"].as_u64().expect("meta.features") as usize;
    println!("records {records}  features {features}  ids per row {:.4}", features as f64 / records as f64);

    let lm_path = format!("{dir}/legal_mask.bin");
    let mi_path = format!("{dir}/move_ids.bin");
    let lm_len = std::fs::metadata(&lm_path).map(|m| m.len()).unwrap_or(0) as usize;
    let mi_len = std::fs::metadata(&mi_path).map(|m| m.len()).unwrap_or(0) as usize;
    if lm_len != records * NUM_ACTIONS || mi_len != records * MOVE_IDS_ROW_BYTES {
        eprintln!(
            "ERROR: legal_mask.bin {lm_len} bytes (want {}), move_ids.bin {mi_len} bytes (want {})",
            records * NUM_ACTIONS,
            records * MOVE_IDS_ROW_BYTES
        );
        std::process::exit(1);
    }
    let mut lm = std::io::BufReader::new(std::fs::File::open(&lm_path).expect("legal_mask.bin"));
    let mut mi = std::io::BufReader::new(std::fs::File::open(&mi_path).expect("move_ids.bin"));
    const CHUNK: usize = 65536;
    let mut lbuf = vec![0u8; CHUNK * NUM_ACTIONS];
    let mut mbuf = vec![0u8; CHUNK * MOVE_IDS_ROW_BYTES];
    let mut move_bytes = 0u64;
    let mut switch_bytes = 0u64;
    let mut tera_bytes = 0u64;
    let mut cat = [0u64; 3];
    let mut zero_id = 0u64;
    let mut bad_id = 0u64;
    let mut rows_with_status = 0u64;
    let mut done = 0usize;
    while done < records {
        let n = CHUNK.min(records - done);
        lm.read_exact(&mut lbuf[..n * NUM_ACTIONS]).expect("legal_mask read");
        mi.read_exact(&mut mbuf[..n * MOVE_IDS_ROW_BYTES]).expect("move_ids read");
        for row in 0..n {
            let legal = &lbuf[row * NUM_ACTIONS..(row + 1) * NUM_ACTIONS];
            let mut status_here = false;
            for b in 0..MOVE_SLOTS {
                if legal[b] == 0 {
                    continue;
                }
                move_bytes += 1;
                let o = row * MOVE_IDS_ROW_BYTES + b * 2;
                let id = u16::from_le_bytes([mbuf[o], mbuf[o + 1]]);
                if id == 0 {
                    zero_id += 1;
                    continue;
                }
                if id as usize >= GEN_MOVES.len() {
                    bad_id += 1;
                    continue;
                }
                let c = move_hot(id).category;
                cat[c as usize] += 1;
                if c == MoveCategory::Status {
                    status_here = true;
                }
            }
            switch_bytes += legal[SWITCH_BASE..TERA_BASE].iter().filter(|&&v| v != 0).count() as u64;
            tera_bytes += legal[TERA_BASE..NUM_ACTIONS].iter().filter(|&&v| v != 0).count() as u64;
            if status_here {
                rows_with_status += 1;
            }
        }
        done += n;
    }
    let rows = records as f64;
    let total = move_bytes + switch_bytes + tera_bytes;
    println!("legal move bytes   {move_bytes}  per row {:.4}", move_bytes as f64 / rows);
    println!("legal switch bytes {switch_bytes}  per row {:.4}", switch_bytes as f64 / rows);
    println!("legal tera bytes   {tera_bytes}  per row {:.4}", tera_bytes as f64 / rows);
    println!("total legal bytes  {total}  per row {:.4}", total as f64 / rows);
    println!("category mix  {}", category_mix(&cat));
    println!("legal move bytes with move id 0: {zero_id}  out of range: {bad_id}");
    println!("rows with >=1 legal Status option: {rows_with_status} ({:.2}%)", rows_with_status as f64 / rows * 100.0);
}

fn require_train_value() {
    #[cfg(not(feature = "train_value"))]
    {
        eprintln!("--per-action requires a train_value build");
        std::process::exit(2);
    }
}

fn per_action_mode(args: &Args) {
    require_train_value();
    println!(
        "policy_costbench per-action  os={} arch={}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let before = uptime_line();
    println!("load before: {before}");
    primitive_costs(args);
    println!("load after: {}", uptime_line());
    corpus_profile(args);
}

fn main() {
    let args = parse_args();
    if args.per_action {
        per_action_mode(&args);
    } else if args.batch {
        batch_mode(&args);
    } else {
        shape_mode(&args);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str) -> Config {
        matrix().into_iter().find(|c| c.name == name).expect(name)
    }

    #[test]
    fn head_input_widths_match_the_shape_table() {
        let want = [
            ("LVP1", 224, 240),
            ("S1-A1", 896, 656),
            ("S1-A3", 904, 664),
            ("S2-A1", 896, 656),
            ("S2-A3", 904, 664),
            ("S3-A1", 576, 400),
            ("S3-A3", 584, 408),
            ("S0-A2", 424, 280),
        ];
        for (name, sw, mv) in want {
            let c = cfg(name);
            assert_eq!(c.switch_in(), sw, "{name} switch");
            assert_eq!(c.move_in(), mv, "{name} move");
        }
    }

    #[test]
    fn every_config_forward_is_finite() {
        for c in matrix() {
            let name = c.name.clone();
            let net = Net::new(c);
            let inp = build_input(INPUT_SEED, VOCAB);
            let (logits, value) = net.forward(&inp);
            for (i, v) in logits.iter().enumerate() {
                assert!(v.is_finite(), "{name} logit {i} = {v}");
            }
            assert!(value.is_finite(), "{name} value");
        }
    }

    #[test]
    fn batched_matches_single() {
        for c in matrix() {
            let name = c.name.clone();
            let net = Net::new(c);
            let inps: Vec<Input> = (0..BATCH)
                .map(|i| build_input(INPUT_SEED ^ (i as u64 * 0x1000_0001), VOCAB))
                .collect();
            let batched = net.forward_batch8(&inps);
            for (i, inp) in inps.iter().enumerate() {
                let single = net.forward(inp);
                for k in 0..NUM_ACTIONS {
                    assert!(
                        (single.0[k] - batched[i].0[k]).abs() < 1e-3,
                        "{name} input {i} logit {k}: single {} batched {}",
                        single.0[k],
                        batched[i].0[k]
                    );
                }
                assert!(
                    (single.1 - batched[i].1).abs() < 1e-3,
                    "{name} input {i} value: single {} batched {}",
                    single.1,
                    batched[i].1
                );
            }
        }
    }

    #[test]
    fn macs_are_in_the_expected_ballpark() {
        let want = [
            ("LVP1", 0.236e6),
            ("S0-A2", 0.38e6),
            ("S3-A3", 2.4e6),
            ("S2-A3", 3.6e6),
            ("S1-A3", 7.7e6),
            ("S1-A4", 7.8212e6),
            ("S1-A5", 8.0076e6),
        ];
        for (name, target) in want {
            let got = cfg(name).macs() as f64;
            assert!(
                (got - target).abs() / target <= 0.10,
                "{name}: macs {got} vs target {target}"
            );
        }
        let base = cfg("S1-A3").macs();
        assert_eq!(cfg("S1-A4").macs() - base, 73_728, "tables term");
        assert_eq!(cfg("S1-A5").macs() - base, 260_160, "attention term");
    }
}
