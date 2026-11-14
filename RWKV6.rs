// RWKV6 (Eagle & Finch) WKV 主架構 —— 教學用 Rust 實作
//
// 這個檔案示範 RWKV6 的核心：WKV (Weighted Key-Value) 遞迴。
//
// 編譯執行： rustc rwkv6_wkv.rs -O && ./rwkv6_wkv
//
// 一句話總結：
//   WKV 就是「帶衰減的加權 Key-Value 累加」。它用兩個 running 累加器
//   （分子 num、分母 den）把 O(T^2) 的加權和壓成 O(T) 的線性遞迴。
//   RWKV6 的關鍵改動是：decay / bonus 不再是固定參數，而是「隨輸入資料
//   變化」(data-dependent) —— 用低秩投影 (LoRA) 從當下 token 的隱狀態
//   即時算出。
//
// 遞迴式（每個通道 c，向量化後一次處理所有 channel）：
//   out_t = ( num_{t-1} + u_t · k_t · v_t ) / ( den_{t-1} + u_t · k_t )
//   num_t = d_t · num_{t-1} + k_t · v_t
//   den_t = d_t · den_{t-1} + k_t
//
// 其中：
//   d_t = exp( -exp( mu_t ) )   ∈ (0,1)   ← data-dependent decay
//   u_t = exp( nu_t )                      ← data-dependent bonus
//   mu_t = lowrank_w(x_t),  nu_t = lowrank_u(x_t)
//
// 數學意義：
//   out_t = ( Σ_{s<t} (Π_{r=s+1}^{t-1} d_r) k_s v_s + u_t k_t v_t )
//         / ( Σ_{s<t} (Π_{r=s+1}^{t-1} d_r) k_s     + u_t k_t )
//   即「每個過去的 token 對現在的貢獻，會被中間每一步的 decay 連乘衰減」。

// ============================ 基本維度 ============================
const T: usize = 6;      // 時間步數
const C: usize = 4;      // 通道數 (channel / head dim)
const D_RANK: usize = 2; // 低秩維度 (LoRA rank)

// ============================ 工具函式 ============================
/// 簡單 xorshift 亂數（教學用、可重現），回傳 [-1, 1)
fn next_rand(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// 低秩投影： d_model -> d_rank -> d_model
/// 這是 RWKV6 製造 data-dependent decay / bonus 的骨架。
fn lowrank(x: &[f32], w_in: &[f32], w_out: &[f32], b: &[f32]) -> Vec<f32> {
    // 1) h = act(x @ w_in^T)，形狀 [D_RANK]
    let mut h = vec![0f32; D_RANK];
    for r in 0..D_RANK {
        let mut acc = 0f32;
        for i in 0..C {
            acc += x[i] * w_in[r * C + i];
        }
        // RWKV 常見的平方 ReLU 激活（示意）
        h[r] = if acc > 0.0 { acc * acc } else { 0.0 };
    }
    // 2) out = h @ w_out + b，形狀 [C]
    let mut out = vec![0f32; C];
    for c in 0..C {
        let mut acc = b[c];
        for r in 0..D_RANK {
            acc += h[r] * w_out[c * D_RANK + r];
        }
        out[c] = acc;
    }
    out
}

// ============================ WKV 狀態 ============================
/// WKV 遞迴狀態：每個通道一組 (num, den) 累加器
#[derive(Clone, Debug)]
pub struct WkvState {
    pub num: Vec<f32>, // 分子累加器
    pub den: Vec<f32>, // 分母累加器
}

impl WkvState {
    pub fn new(c: usize) -> Self {
        Self { num: vec![0.0; c], den: vec![0.0; c] }
    }
}

// ============================ RWKV6 Time-Mix ============================
/// 保存 decay / bonus 的低秩權重，並把隱狀態 x 映射成 (decay, bonus)。
pub struct Rkv6TimeMix {
    w_in: Vec<f32>,  // decay 低秩： x -> mu
    w_out: Vec<f32>,
    b_w: Vec<f32>,
    u_in: Vec<f32>,  // bonus 低秩： x -> nu
    u_out: Vec<f32>,
    b_u: Vec<f32>,
}

impl Rkv6TimeMix {
    /// 產生一組示範權重（教學用，非真實訓練值）
    pub fn demo() -> Self {
        let mut s: u32 = 0x1234_5678;
        Self {
            w_in: (0..D_RANK * C).map(|_| next_rand(&mut s)).collect(),
            w_out: (0..C * D_RANK).map(|_| next_rand(&mut s)).collect(),
            b_w: (0..C).map(|_| next_rand(&mut s)).collect(),
            u_in: (0..D_RANK * C).map(|_| next_rand(&mut s)).collect(),
            u_out: (0..C * D_RANK).map(|_| next_rand(&mut s)).collect(),
            b_u: (0..C).map(|_| next_rand(&mut s)).collect(),
        }
    }

    /// 由當前 token 的隱狀態 x 計算 data-dependent 的 (decay, bonus)
    pub fn decay_bonus(&self, x: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mu = lowrank(x, &self.w_in, &self.w_out, &self.b_w);
        let nu = lowrank(x, &self.u_in, &self.u_out, &self.b_u);
        let decay: Vec<f32> = mu.iter().map(|m| (-m.exp()).exp()).collect();
        let bonus: Vec<f32> = nu.iter().map(|v| v.exp()).collect();
        (decay, bonus)
    }
}

// ============================ 逐步 WKV（O(T·C)）============================
/// 逐步 WKV 遞迴；state 會被就地更新。
pub fn wkv_sequential(
    k: &[Vec<f32>],
    v: &[Vec<f32>],
    decay: &[Vec<f32>],
    bonus: &[Vec<f32>],
    state: &mut WkvState,
) -> Vec<Vec<f32>> {
    let t = k.len();
    let c = k[0].len();
    let mut out = vec![vec![0f32; c]; t];
    for ti in 0..t {
        for ci in 0..c {
            let kt = k[ti][ci];
            let vt = v[ti][ci];
            let u = bonus[ti][ci];
            let d = decay[ti][ci];
            // 輸出：目前狀態 + 當前 token 的 bonus 貢獻
            out[ti][ci] = (state.num[ci] + u * kt * vt) / (state.den[ci] + u * kt + 1e-8);
            // 更新狀態：先用 decay 衰減過去，再加入當前 token
            state.num[ci] = d * state.num[ci] + kt * vt;
            state.den[ci] = d * state.den[ci] + kt;
        }
    }
    out
}

// ============================ 參考 WKV（O(T^2·C)）============================
/// 直接對「過去所有 token 的衰減乘積」求和，用來驗證遞迴的數學正確性。
pub fn wkv_reference(
    k: &[Vec<f32>],
    v: &[Vec<f32>],
    decay: &[Vec<f32>],
    bonus: &[Vec<f32>],
) -> Vec<Vec<f32>> {
    let t = k.len();
    let c = k[0].len();
    let mut out = vec![vec![0f32; c]; t];
    for ti in 0..t {
        for ci in 0..c {
            let mut num = 0f32;
            let mut den = 0f32;
            for s in 0..ti {
                let mut prod = 1f32;
                for r in (s + 1)..ti {
                    prod *= decay[r][ci];
                }
                num += prod * k[s][ci] * v[s][ci];
                den += prod * k[s][ci];
            }
            num += bonus[ti][ci] * k[ti][ci] * v[ti][ci];
            den += bonus[ti][ci] * k[ti][ci];
            out[ti][ci] = num / (den + 1e-8);
        }
    }
    out
}

// ============================ 主程式 ============================
fn main() {
    // 1) 準備示範輸入：每個時間步的隱狀態 x_t、key k_t、value v_t
    let mut seed: u32 = 0xDEAD_BEEF;
    let mut x = Vec::with_capacity(T);
    let mut k = Vec::with_capacity(T);
    let mut v = Vec::with_capacity(T);
    for _ in 0..T {
        x.push((0..C).map(|_| next_rand(&mut seed)).collect::<Vec<f32>>());
        k.push((0..C).map(|_| next_rand(&mut seed)).collect::<Vec<f32>>());
        v.push((0..C).map(|_| next_rand(&mut seed)).collect::<Vec<f32>>());
    }

    // 2) 用 RWKV6 Time-Mix 把 x 變成 data-dependent decay / bonus
    let tm = Rkv6TimeMix::demo();
    let mut decay = Vec::with_capacity(T);
    let mut bonus = Vec::with_capacity(T);
    for ti in 0..T {
        let (d, u) = tm.decay_bonus(&x[ti]);
        decay.push(d);
        bonus.push(u);
    }

    // 3) 逐步遞迴（O(T·C)）
    let mut state = WkvState::new(C);
    let out_seq = wkv_sequential(&k, &v, &decay, &bonus, &mut state);

    // 4) 參考 O(T^2) 實作，驗證一致性
    let out_ref = wkv_reference(&k, &v, &decay, &bonus);

    // 5) 印出結果
    println!("RWKV6 WKV demo   (T = {T}, C = {C}, rank = {D_RANK})");
    println!("--------------------------------------------------------------");
    println!(" t  ch |    k_t   |    v_t   |  decay_t |  bonus_t |    wkv_t");
    println!("-------+----------+----------+----------+----------+------------");
    for ti in 0..T {
        for ci in 0..C {
            println!(
                " {ti}   {ci}  | {:8.4} | {:8.4} | {:8.4} | {:8.4} | {:10.4}",
                k[ti][ci], v[ti][ci], decay[ti][ci], bonus[ti][ci], out_seq[ti][ci]
            );
        }
    }

    // 6) 比對兩種實作
    let mut max_err = 0f32;
    for ti in 0..T {
        for ci in 0..C {
            max_err = max_err.max((out_seq[ti][ci] - out_ref[ti][ci]).abs());
        }
    }
    println!("--------------------------------------------------------------");
    println!("逐步遞迴 vs O(T^2) 參考實作：最大絕對誤差 = {max_err:.2e}");
    assert!(max_err < 1e-5, "遞迴與參考實作不一致！");
    println!("OK: 兩種實作數學等價，WKV 遞迴正確。");
}