# canto 真频谱可视化实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 播放中按 `Y`，上半屏是跟着音乐真实起伏的频谱柱子、下半屏是照旧的歌词；零新依赖，不播放时不付这份算力。

**Architecture:** 五步垂直推进：①纯改名（`lyric_full` → `now_playing_full`，无行为变化）②垂直切片（`spectrum.rs` 纯计算 + `levels.rs` 无锁采样通道 + `player.rs` 包一层 + `App` 三份缓冲 + `draw/spectrum.rs` 柱子）③退化与边界（未播放 / 无歌词 / 矮屏）④帧率提速（33ms）⑤文案、文档与真机验收。每个任务结束时 `cargo test` 与 `cargo clippy --all-targets -- -D warnings` 都必须全绿——**这就是任务边界这么切的原因**：CI 把警告当错误（`-D warnings`），提前提交一个还没人调用的 `pub fn` 会立刻在 bin target 上报 `dead_code`，所以「模块 + 它的消费方」必须落在同一个任务里。

**Tech Stack:** Rust 2024 edition、ratatui 0.30、crossterm 0.29、rodio 0.22（`Decoder<MediaSource>` → `Player::append`）。**不新增任何依赖**：FFT 自己写。

**Spec:** `docs/superpowers/specs/2026-10-11-spectrum-visualizer-design.md`（设计与本计划冲突时以 spec 为准，本计划负责落地）

## Global Constraints

- **只新增两个模块**（`src/spectrum.rs`、`src/levels.rs`）与一个绘制模块（`src/tui/draw/spectrum.rs`）；`Cargo.toml` 一个字节都不改。
- **`Snapshot` 不加字段**：它是 `Clone + PartialEq + Eq` 的低频快照，频谱走 `Engine::levels()` 独立句柄。
- **`LevelMeter<S>` 必须逐项转发 `current_span_len` / `channels` / `sample_rate` / `total_duration` / `try_seek`**。`try_seek` 不转发 = rodio 认为整条音频不可跳转，`←`/`→` 与进度条跳转静默失效（`Source::try_seek` 的默认实现返回 `NotSupported`）。
- 常量按 spec §3 原样：`WINDOW = 2048`、`BANDS = 64`、`FREQ_MIN = 40.0`、`FREQ_MAX = 16_000.0`、`DB_FLOOR = -62.0`、`DB_CEIL = -8.0`、`TILT_PER_BAND = 0.25`、抽稀基准 `BASE_RATE = 44_100`、平滑 τ 起振 0.12s / 回落 0.40s / 峰值 60%/s / elapsed 夹到 0.5s、`TICK` 150ms → `SPECTRUM_TICK` 33ms。
- 尺寸契约：内容区 = 终端高度 − 5；有歌词且内容区 ≥ 14 行才拆，歌词高 `clamp(内容区/2, 7, 12)`，余量全给频谱；**无歌词优先**（整块给频谱，不看 14 行那条线）。
- 中文注释与中文界面文案，风格与现有代码一致（说明「为什么」，不复述「做了什么」）。
- 现有测试除被本计划显式改名的以外保持通过。**特别注意 `draw/mod.rs` 里这两条现有测试在 Task 2 之后依然必须绿**（它们的断言与新布局不冲突，不要去「修」它们）：
  - `fullscreen_lyric_takes_over_the_content_area`（断言整帧里「歌词」只出现一次、标题行以 `┌歌词` 开头、状态行有 `Y/Esc退出`）
  - `the_current_lyric_line_is_highlighted`（断言当前句是黄 + 粗）
- 每个任务结束：`cargo test` 全绿 + `cargo clippy --all-targets -- -D warnings` 无警告 + 一个提交。

---

## 文件结构

| 文件 | 责任 |
|---|---|
| `src/spectrum.rs`（新） | FFT 与对数分频。纯计算，不碰音频设备、不碰 ratatui |
| `src/levels.rs`（新） | 无锁环形缓冲、抽稀、`LevelMeter<S>` 包装器。只依赖 `rodio::Source` |
| `src/tui/draw/spectrum.rs`（新） | 频谱柱绘制（柱高、亚格分辨率、峰值刻度） |
| `src/player.rs`（改） | `Levels` 字段与 `Engine::levels()`；`Cmd::Play` 处包一层 |
| `src/tui/draw/mod.rs`（改） | `now_playing_full` 时把内容区拆成上下两块 |
| `src/tui/state.rs`（改） | 三份缓冲 + `last_tick` + 平滑推进 + 退化判定 |
| `src/tui/keys.rs`（改） | 改名（`LyricFull` → `NowPlaying`） |
| `src/tui/mod.rs`（改） | 刷新间隔按状态取值 |
| `src/tui/draw/overlay.rs`、`README.md`、`docs/design.md`（改） | 文案与文档 |

---

# Task 1: 改名（纯重构，无行为变化）

全屏视图马上就不只是歌词了，先把名字改成「如实描述」的：`now_playing_full`。这一步只改标识符与注释，一行行为都不动，所以它的测试就是**现有整套测试**。

**Files:**
- Modify: `src/tui/keys.rs`（`Action::ToggleLyricFull`、`Context::LyricFull` 及其注释与单测）
- Modify: `src/tui/state.rs`（字段 `lyric_full`、`apply` 派发、`context()` 里的注释）
- Modify: `src/tui/draw/mod.rs`（`lyric_full`、`hints()` 的 match 臂）

**Interfaces:**
- Consumes: 无
- Produces: `keys::Action::ToggleNowPlaying`、`keys::Context::NowPlaying`、`App::now_playing_full: bool`（后续所有任务都用这三个名字）

- [ ] **Step 1: 改名**

三个文件里做**全局替换**（`edit` 工具的 `replace_all`，或 `sed -E -i`）：

| 文件 | 旧 | 新 | 预期处数 |
|---|---|---|---|
| `src/tui/keys.rs` | `LyricFull` | `NowPlaying` | 8（`Action` 变体声明与文档注释、`Context` 变体声明与文档注释、两处 `resolve` 分支、3 处单测） |
| `src/tui/state.rs` | `lyric_full` | `now_playing_full` | 4（字段声明与注释、`new()` 初始化、`context()`、`apply` 的 match 臂） |
| `src/tui/state.rs` | `Context::LyricFull` | `Context::NowPlaying` | 1 |
| `src/tui/state.rs` | `Action::ToggleLyricFull` | `Action::ToggleNowPlaying` | 1 |
| `src/tui/draw/mod.rs` | `lyric_full` | `now_playing_full` | 3（`lyric_wanted` 那行、`if` 分支、注释） |
| `src/tui/draw/mod.rs` | `keys::Context::LyricFull` | `keys::Context::NowPlaying` | 1（`hints()`） |

注意 `Action::ToggleLyricFull` 里也含 `LyricFull`，所以 `keys.rs` 里一次 `LyricFull` → `NowPlaying` 就把变体名与所有引用一起改到位（`ToggleLyricFull` → `ToggleNowPlaying`）。

- [ ] **Step 2: 确认没有漏网的旧名**

```bash
grep -rn "lyric_full\|LyricFull" src || echo "干净"
```

Expected: 只输出 `干净`。

- [ ] **Step 3: 跑测试与 clippy**

```bash
cargo test && cargo clippy --all-targets -- -D warnings
```

Expected: 全绿、无警告。改名不改语义，任何一条测试失败都说明改错了地方（不是测试有问题）。

- [ ] **Step 4: 提交**

```bash
git add -A && git commit -m "refactor(tui): 全屏视图改名为 now_playing（马上就不只是歌词了）"
```

---

# Task 2: 垂直切片——音频侧接线，Y 视图上半屏出现柱子

一个任务把整条链路打通：`spectrum.rs`（FFT + 分频）→ `levels.rs`（无锁环形缓冲 + 抽稀 + `LevelMeter<S>`）→ `player.rs`（包一层 + `Engine::levels()`）→ `App`（三份缓冲 + 平滑）→ `draw/spectrum.rs`（柱子）→ `draw/mod.rs`（拆两块）。

**Files:**
- Create: `src/spectrum.rs`、`src/levels.rs`、`src/tui/draw/spectrum.rs`
- Modify: `src/main.rs`（两行 `mod`）、`src/player.rs`、`src/tui/state.rs`、`src/tui/draw/mod.rs`

**Interfaces:**
- Consumes: Task 1 的 `App::now_playing_full`
- Produces:
  - `spectrum::WINDOW: usize`、`spectrum::BANDS: usize`、`spectrum::fft(&mut [f32], &mut [f32])`、`spectrum::Analyzer::new()`、`Analyzer::analyze_into(&mut self, samples: &[f32], sample_rate: u32, out: &mut Vec<f32>)`
  - `levels::Levels::new()`、`Levels::read_into(&self, out: &mut Vec<f32>) -> u32`、`Levels::clear(&self)`、`Levels: Clone`、`levels::LevelMeter::new(inner: S, levels: Levels)`
  - `Engine::levels(&self) -> Levels`
  - `App::spectrum_scratch / spectrum_raw / spectrum_smooth / spectrum_peak: Vec<f32>`、`App::analyzer: spectrum::Analyzer`、`App::last_tick: std::time::Instant`
  - `draw::spectrum::draw_spectrum(frame, area, bands, peaks, title)`

## Task 2 步骤

- [ ] **Step 1: `src/spectrum.rs` 先写失败的测试**

创建 `src/spectrum.rs`，先把模块文档、常量、空的 `fft` / `Analyzer::analyze_into` 与测试写进去（`fft` 先写成空函数体、`analyze_into` 先只做 `out.clear()`，这样测试能编过并失败）：

```rust
//! 频谱分析：把一段时域采样变成若干「频段能量」。
//!
//! # 为什么自己写 FFT
//!
//! 只需要一个长度是 2 的幂的实序列正变换，radix-2 迭代实现不到 60 行。为这点东西
//! 拖进 `rustfft` 之类（连带它的依赖与 SIMD 后端）不划算——依赖要付体积与维护成本。
//!
//! # 为什么要分频而不是直接画 bin
//!
//! FFT 出来的是**线性**频率：2048 点、44.1kHz 下每格约 21.5Hz，于是整个可闻低频段
//! （20~200Hz）挤在最左边两三个格子里，右边一大半几乎全是几乎不动的高频。人耳对
//! 频率的感知是**对数**的，所以按对数划分频段：低频窄、高频宽，每个频段对应一根柱子。

/// 一次分析取的采样点数（抽稀后）。必须是 2 的幂。
///
/// 2048 在 44.1kHz 下约 46ms：短到能跟上鼓点，又长到有 ~21Hz 的频率分辨率。
pub const WINDOW: usize = 2048;

/// 频段数。渲染时再按实际列数聚合，这里取足够细的分辨率。
///
/// 固定 64 而不是跟着终端宽度变：`TILT_PER_BAND` 那条高频补偿是**按段**算的，
/// 段数一变标定就废了。
pub const BANDS: usize = 64;

/// 最低/最高分析频率（Hz）。
///
/// 低于 40Hz 基本是 DC 与次声，画出来只会让最左边一根柱子常亮；高于 16kHz 大多是
/// 噪声，且多数有损编码在这个区间已经砍掉了内容。
const FREQ_MIN: f32 = 40.0;
const FREQ_MAX: f32 = 16_000.0;

/// dB 显示范围。低于下限算静音，高于上限算顶格。
const DB_FLOOR: f32 = -62.0;
const DB_CEIL: f32 = -8.0;

/// 每个频段向高频递增的补偿（dB）。
///
/// 音乐的能量天然集中在低频，不补偿的话右边大半根柱子永远趴着。按每八度约 +3dB
/// 折算：64 段铺满 log2(16000/40)≈8.6 个八度，约合每段 0.25dB。
/// **这是观感补偿，不是测量。**
const TILT_PER_BAND: f32 = 0.25;

/// 原地 radix-2 FFT。`re` / `im` 长度相同且必须是 2 的幂。
///
/// 长度不合要求时直接返回（不 panic）：这是显示用的旁路数据，不该因为它把播放搞崩。
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let _ = (re, im); // Step 3 填实现
}

/// 复用内部缓冲的分析器：逐帧零分配。
pub struct Analyzer {
    re: Vec<f32>,
    im: Vec<f32>,
    magnitudes: Vec<f32>,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        Self {
            re: Vec::new(),
            im: Vec::new(),
            magnitudes: Vec::new(),
        }
    }

    /// 把一段单声道采样分析成 [`BANDS`] 个 0.0~1.0 的能量值，写进 `out`。
    ///
    /// 窗口不够或采样率未知时把 `out` 清空（调用方保持上一帧即可），不 panic。
    pub fn analyze_into(&mut self, samples: &[f32], sample_rate: u32, out: &mut Vec<f32>) {
        out.clear();
        let _ = (samples, sample_rate); // Step 3 填实现
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成 `frequency` 赫兹的正弦，幅度 0.8。
    fn sine(frequency: f32, sample_rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                0.8 * (2.0 * std::f32::consts::PI * frequency * i as f32 / sample_rate as f32).sin()
            })
            .collect()
    }

    /// 峰值落在第几个频段。
    fn peak_band(bands: &[f32]) -> usize {
        bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).expect("没有 NaN"))
            .map(|(index, _)| index)
            .expect("总该有一段最大")
    }

    #[test]
    fn fft_of_dc_lands_entirely_in_bin_zero() {
        // 常数信号只有直流分量：能量应当全落在第 0 个 bin
        let n = 8;
        let mut re = vec![1.0f32; n];
        let mut im = vec![0.0f32; n];
        fft(&mut re, &mut im);

        assert!((re[0] - n as f32).abs() < 1e-4, "直流分量 = 所有采样之和");
        for i in 1..n {
            assert!(
                re[i].abs() < 1e-4 && im[i].abs() < 1e-4,
                "其余 bin 应当为零"
            );
        }
    }

    #[test]
    fn fft_puts_a_pure_tone_in_its_own_bin() {
        // 4Hz 的信号、8 点、频率 32Hz → 正好落在 bin 1
        let n = 8;
        let sample_rate = 32;
        let mut re = sine(4.0, sample_rate, n);
        let mut im = vec![0.0f32; n];
        fft(&mut re, &mut im);

        let magnitude = (re[1] * re[1] + im[1] * im[1]).sqrt();
        assert!(
            (magnitude - 0.8 * n as f32 / 2.0).abs() < 1e-3,
            "bin 1 应当主峰，实际 {magnitude}"
        );
        let off_peak = (re[3] * re[3] + im[3] * im[3]).sqrt();
        assert!(off_peak < 1e-3, "bin 3 不该有能量，实际 {off_peak}");
    }

    #[test]
    fn fft_ignores_malformed_input() {
        // 长度不是 2 的幂时安静地什么都不做，而不是 panic
        let mut re = vec![1.0f32; 6];
        let mut im = vec![0.0f32; 6];
        fft(&mut re, &mut im);
        assert_eq!(re, vec![1.0f32; 6], "输入应当原样保留");
    }

    #[test]
    fn a_low_tone_lights_the_left_bands_only() {
        // 100Hz 是低频：峰值该落在最左 1/4 段里。**这一条是「对数分频真的生效」
        // 的证据**——线性分频下 100Hz 会挤在 bin 2，也是左边，但下面的高频用例会挂。
        let sample_rate = 44_100;
        let samples = sine(100.0, sample_rate, WINDOW);
        let mut analyzer = Analyzer::new();
        let mut bands = Vec::new();
        analyzer.analyze_into(&samples, sample_rate, &mut bands);

        assert_eq!(bands.len(), BANDS);
        let peak = peak_band(&bands);
        assert!(peak < BANDS / 4, "100Hz 的峰值该在左 1/4，实际在第 {peak} 段");
    }

    #[test]
    fn a_high_tone_lights_the_right_bands() {
        let sample_rate = 44_100;
        let samples = sine(8_000.0, sample_rate, WINDOW);
        let mut analyzer = Analyzer::new();
        let mut bands = Vec::new();
        analyzer.analyze_into(&samples, sample_rate, &mut bands);

        let peak = peak_band(&bands);
        assert!(peak > BANDS / 2, "8kHz 的峰值该在右半，实际在第 {peak} 段");
    }

    #[test]
    fn silence_produces_nothing() {
        let mut analyzer = Analyzer::new();
        let mut bands = Vec::new();
        analyzer.analyze_into(&vec![0.0f32; WINDOW], 44_100, &mut bands);
        assert!(bands.iter().all(|&value| value == 0.0), "静音应当是平的");
    }

    #[test]
    fn degenerate_inputs_yield_no_bands() {
        let mut analyzer = Analyzer::new();
        let mut bands = Vec::new();
        analyzer.analyze_into(&[], 44_100, &mut bands);
        assert!(bands.is_empty());
        analyzer.analyze_into(&[0.0; 64], 0, &mut bands);
        assert!(bands.is_empty(), "采样率未知");
    }
}
```

- [ ] **Step 2: 跑测试看它失败**

```bash
cargo test spectrum 2>&1 | tail -20
```

Expected: `a_low_tone_lights_the_left_bands_only` / `silence_produces_nothing` 等失败（`bands` 是空的），而 `fft_ignores_malformed_input` 会「假绿」（空实现刚好不改输入）。

- [ ] **Step 3: 填实现**

把 `fft` 与 `analyze_into` 的空实现替换成：

```rust
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    if n != im.len() || !n.is_power_of_two() {
        return;
    }

    // 位反转置换：把输入重排成迭代版 FFT 需要的顺序，省掉递归
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2;
    while len <= n {
        let half = len / 2;
        // 旋转因子 w = e^(-2πi/len)，取负号对应正向变换
        let step = -2.0 * std::f32::consts::PI / len as f32;
        let mut start = 0;
        while start < n {
            for k in 0..half {
                let (sin, cos) = (step * k as f32).sin_cos();
                let a = start + k;
                let b = a + half;
                let tr = re[b] * cos - im[b] * sin;
                let ti = re[b] * sin + im[b] * cos;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
            start += len;
        }
        len <<= 1;
    }
}
```

```rust
    pub fn analyze_into(&mut self, samples: &[f32], sample_rate: u32, out: &mut Vec<f32>) {
        out.clear();
        if samples.len() < 2 || sample_rate == 0 {
            return;
        }

        // 取不超过输入长度的最大 2 的幂：FFT 只认这个长度
        let capped = samples.len().min(WINDOW);
        let n = if capped.is_power_of_two() {
            capped
        } else {
            capped.next_power_of_two() / 2
        };
        if n < 2 {
            return;
        }
        // 用窗口**末尾**的采样：那是刚刚播过的声音，画出来才跟得上
        let offset = samples.len() - n;

        // 复用缓冲，逐帧零分配
        self.re.clear();
        self.re.resize(n, 0.0);
        self.im.clear();
        self.im.resize(n, 0.0);

        // Hann 窗：直接截断一段采样会在两端产生频谱泄漏，能量糊到整个频域上，
        // 表现为所有柱子一起微微发抖。加窗把两端平滑到 0 就没有这个问题。
        let mut window_sum = 0.0f32;
        for (i, slot) in self.re.iter_mut().enumerate() {
            let window = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos());
            window_sum += window;
            *slot = samples[offset + i] * window;
        }

        fft(&mut self.re, &mut self.im);

        // 归一化：原始 DFT 幅度与点数成正比（不加这一步幅度动辄几百，换算成 dB 之后
        // 每个频段都会顶格，画出来是一堵齐平的墙）。除以「窗函数之和 / 2」后，
        // 幅度 A 的纯音在它那一列得到的值就是 A。
        let norm = (window_sum / 2.0).max(1.0);

        // 正频率部分：bin i 对应 i * sample_rate / n 赫兹
        let bins = n / 2;
        let nyquist = sample_rate as f32 / 2.0;
        let freq_max = FREQ_MAX.min(nyquist);
        self.magnitudes.clear();
        self.magnitudes.extend(
            (0..bins).map(|i| (self.re[i] * self.re[i] + self.im[i] * self.im[i]).sqrt() / norm),
        );

        // 对数分频：第 k 段覆盖 [FREQ_MIN * ratio^k, FREQ_MIN * ratio^(k+1))
        let ratio = (freq_max / FREQ_MIN).powf(1.0 / BANDS as f32);
        for band in 0..BANDS {
            let low = FREQ_MIN * ratio.powi(band as i32);
            let high = FREQ_MIN * ratio.powi(band as i32 + 1);
            let low_bin = (low * n as f32 / sample_rate as f32).floor() as usize;
            let high_bin = (high * n as f32 / sample_rate as f32).ceil() as usize;

            // 一个频段里可能压着好几个 bin，也可能（低频段）一个 bin 都没有
            let peak = if low_bin >= high_bin {
                self.magnitudes
                    .get(low_bin.min(bins - 1))
                    .copied()
                    .unwrap_or(0.0)
            } else {
                self.magnitudes[low_bin.min(bins)..high_bin.min(bins)]
                    .iter()
                    .fold(0.0f32, |max, &value| max.max(value))
            };

            // 幅度 → dB → 0..1，再叠一点高频补偿
            let db = 20.0 * peak.max(1e-9).log10() + TILT_PER_BAND * band as f32;
            out.push(((db - DB_FLOOR) / (DB_CEIL - DB_FLOOR)).clamp(0.0, 1.0));
        }
    }
```

- [ ] **Step 4: 跑测试看它通过**

```bash
cargo test spectrum 2>&1 | tail -20
```

Expected: 7 条全绿。

- [ ] **Step 5: `src/levels.rs` 先写失败的测试**

创建 `src/levels.rs`（实现先留空，测试先写全）：

```rust
//! 播放采样的采集通道：把音源包一层，采样原样透传给播放器，顺带把第 0 声道写进
//! 一份无锁环形缓冲，供频谱分析用。
//!
//! # 为什么无锁
//!
//! 音频线程每个采样写一次、界面线程每帧读一次。用互斥锁会让两边互相卡住——界面卡
//! 一下不算事，音频线程卡一下就是爆音。f32 没有原子版本，所以存它的位模式。
//!
//! # 为什么抽稀
//!
//! 窗口按**时间**（约 46ms）定义，不是按点数：192kHz 的母带文件若照收 2048 个点，
//! 窗口只有 10.7ms，低频分辨率从 21Hz/格退到 94Hz/格。源采样率高于基准就抽稀到
//! 基准量级，并且先做一次 N 点滑动平均——不做低通直接抽稀会把 Nyquist 以上的内容
//! 折叠进可闻段，显示上就是高频段一片噪。
//!
//! # 为什么只取第 0 声道
//!
//! 立体声的两个声道交错排列，全塞进一个窗口等于把两条不同的波形混在一起，频谱是乱的
//! （6 声道的 FLAC 同理）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use rodio::source::{SeekError, Source};
use rodio::{ChannelCount, SampleRate};

use crate::spectrum::WINDOW;

/// 分析的基准采样率。源比它高就抽稀到这个量级。
const BASE_RATE: u32 = 44_100;

/// 环形缓冲里的采样数（= 一个分析窗口）。
const SLOTS: usize = WINDOW;

/// 音频线程写、界面线程读的一小段波形。
#[derive(Debug, Clone)]
pub struct Levels {
    samples: Arc<Vec<AtomicU32>>,
    cursor: Arc<AtomicUsize>,
    /// 抽稀后的有效采样率（换歌时会变：不同来源的文件采样率未必相同）。
    sample_rate: Arc<AtomicU32>,
}

impl Default for Levels {
    fn default() -> Self {
        Self::new()
    }
}

impl Levels {
    pub fn new() -> Self {
        let mut samples = Vec::with_capacity(SLOTS);
        samples.resize_with(SLOTS, || AtomicU32::new(0.0f32.to_bits()));
        Self {
            samples: Arc::new(samples),
            cursor: Arc::new(AtomicUsize::new(0)),
            sample_rate: Arc::new(AtomicU32::new(0)),
        }
    }

    /// 追加一个（抽稀后的、单声道的）采样。
    fn push(&self, sample: f32) {
        let index = self.cursor.fetch_add(1, Ordering::Relaxed) % SLOTS;
        self.samples[index].store(sample.to_bits(), Ordering::Relaxed);
    }

    fn set_sample_rate(&self, sample_rate: u32) {
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
    }

    /// 把整个窗口按时间顺序（最旧 → 最新）拷进 `out`，返回有效采样率。
    ///
    /// 复用调用方的缓冲：逐帧零分配。
    pub fn read_into(&self, out: &mut Vec<f32>) -> u32 {
        let cursor = self.cursor.load(Ordering::Relaxed);
        out.clear();
        out.extend((0..SLOTS).map(|offset| {
            let index = (cursor + offset) % SLOTS;
            f32::from_bits(self.samples[index].load(Ordering::Relaxed))
        }));
        self.sample_rate.load(Ordering::Relaxed)
    }

    /// 清零。切歌时调用，否则会留着上一首的波形。
    pub fn clear(&self) {
        for sample in self.samples.iter() {
            sample.store(0.0f32.to_bits(), Ordering::Relaxed);
        }
        self.cursor.store(0, Ordering::Relaxed);
    }
}

/// 把音源包起来：采样透传，同时把第 0 声道按抽稀倍率记进 [`Levels`]。
pub struct LevelMeter<S> {
    inner: S,
    levels: Levels,
    channels: u16,
    /// 当前轮到第几声道（只有 0 会被记下来）。
    channel: u16,
    /// 抽稀倍率（1 = 原样收）。
    factor: u16,
    sum: f32,
    counted: u16,
}

// Step 6 填实现

#[cfg(test)]
mod tests {
    use super::*;

    /// 只实现必要的转发，用来验证包装层没有把 `Source` 的那几个方法吞掉。
    struct Fake {
        samples: Vec<f32>,
        index: usize,
        rate: u32,
        seeks: Arc<std::sync::Mutex<Vec<Duration>>>,
    }

    impl Iterator for Fake {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            let sample = self.samples.get(self.index).copied();
            self.index += 1;
            sample
        }
    }

    impl Source for Fake {
        fn current_span_len(&self) -> Option<usize> {
            Some(7)
        }

        fn channels(&self) -> ChannelCount {
            ChannelCount::new(2).expect("2 声道")
        }

        fn sample_rate(&self) -> SampleRate {
            SampleRate::new(self.rate).expect("采样率")
        }

        fn total_duration(&self) -> Option<Duration> {
            Some(Duration::from_secs(9))
        }

        fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
            self.seeks.lock().expect("锁").push(position);
            Ok(())
        }
    }

    /// 一个假音源：`rate` 决定抽稀倍率，`samples` 是逐声道的交错采样。
    fn fake(samples: Vec<f32>, rate: u32) -> (Fake, Arc<std::sync::Mutex<Vec<Duration>>>) {
        let seeks = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Fake {
                samples,
                index: 0,
                rate,
                seeks: seeks.clone(),
            },
            seeks,
        )
    }

    #[test]
    fn the_wrapper_forwards_every_source_method() {
        // 这一条是整个设计里最容易踩的坑：`Source::try_seek` 的**默认实现直接返回
        // NotSupported**，包装层漏掉转发，rodio 就认为整条音频不可跳转——进度条与
        // 方向键全部静默失效，而播放本身完全正常。
        let (inner, seeks) = fake(vec![0.0; 8], 44_100);
        let mut meter = LevelMeter::new(inner, Levels::new());

        assert_eq!(meter.current_span_len(), Some(7));
        assert_eq!(meter.channels().get(), 2);
        assert_eq!(meter.sample_rate().get(), 44_100);
        assert_eq!(meter.total_duration(), Some(Duration::from_secs(9)));

        meter
            .try_seek(Duration::from_millis(500))
            .expect("seek 必须被转发到内层");
        assert_eq!(
            seeks.lock().expect("锁").as_slice(),
            [Duration::from_millis(500)]
        );
    }

    #[test]
    fn samples_pass_through_untouched() {
        let (inner, _seeks) = fake(vec![0.25, -0.5, 1.0], 44_100);
        let meter = LevelMeter::new(inner, Levels::new());
        assert_eq!(meter.collect::<Vec<f32>>(), vec![0.25, -0.5, 1.0]);
    }

    #[test]
    fn only_the_first_channel_is_recorded() {
        // 两个声道交错：第 0 声道全是 1.0、第 1 声道全是 0.0
        let samples: Vec<f32> = (0..SLOTS * 2)
            .map(|index| if index % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        let levels = Levels::new();
        let (inner, _seeks) = fake(samples, 44_100);
        let mut meter = LevelMeter::new(inner, levels.clone());
        for _ in 0..SLOTS * 2 {
            meter.next().expect("有采样");
        }

        let mut window = Vec::new();
        levels.read_into(&mut window);
        assert!(
            window.iter().all(|&sample| sample == 1.0),
            "窗口里应当是第 0 声道的波形"
        );
    }

    #[test]
    fn a_high_rate_source_is_decimated() {
        // 96kHz 的源抽稀 2 倍：有效采样率变 48kHz，窗口里是两个采样的平均。
        //
        // 采样数要算对：填满窗口需要「窗口 × 声道数 × 抽稀倍率」个采样
        // （立体声每 2 个采样才出一个第 0 声道，再每 2 个才凑成一格）。
        //
        // 源是立体声、每帧两个采样同值，于是**第 0 声道**是 0 与 1 交替
        // （写成「按 index 交替」会让第 0 声道恒为 0——那是两条不同的波形）。
        let total = SLOTS * 2 * 2;
        let samples: Vec<f32> = (0..total)
            .map(|index| if (index / 2) % 2 == 0 { 0.0 } else { 1.0 })
            .collect();
        let levels = Levels::new();
        let (inner, _seeks) = fake(samples, 96_000);
        let mut meter = LevelMeter::new(inner, levels.clone());
        assert_eq!(meter.factor, 2);
        for _ in 0..total {
            meter.next().expect("有采样");
        }

        let mut window = Vec::new();
        let rate = levels.read_into(&mut window);
        assert_eq!(rate, 48_000, "有效采样率该是抽稀后的");
        assert_eq!(window.len(), SLOTS);
        assert!(
            window.iter().all(|&sample| (sample - 0.5).abs() < 1e-6),
            "0 与 1 交替的源抽稀后应当处处是 0.5，实际首个是 {}",
            window[0]
        );
    }

    #[test]
    fn a_base_rate_source_is_not_decimated() {
        let (inner, _seeks) = fake(vec![0.0; 4], 44_100);
        let meter = LevelMeter::new(inner, Levels::new());
        assert_eq!(meter.factor, 1);
        assert_eq!(meter.sample_rate().get(), 44_100);
    }

    #[test]
    fn clear_wipes_the_window() {
        let levels = Levels::new();
        levels.push(1.0);
        levels.clear();
        let mut window = Vec::new();
        levels.read_into(&mut window);
        assert!(window.iter().all(|&sample| sample == 0.0));
    }
}
```

- [ ] **Step 6: 填 `LevelMeter` 的实现**

```rust
impl<S> LevelMeter<S>
where
    S: Source,
{
    pub fn new(inner: S, levels: Levels) -> Self {
        let channels = inner.channels().get();
        let rate = inner.sample_rate().get();
        // 基准采样率以下的源原样收（factor = 1，不做平均）
        let factor = (rate / BASE_RATE).clamp(1, u16::MAX as u32) as u16;
        levels.set_sample_rate(rate / u32::from(factor));
        Self {
            inner,
            levels,
            channels: channels.max(1),
            channel: 0,
            factor,
            sum: 0.0,
            counted: 0,
        }
    }

    /// 记一个第 0 声道的采样：按倍率累加，凑满一格才算一个（倍率 1 时即原样）。
    fn record(&mut self, sample: f32) {
        self.sum += sample;
        self.counted += 1;
        if self.counted == self.factor {
            self.levels.push(self.sum / f32::from(self.factor));
            self.sum = 0.0;
            self.counted = 0;
        }
    }
}

impl<S> Iterator for LevelMeter<S>
where
    S: Source,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;

        // 只把第 0 声道写进窗口，保证窗口里是一条连续的波形
        if self.channel == 0 {
            self.record(sample);
        }
        self.channel = (self.channel + 1) % self.channels;

        Some(sample)
    }
}

impl<S> Source for LevelMeter<S>
where
    S: Source,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    /// 必须转发，否则进度条与方向键跳转会完全失效——`Source::try_seek` 的默认实现
    /// 直接返回 `NotSupported`，而这个包装层漏掉它时，rodio 会认为整条音频不可跳转。
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(position)
    }
}
```

- [ ] **Step 7: 跑测试**

```bash
cargo test levels 2>&1 | tail -20
```

Expected: 6 条全绿。若 `a_high_rate_source_is_decimated` 挂在 `meter.factor` 上，检查 `rate / BASE_RATE` 的整除语义（`96_000 / 44_100 = 2`）。

- [ ] **Step 8: 在 `player.rs` 挂上（这一步之后两个新模块就不再是 dead code）**

`src/main.rs` 加两行，按现有的近似字母序插：`mod levels;` 放在 `mod instance;` 之后、`mod media;` 之前；`mod spectrum;` 放在 `mod session;` 之后、`mod tui;` 之前。

```rust
mod levels;
…
mod spectrum;
```

`src/player.rs` 顶部加 `use crate::levels::{LevelMeter, Levels};`，然后四处改动：

```rust
pub struct Engine {
    tx: Sender<Cmd>,
    state: Arc<Mutex<Snapshot>>,
    running: Arc<AtomicBool>,
    /// 刚缓存完的歌（mid、档位、字节数）。界面每帧取走，用来刷新列表里的「已缓存」标记。
    promoted: cache::Promoted,
    /// 频谱分析用的采样通道：音频线程写、界面线程读（见 [`crate::levels`]）。
    levels: Levels,
}
```

`Engine::spawn`：在建 `state` / `running` / `promoted` 的同一处建通道，一份交给线程、一份留给句柄：

```rust
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let running = Arc::new(AtomicBool::new(true));
        let promoted = Arc::new(Mutex::new(Vec::new()));
        // 先建好再分身：句柄与音频线程必须是**同一个** Levels，
        // 否则界面读到的永远是零，柱子不会动。
        let levels = Levels::new();
        let thread_levels = levels.clone();
        let (tx, rx) = channel();
```

线程闭包与 `run` 的签名各加一个 `thread_levels` / `levels: Levels` 参数，`run` 再把它交给 `handle`。

（两边都会越过 clippy 的 7 参数线：`handle` 上原本就有 `#[allow(clippy::too_many_arguments)]`，`run` 要照着补一个——`-D warnings` 下这是硬错误。函数体本来就是「把上下文原样往下传」，包一个结构体只是把参数搬个家，不值当。）

`Engine::levels` 访问器（放在 `snapshot` 旁边）：

```rust
    /// 频谱分析用的采样通道。`Clone` 只是克隆几个 `Arc`，界面拿一份长期持有即可。
    pub fn levels(&self) -> Levels {
        self.levels.clone()
    }
```

`handle` 里 `Cmd::Play` 分支的三处改动——**先清缓冲再加、并且包一层**：

```rust
        Cmd::Play(track) => {
            // 丢掉旧的播放器就是「停掉上一首」。
            *player = None;
            *seek_anchor = None;
            // 上一首的波形不能留着，否则新歌起播时柱子会先亮一下旧的尾巴。
            levels.clear();
            …
            match open(&track, cache_on, promoted) {
                Ok(opened) => {
                    let new_player = Player::connect_new(mixer);
                    new_player.set_volume(*volume);
                    // 包一层：采样原样透传给播放器，顺带记进频谱窗口。
                    new_player.append(LevelMeter::new(opened.decoder, levels.clone()));
                    …
```

`open()` 与其返回值**不动**（仍然给裸 `Decoder<MediaSource>`），缓存命中路径一行不改。

- [ ] **Step 9: 跑测试 + clippy**

```bash
cargo test && cargo clippy --all-targets -- -D warnings
```

Expected: 全绿。播放相关的现有测试（`player.rs` 里那几条）不受影响——它们走哑引擎，压根不会 `append`。

- [ ] **Step 9b: 加一条无声卡的端到端冒烟测试**

上面的单测只能零散地验各段（哑引擎没人拉样本，频谱恒为零；`spectrum.rs` 只喂合成正弦）。在 `src/player.rs` 的 `mod tests` 里补一条**直接拉真解码器**的：

- 用 `write_sine_wav()` 手写一个最小 WAV（44 字节头 + 16bit PCM 单声道，440Hz，1 秒）到临时目录（文件名带 pid，测试并行不撞）；
- `Decoder::new(MediaSource::File(file))` → `LevelMeter::new(decoder, levels.clone())`，拉满 `spectrum::WINDOW` 个采样；
- 断言 `levels.read_into()` 报的有效采样率是 44100、窗口里有 `|sample| > 0.05` 的波形；
- 再喂给 `Analyzer::analyze_into`，断言频段不是全零。

`rodio` 的默认 feature 里带 `wav`，所以不需要额外依赖，也不需要声卡。完整代码见实施时写进 `src/player.rs` 的 `a_real_decoder_feeds_the_spectrum_window`（一并带上 `write_sine_wav` 辅助函数）。这条是整条链路**唯一能进 CI 的端到端证据**；「柱子真的跟着音乐动」仍然只能真跑看。

- [ ] **Step 10: `src/tui/state.rs` 加缓冲、平滑与取数**

`App` 结构体（`now_playing_full` 那一段之后）加字段：

```rust
    /// 全屏正在播放视图：上半屏频谱、下半屏歌词。
    pub(super) now_playing_full: bool,
    /// 频谱：采样窗口的副本（复用缓冲，逐帧零分配）。
    pub(super) spectrum_scratch: Vec<f32>,
    /// 原始频段能量（0.0~1.0），由 FFT 分频得到。
    pub(super) spectrum_raw: Vec<f32>,
    /// 平滑后的频谱（快起慢落），画的是这一份。
    pub(super) spectrum_smooth: Vec<f32>,
    /// 峰值刻度：比柱子落得慢，柱子顶上那条线。
    pub(super) spectrum_peak: Vec<f32>,
    /// FFT 分析器（内部缓冲复用，不再逐帧分配）。
    pub(super) analyzer: crate::spectrum::Analyzer,
    /// 上一次推进平滑的时刻。平滑按**真实经过时间**算，不按帧数——帧率从
    /// 150ms 提到 33ms 时观感才不会变。
    pub(super) last_tick: std::time::Instant,
```

`App::new` 里对应初始化：`spectrum_scratch: Vec::new(), spectrum_raw: Vec::new(), spectrum_smooth: Vec::new(), spectrum_peak: Vec::new(), analyzer: crate::spectrum::Analyzer::new(), last_tick: std::time::Instant::now(),`。

在 `impl App` 里加三个方法：

```rust
    /// 频谱是不是该动（全屏视图开着 + 正在播）。关着的时候不取、不算、不画。
    pub(super) fn spectrum_wanted(&self) -> bool {
        self.now_playing_full && self.engine.snapshot().phase == crate::player::Phase::Playing
    }

    /// 取一次频谱：采样窗口 → 频段能量。`Levels` 里没有数据时 `spectrum_raw` 会是空的。
    fn refresh_spectrum(&mut self) {
        let levels = self.engine.levels();
        let rate = levels.read_into(&mut self.spectrum_scratch);
        self.analyzer
            .analyze_into(&self.spectrum_scratch, rate, &mut self.spectrum_raw);
    }

    /// 按真实经过时间做指数平滑：起振快、回落慢，柱子才跟手又不抖。
    ///
    /// 时间常数必须**远大于帧间隔**，否则块字符会在 ▁ 与 █ 之间跳变（看起来就是一直在闪）。
    fn advance_visualizer(&mut self, elapsed: std::time::Duration) {
        if self.spectrum_raw.is_empty() {
            self.spectrum_smooth.clear();
            self.spectrum_peak.clear();
            return;
        }

        let count = self.spectrum_raw.len();
        self.spectrum_smooth.resize(count, 0.0);
        self.spectrum_peak.resize(count, 0.0);

        // 夹一下：切屏或卡顿后 elapsed 可能很大，避免一帧跳到底
        let seconds = elapsed.as_secs_f32().clamp(0.0, 0.5);
        let attack = 1.0 - (-seconds / 0.12).exp();
        let decay = 1.0 - (-seconds / 0.40).exp();
        // 峰值每秒下落 60%（约 1.7 秒落到底），比柱子慢得多，才有频谱仪的余韵
        let peak_fall = seconds * 0.6;

        for index in 0..count {
            let target = self.spectrum_raw[index].clamp(0.0, 1.0);
            let current = self.spectrum_smooth[index];
            let factor = if target > current { attack } else { decay };
            let next = current + (target - current) * factor;
            self.spectrum_smooth[index] = next;

            let peak = self.spectrum_peak[index];
            self.spectrum_peak[index] = (peak - peak_fall).max(next);
        }
    }
```

`App::tick` 的**开头**插入：

```rust
        // 频谱：只在全屏视图开着且正在播时取，别的屏不付这份算力。
        let now = std::time::Instant::now();
        let elapsed = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        if self.spectrum_wanted() {
            self.refresh_spectrum();
        } else {
            self.spectrum_raw.clear();
        }
        self.advance_visualizer(elapsed);
```

（`advance_visualizer` 在 `spectrum_raw` 为空时会自己清掉平滑与峰值，所以「不在播」与「没数据」两条路都落到底，不需要额外分支。）

- [ ] **Step 11: `src/tui/draw/spectrum.rs`**

```rust
//! 频谱柱：底对齐的柱子，柱高按 1/8 格取整，柱顶之上有一条落得更慢的峰值刻度。
//!
//! 频段数（[`crate::spectrum::BANDS`]）通常多于终端列数，所以先把若干频段压成一根
//! 柱子，压的时候取**最大值**而不是平均值——平均会把鼓点那一下的尖峰抹平，柱子就
//! 只剩一团钝钝的起伏。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

/// 柱顶收口用的竖直填充块：1/8 …… 8/8。
///
/// 只用 `█` 的话柱高被量化到整行，低矮的柱子会在「空 / 满」之间整行跳变；用这一组
/// 字符，柱顶能落在一格之内的任意 1/8 处。
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// 每格切成 8 个刻度。`height` 行的柱区就是 `height * 8` 个刻度。
const EIGHTHS: usize = 8;

/// 柱体最多占几列。宽屏下多出来的列全给空隙，柱子才不成一块块方砖。
const MAX_BODY: usize = 2;

/// 柱体颜色。与播放条的进度条同色：语义都是「电平」。
const BAR: Color = Color::Green;

/// 峰值刻度用终端默认前景色：必须与柱体不同色，否则那条浮线会被读成柱子的一部分。
const PEAK: Color = Color::Reset;

/// 画频谱块（含边框与标题）。`bands` / `peaks` 为空时只画框。
pub(super) fn draw_spectrum(
    frame: &mut Frame,
    area: Rect,
    bands: &[f32],
    peaks: &[f32],
    title: &str,
) {
    let block = Block::bordered().title(title.to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 || bands.is_empty() {
        return;
    }
    bars(frame, inner, bands, peaks);
}

/// 没有数据可画时：一块只有标题的框，中间一句话。
///
/// 不画残留的柱子——暂停时 rodio 不再吐采样，缓冲会冻在最后一帧，画出来就是
/// 「看着在播却不出声」。竖着垫掉一半空行，视觉重心才不在顶上。
pub(super) fn draw_spectrum_message(frame: &mut Frame, area: Rect, title: &str, message: &str) {
    let block = Block::bordered().title(title.to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![Line::default(); inner.height.saturating_sub(1) as usize / 2];
    lines.push(Line::from(Span::styled(
        message,
        Style::new().fg(Color::DarkGray),
    )));
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        inner,
    );
}

/// 柱区：每根柱子一行一个 `Span`（柱体同宽同色，一格 `Span` 就够）。
fn bars(frame: &mut Frame, area: Rect, bands: &[f32], peaks: &[f32]) {
    let height = area.height as usize;
    let stride = bar_stride(area.width as usize, bands.len());
    let count = (area.width as usize / stride).min(bands.len());
    if count == 0 {
        return;
    }
    // 柱体封顶，步长里多出来的列全给空隙
    let body = stride.saturating_sub(1).clamp(1, MAX_BODY);
    let gap = stride.saturating_sub(body);

    let columns = aggregate(bands, count);
    let caps = aggregate(peaks, count);
    let marks: Vec<(usize, usize)> = columns
        .iter()
        .zip(&caps)
        .map(|(&level, &cap)| mark(level, cap, height))
        .collect();

    // 柱子整块居中：靠左铺的话右边会空出一条，看着像图没画完。宽度有余量时两侧对称留白。
    let used = (count - 1) * stride + body;
    let pad = (area.width as usize).saturating_sub(used) / 2;

    let mut lines = Vec::with_capacity(height);
    for row in 0..height {
        // 从底部数起的行号，用来判断这一格要不要点亮
        let from_bottom = height - row;
        let mut spans: Vec<Span> = Vec::with_capacity(count + 1);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        for (index, &(filled, peak_cell)) in marks.iter().enumerate() {
            let (glyph, color) = cell(from_bottom, filled, peak_cell);
            let mut chunk = String::with_capacity(body + gap);
            for _ in 0..body {
                chunk.push(glyph);
            }
            // 柱间空隙。最后一根后面不留，否则右边会多出一列空白
            if index + 1 < count {
                for _ in 0..gap {
                    chunk.push(' ');
                }
            }
            spans.push(Span::styled(chunk, Style::new().fg(color)));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// 每根柱子占几列：由可用宽度与频段数算出来，而不是写死。
///
/// * 铺满宽度：柱子只占左边一小段、右边大片空着，看着像图没画完。
/// * 柱子之间要有空隙：铺满每一列的话相邻柱子会粘成一整片，看不出是一根根柱子。
fn bar_stride(area_width: usize, bands: usize) -> usize {
    if bands == 0 {
        return 1;
    }
    let fit = area_width / bands;
    if fit >= 2 {
        return fit;
    }
    // fit <= 1：柱子只能 1 列宽。此时若还有余量，走 2 列步长换取空隙 + 铺满
    if area_width > bands { 2 } else { 1 }
}

/// 把 `values` 压成 `count` 根柱子，每根取组内最大值（不抹平尖峰）。
fn aggregate(values: &[f32], count: usize) -> Vec<f32> {
    (0..count)
        .map(|bar| {
            let start = bar * values.len() / count;
            let end = ((bar + 1) * values.len() / count).max(start + 1);
            values[start..end.min(values.len())]
                .iter()
                .fold(0.0f32, |max, &value| max.max(value))
                .clamp(0.0, 1.0)
        })
        .collect()
}

/// 一根柱子的刻度：柱体高度（1/8 格为单位）与峰值所在的行（从底部数，0 = 不画）。
fn mark(level: f32, cap: f32, height: usize) -> (usize, usize) {
    let scale = (height * EIGHTHS) as f32;
    let filled = (level.clamp(0.0, 1.0) * scale).round() as usize;
    let peak = (cap.clamp(0.0, 1.0) * scale).round() as usize;

    // 峰值只在**高过柱顶一整格**时才画：否则它会和柱顶撞在同一格里，要么被柱体
    // 盖住、要么糊成一团。柱顶追上去时峰值消失、回落时再浮现，正是频谱仪该有的行为。
    let fill_cell = filled.div_ceil(EIGHTHS);
    let peak_cell = peak.div_ceil(EIGHTHS);
    (filled, if peak_cell > fill_cell { peak_cell } else { 0 })
}

/// 一格该画什么字符、什么颜色（`from_bottom` 从 1 数起）。
fn cell(from_bottom: usize, filled: usize, peak_cell: usize) -> (char, Color) {
    let lo = (from_bottom - 1) * EIGHTHS;
    let hi = from_bottom * EIGHTHS;
    if filled >= hi {
        ('█', BAR)
    } else if filled > lo {
        // 柱顶落在这一格之内：取对应高度的填充块收口
        (BLOCKS[filled - lo - 1], BAR)
    } else if peak_cell == from_bottom {
        ('▔', PEAK)
    } else {
        (' ', PEAK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// 画一块频谱并拿回缓冲区。
    ///
    /// 柱子位置**不能写死列号**：柱体宽度与两侧留白都是由可用宽度算出来的
    /// （`bar_stride` + 居中），写死列号会在调宽度时莫名其妙地挂。
    fn bars_of(width: u16, height: u16, bands: &[f32], peaks: &[f32]) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("测试终端");
        terminal
            .draw(|frame| draw_spectrum(frame, Rect::new(0, 0, width, height), bands, peaks, "频谱"))
            .expect("绘制");
        terminal.backend().buffer().clone()
    }

    /// 数一行里有几段被点亮的柱子。
    fn lit_groups(buffer: &ratatui::buffer::Buffer, width: u16, row: u16) -> usize {
        let (mut groups, mut previous) = (0, false);
        for x in 0..width {
            let lit = buffer[(x, row)].symbol() == "█";
            if lit && !previous {
                groups += 1;
            }
            previous = lit;
        }
        groups
    }

    /// 某一列从底往上的柱体格子（`▁▂▃▄▅▆▇█` 都算），遇到第一个空就停。
    fn column(buffer: &ratatui::buffer::Buffer, x: u16, bottom: u16, top: u16) -> String {
        let mut text = String::new();
        for y in (top..=bottom).rev() {
            let symbol = buffer[(x, y)].symbol();
            if symbol == " " {
                break;
            }
            text.push_str(symbol);
        }
        text
    }

    #[test]
    fn mark_quantises_to_eighths() {
        // 柱区 3 行 = 24 个刻度：0.5 → 12 刻度（底部满格 + 第二行半格）
        assert_eq!(mark(0.5, 0.0, 3), (12, 0));
        // 峰值低过柱顶时不该画（否则会和柱顶糊在一起）
        assert_eq!(mark(0.5, 0.5, 3), (12, 0));
        // 峰值高过柱顶一整格才画
        assert_eq!(mark(0.25, 0.75, 3), (6, 3));
    }

    #[test]
    fn cell_picks_the_sub_cell_block() {
        // filled = 12、柱区 3 行：最底一格满、第二格 4/8、第三格空
        assert_eq!(cell(1, 12, 0), ('█', BAR));
        assert_eq!(cell(2, 12, 0), ('▄', BAR));
        assert_eq!(cell(3, 12, 0), (' ', PEAK));
        // 峰值所在的那一格画 `▔`
        assert_eq!(cell(3, 6, 3), ('▔', PEAK));
    }

    #[test]
    fn bar_height_follows_the_level() {
        // 柱区 4 行：满格那根四行全亮，半格那根只亮两行
        let buffer = bars_of(20, 6, &[1.0, 0.0, 0.5], &[0.0, 0.0, 0.0]);
        // 空的那根把三段柱子隔开，所以从最底一行数出三段亮块就是三根柱子
        assert_eq!(lit_groups(&buffer, 20, 4), 2, "1.0 与 0.5 两根该亮");

        // 第一根（满格）那一列：4 行全亮
        let full_x = (0..20u16)
            .find(|&x| buffer[(x, 1)].symbol() == "█")
            .expect("满格柱子的最顶一行");
        assert_eq!(column(&buffer, full_x, 4, 1), "████");

        // 半格那根：底部两行亮、第三行空
        let half_x = (0..20u16)
            .rfind(|&x| buffer[(x, 3)].symbol() == "█")
            .expect("半格柱子");
        assert_eq!(column(&buffer, half_x, 4, 1), "██");
    }

    #[test]
    fn the_bar_top_uses_sub_cell_blocks() {
        // 柱区 3 行 = 24 个刻度。0.5 → 12 刻度 → 底部一行满格、第二行 4/8 块
        let buffer = bars_of(8, 5, &[0.5], &[0.0]);
        let x = (0..8u16)
            .find(|&x| buffer[(x, 3)].symbol() == "█")
            .expect("柱子");
        assert_eq!(column(&buffer, x, 3, 1), "█▄");
    }

    #[test]
    fn the_peak_marker_sits_above_the_bar() {
        // 柱子 0.25（6 刻度 → 底格 6/8 块），峰值 0.75（18 刻度）→ 峰值画在从底数第 3 格，
        // 与柱顶之间隔着一格空白（`column` 那种自底向上、遇空即停的读法跨不过这一格）。
        let buffer = bars_of(8, 5, &[0.25], &[0.75]);
        let x = (0..8u16)
            .find(|&x| buffer[(x, 1)].symbol() == "▔")
            .expect("峰值刻度");
        assert_eq!(buffer[(x, 3)].symbol(), "▆", "柱顶是 6/8 块");
        assert_eq!(buffer[(x, 2)].symbol(), " ", "柱与峰值之间是空的");
        assert_eq!(buffer[(x, 1)].symbol(), "▔", "峰值刻度在柱顶之上");
    }

    #[test]
    fn bar_count_follows_the_band_count() {
        let bands = vec![1.0f32; 6];
        let buffer = bars_of(60, 6, &bands, &bands);
        assert_eq!(lit_groups(&buffer, 60, 4), 6, "柱子数应当等于频段数");
    }

    #[test]
    fn nothing_is_drawn_without_bands() {
        let buffer = bars_of(20, 6, &[], &[]);
        assert_eq!(buffer[(1, 1)].symbol(), " ", "没有数据时柱区应当是空的");
        assert!(
            (0..20u16).any(|x| buffer[(x, 0)].symbol() == "┌"),
            "框还是要画"
        );
    }

    #[test]
    fn a_message_is_centered_when_there_is_nothing_to_show() {
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).expect("测试终端");
        terminal
            .draw(|frame| {
                draw_spectrum_message(frame, Rect::new(0, 0, 40, 8), " 频谱 ", "未在播放");
            })
            .expect("绘制");
        let buffer = terminal.backend().buffer();

        let row = (0..8u16)
            .find(|&y| (0..40u16).any(|x| buffer[(x, y)].symbol() == "未"))
            .expect("提示文案");
        assert_eq!(row, 3, "6 行内区里 2 行垫空，文案落在第 3 行（大致居中）");
        assert!(
            !(0..40u16).any(|x| (0..8u16).any(|y| buffer[(x, y)].symbol() == "█")),
            "提示态不该有柱子"
        );
    }
}
```

- [ ] **Step 12: 跑测试**

```bash
cargo test spectrum 2>&1 | tail -25
```

Expected: 5 条全绿（`mod tests` 在 `spectrum.rs` 与 `draw/spectrum.rs` 里各有一份，`spectrum` 过滤会同时选中）。

- [ ] **Step 13: `src/tui/draw/mod.rs` 拆两块**

顶部加常量与 `use`：

```rust
/// 全屏视图拆两半的最低内容区高度。再矮就只画歌词——15 行的下限终端拆出来是
/// 3 行频谱，废的。
pub const SPLIT_MIN_HEIGHT: u16 = 14;
```

`draw()` 里把 `if self.now_playing_full { self.draw_lyric(frame, areas.content); }` 改成：

```rust
        if self.now_playing_full {
            self.draw_now_playing(frame, areas.content);
        } else {
            self.draw_content(frame, areas.content);
        }
```

`impl App` 里新增：

```rust
    /// 全屏正在播放视图：上半屏频谱、下半屏歌词。
    ///
    /// 顺序是「频谱在上」：频谱永远在动、歌词是要读的，动态元素放上方当氛围层，视线
    /// 主焦点留在下半屏；这也让歌词与日常模式（底部那块）在屏幕上的位置基本一致，
    /// 按 Y 时眼睛不用跳。
    ///
    /// 三条规则，按先后：**没有歌词**就把整块高度给频谱（不画空框）；**内容区太矮**
    /// （< [`SPLIT_MIN_HEIGHT`]）就不拆，只画歌词；否则按
    /// `歌词 = clamp(内容区 / 2, 7, 12)` 拆，余量全给频谱（频谱的高度收益是线性的，
    /// 歌词的有用高度有上限）。
    fn draw_now_playing(&mut self, frame: &mut Frame, area: Rect) {
        let has_lyric = self.lyric.is_some() || self.lyric_loading;
        if !has_lyric {
            // 没有歌词时歌名只能挂在这一块上（下方没有歌词块了）
            let title = spectrum_title(self.engine.snapshot().title.as_deref());
            self.draw_spectrum_area(frame, area, &title);
            return;
        }
        if area.height < SPLIT_MIN_HEIGHT {
            self.draw_lyric(frame, area);
            return;
        }
        let lyric_height = (area.height / 2).clamp(7, MAX_LYRIC_HEIGHT);
        let rows =
            Layout::vertical([Constraint::Min(3), Constraint::Length(lyric_height)]).split(area);
        self.draw_spectrum_area(frame, rows[0], SPLIT_TITLE);
        self.draw_lyric(frame, rows[1]);
    }

    /// 画频谱那一块：有数据就画柱子，没有就给提示。
    ///
    /// 「有没有数据」**不按播放态判断，按数据本身判断**：采样窗口只在
    /// `spectrum_wanted()` 为真时才更新，其余时候 `tick()` 会把平滑与峰值清空，
    /// 所以「空」就等于「现在没有真实电平可画」。这样暂停时不会留下上一帧的柱子，
    /// 而正在播但此刻真是静音的歌，画出来的是一条平的柱子（不是提示）。
    fn draw_spectrum_area(&self, frame: &mut Frame, area: Rect, title: &str) {
        if self.spectrum_smooth.is_empty() {
            draw::spectrum::draw_spectrum_message(frame, area, title, "未在播放");
            return;
        }
        draw::spectrum::draw_spectrum(
            frame,
            area,
            &self.spectrum_smooth,
            &self.spectrum_peak,
            title,
        );
    }
}
```

文件底部（`format_duration` 旁边）加：

```rust
/// 拆两半时频谱块的标题：歌名在下方的「歌词 · 歌名」里已经出现一次，这里只报
/// 「频谱」——标题只报「这是什么」。
const SPLIT_TITLE: &str = " 频谱 ";

/// 整块归频谱时的标题：歌名得带上，因为下方没有歌词块了。
fn spectrum_title(song: Option<&str>) -> String {
    match song.filter(|song| !song.is_empty()) {
        Some(song) => format!(" 频谱 · {song} "),
        None => SPLIT_TITLE.to_string(),
    }
}
```

（Step 13 的代码块到此结束——上面那段 `}` 闭合的是 `impl App`，后面紧接 `SPLIT_TITLE` 与 `spectrum_title` 两个自由项，别把 `spectrum_title` 放进 `impl`。）

- [ ] **Step 14: 加绘制测试**

`src/tui/draw/mod.rs` 的 `mod tests` 里加（用现成的 `frame_rows` / `frame_flat` / `test_support`）：

```rust
    /// 全屏视图：上半屏频谱、下半屏歌词，两块各带自己的标题。
    #[test]
    fn the_now_playing_view_splits_into_spectrum_above_lyric() {
        let (mut app, _runtime) = test_app();
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.now_playing_full = true;
        app.spectrum_smooth = vec![1.0; crate::spectrum::BANDS];
        app.spectrum_peak = vec![1.0; crate::spectrum::BANDS];

        let rows = frame_rows(&mut app, 120, 30);
        let spectrum = rows
            .iter()
            .position(|row| row.starts_with("┌频谱"))
            .expect("频谱块");
        let lyric = rows
            .iter()
            .position(|row| row.starts_with("┌歌词"))
            .expect("歌词块");
        assert!(spectrum < lyric, "频谱该在歌词上面：{spectrum} vs {lyric}");
        assert!(rows[spectrum + 1].contains('█'), "频谱块里该有柱子");
        assert!(rows[lyric + 1..].iter().any(|row| row.contains("第1句")));

        // 歌名只在歌词块上出现一次（频谱块整块归它时才带歌名）。
        let text = frame_flat(&mut app, 120, 30);
        assert_eq!(text.matches("频谱").count(), 1, "频谱块只有一块：{text}");
    }

    /// 没有歌词时整块给频谱，标题带上歌名。
    #[test]
    fn without_a_lyric_the_spectrum_takes_the_whole_area() {
        let (mut app, _runtime) = test_app();
        app.lyric = None;
        app.lyric_loading = false;
        app.now_playing_full = true;
        app.spectrum_smooth = vec![1.0; crate::spectrum::BANDS];
        app.spectrum_peak = vec![1.0; crate::spectrum::BANDS];

        let rows = frame_rows(&mut app, 120, 30);
        assert!(
            rows.iter().any(|row| row.starts_with("┌频谱")),
            "频谱块：{rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.starts_with("┌歌词")),
            "没有歌名块：{rows:?}"
        );
    }
```

另外在 `src/tui/state.rs` 的 `mod tests` 里加两条平滑的（`test_app` 已是现成构造器）：

```rust
    #[test]
    fn a_single_frame_of_sound_does_not_jump_to_the_top() {
        // 静音中突然来一记鼓点：一帧就该走完的话，块字符会在 ▁ 与 █ 之间跳变
        let (mut app, _runtime) = test_app();
        app.spectrum_raw = vec![1.0; crate::spectrum::BANDS];
        app.advance_visualizer(std::time::Duration::from_millis(33));
        let after = app.spectrum_smooth[0];
        assert!(after > 0.0 && after < 0.5, "一帧就走了 {after:.2}，太快了");
        // 峰值不会低于柱体
        assert!(app.spectrum_peak[0] >= after);
    }

    #[test]
    fn smoothing_is_frame_rate_independent() {
        // 同一段真实时间，拆成两帧推进与一帧推进结果一致
        let (mut one, _runtime) = test_app();
        one.spectrum_raw = vec![1.0; crate::spectrum::BANDS];
        one.advance_visualizer(std::time::Duration::from_millis(66));

        let (mut two, _runtime) = test_app();
        two.spectrum_raw = vec![1.0; crate::spectrum::BANDS];
        two.advance_visualizer(std::time::Duration::from_millis(33));
        two.advance_visualizer(std::time::Duration::from_millis(33));

        assert!(
            (one.spectrum_smooth[0] - two.spectrum_smooth[0]).abs() < 1e-3,
            "帧率不该改变观感：{} vs {}",
            one.spectrum_smooth[0],
            two.spectrum_smooth[0]
        );
    }
```

- [ ] **Step 15: 跑测试 + clippy**

```bash
cargo test && cargo clippy --all-targets -- -D warnings
```

Expected: 全绿。特别确认两条现有测试仍然绿：`fullscreen_lyric_takes_over_the_content_area`、`the_current_lyric_line_is_highlighted`（新布局与它们的断言不冲突，不要去改它们）。

- [ ] **Step 16: 提交**

```bash
git add -A && git commit -m "feat(tui): Y 视图上半屏接上真频谱（FFT + 对数分频）

- spectrum.rs：自写 radix-2 FFT、Hann 窗、64 段对数分频，零依赖
- levels.rs：无锁环形缓冲 + 抽稀（保住 hi-res 下的低频分辨率）+ LevelMeter 包装器
- player.rs：Cmd::Play 处包一层并逐项转发 Source 方法（try_seek 漏掉会让 seek 静默失效）
- Y 视图拆成「上频谱 / 下歌词」，歌词高度 clamp(内容区/2, 7, 12)"
```

---

# Task 3: 退化与边界

Task 2 已经按「有没有数据」决定画柱子还是提示，但提示文案只有「未在播放」一种。这一步把文案按播放态分开（缓冲中 vs 没在播），并把两条边界用 App 级测试钉住：没在播时不留残影、矮屏不拆。

**Files:**
- Modify: `src/tui/draw/mod.rs`（`spectrum_message` 纯函数 + `draw_spectrum_area` 用它 + 单测）

**Interfaces:**
- Consumes: Task 2 的 `draw_spectrum` / `draw_spectrum_message` / `draw_now_playing` / `App::spectrum_wanted` / `SPLIT_MIN_HEIGHT`
- Produces: `spectrum_message(&Phase) -> &'static str`（自由函数，`draw/mod.rs` 内可见）

- [ ] **Step 1: 先写失败的测试（提示文案按播放态选）**

文案的挑选是个纯函数，所以它可以直接单测——哑引擎走不到 `Buffering`/`Paused` 这些状态，只有纯函数能钉住。`src/tui/draw/mod.rs` 的 `mod tests` 加：

```rust
    /// 频谱块的提示文案按播放态选：缓冲中与「没在播」不是一回事。
    #[test]
    fn the_spectrum_message_follows_the_playback_phase() {
        use crate::player::Phase;
        assert_eq!(spectrum_message(&Phase::Buffering), "缓冲中…");
        assert_eq!(spectrum_message(&Phase::Idle), "未在播放");
        assert_eq!(spectrum_message(&Phase::Paused), "未在播放");
        assert_eq!(spectrum_message(&Phase::Playing), "未在播放");
        assert_eq!(spectrum_message(&Phase::Failed("x".into())), "未在播放");
    }
```

- [ ] **Step 2: 跑测试看它失败**

```bash
cargo test the_spectrum_message 2>&1 | tail -6   # canto 只有 bin target，没有 --lib
```

Expected: 编译错误 `cannot find function spectrum_message`。

- [ ] **Step 3: 实现**

`src/tui/draw/mod.rs`（`spectrum_title` 旁边）：

```rust
/// 频谱块没数据可画时那句话。缓冲与「没在播」要分开报——前者等一会儿就有了。
fn spectrum_message(phase: &Phase) -> &'static str {
    match phase {
        Phase::Buffering => "缓冲中…",
        // Idle / Paused / Failed / Playing（刚起播、窗口还没填满）
        _ => "未在播放",
    }
}
```

同时把 Task 2 的 `draw_spectrum_area` 里那句写死的 `"未在播放"` 换成按当前播放态取：

```rust
    fn draw_spectrum_area(&self, frame: &mut Frame, area: Rect, title: &str) {
        if self.spectrum_smooth.is_empty() {
            let message = spectrum_message(&self.engine.snapshot().phase);
            draw::spectrum::draw_spectrum_message(frame, area, title, message);
            return;
        }
        …
```

- [ ] **Step 4: 跑测试看它通过**

```bash
cargo test 2>&1 | tail -8
```

Expected: 新增那条绿，其余全绿。

- [ ] **Step 5: App 级的两条边界测试**

`src/tui/draw/mod.rs` 的 `mod tests` 加：

```rust
    /// 没在播时：进过 tick 之后不该留着上一帧的柱子（`tick()` 会清掉缓冲）。
    #[test]
    fn an_idle_engine_shows_a_message_instead_of_stale_bars() {
        let (mut app, _runtime) = test_app();
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.now_playing_full = true;
        // 故意塞一份「上一帧」的柱子：没在播时它不该被画出来
        app.spectrum_smooth = vec![1.0; crate::spectrum::BANDS];
        app.spectrum_peak = vec![1.0; crate::spectrum::BANDS];
        // 真实运行时每帧都会走 tick：它发现「没在播」就把缓冲清空
        app.tick();

        let rows = frame_rows(&mut app, 120, 30);
        let spectrum = rows
            .iter()
            .position(|row| row.starts_with("┌频谱"))
            .expect("频谱块");
        let lyric = rows
            .iter()
            .position(|row| row.starts_with("┌歌词"))
            .expect("歌词块");
        assert!(
            rows[spectrum..lyric].iter().any(|row| row.contains("未在播放")),
            "空闲该给提示：{rows:?}"
        );
        assert!(
            !rows[spectrum..lyric].iter().any(|row| row.contains('█')),
            "不该画残留的柱子：{rows:?}"
        );
    }

    /// 太矮的终端不拆：15 行高的窗口只画歌词（就是今天的 Y）。
    #[test]
    fn a_short_window_keeps_the_lyric_only_view() {
        let (mut app, _runtime) = test_app();
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.now_playing_full = true;

        // 15 行的下限窗口：内容区 10 行 < SPLIT_MIN_HEIGHT
        let rows = frame_rows(&mut app, 100, 15);
        assert!(
            rows.iter().any(|row| row.starts_with("┌歌词")),
            "歌词块还在：{rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.starts_with("┌频谱")),
            "矮屏不拆：{rows:?}"
        );
    }
```

- [ ] **Step 6: 跑测试 + clippy**

```bash
cargo test && cargo clippy --all-targets -- -D warnings
```

Expected: 全绿。特别确认 Task 2 那条 `the_now_playing_view_splits_into_spectrum_above_lyric` 仍然绿——它在测试里直接塞了 `spectrum_smooth`，所以走的是「有数据」那条路（这正是**不按播放态判断、按数据判断**的好处）。

- [ ] **Step 7: 提交**

```bash
git add -A && git commit -m "feat(tui): 频谱块的退化处理（缓冲中与未在播放分开报、矮屏不拆）"
```

---

# Task 4: 帧率提速到 ~30fps

150ms 一跳（6.7fps）下柱子是一格一格蹦的。只在「全屏视图开着 + 正在播」时提速。

**Files:**
- Modify: `src/tui/mod.rs`（`SPECTRUM_TICK` + 事件循环）
- Modify: `src/tui/state.rs`（`spectrum_animating` 及其单测）

**Interfaces:**
- Consumes: Task 2 的 `App::now_playing_full`
- Produces: `App::spectrum_animating(&self) -> bool`

- [ ] **Step 1: 先写失败的测试**

`src/tui/state.rs` 的 `mod tests` 加：

```rust
    #[test]
    fn only_a_playing_fullscreen_view_raises_the_frame_rate() {
        let (mut app, _runtime) = test_app();
        assert!(!app.spectrum_animating(), "默认（没开视图）不提速");

        app.now_playing_full = true;
        // 哑引擎：没播过，phase 还是 Idle
        assert!(!app.spectrum_animating(), "没在播不提速");
    }
```

- [ ] **Step 2: 跑测试看它失败**

```bash
cargo test raises_the_frame_rate 2>&1 | tail -6
```

Expected: 编译错误 `no method named spectrum_animating`。

- [ ] **Step 3: 实现**

`src/tui/state.rs`（放在 `spectrum_wanted` 旁边）：

```rust
    /// 频谱是不是正在动——界面刷新据此把间隔提到 ~30fps。
    ///
    /// 只在全屏视图开着**且**正在播时为真：别的屏不显示频谱（不提速），暂停时那几格
    /// 不会动（也不需要重绘）。
    pub(super) fn spectrum_animating(&self) -> bool {
        self.spectrum_wanted()
            && matches!(self.screen, Screen::Main)
    }
```

`src/tui/mod.rs`：

```rust
/// 界面刷新间隔，同时也是按键与后台消息的最长等待。
const TICK: StdDuration = StdDuration::from_millis(150);
/// 频谱在动时的刷新间隔（约 30fps）。终端一次重绘是整屏 diff，再往上（60fps）
/// 观感差别很小、重绘成本却翻倍，所以取这个折中。
const SPECTRUM_TICK: StdDuration = StdDuration::from_millis(33);
```

事件循环里 `event::poll(TICK)` 改成：

```rust
        // 频谱在动时提速：6.7fps 下柱子是一格一跳。
        let tick = if app.spectrum_animating() {
            SPECTRUM_TICK
        } else {
            TICK
        };
        if event::poll(tick).context("读取键盘事件失败")?
```

`src/tui/mod.rs` 顶部注释里那句「界面刷新间隔」保持不动（它描述的是默认值），另外把文件头文档里 `y 开/关歌词面板` 那条列表补一行 `Y`（文档与键位表一致）。

- [ ] **Step 4: 跑测试 + clippy**

```bash
cargo test && cargo clippy --all-targets -- -D warnings
```

Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add -A && git commit -m "feat(tui): 频谱在动时把刷新间隔提到 33ms（约 30fps）"
```

---

# Task 5: 文案、文档与真机验收

**Files:**
- Modify: `src/tui/draw/overlay.rs`、`README.md`、`docs/design.md`
- 验收：真跑一次

- [ ] **Step 1: 更新帮助浮层与状态行文案**

`src/tui/draw/overlay.rs` 的 `help_lines`：

```rust
        Line::from("全局  / 筛选当前列表 · Y 全屏：歌词 + 频谱 · ? 帮助 · q 退出"),
```

`src/tui/draw/mod.rs` 的 `hints()` 里 `Context::NowPlaying` 那一行加一个词，说明这块视图现在有两半：

```rust
        keys::Context::NowPlaying => {
            "Y/Esc 退出 · 上频谱下歌词 · Space 暂停 · n/b 切歌 · ←/→ 10 秒 · ? 帮助"
        }
```

（`←/→ 10 秒` 与 `+/- 音量` 两条提示原来同在一行，现在只留前者——`+`/`-` 仍然可用，帮助浮层里有完整表；状态行只放最常用的几个。）

- [ ] **Step 2: 跑测试确认没有测试被文案改动带崩**

```bash
cargo test 2>&1 | tail -15
```

Expected: 全绿。`draw/mod.rs` 里那条断言 `status_row.contains("Y/Esc退出")` 仍然成立；帮助浮层的测试断言的是「切屏」「歌单」「关闭Esc或?」那几行，不受影响。

- [ ] **Step 3: README**

功能表加一行（放在「歌词」那行后面）：

```markdown
| 可视化 | 全屏正在播放（`Y`）：上半屏**真频谱**（对音频采样做 FFT + 对数分频）+ 下半屏歌词 | ✅ |
```

键位表里 `y` / `Y` 那行改成：

```markdown
| `y` / `Y` | 底部歌词块开 / 关 · **全屏正在播放**（上半屏真频谱 + 下半屏歌词；只占内容区，播放条与状态行不动；`Y` 或 `Esc` 退出） |
```

- [ ] **Step 4: `docs/design.md`**

在「`## 设计要点`」的「**界面**」小节里加一段，并在「已知限制（细节）」里加一条：

```markdown
- **全屏正在播放（`Y`）是两块**：上半屏真频谱、下半屏歌词。频谱的数据来自音频线程：
  `player.rs` 在 `append` 之前把解码器包一层（`levels::LevelMeter`），采样原样透传给
  播放器，同时把**第 0 声道**写进一份无锁环形缓冲（2048 个 `AtomicU32`，存 f32 的位模式
  ——互斥锁会让音频线程被界面卡住，那是爆音）。界面每帧取一次窗口做 FFT，**在界面线程
  算**（放音频线程会拖住播放）。分三步都是必需的：Hann 窗（不然两端截断的泄漏让所有
  柱子一起抖）、幅度归一化（不然 dB 之后每段都顶格，是一堵平墙）、**对数分频**
  （线性 bin 下 20~200Hz 挤在最左两三格，右边一大半不动——不做这一步画出来的不是频谱仪）。
  源采样率高于 44.1kHz 时按整数倍抽稀（并做一次 N 点滑动平均，防止 Nyquist 以上的内容
  折叠进可闻段），让窗口是「约 46ms」而不是「2048 个点」。频谱只在 `Y` 视图可见且正在播时
  才取、才画，并把刷新间隔从 150ms 提到 33ms。
- **`LevelMeter` 必须逐项转发 `Source` 的方法**，尤其是 `try_seek`：它的默认实现直接返回
  `NotSupported`，包装层漏掉转发，rodio 就认为整条音频不可跳转——`←`/`→` 与进度条跳转会
  静默失效，而播放本身完全正常。`levels.rs` 里有一条单测钉住这件事。

### 已知限制

- **hi-res（192kHz）的高频段显示质量**：抽稀用滑动平均做了粗低通，但 16kHz 以上仍可能有
  轻微噪底。**只影响显示**，音频一个采样都没动（包装器是透传的）。
- **频谱这条链路没有端到端测试**：测试用的哑引擎（`Engine::silent`）的 mixer 没人拉样本，
  音频源根本不推进，所以测试里频谱恒为零——`analyze()` 是纯函数可以单测（喂正弦波看峰值
  落在哪一段），绘制用 `TestBackend` 断言，但「柱子真的跟着音乐动」只能真跑一次看。
- **观感常数（平滑 τ、dB 窗口、倾斜补偿）** 是本模块里唯一不能靠单测定的部分。
```

- [ ] **Step 5: 真机验收（这一条不能省）**

```bash
cargo run --release
```

逐条对照 spec 的验收标准，边听边看：

1. **有声音就有柱子**：播一首鼓点明显的歌，按 `Y`。低频（左）随鼓点起落、人声在中段、镲片在右边；柱子起振跟手、回落有拖尾，不抖成一团。
2. **峰值刻度**：柱子顶上那条 `▔` 比柱体落得慢。
3. **暂停**：按 `Space`，柱区变成「未在播放」并且不再有残留柱子；再按 `Space` 恢复。
4. **跳转**：按 `←`/`→` 与点进度条，进度条真的动、声音真的跳（**这条专门验 `try_seek` 转发**）。
5. **切歌**：按 `n`，新歌起播时没有上一首的柱子尾巴。
6. **没有歌词的歌**（或歌词加载完成但为空）：整块归频谱，标题带歌名。
7. **矮屏**：把窗口调到 15–19 行按 `Y`，只剩歌词块；拉到 24 行以上恢复两块。
8. **关闭视图不费算力**：退出 `Y` 后按任意键的响应与以前一样跟手（`top` 里 CPU 应该落回去）。
9. **观感微调**：如果柱子显得太"肉"或太抖，改 `state.rs` 里 `advance_visualizer` 的 `0.12` / `0.40`；如果低频柱子常年顶格，改 `spectrum.rs` 的 `DB_CEIL`。**改完重跑一遍 1–3 条。**

把 1–8 条的实际结果（尤其是第 4 条与第 5 条）写进提交信息或 `docs/design.md` 的那段里——**没有真跑过就不要声称端到端可用**。

- [ ] **Step 6: 全量检查 + 提交**

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo build --release
git add -A && git commit -m "docs: 全屏正在播放（频谱 + 歌词）的文档与真机验收记录"
```

---

## 自查记录（写计划时跑过）

- **spec 覆盖**：布局与高度分配与「无歌词优先于矮屏不拆」的规则先后（Task 2 Step 13 + Step 14 测试、Task 3 Step 5）、频谱不进 `Snapshot`（Global Constraints）、零依赖 FFT（Task 2 Step 3）、Hann 窗 / 归一化 / 对数分频（Task 2 Step 3）、抽稀与防折叠（Task 2 Step 5–6 + 测试）、`try_seek` 转发（Task 2 Step 5 测试）、零分配（`Analyzer` 与 `read_into` 复用缓冲）、帧率（Task 4）、退化与「不留残影」（Task 3）、改名（Task 1）、文档与真机验收（Task 5）。无遗漏。
- **占位符**：无 TBD / 「稍后实现」；每个代码步骤都给了可粘贴的完整代码（唯一一处 `…` 是 Task 3 Step 3 里「把这一行换掉」的省略，紧邻的上一段就是完整函数）。
- **类型一致性**：`Analyzer::analyze_into(&mut self, &[f32], u32, &mut Vec<f32>)`、`Levels::read_into(&self, &mut Vec<f32>) -> u32`、`LevelMeter::new(inner, levels)`、`draw_spectrum(frame, area, bands, peaks, title)`、`draw_spectrum_message(frame, area, title, message)`、`spectrum_title(Option<&str>)`、`spectrum_message(&Phase)`、`App::spectrum_wanted / spectrum_animating / draw_now_playing / draw_spectrum_area`、`now_playing_full`、`SPLIT_TITLE`、`SPLIT_MIN_HEIGHT`、`SPECTRUM_TICK` 在各任务间一致。
- **任务间不许互相踩**：Task 2 加的两条 App 级绘制测试（`the_now_playing_view_splits_into_spectrum_above_lyric`、`without_a_lyric_the_spectrum_takes_the_whole_area`）在 Task 3 之后必须仍然绿——它们靠注入 `spectrum_smooth` 走「有数据」这条路，而 Task 3 只改「没数据」时的文案。写 Task 3 时**不要**去改这两条测试；它们挂了说明「按数据判断」被改成了「按播放态判断」。
- **已知的执行期注意点**：`Levels` / `LevelMeter` / `spectrum` 在 Task 2 Step 8 之前没有非测试调用方，所以**不要在 Step 8 之前提交**——CI 用 `-D warnings` 跑 `clippy --all-targets`，bin target 上的 `dead_code` 会直接把构建判失败。
